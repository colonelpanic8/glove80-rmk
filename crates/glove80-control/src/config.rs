//! The `config` subcommand: files, device I/O, and reporting around the pure
//! runtime-state model in [`glove80_config`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use glove80_config::{
    background_from_wire, background_to_wire, conditional_scene_from_wire,
    conditional_scene_to_wire, differences, effects_from_wire, effects_to_wire, live_param_tables,
    output_mode_from_wire, output_mode_to_wire, params_to_writes, runtime_config_from_moergo_json,
    scene_from_wire, scene_policy_from_wire, scene_policy_to_wire, scene_to_wire,
    snapshot_to_moergo_json, trim_trailing_transparent_layers, BehaviorSnapshot, EffectParams,
    EffectsConfig, LightingSnapshot, OutputModeConfig, ParamSpec, RuntimeConfig, Snapshot, COLS,
    LAYER_SIZE, ROWS,
};
use rynk::rmk_types::protocol::rynk::{
    Cmd, LightingError, LightingExtendedConditionalSceneCell, LightingExtensionNameKind,
    LightingExtensionParamsRequest, LightingFeatureFlags, LightingMutableState, RynkError,
    SetLightingExtensionLayersRequest, SetLightingExtensionParamRequest,
    SetLightingExtensionStateRequest, SetLightingLayerPolicyRequest, SetLightingOutputModeRequest,
    SetLightingStateRequest,
};
use rynk::{Client, RynkHostError};

use crate::transport::Selector;

pub use glove80_config::DiffFound;

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Validate a runtime TOML or MoErgo Layout Editor JSON file offline.
    Validate { file: PathBuf },
    /// Compare a runtime TOML or MoErgo JSON file with the keyboard.
    Diff { file: PathBuf },
    /// Apply a runtime TOML or MoErgo JSON file and verify it by read-back.
    Apply {
        file: PathBuf,
        /// Show differences without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pull the connected keyboard's runtime state into a TOML or JSON file.
    Pull {
        file: PathBuf,
        /// Output format. Inferred from an existing file or its extension when omitted.
        #[arg(long, value_enum)]
        format: Option<ConfigFormat>,
    },
    /// Print the connected keyboard's runtime state.
    Show {
        #[arg(long, value_enum, default_value_t = ConfigFormat::Toml)]
        format: ConfigFormat,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ConfigFormat {
    #[default]
    Toml,
    /// Experimental JSON backup format from the MoErgo Layout Editor.
    #[value(name = "moergo-json", alias = "json")]
    MoergoJson,
}

fn file_format(path: &Path, text: Option<&str>) -> ConfigFormat {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        || text.is_some_and(|text| text.trim_start().starts_with('{'))
    {
        ConfigFormat::MoergoJson
    } else {
        ConfigFormat::Toml
    }
}

fn parse_text(text: &str, format: ConfigFormat) -> Result<RuntimeConfig> {
    match format {
        ConfigFormat::Toml => RuntimeConfig::from_toml(text),
        ConfigFormat::MoergoJson => runtime_config_from_moergo_json(text),
    }
}

fn parse(path: &Path) -> Result<RuntimeConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    parse_text(&text, file_format(path, Some(&text)))
        .with_context(|| format!("could not parse {}", path.display()))
}

fn render(
    config: &RuntimeConfig,
    snapshot: &Snapshot,
    format: ConfigFormat,
    template: Option<&str>,
) -> Result<String> {
    match format {
        ConfigFormat::Toml => config.to_toml(),
        ConfigFormat::MoergoJson => snapshot_to_moergo_json(snapshot, Some(config), template),
    }
}

pub fn run(selector: &Selector, command: &ConfigCommand) -> Result<()> {
    if let ConfigCommand::Validate { file } = command {
        parse(file)?;
        println!("{} is valid", file.display());
        return Ok(());
    }
    crate::rynk_client::run_config(selector, command)
}

pub async fn operate(client: &Client, command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Validate { .. } => unreachable!("validate is offline"),
        ConfigCommand::Show { format } => {
            let snapshot = read_snapshot(client).await?;
            let config = RuntimeConfig::from_snapshot(&snapshot, None);
            print!("{}", render(&config, &snapshot, *format, None)?);
        }
        ConfigCommand::Pull { file, format } => {
            let snapshot = read_snapshot(client).await?;
            let old_text = std::fs::read_to_string(file).ok();
            let inferred = format.unwrap_or_else(|| file_format(file, old_text.as_deref()));
            let labels = old_text
                .as_deref()
                .and_then(|text| parse_text(text, file_format(file, Some(text))).ok());
            let mut config = RuntimeConfig::from_snapshot(&snapshot, labels.as_ref());
            config.retain_non_default_params(&snapshot);
            let template = (inferred == ConfigFormat::MoergoJson)
                .then_some(old_text.as_deref())
                .flatten();
            let text = render(&config, &snapshot, inferred, template)?;
            std::fs::write(file, text)
                .with_context(|| format!("could not write {}", file.display()))?;
            println!("pulled live runtime configuration into {}", file.display());
        }
        ConfigCommand::Diff { file } => {
            let desired = parse(file)?.snapshot()?;
            let live = read_snapshot(client).await?;
            if !print_diff(&desired, &live) {
                return Err(DiffFound.into());
            }
        }
        ConfigCommand::Apply { file, dry_run } => {
            let desired = parse(file)?.snapshot()?;
            let before = read_snapshot(client).await?;
            let pending = differences(&desired, &before);
            if pending.is_empty() {
                println!("keyboard already matches {}", file.display());
                return Ok(());
            }
            for difference in &pending {
                println!("{difference}");
            }
            if *dry_run {
                println!("dry run: no changes written");
                return Ok(());
            }
            apply_snapshot(client, &desired, &before).await?;
            let after = read_snapshot(client).await?;
            let remaining = differences(&desired, &after);
            if !remaining.is_empty() {
                bail!("read-back verification failed:\n{}", remaining.join("\n"));
            }
            println!("applied and verified {}", file.display());
        }
    }
    Ok(())
}

