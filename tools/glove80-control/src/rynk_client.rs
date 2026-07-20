//! Rynk-backed keymap, lighting, and bootloader operations.
//!
//! The transactional Glove80 configuration format remains a legacy path, but
//! live keymap and lighting state each have one owner: Rynk.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use embassy_futures::select::{select, Either};
use rynk::rmk_types::action::KeyAction;
use rynk::rmk_types::protocol::rynk::{
    AbortLightingOverlayReplaceRequest, BeginLightingOverlayReplaceRequest,
    ClearLightingOverlayRequest, CommitLightingOverlayReplaceRequest, LightingBackgroundMode,
    LightingEffect, LightingEffectFlags, LightingFeatureFlags, LightingLedId, LightingMutableState,
    LightingOverlayCell, LightingRgb8, LightingState, PutLightingOverlayChunkRequest,
    SetKeymapBulkRequest, SetLightingOverlayRequest, SetLightingStateRequest,
    UnsetLightingOverlayRequest,
};
use rynk::{Client, RynkDevice};
use rynk_ble::BleDevice;
use rynk_serial::SerialDevice;

use crate::keymap::{self, KeymapCommand};
use crate::keymapcfg::{KeymapReport, KeymapStage, LayerPlan};
use crate::lighting::{EffectArg, LightingCommand};
use crate::rynk_hid::HidDevice;
use crate::transport::{Preference, Selector};

const GLOVE80_ROWS: u8 = 6;
const GLOVE80_COLS: u8 = 14;
const GLOVE80_HOLES: [u8; 4] = [5, 8, 75, 78];
const RYNK_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

enum Device {
    Hid(HidDevice),
    Serial(SerialDevice),
    Ble(BleDevice),
}

pub fn run_keymap(selector: &Selector, command: &KeymapCommand) -> Result<()> {
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => run_device(device, command).await,
            Device::Serial(device) => run_device(device, command).await,
            Device::Ble(device) => run_device(device, command).await,
        }
    })
}

/// Run topology-aware lighting operations over the same Rynk session used by
/// keymap control. Every mutation uses the authoritative revision returned by
/// the preceding read or write.
pub fn run_lighting(selector: &Selector, command: &LightingCommand) -> Result<()> {
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => run_lighting_device(device, command).await,
            Device::Serial(device) => run_lighting_device(device, command).await,
            Device::Ble(device) => run_lighting_device(device, command).await,
        }
    })
}

/// Query the distinct protocol, application-build, and RMK identities over
/// one Rynk session.
pub fn run_version(selector: &Selector) -> Result<()> {
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => run_version_device(device).await,
            Device::Serial(device) => run_version_device(device).await,
            Device::Ble(device) => run_version_device(device).await,
        }
    })
}

/// Enter the central bootloader through Rynk. The right-half request remains
/// a board-specific split action and is currently only available from the
/// keyboard's physical bootloader binding.
pub fn run_bootloader(selector: &Selector, peripheral: bool) -> Result<()> {
    if peripheral {
        bail!(
            "Rynk bootloader entry targets the central half; use the right-half physical bootloader key for the peripheral"
        );
    }
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => run_bootloader_device(device).await,
            Device::Serial(device) => run_bootloader_device(device).await,
            Device::Ble(device) => run_bootloader_device(device).await,
        }
    })
}

async fn run_lighting_device<D: RynkDevice>(device: D, command: &LightingCommand) -> Result<()> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), operate_lighting(&client, command)).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result,
    }
}

async fn run_bootloader_device<D: RynkDevice>(device: D) -> Result<()> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), client.bootloader_jump()).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result.map_err(Into::into),
    }
}

async fn run_version_device<D: RynkDevice>(device: D) -> Result<()> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), read_version(&client)).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result,
    }
}

async fn read_version(client: &Client) -> Result<()> {
    let protocol = client.get_version().await?;
    let device = client.get_device_info().await?;
    let build = client.get_build_info().await?;
    print!("{}", crate::version::render(protocol, &device, &build));
    Ok(())
}

