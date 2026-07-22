//! Bidirectional TOML representation of managed Rynk runtime state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use rynk::rmk_types::action::KeyAction;
use rynk::rmk_types::protocol::rynk::{
    LightingBackgroundMode, LightingBackgroundState, LightingEffect, LightingExtensionNameKind,
    LightingExtensionState, LightingFeatureFlags, LightingLayerPolicy, LightingLedId,
    LightingMutableState, LightingOutputMode, LightingRgb8, LightingSceneCell,
    SetLightingExtensionStateRequest, SetLightingLayerPolicyRequest, SetLightingOutputModeRequest,
    SetLightingStateRequest,
};
use rynk::Client;
use serde::{Deserialize, Serialize};

use crate::transport::Selector;

const ROWS: u8 = 6;
const COLS: u8 = 14;
const LAYER_SIZE: usize = ROWS as usize * COLS as usize;
const HOLES: [usize; 4] = [5, 8, 75, 78];

#[derive(Debug)]
pub struct DiffFound;

impl std::fmt::Display for DiffFound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("keyboard configuration differs")
    }
}

impl std::error::Error for DiffFound {}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Validate a runtime TOML file without connecting to a keyboard.
    Validate { file: PathBuf },
    /// Compare a runtime TOML file with the connected keyboard.
    Diff { file: PathBuf },
    /// Apply a runtime TOML file and verify it by reading the keyboard back.
    Apply {
        file: PathBuf,
        /// Show differences without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pull the connected keyboard's runtime state into a TOML file.
    Pull { file: PathBuf },
    /// Print the connected keyboard's runtime state as TOML.
    Show,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub default_layer: u8,
    #[serde(default, rename = "layer")]
    pub layers: Vec<LayerConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<LightingConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerConfig {
    pub id: String,
    pub name: String,
    pub keys: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LightingConfig {
    pub brightness: u8,
    pub output_mode: OutputModeConfig,
    pub scene_policy: ScenePolicyConfig,
    pub background: BackgroundConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<EffectsConfig>,
    #[serde(default, rename = "scene")]
    pub scenes: Vec<SceneConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputModeConfig {
    AlwaysOn,
    AlwaysOff,
    PoweredOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScenePolicyConfig {
    EffectiveOnly,
    ActiveStack,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundModeConfig {
    Solid,
    Breathe,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub hue: u8,
    pub saturation: u8,
    pub value: u8,
    pub speed: u8,
    pub mode: BackgroundModeConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EffectsConfig {
    pub effect: String,
    pub palette: String,
    pub value: u8,
    pub speed: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum EffectKind {
    Solid,
    Blink,
    Breathe,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneConfig {
    pub layer: u8,
    pub led: u16,
    pub color: String,
    #[serde(default = "solid")]
    pub effect: EffectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duty: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_ms: Option<u16>,
}

const fn solid() -> EffectKind {
    EffectKind::Solid
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    default_layer: u8,
    layers: Vec<Vec<u16>>,
    lighting: Option<LightingSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LightingSnapshot {
    brightness: u8,
    output_mode: OutputModeConfig,
    scene_policy: ScenePolicyConfig,
    background: BackgroundConfig,
    effects: Option<EffectsConfig>,
    scenes: Vec<SceneConfig>,
}

impl RuntimeConfig {
    fn parse(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;
        config.snapshot().map(|_| config)
    }

    fn snapshot(&self) -> Result<Snapshot> {
        if self.layers.is_empty() {
            bail!("configuration must contain at least one [[layer]]");
        }
        let mut ids = BTreeMap::new();
        let mut layers = Vec::with_capacity(self.layers.len());
        for (index, layer) in self.layers.iter().enumerate() {
            if layer.id.trim().is_empty() || layer.name.trim().is_empty() {
                bail!("layer {index} must have non-empty id and name");
            }
            if ids.insert(&layer.id, index).is_some() {
                bail!("duplicate layer id '{}'", layer.id);
            }
            layers.push(
                parse_keys(&layer.keys)
                    .with_context(|| format!("layer {} ({})", index, layer.id))?,
            );
        }
        if usize::from(self.default_layer) >= layers.len() {
            bail!(
                "default_layer {} is outside the {} configured layers",
                self.default_layer,
                layers.len()
            );
        }
        let lighting = self
            .lighting
            .as_ref()
            .map(LightingConfig::snapshot)
            .transpose()?;
        Ok(Snapshot {
            default_layer: self.default_layer,
            layers,
            lighting,
        })
    }

    fn from_snapshot(snapshot: &Snapshot, labels: Option<&RuntimeConfig>) -> Self {
        let layers = snapshot
            .layers
            .iter()
            .enumerate()
            .map(|(index, keys)| {
                let old = labels.and_then(|config| config.layers.get(index));
                LayerConfig {
                    id: old.map_or_else(|| format!("layer{index}"), |layer| layer.id.clone()),
                    name: old.map_or_else(|| format!("Layer {index}"), |layer| layer.name.clone()),
                    keys: render_keys(keys),
                }
            })
            .collect();
        Self {
            default_layer: snapshot.default_layer,
            layers,
            lighting: snapshot
                .lighting
                .as_ref()
                .map(LightingConfig::from_snapshot),
        }
    }

    fn to_toml(&self) -> Result<String> {
        let mut text =
            toml::to_string_pretty(self).context("could not serialize runtime configuration")?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(text)
    }
}

impl LightingConfig {
    fn snapshot(&self) -> Result<LightingSnapshot> {
        let mut scenes = self.scenes.clone();
        for cell in &mut scenes {
            cell.color = normalize_color(&cell.color)?;
            validate_scene(cell)?;
        }
        scenes.sort();
        let duplicate = scenes
            .windows(2)
            .find(|pair| pair[0].layer == pair[1].layer && pair[0].led == pair[1].led);
        if let Some(pair) = duplicate {
            bail!(
                "duplicate scene cell for layer {} LED {}",
                pair[0].layer,
                pair[0].led
            );
        }
        Ok(LightingSnapshot {
            brightness: self.brightness,
            output_mode: self.output_mode,
            scene_policy: self.scene_policy,
            background: self.background.clone(),
            effects: self.effects.clone(),
            scenes,
        })
    }

    fn from_snapshot(snapshot: &LightingSnapshot) -> Self {
        Self {
            brightness: snapshot.brightness,
            output_mode: snapshot.output_mode,
            scene_policy: snapshot.scene_policy,
            background: snapshot.background.clone(),
            effects: snapshot.effects.clone(),
            scenes: snapshot.scenes.clone(),
        }
    }
}

pub fn run(selector: &Selector, command: &ConfigCommand) -> Result<()> {
    if let ConfigCommand::Validate { file } = command {
        RuntimeConfig::parse(file)?;
        println!("{} is valid", file.display());
        return Ok(());
    }
    crate::rynk_client::run_config(selector, command)
}

pub async fn operate(client: &Client, command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Validate { .. } => unreachable!("validate is offline"),
        ConfigCommand::Show => {
            let snapshot = read_snapshot(client).await?;
            print!(
                "{}",
                RuntimeConfig::from_snapshot(&snapshot, None).to_toml()?
            );
        }
        ConfigCommand::Pull { file } => {
            let snapshot = read_snapshot(client).await?;
            let labels = RuntimeConfig::parse(file).ok();
            let text = RuntimeConfig::from_snapshot(&snapshot, labels.as_ref()).to_toml()?;
            std::fs::write(file, text)
                .with_context(|| format!("could not write {}", file.display()))?;
            println!("pulled live runtime configuration into {}", file.display());
        }
        ConfigCommand::Diff { file } => {
            let desired = RuntimeConfig::parse(file)?.snapshot()?;
            let live = read_snapshot(client).await?;
            if !print_diff(&desired, &live) {
                return Err(DiffFound.into());
            }
        }
        ConfigCommand::Apply { file, dry_run } => {
            let desired = RuntimeConfig::parse(file)?.snapshot()?;
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
    let actions = if capabilities.bulk_transfer_supported {
        client.read_all_keymap().await?
    } else {
        let mut actions = Vec::new();
        for layer in 0..capabilities.num_layers {
            for row in 0..ROWS {
                for col in 0..COLS {
                    actions.push(client.get_key(layer, row, col).await?);
                }
            }
        }
        actions
    };
    let mut layers = actions
        .chunks(LAYER_SIZE)
        .enumerate()
        .map(|(layer, actions)| {
            actions
                .iter()
                .copied()
                .enumerate()
                .map(|(offset, action)| action_to_code(action, layer, offset))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    // RMK initializes unused layers as transparent. Omit trailing layers that
    // contain only No/Transparent so a five-layer source file can round-trip
    // against firmware whose fixed capacity is eight layers.
    while layers.len() > 1
        && layers
            .last()
            .is_some_and(|layer| layer.iter().all(|code| matches!(*code, 0 | 1)))
    {
        layers.pop();
    }

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
    let mut scenes = scene_cells
        .into_iter()
        .map(scene_from_wire)
        .collect::<Vec<_>>();
    scenes.sort();
    let effects = if lighting_caps
        .features
        .contains(LightingFeatureFlags::EXTENSION_EFFECTS)
    {
        let extension = client.get_lighting_extension().await?;
        let effect_names = client
            .read_all_lighting_extension_names(LightingExtensionNameKind::Effects)
            .await?;
        let palette_names = client
            .read_all_lighting_extension_names(LightingExtensionNameKind::Palettes)
            .await?;
        let effect = effect_names
            .get(usize::from(extension.state.effect))
            .context("extension effect index is outside its advertised name list")?
            .to_string();
        let palette = palette_names
            .get(usize::from(extension.state.palette))
            .context("extension palette index is outside its advertised name list")?
            .to_string();
        Some(EffectsConfig {
            effect,
            palette,
            value: extension.state.value,
            speed: extension.state.speed,
        })
    } else {
        None
    };
    Ok(Snapshot {
        default_layer: client.get_default_layer().await?,
        layers,
        lighting: Some(LightingSnapshot {
            brightness: state.output_brightness,
            output_mode,
            scene_policy: scene_policy_from_wire(scene_status.policy),
            background: background_from_wire(state.background),
            effects,
            scenes,
        }),
    })
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
        if wanted.effects != present.effects {
            let wanted = wanted
                .effects
                .as_ref()
                .context("cannot remove a firmware-provided effects extension")?;
            let effect_names = client
                .read_all_lighting_extension_names(LightingExtensionNameKind::Effects)
                .await?;
            let palette_names = client
                .read_all_lighting_extension_names(LightingExtensionNameKind::Palettes)
                .await?;
            let effect = effect_names
                .iter()
                .position(|name| name.as_str() == wanted.effect)
                .with_context(|| format!("unknown extension effect '{}'", wanted.effect))?;
            let palette = palette_names
                .iter()
                .position(|name| name.as_str() == wanted.palette)
                .with_context(|| format!("unknown extension palette '{}'", wanted.palette))?;
            let revision = client.get_lighting_state().await?.revision;
            client
                .set_lighting_extension_state(SetLightingExtensionStateRequest {
                    expected_revision: revision,
                    state: LightingExtensionState {
                        effect: u8::try_from(effect).context("effect index exceeds u8")?,
                        palette: u8::try_from(palette).context("palette index exceeds u8")?,
                        value: wanted.value,
                        speed: wanted.speed,
                    },
                })
                .await?;
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

fn differences(desired: &Snapshot, live: &Snapshot) -> Vec<String> {
    let mut result = Vec::new();
    if desired.default_layer != live.default_layer {
        result.push(format!(
            "default layer: file {} != keyboard {}",
            desired.default_layer, live.default_layer
        ));
    }
    for layer in 0..desired.layers.len() {
        for offset in 0..LAYER_SIZE {
            let wanted = desired.layers.get(layer).map_or(0, |keys| keys[offset]);
            let present = live.layers.get(layer).map_or(0, |keys| keys[offset]);
            if wanted != present {
                result.push(format!(
                    "layer {layer} r{},c{}: file {} != keyboard {}",
                    offset / usize::from(COLS),
                    offset % usize::from(COLS),
                    crate::keycodes::format_keycode(wanted),
                    crate::keycodes::format_keycode(present),
                ));
            }
        }
    }
    match (&desired.lighting, &live.lighting) {
        (Some(wanted), Some(present)) => {
            if wanted.brightness != present.brightness {
                result.push(format!(
                    "lighting brightness: file {} != keyboard {}",
                    wanted.brightness, present.brightness
                ));
            }
            if wanted.output_mode != present.output_mode {
                result.push(format!(
                    "lighting output mode: file {:?} != keyboard {:?}",
                    wanted.output_mode, present.output_mode
                ));
            }
            if wanted.scene_policy != present.scene_policy {
                result.push(format!(
                    "lighting scene policy: file {:?} != keyboard {:?}",
                    wanted.scene_policy, present.scene_policy
                ));
            }
            if wanted.background != present.background {
                result.push("lighting background differs".into());
            }
            if wanted.effects != present.effects {
                result.push(format!(
                    "effects state: file {:?} != keyboard {:?}",
                    wanted.effects, present.effects
                ));
            }
            let wanted_cells = wanted
                .scenes
                .iter()
                .map(|cell| ((cell.layer, cell.led), cell))
                .collect::<BTreeMap<_, _>>();
            let present_cells = present
                .scenes
                .iter()
                .map(|cell| ((cell.layer, cell.led), cell))
                .collect::<BTreeMap<_, _>>();
            for key in wanted_cells.keys().chain(present_cells.keys()) {
                if wanted_cells.get(key) != present_cells.get(key) {
                    result.push(format!(
                        "lighting scene layer {} LED {}: file {:?} != keyboard {:?}",
                        key.0,
                        key.1,
                        wanted_cells.get(key),
                        present_cells.get(key),
                    ));
                }
            }
            result.sort();
            result.dedup();
        }
        (Some(_), None) => result.push("file configures lighting but keyboard exposes none".into()),
        (None, _) => {}
    }
    result
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

fn parse_keys(text: &str) -> Result<Vec<u16>> {
    let rows = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if rows.len() != usize::from(ROWS) {
        bail!(
            "keys must contain {ROWS} non-empty rows, found {}",
            rows.len()
        );
    }
    let mut result = Vec::with_capacity(LAYER_SIZE);
    for (row, line) in rows.iter().enumerate() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.len() != usize::from(COLS) {
            bail!("row {row} must contain {COLS} keys, found {}", tokens.len());
        }
        for token in tokens {
            result.push(if token == "--" {
                0
            } else {
                crate::keycodes::parse_keycode(token)?
            });
        }
    }
    for hole in HOLES {
        if result[hole] != 0 {
            bail!(
                "physical hole r{},c{} must be --",
                hole / usize::from(COLS),
                hole % usize::from(COLS)
            );
        }
    }
    Ok(result)
}

fn render_keys(keys: &[u16]) -> String {
    let mut text = String::from("\n");
    for row in 0..usize::from(ROWS) {
        for col in 0..usize::from(COLS) {
            if col > 0 {
                text.push(' ');
            }
            let offset = row * usize::from(COLS) + col;
            if keys[offset] == 0 {
                text.push_str("--");
            } else {
                // The grid format is whitespace-delimited, so keep composite
                // keycodes as a single token even when the human formatter
                // normally inserts a space after a comma.
                text.push_str(&crate::keycodes::format_keycode(keys[offset]).replace(", ", ","));
            }
        }
        text.push('\n');
    }
    text
}

fn action_to_code(action: KeyAction, layer: usize, offset: usize) -> Result<u16> {
    let code = crate::rynk_keycode::to_via_keycode(action);
    if code == 0 && !matches!(action, KeyAction::No) {
        bail!(
            "action {action:?} at layer {layer} r{},c{} cannot be represented in runtime TOML",
            offset / usize::from(COLS),
            offset % usize::from(COLS)
        );
    }
    Ok(code)
}

fn normalize_color(text: &str) -> Result<String> {
    let (r, g, b) = crate::lighting::parse_color(text)?;
    Ok(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn validate_scene(cell: &SceneConfig) -> Result<()> {
    match cell.effect {
        EffectKind::Solid => {
            if cell.period_ms.is_some()
                || cell.phase_ms.is_some()
                || cell.duty.is_some()
                || cell.step_ms.is_some()
            {
                bail!(
                    "solid scene layer {} LED {} has timing options",
                    cell.layer,
                    cell.led
                );
            }
        }
        EffectKind::Blink => {
            if cell.period_ms.unwrap_or(0) == 0
                || cell.duty.unwrap_or(101) > 100
                || cell.step_ms.is_some()
            {
                bail!(
                    "invalid blink scene at layer {} LED {}",
                    cell.layer,
                    cell.led
                );
            }
        }
        EffectKind::Breathe => {
            if cell.period_ms.unwrap_or(0) < 2
                || cell.step_ms.unwrap_or(0) == 0
                || cell.duty.is_some()
            {
                bail!(
                    "invalid breathe scene at layer {} LED {}",
                    cell.layer,
                    cell.led
                );
            }
        }
    }
    Ok(())
}

fn scene_from_wire(cell: LightingSceneCell) -> SceneConfig {
    let (color, effect, period_ms, phase_ms, duty, step_ms) = match cell.effect {
        LightingEffect::Solid { color } => (color, EffectKind::Solid, None, None, None, None),
        LightingEffect::Blink {
            color,
            period_ms,
            phase_ms,
            duty,
        } => (
            color,
            EffectKind::Blink,
            Some(period_ms),
            Some(phase_ms),
            Some(duty),
            None,
        ),
        LightingEffect::Breathe {
            color,
            period_ms,
            phase_ms,
            step_ms,
        } => (
            color,
            EffectKind::Breathe,
            Some(period_ms),
            Some(phase_ms),
            None,
            Some(step_ms),
        ),
    };
    SceneConfig {
        layer: cell.layer,
        led: cell.led_id.0,
        color: format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
        effect,
        period_ms,
        phase_ms,
        duty,
        step_ms,
    }
}

fn scene_to_wire(cell: &SceneConfig) -> Result<LightingSceneCell> {
    let (r, g, b) = crate::lighting::parse_color(&cell.color)?;
    let color = LightingRgb8 { r, g, b };
    let effect = match cell.effect {
        EffectKind::Solid => LightingEffect::Solid { color },
        EffectKind::Blink => LightingEffect::Blink {
            color,
            period_ms: cell.period_ms.context("blink period_ms is required")?,
            phase_ms: cell.phase_ms.unwrap_or(0),
            duty: cell.duty.context("blink duty is required")?,
        },
        EffectKind::Breathe => LightingEffect::Breathe {
            color,
            period_ms: cell.period_ms.context("breathe period_ms is required")?,
            phase_ms: cell.phase_ms.unwrap_or(0),
            step_ms: cell.step_ms.context("breathe step_ms is required")?,
        },
    };
    Ok(LightingSceneCell {
        layer: cell.layer,
        led_id: LightingLedId(cell.led),
        effect,
    })
}

fn background_from_wire(state: LightingBackgroundState) -> BackgroundConfig {
    BackgroundConfig {
        enabled: state.enabled,
        hue: state.hue,
        saturation: state.saturation,
        value: state.value,
        speed: state.speed,
        mode: match state.mode {
            LightingBackgroundMode::Solid => BackgroundModeConfig::Solid,
            LightingBackgroundMode::Breathe => BackgroundModeConfig::Breathe,
        },
    }
}

fn background_to_wire(state: &BackgroundConfig) -> LightingBackgroundState {
    LightingBackgroundState {
        enabled: state.enabled,
        hue: state.hue,
        saturation: state.saturation,
        value: state.value,
        speed: state.speed,
        mode: match state.mode {
            BackgroundModeConfig::Solid => LightingBackgroundMode::Solid,
            BackgroundModeConfig::Breathe => LightingBackgroundMode::Breathe,
        },
    }
}

fn output_mode_from_wire(mode: LightingOutputMode) -> OutputModeConfig {
    match mode {
        LightingOutputMode::AlwaysOn => OutputModeConfig::AlwaysOn,
        LightingOutputMode::AlwaysOff => OutputModeConfig::AlwaysOff,
        LightingOutputMode::PoweredOnly => OutputModeConfig::PoweredOnly,
    }
}

fn output_mode_to_wire(mode: OutputModeConfig) -> LightingOutputMode {
    match mode {
        OutputModeConfig::AlwaysOn => LightingOutputMode::AlwaysOn,
        OutputModeConfig::AlwaysOff => LightingOutputMode::AlwaysOff,
        OutputModeConfig::PoweredOnly => LightingOutputMode::PoweredOnly,
    }
}

fn scene_policy_from_wire(policy: LightingLayerPolicy) -> ScenePolicyConfig {
    match policy {
        LightingLayerPolicy::EffectiveOnly => ScenePolicyConfig::EffectiveOnly,
        LightingLayerPolicy::ActiveStack => ScenePolicyConfig::ActiveStack,
    }
}

fn scene_policy_to_wire(policy: ScenePolicyConfig) -> LightingLayerPolicy {
    match policy {
        ScenePolicyConfig::EffectiveOnly => LightingLayerPolicy::EffectiveOnly,
        ScenePolicyConfig::ActiveStack => LightingLayerPolicy::ActiveStack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_style_keymap_round_trips() {
        let keys = "\n-- -- KC_A KC_TRNS LT(1,KC_ESC) -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n";
        let parsed = parse_keys(keys).unwrap();
        assert_eq!(parsed[2], 0x0004);
        assert_eq!(parsed[3], 0x0001);
        assert_eq!(parsed[4], 0x4129);
        assert_eq!(parse_keys(&render_keys(&parsed)).unwrap(), parsed);
    }

    #[test]
    fn scene_colors_are_canonicalized() {
        assert_eq!(normalize_color("C000C0").unwrap(), "#c000c0");
    }
}