async fn read_snapshot(client: &Client) -> Result<Snapshot> {
    let capabilities = client.get_capabilities().await?;
    if capabilities.num_rows != ROWS || capabilities.num_cols != COLS {
        bail!(
            "expected a {ROWS}x{COLS} Glove80, device reports {}x{}",
            capabilities.num_rows,
            capabilities.num_cols
        );
    }
    // The bulk read is an optimization, so a device that advertises it but
    // cannot deliver a decodable response falls back to reading key by key
    // rather than failing the whole command. Firmware carrying a keymap
    // written by an older keycode table can land in exactly that state.
    let bulk = if capabilities.bulk_transfer_supported {
        match client.read_all_keymap().await {
            Ok(actions) => Some(actions),
            Err(error) => {
                eprintln!("bulk keymap read failed ({error}); falling back to key-by-key");
                None
            }
        }
    } else {
        None
    };
    let actions = match bulk {
        Some(actions) => actions,
        None => {
            let mut actions = Vec::new();
            for layer in 0..capabilities.num_layers {
                for row in 0..ROWS {
                    for col in 0..COLS {
                        actions.push(client.get_key(layer, row, col).await?);
                    }
                }
            }
            actions
        }
    };
    let mut layers = actions
        .chunks(LAYER_SIZE)
        .enumerate()
        .map(|(layer, actions)| {
            actions
                .iter()
                .copied()
                .enumerate()
                .map(|(offset, action)| glove80_config::action_to_code(action, layer, offset))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    trim_trailing_transparent_layers(&mut layers);

    let lighting_caps = client.get_lighting_capabilities().await?;
    let state = client.get_lighting_state().await?;
    let output_mode = if lighting_caps
        .features
        .contains(LightingFeatureFlags::OUTPUT_MODE)
    {
        output_mode_from_wire(client.get_lighting_output_mode().await?.mode)
    } else {
        OutputModeConfig::AlwaysOn
    };
    let scene_status = client.get_lighting_scene_status().await?;
    let (_, scene_cells) = client.read_all_lighting_scenes().await?;
    // Firmware that predates the runtime conditional table reports nothing
    // rather than an empty table, so a file that names no rules does not read
    // as "delete what the board has".
    let conditional_scenes = if lighting_caps
        .features
        .contains(LightingFeatureFlags::RUNTIME_EFFECTS_CONDITIONS)
    {
        let (_, cells) = client
            .read_all_lighting_extended_runtime_conditional_scenes()
            .await?;
        Some(
            cells
                .into_iter()
                .map(conditional_scene_from_wire)
                .collect::<Vec<_>>(),
        )
    } else if lighting_caps
        .features
        .contains(LightingFeatureFlags::RUNTIME_CONDITIONAL_SCENES)
    {
        let (_, cells) = client
            .read_all_lighting_runtime_conditional_scenes()
            .await?;
        Some(
            cells
                .into_iter()
                .map(|cell| {
                    conditional_scene_from_wire(LightingExtendedConditionalSceneCell {
                        cell,
                        connection: None,
                        effects: None,
                    })
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let mut scenes = scene_cells
        .into_iter()
        .map(scene_from_wire)
        .collect::<Vec<_>>();
    scenes.sort();
    let (effects, extension_params) = if lighting_caps
        .features
        .contains(LightingFeatureFlags::EXTENSION_EFFECTS)
    {
        let extension = client.get_lighting_extension().await?;
        let effect_names = read_extension_names(client, LightingExtensionNameKind::Effects).await?;
        let palette_names =
            read_extension_names(client, LightingExtensionNameKind::Palettes).await?;
        let overlay = if lighting_caps
            .features
            .contains(LightingFeatureFlags::EXTENSION_LAYERING)
        {
            client.get_lighting_extension_layers().await?.overlay
        } else {
            None
        };
        let extension_params = read_extension_params(client, &effect_names).await?;
        (
            Some(effects_from_wire(
                extension.state,
                overlay,
                &effect_names,
                &palette_names,
                live_param_tables(extension_params.as_deref()),
            )?),
            extension_params,
        )
    } else {
        (None, None)
    };
    Ok(Snapshot {
        default_layer: client.get_default_layer().await?,
        layers,
        behaviors: read_behaviors(client).await?,
        lighting: Some(LightingSnapshot {
            brightness: state.output_brightness,
            output_mode,
            scene_policy: scene_policy_from_wire(scene_status.policy),
            conditional_scenes,
            background: background_from_wire(state.background),
            effects,
            params: extension_params,
            scenes,
        }),
    })
}

/// Read every extension name of one kind as owned strings.
pub(crate) async fn read_extension_names(
    client: &Client,
    kind: LightingExtensionNameKind,
) -> Result<Vec<String>> {
    Ok(client
        .read_all_lighting_extension_names(kind)
        .await?
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect())
}

/// Read the parameters of every effect that advertises any, or `None` when the
/// keyboard has no parameter surface at all. Effects without parameters are
/// omitted rather than recorded as empty lists.
pub(crate) async fn read_extension_params(
    client: &Client,
    effect_names: &[String],
) -> Result<Option<Vec<EffectParams>>> {
    let mut sets = Vec::new();
    for (index, effect) in effect_names.iter().enumerate() {
        let index = u8::try_from(index).context("effect index exceeds u8")?;
        let params = match read_effect_params(client, index).await {
            Ok(params) => params,
            Err(error) if params_unsupported(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !params.is_empty() {
            sets.push(EffectParams {
                index,
                effect: effect.clone(),
                params,
            });
        }
    }
    Ok(Some(sets))
}

/// Page one effect's parameter list. Pages are shaped like extension-name
/// pages, so they are walked the same way: advance by the items returned and
/// stop at the advertised total.
async fn read_effect_params(client: &Client, effect: u8) -> Result<Vec<ParamSpec>, RynkHostError> {
    let mut params = Vec::new();
    let mut offset: u8 = 0;
    loop {
        let page = client
            .get_lighting_extension_params(LightingExtensionParamsRequest { effect, offset })
            .await?;
        if offset >= page.total {
            break;
        }
        if page.items.is_empty() || usize::from(offset) + page.items.len() > usize::from(page.total)
        {
            return Err(RynkHostError::InconsistentResponse {
                cmd: Cmd::GetLightingExtensionParams,
                reason: "parameter page is empty or extends beyond the advertised total",
            });
        }
        offset += page.items.len() as u8;
        params.extend(page.items.iter().map(|item| ParamSpec {
            name: item.name.as_str().to_owned(),
            min: item.min,
            max: item.max,
            default: item.default,
            value: item.value,
        }));
    }
    Ok(params)
}

/// Firmware without the parameter commands answers `UnknownCmd`, and firmware
/// whose lighting source advertises no parameter descriptor answers
/// `Unsupported`. Both mean the same thing to a host: there is nothing to read.
fn params_unsupported(error: &RynkHostError) -> bool {
    matches!(
        error,
        RynkHostError::Rejected(RynkError::UnknownCmd | RynkError::Unimplemented)
            | RynkHostError::LightingRejected(LightingError::Unsupported)
            | RynkHostError::Unsupported(..)
    )
}

/// Read the behavior tables a keymap cell addresses by index.
///
/// Firmware without these commands answers `UnknownCmd`, which reads as "this
/// keyboard has no such table" rather than as a failure, so an older device
/// still pulls and diffs its keymap.
/// One macro chunk, which the protocol fixes for both directions.
const MACRO_CHUNK: usize = rynk::rmk_types::constants::MACRO_DATA_SIZE;

async fn read_behaviors(client: &Client) -> Result<BehaviorSnapshot> {
    Ok(BehaviorSnapshot {
        morses: client.read_all_morses().await.ok(),
        combos: client.read_all_combos().await.ok(),
        macros: read_macro_space(client).await.ok(),
        forks: read_all_forks(client).await.ok(),
    })
}

/// Read the fork table a slot at a time.
///
/// The protocol has no bulk form for forks, so this walks to the capacity the
/// device advertises. Any rejection means the firmware has no fork table, which
/// the caller reads as "nothing to manage" rather than as a failure.
async fn read_all_forks(client: &Client) -> Result<Vec<rynk::rmk_types::fork::Fork>> {
    let capabilities = client.get_capabilities().await?;
    let mut forks = Vec::new();
    for index in 0..capabilities.max_forks {
        forks.push(client.get_fork(index).await?);
    }
    Ok(forks)
}

/// Read macro space by walking it a chunk at a time.
///
/// Chunks come back full size and zero-filled past the end, so there is no
/// short read to stop on; the walk stops when a chunk adds nothing but padding.
async fn read_macro_space(client: &Client) -> Result<Vec<u8>> {
    let mut space = Vec::new();
    let mut offset = 0u16;
    loop {
        let chunk = client.get_macro(offset).await?;
        if chunk.data.is_empty() || chunk.data.iter().all(|byte| *byte == 0) {
            break;
        }
        space.extend_from_slice(&chunk.data);
        offset = offset
            .checked_add(u16::try_from(chunk.data.len()).context("macro chunk too large")?)
            .context("macro space offset overflowed")?;
    }
    // Trailing padding is not part of any sequence.
    while space.last() == Some(&0) {
        space.pop();
    }
    Ok(space)
}

/// Write the behavior tables, skipping any the source is silent about.
async fn apply_behaviors(
    client: &Client,
    desired: &BehaviorSnapshot,
    before: &BehaviorSnapshot,
) -> Result<()> {
    if let Some(morses) = &desired.morses {
        if before.morses.as_ref() != Some(morses) {
            client
                .write_all_morses(morses.clone())
                .await
                .context("could not write the morse table")?;
        }
    }
    if let Some(combos) = &desired.combos {
        if before.combos.as_ref() != Some(combos) {
            client
                .write_all_combos(combos.clone())
                .await
                .context("could not write the combo table")?;
        }
    }
    if let Some(forks) = &desired.forks {
        // No bulk form for forks, so write only the slots that differ.
        let present = before.forks.as_deref().unwrap_or_default();
        for (index, fork) in forks.iter().enumerate() {
            if present.get(index) == Some(fork) {
                continue;
            }
            let index = u8::try_from(index).context("more forks than the protocol can address")?;
            client
                .set_fork(index, *fork)
                .await
                .with_context(|| format!("could not write fork {index}"))?;
        }
    }
    if let Some(macros) = &desired.macros {
        if before.macros.as_ref() != Some(macros) {
            write_macro_space(client, macros).await?;
        }
    }
    Ok(())
}

async fn write_macro_space(client: &Client, space: &[u8]) -> Result<()> {
    // One extra terminator so a shorter sequence set does not leave the tail
    // of a longer one behind to be parsed as another macro.
    let mut payload = space.to_vec();
    payload.push(0);
    for (index, chunk) in payload.chunks(MACRO_CHUNK).enumerate() {
        let offset = u16::try_from(index * MACRO_CHUNK).context("macro space is too large")?;
        let data = rynk::rmk_types::protocol::rynk::MacroData {
            data: heapless::Vec::from_slice(chunk)
                .map_err(|_| anyhow::anyhow!("macro chunk exceeds the protocol's chunk size"))?,
        };
        client
            .set_macro(offset, data)
            .await
            .context("could not write macro space")?;
    }
    Ok(())
}

async fn apply_snapshot(client: &Client, desired: &Snapshot, before: &Snapshot) -> Result<()> {
    let capabilities = client.get_capabilities().await?;
    if desired.layers.len() > usize::from(capabilities.num_layers) {
        bail!(
            "configuration has {} layers but device supports {}",
            desired.layers.len(),
            capabilities.num_layers
        );
    }
    // Before the keymap: a cell holding `TD(n)` or `TriggerMacro(n)` addresses
    // a table slot by index, so the tables have to be in place before any key
    // can point at them.
    apply_behaviors(client, &desired.behaviors, &before.behaviors).await?;

    // A source file owns the layers it lists. Fixed-capacity trailing layers
    // remain untouched rather than being destructively cleared.
    for layer in 0..u8::try_from(desired.layers.len()).context("too many configured layers")? {
        for offset in 0..LAYER_SIZE {
            let wanted = desired
                .layers
                .get(usize::from(layer))
                .map_or(0, |keys| keys[offset]);
            let present = before
                .layers
                .get(usize::from(layer))
                .map_or(0, |keys| keys[offset]);
            if wanted != present {
                let row = offset as u8 / COLS;
                let col = offset as u8 % COLS;
                client
                    .set_key(
                        layer,
                        row,
                        col,
                        crate::rynk_keycode::from_via_keycode(wanted),
                    )
                    .await
                    .with_context(|| format!("writing layer {layer} r{row},c{col}"))?;
            }
        }
    }
    if desired.default_layer != before.default_layer {
        client.set_default_layer(desired.default_layer).await?;
    }

    if let Some(wanted) = &desired.lighting {
        let present = before
            .lighting
            .as_ref()
            .context("device has no lighting state")?;
        if wanted.output_mode != present.output_mode {
            let revision = client.get_lighting_state().await?.revision;
            client
                .set_lighting_output_mode(SetLightingOutputModeRequest {
                    expected_revision: revision,
                    mode: output_mode_to_wire(wanted.output_mode),
                })
                .await?;
        }
        if wanted.brightness != present.brightness || wanted.background != present.background {
            let state = client.get_lighting_state().await?;
            client
                .set_lighting_state(SetLightingStateRequest {
                    expected_revision: state.revision,
                    state: LightingMutableState {
                        output_enabled: state.output_enabled,
                        output_brightness: wanted.brightness,
                        background: background_to_wire(&wanted.background),
                    },
                })
                .await?;
        }
        let selection_differs = wanted.effects.as_ref().map(EffectsConfig::selection)
            != present.effects.as_ref().map(EffectsConfig::selection);
        if selection_differs {
            let wanted = wanted
                .effects
                .as_ref()
                .context("cannot remove a firmware-provided effects extension")?;
            let effect_names =
                read_extension_names(client, LightingExtensionNameKind::Effects).await?;
            let palette_names =
                read_extension_names(client, LightingExtensionNameKind::Palettes).await?;
            let (state, overlay) = effects_to_wire(wanted, &effect_names, &palette_names)?;
            let revision = client.get_lighting_state().await?.revision;
            client
                .set_lighting_extension_state(SetLightingExtensionStateRequest {
                    expected_revision: revision,
                    state,
                })
                .await?;
            if wanted.overlay.is_some()
                || present
                    .effects
                    .as_ref()
                    .and_then(|effects| effects.overlay.as_ref())
                    .is_some()
            {
                let revision = client.get_lighting_state().await?.revision;
                client
                    .set_lighting_extension_layers(SetLightingExtensionLayersRequest {
                        expected_revision: revision,
                        overlay,
                    })
                    .await?;
            }
        }
        if let Some(effects) = wanted.effects.as_ref().filter(|it| !it.params.is_empty()) {
            apply_params(client, &effects.params, present.params.as_deref()).await?;
        }
        if wanted.scene_policy != present.scene_policy {
            let status = client.get_lighting_scene_status().await?;
            client
                .set_lighting_layer_policy(SetLightingLayerPolicyRequest {
                    expected_revision: status.revision,
                    policy: scene_policy_to_wire(wanted.scene_policy),
                })
                .await?;
        }
        // Order carries meaning here, so this compares and writes the table as
        // a sequence rather than as a set of addressable cells.
        if let Some(wanted_conditional) = wanted.conditional_scenes.as_ref() {
            match present.conditional_scenes.as_ref() {
                None if wanted_conditional.is_empty() => {}
                None => bail!(
                    "file configures {} conditional lighting rule(s) but the keyboard does not expose a runtime conditional table",
                    wanted_conditional.len()
                ),
                Some(live) if live == wanted_conditional => {}
                Some(_) => {
                    let cells = wanted_conditional
                        .iter()
                        .map(conditional_scene_to_wire)
                        .collect::<Result<Vec<_>>>()?;
                    let status = client.get_lighting_runtime_conditional_scene_status().await?;
                    let extended_conditionals = client
                        .get_lighting_capabilities()
                        .await?
                        .features
                        .contains(LightingFeatureFlags::RUNTIME_EFFECTS_CONDITIONS);
                    if extended_conditionals {
                        client
                            .replace_all_lighting_extended_runtime_conditional_scenes(
                                status.revision,
                                &cells,
                            )
                            .await?;
                    } else {
                        if let Some(gated) = wanted_conditional
                            .iter()
                            .position(|c| c.connection.is_some() || c.effects.is_some())
                        {
                            bail!(
                                "conditional rule {gated} names a connection or effects condition but the keyboard's firmware predates the extended conditional cell"
                            );
                        }
                        let legacy = cells.into_iter().map(|c| c.cell).collect::<Vec<_>>();
                        client
                            .replace_all_lighting_runtime_conditional_scenes(status.revision, &legacy)
                            .await?;
                    }
                }
            }
        }
        if wanted.scenes != present.scenes {
            let state = client.get_lighting_state().await?;
            let cells = wanted
                .scenes
                .iter()
                .map(scene_to_wire)
                .collect::<Result<Vec<_>>>()?;
            client
                .replace_all_lighting_scenes(state.revision, &cells)
                .await?;
        }
    }
    Ok(())
}

/// Write the parameters a file lists, resolving names against what the
/// keyboard advertises. Parameters that already hold the wanted value are left
/// alone, and parameters the file does not mention are never touched.
async fn apply_params(
    client: &Client,
    wanted: &BTreeMap<String, BTreeMap<String, u8>>,
    advertised: Option<&[EffectParams]>,
) -> Result<()> {
    for write in params_to_writes(wanted, advertised)? {
        if write.value == write.current {
            continue;
        }
        let revision = client.get_lighting_state().await?.revision;
        client
            .set_lighting_extension_param(SetLightingExtensionParamRequest {
                expected_revision: revision,
                effect: write.effect,
                index: write.index,
                value: write.value,
            })
            .await
            .with_context(|| format!("writing parameter '{}'", write.label))?;
    }
    Ok(())
}

fn print_diff(desired: &Snapshot, live: &Snapshot) -> bool {
    let differences = differences(desired, live);
    if differences.is_empty() {
        println!("keyboard matches configuration");
        true
    } else {
        for difference in &differences {
            println!("{difference}");
        }
        println!("{} difference(s)", differences.len());
        false
    }
}