async fn operate_lighting(client: &Client, command: &LightingCommand) -> Result<()> {
    match command {
        LightingCommand::Ping { data } => {
            if data.is_some() {
                bail!("Rynk does not accept ping payloads; omit --data");
            }
            let started = std::time::Instant::now();
            let version = client.get_version().await?;
            println!(
                "Rynk v{}.{} round trip: {:.1} ms",
                version.major,
                version.minor,
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        LightingCommand::Caps => {
            let caps = client.get_lighting_capabilities().await?;
            let effects = [
                (LightingEffectFlags::SOLID, "solid"),
                (LightingEffectFlags::BLINK, "blink"),
                (LightingEffectFlags::BREATHE, "breathe"),
            ]
            .into_iter()
            .filter_map(|(bit, name)| caps.effects.contains(bit).then_some(name))
            .collect::<Vec<_>>()
            .join(", ");
            let features = [
                (LightingFeatureFlags::OVERLAY_TTL, "overlay TTL"),
                (
                    LightingFeatureFlags::ATOMIC_OVERLAY_REPLACE,
                    "atomic replace",
                ),
                (LightingFeatureFlags::LAYER_AWARE, "layer-aware"),
                (LightingFeatureFlags::PHYSICAL_GEOMETRY, "physical geometry"),
                (LightingFeatureFlags::ZONES, "zones"),
                (LightingFeatureFlags::ROUTING, "routing"),
            ]
            .into_iter()
            .filter_map(|(bit, name)| caps.features.contains(bit).then_some(name))
            .collect::<Vec<_>>()
            .join(", ");
            println!(
                "topology revision: {}\nLEDs: {}\nlogical keys: {}\nphysical keys: {}\noutputs: {}\nroutes: {}\noverlay capacity: {}\neffects: {}\nfeatures: {}",
                caps.topology_revision,
                caps.led_count,
                caps.logical_key_count,
                caps.physical_key_count,
                caps.output_count,
                caps.route_count,
                caps.overlay_capacity,
                effects,
                features,
            );
        }
        LightingCommand::Set {
            keys,
            color,
            effect,
            period,
            phase,
            duty,
            ttl,
        } => {
            let keys = crate::lighting::parse_key_list(keys)?;
            let effect = rynk_effect(
                *effect,
                crate::lighting::parse_color(color)?,
                *period,
                *phase,
                *duty,
            )?;
            let ttl_ms = ttl.map(positive_ttl).transpose()?;
            let mut state = client.get_lighting_state().await?;
            for key in &keys {
                state = client
                    .set_lighting_overlay(SetLightingOverlayRequest {
                        expected_revision: state.revision,
                        cell: LightingOverlayCell {
                            led_id: LightingLedId(u16::from(*key)),
                            effect,
                            ttl_ms,
                        },
                    })
                    .await?;
            }
            println!(
                "set {} LED(s); {}",
                keys.len(),
                render_lighting_state(state)
            );
        }
        LightingCommand::Unset { keys } => {
            let mut ids = Vec::new();
            for list in keys {
                ids.extend(crate::lighting::parse_key_list(list)?);
            }
            let mut state = client.get_lighting_state().await?;
            for id in &ids {
                state = client
                    .unset_lighting_overlay(UnsetLightingOverlayRequest {
                        expected_revision: state.revision,
                        led_id: LightingLedId(u16::from(*id)),
                    })
                    .await?;
            }
            println!(
                "unset {} LED(s); {}",
                ids.len(),
                render_lighting_state(state)
            );
        }
        LightingCommand::Clear => {
            let state = client.get_lighting_state().await?;
            let state = client
                .clear_lighting_overlay(ClearLightingOverlayRequest {
                    expected_revision: state.revision,
                })
                .await?;
            println!("cleared overlay; {}", render_lighting_state(state));
        }
        LightingCommand::Read => {
            println!(
                "{}",
                render_lighting_state(client.get_lighting_state().await?)
            );
        }
        LightingCommand::Replace { file, ttl } => {
            let spec = match file.as_deref() {
                None => read_stdin()?,
                Some(path) if path.as_os_str() == "-" => read_stdin()?,
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("could not read {}", path.display()))?,
            };
            let parsed = crate::lighting::parse_replace_spec(&spec)?;
            let ttl_ms = ttl.map(positive_ttl).transpose()?;
            let state = client.get_lighting_state().await?;
            let transaction = client
                .begin_lighting_overlay_replace(BeginLightingOverlayReplaceRequest {
                    expected_revision: state.revision,
                    cell_count: u16::try_from(parsed.len()).context("too many overlay cells")?,
                })
                .await?;
            let staged = async {
                for (chunk_index, cells) in parsed.chunks(8).enumerate() {
                    let mut request = PutLightingOverlayChunkRequest {
                        transaction_id: transaction.id,
                        offset: u16::try_from(chunk_index * 8).unwrap(),
                        cells: Default::default(),
                    };
                    for cell in cells {
                        request
                            .cells
                            .push(LightingOverlayCell {
                                led_id: LightingLedId(u16::from(cell.key)),
                                effect: legacy_effect(cell.effect),
                                ttl_ms,
                            })
                            .map_err(|_| anyhow!("lighting transaction chunk overflow"))?;
                    }
                    client.put_lighting_overlay_chunk(request).await?;
                }
                client
                    .commit_lighting_overlay_replace(CommitLightingOverlayReplaceRequest {
                        transaction_id: transaction.id,
                    })
                    .await
                    .map_err(anyhow::Error::from)
            }
            .await;
            match staged {
                Ok(state) => println!(
                    "replaced overlay with {} LED(s); {}",
                    parsed.len(),
                    render_lighting_state(state)
                ),
                Err(error) => {
                    let _ = client
                        .abort_lighting_overlay_replace(AbortLightingOverlayReplaceRequest {
                            transaction_id: transaction.id,
                        })
                        .await;
                    return Err(error);
                }
            }
        }
        LightingCommand::Brightness { value } => {
            let state = client.get_lighting_state().await?;
            let state = if let Some(value) = value {
                client
                    .set_lighting_state(SetLightingStateRequest {
                        expected_revision: state.revision,
                        state: LightingMutableState {
                            output_enabled: state.output_enabled,
                            output_brightness: *value,
                            background: state.background,
                        },
                    })
                    .await?
            } else {
                state
            };
            println!(
                "brightness: {}; revision: {}",
                state.output_brightness, state.revision
            );
        }
        LightingCommand::Toggle { .. } => {
            bail!("named toggle overlays are not part of the RMK lighting model")
        }
    }
    Ok(())
}

fn positive_ttl(value: u32) -> Result<u32> {
    if value == 0 {
        bail!("TTL must be greater than zero; omit --ttl for no expiry");
    }
    Ok(value)
}

fn rynk_effect(
    kind: EffectArg,
    rgb: (u8, u8, u8),
    period: Option<u16>,
    phase: Option<u16>,
    duty: Option<u8>,
) -> Result<LightingEffect> {
    let legacy = crate::lighting::build_effect(kind.kind(), rgb, period, phase, duty)?;
    Ok(legacy_effect(legacy))
}

fn legacy_effect(effect: glove80_host_protocol::Effect) -> LightingEffect {
    let color = LightingRgb8 {
        r: effect.r,
        g: effect.g,
        b: effect.b,
    };
    match effect.kind {
        glove80_host_protocol::EffectKind::Solid => LightingEffect::Solid { color },
        glove80_host_protocol::EffectKind::Blink => LightingEffect::Blink {
            color,
            period_ms: u32::from(effect.period_ms),
            phase_ms: u32::from(effect.phase_ms),
            duty: effect.duty_percent,
        },
        glove80_host_protocol::EffectKind::Breathe => LightingEffect::Breathe {
            color,
            period_ms: u32::from(effect.period_ms),
            phase_ms: u32::from(effect.phase_ms),
            step_ms: 16,
        },
    }
}

fn render_lighting_state(state: LightingState) -> String {
    let background_mode = match state.background.mode {
        LightingBackgroundMode::Solid => "solid",
        LightingBackgroundMode::Breathe => "breathe",
    };
    format!(
        "revision: {}\noutput: {}\nbrightness: {}\noverlay cells: {}\nbackground: {} (mode {}, HSV {},{},{}; speed {})",
        state.revision,
        if state.output_enabled { "on" } else { "off" },
        state.output_brightness,
        state.overlay_len,
        if state.background.enabled { "on" } else { "off" },
        background_mode,
        state.background.hue,
        state.background.saturation,
        state.background.value,
        state.background.speed,
    )
}

fn read_stdin() -> Result<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut text)
        .context("could not read the cell spec from stdin")?;
    Ok(text)
}

/// Read every Rynk keymap layer and convert it to the canonical config's
/// transitional VIA `u16` representation.
pub fn read_all_layers(selector: &Selector) -> Result<Vec<Vec<u16>>> {
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => read_layers_device(device).await,
            Device::Serial(device) => read_layers_device(device).await,
            Device::Ble(device) => read_layers_device(device).await,
        }
    })
}

/// Apply canonical layer plans through Rynk, returning the same report shape
/// the previous Glove80 keymap bridge used.
pub fn apply_plans(
    selector: &Selector,
    plans: &[LayerPlan],
    mut stage: impl FnMut(KeymapStage),
) -> Result<KeymapReport> {
    let runtime =
        tokio::runtime::Runtime::new().context("could not create the Rynk async runtime")?;
    let (report, stages) = runtime.block_on(async {
        match select_device(selector).await? {
            Device::Hid(device) => apply_plans_device(device, plans).await,
            Device::Serial(device) => apply_plans_device(device, plans).await,
            Device::Ble(device) => apply_plans_device(device, plans).await,
        }
    })?;
    for event in stages {
        stage(event);
    }
    Ok(report)
}

async fn run_device<D: RynkDevice>(device: D, command: &KeymapCommand) -> Result<()> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), operate(&client, command)).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result,
    }
}

async fn read_layers_device<D: RynkDevice>(device: D) -> Result<Vec<Vec<u16>>> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), read_layers(&client)).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result,
    }
}

async fn apply_plans_device<D: RynkDevice>(
    device: D,
    plans: &[LayerPlan],
) -> Result<(KeymapReport, Vec<KeymapStage>)> {
    let label = device.label();
    let (client, mut driver) = connect_device(device, &label).await?;
    match select(driver.run(&client), apply(&client, plans)).await {
        Either::First(error) => Err(anyhow!("Rynk connection to {label} ended: {error}")),
        Either::Second(result) => result,
    }
}

async fn connect_device<D: RynkDevice>(
    device: D,
    label: &str,
) -> Result<(Client, rynk::Driver<D::Read, D::Write>)> {
    tokio::time::timeout(RYNK_CONNECT_TIMEOUT, device.connect())
        .await
        .with_context(|| format!("timed out establishing a Rynk session with {label}"))?
        .with_context(|| format!("could not establish a Rynk session with {label}"))
}

async fn operate(client: &Client, command: &KeymapCommand) -> Result<()> {
    let capabilities = client.get_capabilities().await?;
    check_grid(
        capabilities.num_rows,
        capabilities.num_cols,
        capabilities.num_layers,
    )?;

    match command {
        KeymapCommand::Read { layer, all, raw } => {
            let actions = read_all_actions(client, &capabilities).await?;
            let layer_size =
                usize::from(capabilities.num_rows) * usize::from(capabilities.num_cols);
            let layers: Vec<u8> = if *all {
                (0..capabilities.num_layers).collect()
            } else {
                vec![layer.unwrap_or(0)]
            };
            for (index, layer) in layers.into_iter().enumerate() {
                if layer >= capabilities.num_layers {
                    bail!(
                        "layer {layer} is out of range; Rynk reports {} layer(s)",
                        capabilities.num_layers
                    );
                }
                let start = usize::from(layer) * layer_size;
                let codes = actions[start..start + layer_size]
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(offset, action)| action_to_via(action, layer, offset))
                    .collect::<Result<Vec<_>>>()?;
                if index > 0 {
                    println!();
                }
                println!(
                    "{}",
                    keymap::render_layer(
                        layer,
                        &codes,
                        capabilities.num_rows,
                        capabilities.num_cols,
                        &GLOVE80_HOLES,
                        *raw,
                    )
                );
            }
        }
        KeymapCommand::Set { entries } => {
            let parsed =
                keymap::parse_set_entries(entries, capabilities.num_rows, capabilities.num_cols)?;
            for entry in &parsed {
                if entry.layer >= capabilities.num_layers {
                    bail!(
                        "layer {} is out of range; Rynk reports {} layer(s)",
                        entry.layer,
                        capabilities.num_layers
                    );
                }
            }

            let mut readback = Vec::with_capacity(parsed.len());
            for entry in &parsed {
                let row = entry.key / capabilities.num_cols;
                let col = entry.key % capabilities.num_cols;
                client
                    .set_key(
                        entry.layer,
                        row,
                        col,
                        crate::rynk_keycode::from_via_keycode(entry.keycode),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "Rynk could not write layer {} key {} (r{row},c{col})",
                            entry.layer, entry.key
                        )
                    })?;
                let stored = client.get_key(entry.layer, row, col).await?;
                readback.push(crate::rynk_keycode::to_via_keycode(stored));
            }
            println!(
                "{}",
                keymap::render_write_outcome(&parsed, &readback, capabilities.num_cols)
            );
        }
        KeymapCommand::Find { fragment } => println!("{}", keymap::render_find(fragment)),
    }
    Ok(())
}

async fn read_all_actions(
    client: &Client,
    capabilities: &rynk::rmk_types::protocol::rynk::DeviceCapabilities,
) -> Result<Vec<KeyAction>> {
    if capabilities.bulk_transfer_supported {
        return client.read_all_keymap().await.map_err(Into::into);
    }

    let mut actions = Vec::with_capacity(
        usize::from(capabilities.num_layers)
            * usize::from(capabilities.num_rows)
            * usize::from(capabilities.num_cols),
    );
    for layer in 0..capabilities.num_layers {
        for row in 0..capabilities.num_rows {
            for col in 0..capabilities.num_cols {
                actions.push(client.get_key(layer, row, col).await?);
            }
        }
    }
    Ok(actions)
}

async fn read_layers(client: &Client) -> Result<Vec<Vec<u16>>> {
    let capabilities = client.get_capabilities().await?;
    check_grid(
        capabilities.num_rows,
        capabilities.num_cols,
        capabilities.num_layers,
    )?;
    let actions = read_all_actions(client, &capabilities).await?;
    let layer_size = usize::from(capabilities.num_rows) * usize::from(capabilities.num_cols);
    actions
        .chunks(layer_size)
        .enumerate()
        .map(|(layer, actions)| {
            actions
                .iter()
                .copied()
                .enumerate()
                .map(|(offset, action)| action_to_via(action, layer as u8, offset))
                .collect()
        })
        .collect()
}

async fn apply(client: &Client, plans: &[LayerPlan]) -> Result<(KeymapReport, Vec<KeymapStage>)> {
    let capabilities = client.get_capabilities().await?;
    check_grid(
        capabilities.num_rows,
        capabilities.num_cols,
        capabilities.num_layers,
    )?;
    if let Some(plan) = plans
        .iter()
        .find(|plan| plan.slot >= capabilities.num_layers)
    {
        bail!(
            "layer \"{}\" occupies slot {} but Rynk reports only {} layer(s)",
            plan.id,
            plan.slot,
            capabilities.num_layers
        );
    }

    let mut report = KeymapReport::default();
    let mut stages = Vec::new();
    let page_size = if capabilities.bulk_transfer_supported {
        usize::from(capabilities.max_bulk_keys.max(1))
    } else {
        1
    };
    for plan in plans {
        stages.push(KeymapStage::LayerBegun {
            slot: plan.slot,
            id: plan.id.clone(),
        });
        let actions: Vec<KeyAction> = plan
            .codes
            .iter()
            .copied()
            .map(crate::rynk_keycode::from_via_keycode)
            .collect();
        let mut layer_lossy = 0usize;
        for (page_index, page) in actions.chunks(page_size).enumerate() {
            let start = page_index * page_size;
            let row = (start / usize::from(capabilities.num_cols)) as u8;
            let col = (start % usize::from(capabilities.num_cols)) as u8;
            if capabilities.bulk_transfer_supported {
                client
                    .set_keymap_bulk(SetKeymapBulkRequest {
                        layer: plan.slot,
                        start_row: row,
                        start_col: col,
                        actions: page.to_vec(),
                    })
                    .await?;
            } else {
                client.set_key(plan.slot, row, col, page[0]).await?;
            }

            for (offset, requested) in page.iter().copied().enumerate() {
                let flat = start + offset;
                let read_row = (flat / usize::from(capabilities.num_cols)) as u8;
                let read_col = (flat % usize::from(capabilities.num_cols)) as u8;
                let stored = client.get_key(plan.slot, read_row, read_col).await?;
                let requested_code = crate::rynk_keycode::to_via_keycode(requested);
                let stored_code = crate::rynk_keycode::to_via_keycode(stored);
                if requested_code != stored_code {
                    layer_lossy += 1;
                    report
                        .lossy
                        .push((plan.slot, flat as u8, requested_code, stored_code));
                }
            }
            report.entries_written += page.len();
            stages.push(KeymapStage::Batch {
                slot: plan.slot,
                written: (start + page.len()).min(actions.len()),
                total: actions.len(),
            });
        }
        stages.push(KeymapStage::LayerDone {
            slot: plan.slot,
            lossy: layer_lossy,
        });
    }
    Ok((report, stages))
}

fn action_to_via(action: KeyAction, layer: u8, offset: usize) -> Result<u16> {
    let code = crate::rynk_keycode::to_via_keycode(action);
    if code == 0 && !matches!(action, KeyAction::No) {
        let row = offset / usize::from(GLOVE80_COLS);
        let col = offset % usize::from(GLOVE80_COLS);
        bail!(
            "Rynk action {action:?} at layer {layer} r{row},c{col} cannot be represented by the CLI's legacy VIA keycode format"
        );
    }
    Ok(code)
}

fn check_grid(rows: u8, cols: u8, layers: u8) -> Result<()> {
    if rows != GLOVE80_ROWS || cols != GLOVE80_COLS {
        bail!(
            "expected the Glove80 {GLOVE80_ROWS}x{GLOVE80_COLS} keymap, but Rynk reports {rows}x{cols}"
        );
    }
    if layers == 0 {
        bail!("Rynk reports no keymap layers");
    }
    Ok(())
}

async fn select_device(selector: &Selector) -> Result<Device> {
    match selector.preference {
        Preference::Usb => select_usb(selector.device.as_deref()),
        Preference::Ble => select_ble(selector.device.as_deref())
            .await
            .map(Device::Ble),
        Preference::Auto => {
            if selector
                .device
                .as_deref()
                .is_some_and(crate::transport::is_ble_address)
            {
                return select_ble(selector.device.as_deref())
                    .await
                    .map(Device::Ble);
            }
            match select_usb(selector.device.as_deref()) {
                Ok(device) => Ok(device),
                Err(usb_error) => select_ble(selector.device.as_deref())
                    .await
                    .map(Device::Ble)
                    .with_context(|| format!("USB Rynk discovery also failed: {usb_error:#}")),
            }
        }
    }
}

fn select_usb(requested: Option<&str>) -> Result<Device> {
    if requested.is_some_and(crate::transport::is_ble_address) {
        bail!("a BLE address cannot be used with --usb");
    }
    if requested.is_some_and(|path| path.starts_with("/dev/tty")) {
        return select_serial(requested).map(Device::Serial);
    }
    match select_hid(requested) {
        Ok(device) => Ok(Device::Hid(device)),
        Err(hid_error) => select_serial(requested)
            .map(Device::Serial)
            .with_context(|| format!("Rynk USB HID discovery also failed: {hid_error:#}")),
    }
}

fn select_hid(requested: Option<&str>) -> Result<HidDevice> {
    let devices = HidDevice::discover().context("Rynk USB HID discovery failed")?;
    if let Some(path) = requested.filter(|path| path.starts_with("/dev/hidraw")) {
        if let Some(index) = devices
            .iter()
            .position(|device| device.path() == std::path::Path::new(path))
        {
            return Ok(devices
                .into_iter()
                .nth(index)
                .expect("index came from devices"));
        }
        // Combined config commands pass the sibling product-protocol hidraw
        // node, so a unique Rynk HID interface is still an unambiguous match.
    }
    one_device(devices, "Rynk USB HID")
}

fn select_serial(requested: Option<&str>) -> Result<SerialDevice> {
    if requested.is_some_and(crate::transport::is_ble_address) {
        bail!("a BLE address cannot be used with --usb");
    }
    let devices = SerialDevice::discover().context("Rynk USB discovery failed")?;
    if let Some(path) = requested {
        if path.starts_with("/dev/hidraw") {
            // Combined config operations use the hidraw path for the Glove80
            // lighting transport. Older Rynk firmware exposes a sibling CDC
            // interface, so select it by its immutable RYNK serial marker.
            return one_device(
                devices,
                "Rynk USB serial associated with the selected hidraw device",
            );
        }
        return devices
            .into_iter()
            .find(|device| device.path == path)
            .ok_or_else(|| anyhow!("no discovered Rynk serial device matches {path}"));
    }
    one_device(devices, "Rynk USB serial")
}

async fn select_ble(requested: Option<&str>) -> Result<BleDevice> {
    if requested.is_some_and(|value| value.starts_with("/dev/")) {
        bail!("a device path cannot be used with --ble");
    }
    let devices = BleDevice::discover()
        .await
        .context("Rynk BLE discovery failed")?;
    if let Some(address) = requested {
        let needle = address.replace(':', "").to_ascii_lowercase();
        return devices
            .into_iter()
            .find(|device| {
                format!("{:?}", device.id())
                    .chars()
                    .filter(|character| character.is_ascii_hexdigit())
                    .collect::<String>()
                    .to_ascii_lowercase()
                    .contains(&needle)
            })
            .ok_or_else(|| anyhow!("no connected Rynk BLE device matches {address}"));
    }
    one_device(devices, "connected Rynk BLE")
}

fn one_device<T>(mut devices: Vec<T>, kind: &str) -> Result<T> {
    match devices.len() {
        0 => bail!("no {kind} device found"),
        1 => Ok(devices.pop().expect("length checked")),
        count => bail!("found {count} {kind} devices; pass --device to select one"),
    }
}
