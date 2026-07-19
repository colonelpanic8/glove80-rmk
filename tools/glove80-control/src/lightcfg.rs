//! The canonical configuration file — the keyboard's whole personality in
//! one TOML: `[[layer]]` keymap entries (see [`crate::keymapcfg`]) plus the
//! persistent lighting config, and the `config apply/export/show/validate`
//! subcommands that move both over the host protocol.
//!
//! The two sections travel differently, and the CLI is explicit about it:
//!
//! - **lighting** is one blob applied through the v1.1 CONFIG session —
//!   atomic: the keyboard ends up with the whole new lighting config or
//!   keeps the old one;
//! - **keymap** goes through batched v1.2 KEYMAP_WRITE calls — best-effort
//!   per batch with read-back verification; a failed batch aborts the rest
//!   and the CLI reports exactly what was written.
//!
//! Either section may be omitted: no `[[layer]]` keys = lighting-only
//! (today's behavior), no `[[toggle]]`/`[[record]]` = keymap-only (the
//! stored lighting config is left untouched).
//!
//! The TOML file is the editing surface; the lighting **blob** (encoded and
//! validated exclusively by the `glove80-host-protocol` crate) is the unit
//! of transfer and persistence. Round-trip guarantees:
//!
//! - text → config → blob → config → text is semantically stable (comments,
//!   toggle names, and layer IDs/names, which never enter the blob or the
//!   firmware, are lost on export — layer IDs are re-synthesized);
//! - blob → config → blob is byte-stable (protocol-crate guarantee, tested
//!   end to end here).

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use glove80_host_protocol::{
    decode_lighting_config, encode_lighting_config, CellWrite, ConfigActivation, ConfigGate,
    ConfigRecord, Effect, EffectKind, LightingConfig, CONFIG_LAYER_COUNT, CONFIG_MAGIC,
    CONFIG_TOGGLE_COUNT, MAX_CONFIG_BLOB_LEN,
};

use crate::hostproto::{ApplyStage, HostClient};
use crate::keymapcfg::{self, LayerEntry, LayerPlan, LayerRef};
use crate::lighting::{build_effect, parse_color, parse_key_list};
use crate::transport::{self, Selector};

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

/// Top-level canonical configuration file: keymap layers + lighting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Keymap layers; position in the file = firmware layer slot. Layer
    /// IDs/names are host-side only. Optional: omit for lighting-only.
    #[serde(default, rename = "layer", skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerEntry>,
    /// Per-toggle persistence/boot-state entries; unlisted toggles neither
    /// persist nor start on.
    #[serde(default, rename = "toggle", skip_serializing_if = "Vec::is_empty")]
    pub toggles: Vec<ToggleEntry>,
    /// Lighting records; order = composition order within each activation
    /// class.
    #[serde(default, rename = "record", skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<RecordEntry>,
}

impl ConfigFile {
    /// Does the file carry a lighting section? Only then does apply run
    /// the (atomic) lighting session; a keymap-only file leaves the stored
    /// lighting config untouched.
    pub fn has_lighting(&self) -> bool {
        !self.toggles.is_empty() || !self.records.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToggleEntry {
    /// Toggle id (0-31), as configured on the device.
    pub id: u8,
    /// Optional human-readable name. Documentation only: it does not enter
    /// the blob and is therefore lost on export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Persist this toggle's runtime state across reboots (opt-in).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persist: bool,
    /// Boot state for a toggle without a persisted runtime state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub initial_on: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordEntry {
    /// `"always"`, `{ layer = N }` (N < 8), `{ layer = "id" }` (a stable
    /// `[[layer]]` id, resolved to its slot), or `{ toggle = N }` (N < 32).
    pub activation: ActivationSpec,
    /// Optional second predicate ANDed with activation: `"usb"`,
    /// `"charging"`, `"split-link"`, `{ layer = "id-or-n" }`, or
    /// `{ toggle = n }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateSpec>,
    /// Sparse key → effect entries; an unlisted key is transparent.
    #[serde(default, rename = "cells", skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<CellSpec>,
}

/// Activation predicate, `"always"` or a one-key table. Layer references
/// may be literal slot numbers (back-compat) or stable layer IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActivationSpec {
    Named(NamedActivation),
    Layer { layer: LayerRef },
    Toggle { toggle: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NamedActivation {
    Always,
}

/// Optional conditional-lighting gate. Layer references use the same stable
/// ID-or-slot form as layer activations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GateSpec {
    Named(NamedGate),
    Layer { layer: LayerRef },
    Toggle { toggle: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamedGate {
    Usb,
    Charging,
    SplitLink,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellSpec {
    /// Key list in the CLI's usual syntax: comma-separated, ranges allowed
    /// ("0-5,12"). Keys are 0-79 (left half 0-39, right half 40-79).
    pub keys: String,
    /// `#RRGGBB`, bare hex, or a named color (red, green, blue, ...).
    pub color: String,
    /// "solid" (default), "blink", or "breathe".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// Blink/breathe period (default 1000 ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<u16>,
    /// Blink/breathe phase offset (default 0 ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_ms: Option<u16>,
    /// Blink duty cycle percent (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duty_pct: Option<u8>,
}

// ---------------------------------------------------------------------------
// TOML ↔ LightingConfig
// ---------------------------------------------------------------------------

impl ActivationSpec {
    /// Resolve to the wire representation; layer IDs become slot numbers
    /// via the file's `[[layer]]` list.
    fn to_config(&self, layers: &[LayerEntry]) -> Result<ConfigActivation> {
        Ok(match self {
            ActivationSpec::Named(NamedActivation::Always) => ConfigActivation::Always,
            ActivationSpec::Layer { layer } => {
                ConfigActivation::LayerActive(keymapcfg::resolve_layer_ref(layer, layers)?)
            }
            ActivationSpec::Toggle { toggle } => ConfigActivation::Toggle(*toggle),
        })
    }

    fn from_config(activation: ConfigActivation) -> ActivationSpec {
        match activation {
            ConfigActivation::Always => ActivationSpec::Named(NamedActivation::Always),
            ConfigActivation::LayerActive(layer) => {
                ActivationSpec::Layer { layer: LayerRef::Index(layer) }
            }
            ConfigActivation::Toggle(toggle) => ActivationSpec::Toggle { toggle },
        }
    }

    fn describe(&self) -> String {
        match self {
            ActivationSpec::Named(NamedActivation::Always) => "always".into(),
            ActivationSpec::Layer { layer } => layer.describe(),
            ActivationSpec::Toggle { toggle } => format!("toggle {toggle}"),
        }
    }
}

impl GateSpec {
    fn to_config(&self, layers: &[LayerEntry]) -> Result<ConfigGate> {
        Ok(match self {
            GateSpec::Named(NamedGate::Usb) => ConfigGate::UsbConnected,
            GateSpec::Named(NamedGate::Charging) => ConfigGate::Charging,
            GateSpec::Named(NamedGate::SplitLink) => ConfigGate::SplitLinkUp,
            GateSpec::Layer { layer } => {
                ConfigGate::LayerActive(keymapcfg::resolve_layer_ref(layer, layers)?)
            }
            GateSpec::Toggle { toggle } => ConfigGate::Toggle(*toggle),
        })
    }

    fn from_config(gate: ConfigGate) -> GateSpec {
        match gate {
            ConfigGate::LayerActive(layer) => GateSpec::Layer { layer: LayerRef::Index(layer) },
            ConfigGate::Toggle(toggle) => GateSpec::Toggle { toggle },
            ConfigGate::UsbConnected => GateSpec::Named(NamedGate::Usb),
            ConfigGate::Charging => GateSpec::Named(NamedGate::Charging),
            ConfigGate::SplitLinkUp => GateSpec::Named(NamedGate::SplitLink),
        }
    }

    fn describe(&self) -> String {
        match self {
            GateSpec::Named(NamedGate::Usb) => "usb".into(),
            GateSpec::Named(NamedGate::Charging) => "charging".into(),
            GateSpec::Named(NamedGate::SplitLink) => "split-link".into(),
            GateSpec::Layer { layer } => layer.describe(),
            GateSpec::Toggle { toggle } => format!("toggle {toggle}"),
        }
    }
}

fn effect_kind(name: Option<&str>) -> Result<EffectKind> {
    match name.unwrap_or("solid") {
        "solid" => Ok(EffectKind::Solid),
        "blink" => Ok(EffectKind::Blink),
        "breathe" => Ok(EffectKind::Breathe),
        other => bail!("unknown effect '{other}' (solid, blink, breathe)"),
    }
}

/// Build the protocol-crate model from the parsed file. Structural limits
/// (record/cell counts, key ranges, duplicate keys, activation args) are
/// deliberately *not* re-checked here: [`file_to_blob`] runs the blob
/// through the protocol crate's validator, the same code the firmware runs.
pub fn file_to_config(file: &ConfigFile) -> Result<LightingConfig> {
    let mut config = LightingConfig::default();
    for (index, toggle) in file.toggles.iter().enumerate() {
        if toggle.id >= CONFIG_TOGGLE_COUNT {
            bail!(
                "toggle entry {index}: id {} out of range (< {CONFIG_TOGGLE_COUNT})",
                toggle.id
            );
        }
        let bit = 1u32 << toggle.id;
        if (config.toggle_persist_mask | config.toggle_initial_state) & bit != 0 {
            bail!("toggle entry {index}: toggle {} listed twice", toggle.id);
        }
        if !toggle.persist && !toggle.initial_on {
            // A pure name/documentation entry; nothing to encode.
            continue;
        }
        if toggle.persist {
            config.toggle_persist_mask |= bit;
        }
        if toggle.initial_on {
            config.toggle_initial_state |= bit;
        }
    }
    for (record_index, record) in file.records.iter().enumerate() {
        let mut cells = heapless::Vec::new();
        for (cell_index, spec) in record.cells.iter().enumerate() {
            let in_record = || format!("record {record_index}, cell entry {cell_index}");
            let keys = parse_key_list(&spec.keys).with_context(in_record)?;
            let kind = effect_kind(spec.effect.as_deref()).with_context(in_record)?;
            let color = parse_color(&spec.color).with_context(in_record)?;
            let effect = build_effect(kind, color, spec.period_ms, spec.phase_ms, spec.duty_pct)
                .with_context(in_record)?;
            for key in keys {
                cells
                    .push(CellWrite { key, effect })
                    .map_err(|_| anyhow!("record {record_index}: too many cells"))?;
            }
        }
        let activation = record
            .activation
            .to_config(&file.layers)
            .with_context(|| format!("record {record_index} activation"))?;
        let gate = record
            .gate
            .as_ref()
            .map(|gate| gate.to_config(&file.layers))
            .transpose()
            .with_context(|| format!("record {record_index} gate"))?;
        config
            .records
            .push(ConfigRecord { activation, gate, cells })
            .map_err(|_| anyhow!("too many [[record]] entries"))?;
    }
    Ok(config)
}

/// Format a key sequence back into the CLI list syntax, coalescing runs of
/// consecutive ascending keys into ranges.
pub fn format_key_list(keys: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<(u8, u8)> = None;
    for &key in keys {
        run = Some(match run {
            Some((start, end)) if key == end.wrapping_add(1) && key > end => (start, key),
            Some((start, end)) => {
                parts.push(format_run(start, end));
                (key, key)
            }
            None => (key, key),
        });
    }
    if let Some((start, end)) = run {
        parts.push(format_run(start, end));
    }
    parts.join(",")
}

fn format_run(start: u8, end: u8) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn effect_to_spec(keys: String, effect: &Effect) -> CellSpec {
    let color = format!("#{:02x}{:02x}{:02x}", effect.r, effect.g, effect.b);
    match effect.kind {
        EffectKind::Solid => CellSpec { keys, color, ..CellSpec::default() },
        EffectKind::Blink => CellSpec {
            keys,
            color,
            effect: Some("blink".into()),
            period_ms: Some(effect.period_ms),
            phase_ms: (effect.phase_ms != 0).then_some(effect.phase_ms),
            duty_pct: Some(effect.duty_percent),
        },
        EffectKind::Breathe => CellSpec {
            keys,
            color,
            effect: Some("breathe".into()),
            period_ms: Some(effect.period_ms),
            phase_ms: (effect.phase_ms != 0).then_some(effect.phase_ms),
            duty_pct: None,
        },
    }
}

/// Render the protocol-crate model as the canonical file, coalescing
/// consecutive cells that share an effect into one keys entry. Cell order is
/// preserved exactly, so re-importing reproduces the same [`LightingConfig`]
/// (and therefore the same blob bytes).
pub fn config_to_file(config: &LightingConfig) -> ConfigFile {
    let mut toggles = Vec::new();
    for id in 0..CONFIG_TOGGLE_COUNT {
        let bit = 1u32 << id;
        let persist = config.toggle_persist_mask & bit != 0;
        let initial_on = config.toggle_initial_state & bit != 0;
        if persist || initial_on {
            toggles.push(ToggleEntry { id, name: None, persist, initial_on });
        }
    }
    let records = config
        .records
        .iter()
        .map(|record| {
            let mut cells: Vec<CellSpec> = Vec::new();
            let mut group: Vec<u8> = Vec::new();
            let mut group_effect: Option<Effect> = None;
            for cell in &record.cells {
                match group_effect {
                    Some(effect) if effect == cell.effect => group.push(cell.key),
                    Some(effect) => {
                        cells.push(effect_to_spec(format_key_list(&group), &effect));
                        group = vec![cell.key];
                        group_effect = Some(cell.effect);
                    }
                    None => {
                        group = vec![cell.key];
                        group_effect = Some(cell.effect);
                    }
                }
            }
            if let Some(effect) = group_effect {
                cells.push(effect_to_spec(format_key_list(&group), &effect));
            }
            RecordEntry {
                activation: ActivationSpec::from_config(record.activation),
                gate: record.gate.map(GateSpec::from_config),
                cells,
            }
        })
        .collect();
    ConfigFile { layers: Vec::new(), toggles, records }
}

// ---------------------------------------------------------------------------
// File I/O: TOML text or raw blob
// ---------------------------------------------------------------------------

/// Encode a parsed file into a blob, running the protocol crate's full
/// validation (the same checks the firmware performs before commit).
pub fn file_to_blob(file: &ConfigFile) -> Result<Vec<u8>> {
    let config = file_to_config(file)?;
    let mut buffer = vec![0u8; MAX_CONFIG_BLOB_LEN];
    let length = encode_lighting_config(&config, &mut buffer)
        .map_err(|error| anyhow!("could not encode the config blob: {error:?}"))?;
    buffer.truncate(length);
    decode_lighting_config(&buffer)
        .map_err(|error| anyhow!("config failed validation: {error}"))?;
    Ok(buffer)
}

pub fn parse_toml(text: &str) -> Result<ConfigFile> {
    toml::from_str(text).context("could not parse the canonical config TOML")
}

/// Serialize a canonical file with the export header.
pub fn to_toml_file(file: &ConfigFile) -> Result<String> {
    let body = toml::to_string_pretty(file)
        .context("could not serialize the canonical config as TOML")?;
    Ok(format!(
        "# Glove80 canonical configuration (keymap + lighting), exported from\n\
         # the device. Comments, toggle names, and layer IDs/names are host-side\n\
         # only and do not survive a round trip through the keyboard: layer IDs\n\
         # below are synthesized as layer0..layerN (position = firmware slot).\n\
         # Keep your hand-edited file in version control; the device round-trips\n\
         # the semantics, not the prose.\n\n{body}"
    ))
}

#[cfg(test)]
pub fn to_toml(config: &LightingConfig) -> Result<String> {
    to_toml_file(&config_to_file(config))
}

fn blob_magic_bytes() -> [u8; 4] {
    CONFIG_MAGIC.to_le_bytes()
}

/// A canonical config loaded from disk, both sections validated offline.
pub struct Loaded {
    /// The encoded + validated lighting blob (possibly of an empty config).
    pub blob: Vec<u8>,
    /// Keymap layers with keys, resolved to firmware slots.
    pub plans: Vec<LayerPlan>,
    /// Whether the file carries a lighting section (raw blobs always do).
    /// When false, apply leaves the stored lighting config untouched.
    pub apply_lighting: bool,
    pub source: &'static str,
}

/// Load `path` as a validated canonical config. Raw lighting blobs are
/// detected by content (the "G80L" magic) or a `.bin` extension; anything
/// else is parsed as canonical TOML (keymap + lighting).
pub fn load_config(path: &Path) -> Result<Loaded> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let is_bin_ext = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"));
    if bytes.starts_with(&blob_magic_bytes()) || is_bin_ext {
        decode_lighting_config(&bytes)
            .map_err(|error| anyhow!("raw config blob failed validation: {error}"))?;
        return Ok(Loaded {
            blob: bytes,
            plans: Vec::new(),
            apply_lighting: true,
            source: "raw blob",
        });
    }
    let text = String::from_utf8(bytes)
        .context("file is neither a config blob (no G80L magic) nor UTF-8 TOML")?;
    let file = parse_toml(&text)?;
    let plans = keymapcfg::build_layer_plans(&file.layers)?;
    Ok(Loaded {
        blob: file_to_blob(&file)?,
        plans,
        apply_lighting: file.has_lighting(),
        source: "TOML",
    })
}

/// Load `path` as a validated lighting blob (raw or TOML).
#[cfg(test)]
pub fn load_blob(path: &Path) -> Result<(Vec<u8>, &'static str)> {
    let loaded = load_config(path)?;
    Ok((loaded.blob, loaded.source))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Human summary of a config: one row per record, plus toggle persistence.
pub fn render_summary(config: &LightingConfig, blob_len: usize) -> String {
    let mut rows = vec![[
        "REC".to_string(),
        "ACTIVATION".into(),
        "GATE".into(),
        "CELLS".into(),
        "KEYS".into(),
        "EFFECTS".into(),
    ]];
    for (index, record) in config.records.iter().enumerate() {
        let keys: Vec<u8> = record.cells.iter().map(|cell| cell.key).collect();
        let mut kinds: Vec<&str> = Vec::new();
        for cell in &record.cells {
            let name = crate::hostproto::effect_name(cell.effect.kind);
            if !kinds.contains(&name) {
                kinds.push(name);
            }
        }
        rows.push([
            index.to_string(),
            ActivationSpec::from_config(record.activation).describe(),
            record
                .gate
                .map(GateSpec::from_config)
                .map_or_else(|| "-".into(), |gate| gate.describe()),
            record.cells.len().to_string(),
            if keys.is_empty() { "-".into() } else { format_key_list(&keys) },
            if kinds.is_empty() { "-".into() } else { kinds.join(", ") },
        ]);
    }
    let mut widths = [0usize; 6];
    for row in &rows {
        for (width, column) in widths.iter_mut().zip(row) {
            *width = (*width).max(column.len());
        }
    }
    let table = rows
        .iter()
        .map(|row| {
            row.iter()
                .zip(widths)
                .map(|(column, width)| format!("{column:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mask_list = |mask: u32| {
        let ids: Vec<u8> = (0..CONFIG_TOGGLE_COUNT).filter(|id| mask & (1 << id) != 0).collect();
        if ids.is_empty() { "none".to_string() } else { format_key_list(&ids) }
    };
    format!(
        "{} record(s), {blob_len}-byte blob\n{table}\n\
         toggles persisted across reboots: {}\n\
         toggles initially on: {}",
        config.records.len(),
        mask_list(config.toggle_persist_mask),
        mask_list(config.toggle_initial_state),
    )
}

// ---------------------------------------------------------------------------
// Command runners
// ---------------------------------------------------------------------------

/// Print the offline summary of a loaded config: which sections it has
/// and what they contain.
fn print_loaded_summary(loaded: &Loaded) {
    if loaded.plans.is_empty() {
        println!("keymap: no [[layer]] keys — apply leaves the keymap untouched");
    } else {
        println!("keymap: {} layer grid(s) to write", loaded.plans.len());
        println!("{}", keymapcfg::render_keymap_summary(&loaded.plans));
    }
    if loaded.apply_lighting {
        let config = decode_lighting_config(&loaded.blob).expect("load_config validated");
        println!("lighting:");
        println!("{}", render_summary(&config, loaded.blob.len()));
    } else {
        println!(
            "lighting: no [[toggle]]/[[record]] section — apply leaves the stored \
             lighting config untouched"
        );
    }
}

/// `config validate FILE`: offline parse + full protocol-crate validation.
pub fn run_validate(path: &Path) -> Result<()> {
    let loaded = load_config(path)?;
    println!("valid canonical config ({})", loaded.source);
    print_loaded_summary(&loaded);
    Ok(())
}

/// `config apply FILE [--dry-run]`.
pub fn run_apply(selector: &Selector, path: &Path, dry_run: bool) -> Result<()> {
    let loaded = load_config(path)?;
    println!("parsed {} ({}); client-side validation passed", path.display(), loaded.source);
    print_loaded_summary(&loaded);
    if dry_run {
        println!("dry run: not touching the device");
        return Ok(());
    }
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
    apply_loaded(&mut client, &loaded)
}

/// Transport-independent unified apply (unit-tested on the mock transport).
///
/// Keymap first (best-effort per batch, read-back verified), then lighting
/// (one atomic session). Ordering is deliberate: if a keymap batch fails,
/// the apply stops before touching the lighting config, so the error
/// describes the only thing that changed.
pub fn apply_loaded(client: &mut HostClient, loaded: &Loaded) -> Result<()> {
    if !loaded.plans.is_empty() {
        println!(
            "applying the keymap ({} layer(s)) — batched writes, verified by \
             read-back, NOT atomic across batches",
            loaded.plans.len()
        );
        let report = keymapcfg::apply_keymap(client, &loaded.plans, |stage| match stage {
            keymapcfg::KeymapStage::LayerBegun { slot, id } => {
                println!("  layer {slot} \"{id}\":");
            }
            keymapcfg::KeymapStage::Batch { written, total, .. } => {
                println!("    wrote {written}/{total} positions");
            }
            keymapcfg::KeymapStage::LayerDone { lossy, .. } => {
                if lossy > 0 {
                    println!("    {lossy} position(s) stored differently than requested");
                }
            }
        })?;
        println!(
            "keymap applied: {} positions written across {} layer(s); changes are \
             live and persisted",
            report.entries_written,
            loaded.plans.len()
        );
        for (layer, key, requested, stored) in &report.lossy {
            println!(
                "  LOSSY: layer {layer} key {key}: wrote {} (0x{requested:04X}) but the \
                 firmware stored {} (0x{stored:04X})",
                crate::keycodes::format_keycode(*requested),
                crate::keycodes::format_keycode(*stored),
            );
        }
    }
    if loaded.apply_lighting {
        println!("applying the lighting config (one atomic session)");
        apply_blob(client, &loaded.blob)?;
    }
    Ok(())
}

/// Transport-independent apply (unit-tested against the mock transport).
pub fn apply_blob(client: &mut HostClient, blob: &[u8]) -> Result<()> {
    let decoded = decode_lighting_config(blob)
        .map_err(|error| anyhow!("config failed validation before apply: {error}"))?;
    if decoded.records.iter().any(|record| record.gate.is_some()) {
        client.config_gate_capabilities()?;
    }
    let result = client.apply_config(blob, |stage| match stage {
        ApplyStage::Begun { total_len, blob_crc32 } => {
            println!("session opened: {total_len} bytes, crc32 {blob_crc32:08x}");
        }
        ApplyStage::Sent { bytes, total } => {
            println!("transferred {bytes}/{total} bytes");
        }
        ApplyStage::Committed => {
            println!("commit OK: the new lighting config is active and persisted");
        }
    });
    result.map_err(|error| {
        error.context(
            "the lighting apply failed; the keyboard keeps its previous lighting \
             configuration untouched (keymap writes already made in this run, if \
             any, remain in place)",
        )
    })
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Build a canonical TOML fragment containing one layer-indicator record per
/// key/color. Layer slots are assigned in order from 0; `gate_on_magic`
/// attaches the conventional Magic-layer (slot 2) press-and-hold gate.
pub fn generate_layer_indicators(
    keys: &str,
    gate_on_magic: bool,
    colors: &str,
) -> Result<String> {
    let keys = parse_key_list(keys).context("invalid --keys list")?;
    let colors: Vec<&str> = colors
        .split(',')
        .map(str::trim)
        .filter(|color| !color.is_empty())
        .collect();
    if keys.is_empty() {
        bail!("--keys must name at least one indicator key");
    }
    if keys.len() > CONFIG_LAYER_COUNT as usize {
        bail!("layer indicators support at most {CONFIG_LAYER_COUNT} keys/layers");
    }
    if colors.len() != keys.len() {
        bail!(
            "--colors has {} entries but --keys expands to {} keys",
            colors.len(),
            keys.len()
        );
    }

    let mut file = ConfigFile::default();
    for (layer, (&key, color)) in keys.iter().zip(colors).enumerate() {
        // Validate/color-normalize through the same parser used by ordinary
        // records, but retain the friendly input spelling in generated TOML.
        parse_color(color).with_context(|| format!("invalid --colors entry {layer}"))?;
        file.records.push(RecordEntry {
            activation: ActivationSpec::Layer { layer: LayerRef::Index(layer as u8) },
            gate: gate_on_magic.then_some(GateSpec::Layer { layer: LayerRef::Index(2) }),
            cells: vec![CellSpec {
                keys: key.to_string(),
                color: color.to_string(),
                ..CellSpec::default()
            }],
        });
    }
    // Exercise full blob validation before emitting a fragment users may
    // paste/apply.
    file_to_blob(&file)?;
    to_toml_file(&file)
}

/// `config export FILE [--raw]` and `config show` share this read path.
fn read_active_config(client: &mut HostClient) -> Result<Vec<u8>> {
    let blob = client
        .read_config()
        .context("could not read the active config from the keyboard")?;
    if blob.is_empty() {
        bail!(
            "the keyboard has no stored lighting config (it is running its \
             compiled-in defaults); apply one first with `config apply`"
        );
    }
    decode_lighting_config(&blob)
        .map_err(|error| anyhow!("device returned an invalid config blob: {error}"))?;
    Ok(blob)
}

/// Read both sections from the device into one canonical [`ConfigFile`].
/// Layer IDs are synthesized as `layer0..layerN` (the firmware stores no
/// IDs or names). Either section degrades gracefully:
///
/// - no keymap capability → lighting-only export (with a note);
/// - no stored lighting config (compiled-in defaults) → keymap-only export
///   (with a note), so applying the file leaves lighting untouched.
pub fn export_file(client: &mut HostClient) -> Result<ConfigFile> {
    let layers = match keymapcfg::read_all_layers(client) {
        Ok(layers) => keymapcfg::layers_to_entries(&layers),
        Err(error) if client.lacks_feature(glove80_host_protocol::feature::KEYMAP) => {
            println!("note: {error:#}; exporting the lighting config only");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let mut file = match client.read_config() {
        Ok(blob) if blob.is_empty() => {
            if layers.is_empty() {
                bail!(
                    "nothing to export: the keyboard advertises no keymap editing and \
                     has no stored lighting config (compiled-in defaults)"
                );
            }
            println!(
                "note: no stored lighting config (compiled-in defaults); exporting \
                 the keymap only"
            );
            ConfigFile::default()
        }
        Ok(blob) => {
            let config = decode_lighting_config(&blob)
                .map_err(|error| anyhow!("device returned an invalid config blob: {error}"))?;
            config_to_file(&config)
        }
        Err(error) => return Err(error.context("could not read the active lighting config")),
    };
    file.layers = layers;
    Ok(file)
}

pub fn run_export(selector: &Selector, path: &Path, raw: bool) -> Result<()> {
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
    if raw {
        let blob = read_active_config(&mut client)?;
        std::fs::write(path, &blob)
            .with_context(|| format!("could not write {}", path.display()))?;
        println!(
            "exported the active lighting config blob ({} bytes) to {} \
             (--raw is lighting-only; the keymap has no blob form)",
            blob.len(),
            path.display()
        );
        return Ok(());
    }
    let file = export_file(&mut client)?;
    std::fs::write(path, to_toml_file(&file)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!(
        "exported the active config ({} layer(s), {} record(s)) to {}",
        file.layers.len(),
        file.records.len(),
        path.display()
    );
    Ok(())
}

pub fn run_show(selector: &Selector) -> Result<()> {
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
    match keymapcfg::read_all_layers(&mut client) {
        Ok(layers) => {
            let entries = keymapcfg::layers_to_entries(&layers);
            let plans = keymapcfg::build_layer_plans(&entries)?;
            println!("keymap ({} populated layer(s)):", plans.len());
            if !plans.is_empty() {
                println!("{}", keymapcfg::render_keymap_summary(&plans));
            }
        }
        Err(error) if client.lacks_feature(glove80_host_protocol::feature::KEYMAP) => {
            println!("keymap: not readable ({error:#})");
        }
        Err(error) => return Err(error),
    }
    println!("lighting:");
    let blob = read_active_config(&mut client)?;
    let config = decode_lighting_config(&blob).expect("validated in read_active_config");
    println!("{}", render_summary(&config, blob.len()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glove80_host_protocol::{
        crc32, Capabilities, Command, Request, Response, ResponsePayload, Status,
        MAX_CELLS_PER_RECORD, MAX_CONFIG_RECORDS, MAX_MESSAGE_LEN, PROTOCOL_VERSION_MAJOR,
        MAX_CONFIG_DATA_PER_MESSAGE,
    };

    use crate::transport::mock::MockTransport;

    fn test_capabilities() -> Capabilities {
        Capabilities {
            protocol_major: PROTOCOL_VERSION_MAJOR,
            protocol_minor: 1,
            led_count_left: 40,
            led_count_right: 40,
            layer_capacity: 8,
            max_cells_per_op: 8,
            effect_mask: 0b111,
            overlay_cell_capacity: 80,
            max_message_len: MAX_MESSAGE_LEN as u16,
            feature_bits: 0x7F, // includes PERSISTENT_CONFIG (bit 6)
            max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
            keymap_rows: 0,
            keymap_cols: 0,
            max_keymap_entries_per_op: 0,
        }
    }

    fn gated_capabilities() -> Capabilities {
        Capabilities {
            protocol_minor: glove80_host_protocol::PROTOCOL_VERSION_MINOR,
            feature_bits: test_capabilities().feature_bits | feature::CONFIG_GATES,
            ..test_capabilities()
        }
    }

    fn caps_handler(
        capabilities: Capabilities,
    ) -> impl FnMut(u8, &Request) -> Vec<Response> + Send + 'static {
        move |request_id, request| {
            assert!(matches!(request, Request::GetCapabilities { .. }));
            vec![Response {
                request_id,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(capabilities),
            }]
        }
    }

    fn empty_ok(request_id: u8, command: Command) -> Response {
        Response { request_id, command, status: Status::Ok, payload: ResponsePayload::Empty }
    }

    fn empty_err(request_id: u8, command: Command, status: Status) -> Response {
        Response { request_id, command, status, payload: ResponsePayload::Empty }
    }

    const SAMPLE_TOML: &str = r##"
# comments survive parsing (but not export)
[[toggle]]
id = 0
name = "caps hint"
persist = true

[[toggle]]
id = 3
initial_on = true

[[record]]
activation = "always"
cells = [{ keys = "0-5,40-45", color = "#181818" }]

[[record]]
activation = { layer = 1 }
cells = [
  { keys = "0-2,6-9", color = "green" },
  { keys = "12", color = "#ff0000", effect = "blink", period_ms = 750, duty_pct = 30 },
  { keys = "70", color = "blue", effect = "breathe", period_ms = 3000, phase_ms = 1500 },
]

[[record]]
activation = { toggle = 31 }
"##;

    const GATED_TOML: &str = r##"
[[layer]]
id = "base"

[[layer]]
id = "magic"

[[record]]
activation = "always"
gate = "usb"
cells = [{ keys = "0", color = "red" }]

[[record]]
activation = "always"
gate = "charging"

[[record]]
activation = "always"
gate = "split-link"

[[record]]
activation = "always"
gate = { layer = "magic" }

[[record]]
activation = "always"
gate = { toggle = 7 }
"##;

    fn sample_config() -> LightingConfig {
        file_to_config(&parse_toml(SAMPLE_TOML).unwrap()).unwrap()
    }

    #[test]
    fn parses_the_sample_toml() {
        let config = sample_config();
        assert_eq!(config.toggle_persist_mask, 0b0001);
        assert_eq!(config.toggle_initial_state, 0b1000);
        assert_eq!(config.records.len(), 3);
        assert_eq!(config.records[0].activation, ConfigActivation::Always);
        assert_eq!(config.records[0].cells.len(), 12);
        assert_eq!(config.records[0].cells[6].key, 40);
        assert_eq!(config.records[0].cells[0].effect, Effect::solid(0x18, 0x18, 0x18));
        assert_eq!(config.records[1].activation, ConfigActivation::LayerActive(1));
        assert_eq!(config.records[1].cells[7].key, 12);
        assert_eq!(
            config.records[1].cells[7].effect,
            Effect::blink(0xFF, 0, 0, 750, 0, 30)
        );
        assert_eq!(
            config.records[1].cells[8].effect,
            Effect::breathe(0, 0, 0xFF, 3000, 1500)
        );
        assert_eq!(config.records[2].activation, ConfigActivation::Toggle(31));
        assert!(config.records[2].cells.is_empty());
    }

    /// text → config → blob → config → text → config: semantically stable,
    /// and the two blob generations are byte-identical.
    #[test]
    fn toml_round_trip_is_stable() {
        for toml_text in [SAMPLE_TOML, GATED_TOML, "", "[[record]]\nactivation = \"always\"\n"] {
            let config = file_to_config(&parse_toml(toml_text).unwrap()).unwrap();
            let blob = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap();
            let decoded = decode_lighting_config(&blob).unwrap();
            assert_eq!(decoded, config);
            let exported = to_toml(&decoded).unwrap();
            let config2 = file_to_config(&parse_toml(&exported).unwrap()).unwrap();
            assert_eq!(config2, config, "export not semantically stable:\n{exported}");
            let blob2 = file_to_blob(&config_to_file(&config2)).unwrap();
            assert_eq!(blob2, blob, "blob-level round trip not byte-stable");
        }
    }

    #[test]
    fn gate_syntax_maps_and_exports_all_wire_conditions() {
        let file = parse_toml(GATED_TOML).unwrap();
        let config = file_to_config(&file).unwrap();
        let gates: Vec<Option<ConfigGate>> =
            config.records.iter().map(|record| record.gate).collect();
        assert_eq!(
            gates,
            vec![
                Some(ConfigGate::UsbConnected),
                Some(ConfigGate::Charging),
                Some(ConfigGate::SplitLinkUp),
                Some(ConfigGate::LayerActive(1)),
                Some(ConfigGate::Toggle(7)),
            ]
        );

        let exported = to_toml(&config).unwrap();
        assert!(exported.contains("gate = \"usb\""), "{exported}");
        assert!(exported.contains("gate = \"charging\""), "{exported}");
        assert!(exported.contains("gate = \"split-link\""), "{exported}");
        assert!(exported.contains("[record.gate]\nlayer = 1"), "{exported}");
        assert!(exported.contains("[record.gate]\ntoggle = 7"), "{exported}");
    }

    #[test]
    fn export_coalesces_key_runs() {
        let file = config_to_file(&sample_config());
        assert_eq!(file.records[0].cells.len(), 1);
        assert_eq!(file.records[0].cells[0].keys, "0-5,40-45");
        assert_eq!(file.records[1].cells[0].keys, "0-2,6-9");
        // Toggle names are documentation and are lost on export.
        assert_eq!(file.toggles[0].name, None);
        assert!(file.toggles[0].persist);
        assert!(file.toggles[1].initial_on);
    }

    #[test]
    fn format_key_list_handles_runs_and_singletons() {
        assert_eq!(format_key_list(&[]), "");
        assert_eq!(format_key_list(&[7]), "7");
        assert_eq!(format_key_list(&[0, 1, 2, 6, 7, 8, 9, 12]), "0-2,6-9,12");
        // Non-ascending order is preserved, never coalesced across breaks.
        assert_eq!(format_key_list(&[5, 4, 3]), "5,4,3");
        assert_eq!(parse_key_list(&format_key_list(&[5, 4, 3])).unwrap(), vec![5, 4, 3]);
    }

    #[test]
    fn max_records_config_round_trips() {
        let mut text = String::new();
        for record in 0..MAX_CONFIG_RECORDS {
            text.push_str(&format!(
                "[[record]]\nactivation = {{ toggle = {} }}\ncells = [{{ keys = \"0-{}\", \
                 color = \"#0000ff\" }}]\n",
                record % 32,
                MAX_CELLS_PER_RECORD - 1
            ));
        }
        let blob = file_to_blob(&parse_toml(&text).unwrap()).unwrap();
        assert_eq!(blob.len(), MAX_CONFIG_BLOB_LEN);
        let decoded = decode_lighting_config(&blob).unwrap();
        let re_exported = to_toml(&decoded).unwrap();
        let config2 = file_to_config(&parse_toml(&re_exported).unwrap()).unwrap();
        assert_eq!(config2, decoded);

        // One record too many is rejected client-side.
        text.push_str("[[record]]\nactivation = \"always\"\n");
        let error = file_to_config(&parse_toml(&text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("too many"), "{error}");
    }

    #[test]
    fn rejections_surface_from_the_protocol_crate() {
        // Duplicate key inside one record: caught by the crate validator.
        let toml_text = "[[record]]\nactivation = \"always\"\ncells = [\n  \
                         { keys = \"3\", color = \"red\" },\n  \
                         { keys = \"1-3\", color = \"blue\" },\n]\n";
        let error = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("key 3 appears twice"), "{error}");

        // Key out of range.
        let toml_text =
            "[[record]]\nactivation = \"always\"\ncells = [{ keys = \"80\", color = \"red\" }]\n";
        let error = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("out of range"), "{error}");

        // Layer out of range.
        let toml_text = "[[record]]\nactivation = { layer = 8 }\n";
        let error = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("layer 8 out of range"), "{error}");

        // Gate arguments use the same protocol bounds.
        let toml_text =
            "[[record]]\nactivation = \"always\"\ngate = { layer = 8 }\n";
        let error = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("gate layer 8 out of range"), "{error}");
        let toml_text =
            "[[record]]\nactivation = \"always\"\ngate = { toggle = 32 }\n";
        let error = file_to_blob(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("gate toggle 32 out of range"), "{error}");

        // Toggle id out of range (checked while building the masks).
        let toml_text = "[[toggle]]\nid = 32\npersist = true\n";
        let error = file_to_config(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("out of range"), "{error}");

        // Duplicate toggle entry.
        let toml_text = "[[toggle]]\nid = 2\npersist = true\n[[toggle]]\nid = 2\ninitial_on = true\n";
        let error = file_to_config(&parse_toml(toml_text).unwrap()).unwrap_err();
        assert!(error.to_string().contains("listed twice"), "{error}");

        // Solid cells reject animation parameters (shared build_effect rule).
        let toml_text = "[[record]]\nactivation = \"always\"\ncells = [{ keys = \"0\", \
                         color = \"red\", period_ms = 500 }]\n";
        let error = file_to_config(&parse_toml(toml_text).unwrap()).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("--effect blink|breathe"), "{chain}");
        assert!(chain.contains("record 0, cell entry 0"), "{chain}");
    }

    #[test]
    fn detects_raw_blobs_by_magic() {
        let dir = std::env::temp_dir().join(format!("lightcfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();

        let bin_path = dir.join("config.dat"); // magic detection, not extension
        std::fs::write(&bin_path, &blob).unwrap();
        let (loaded, source) = load_blob(&bin_path).unwrap();
        assert_eq!(loaded, blob);
        assert_eq!(source, "raw blob");

        let toml_path = dir.join("config.toml");
        std::fs::write(&toml_path, SAMPLE_TOML).unwrap();
        let (loaded, source) = load_blob(&toml_path).unwrap();
        assert_eq!(loaded, blob);
        assert_eq!(source, "TOML");

        // A corrupted blob is rejected, not silently applied.
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        let bad_path = dir.join("bad.bin");
        std::fs::write(&bad_path, &bad).unwrap();
        let error = load_blob(&bad_path).unwrap_err();
        assert!(error.to_string().contains("failed validation"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_summary_lists_records_and_toggles() {
        let config = sample_config();
        let text = render_summary(&config, 321);
        assert!(text.contains("3 record(s), 321-byte blob"), "{text}");
        assert!(text.contains("always"), "{text}");
        assert!(text.contains("layer 1"), "{text}");
        assert!(text.contains("toggle 31"), "{text}");
        assert!(text.contains("0-5,40-45"), "{text}");
        assert!(text.contains("solid, blink, breathe"), "{text}");
        assert!(text.contains("persisted across reboots: 0"), "{text}");
        assert!(text.contains("initially on: 3"), "{text}");
    }

    // -----------------------------------------------------------------------
    // Apply-session tests over the mock transport
    // -----------------------------------------------------------------------

    /// Happy path: BEGIN with the right length/CRC, strictly sequential DATA
    /// chunks that respect MAX_CONFIG_DATA_PER_MESSAGE, then COMMIT.
    #[test]
    fn apply_session_happy_path() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();
        let expected_crc = crc32(&blob);
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let mut mock = mock
            .expect(caps_handler(test_capabilities()))
            .expect(move |request_id, request| {
                let Request::ConfigBegin { total_len, blob_crc32 } = request else {
                    panic!("expected ConfigBegin, got {request:?}");
                };
                assert_eq!(*blob_crc32, expected_crc);
                assert!(*total_len > 0);
                vec![empty_ok(request_id, Command::ConfigBegin)]
            });
        // The mock advertises max_message_len = MAX_MESSAGE_LEN (1536), so
        // chunks are bounded by MAX_CONFIG_DATA_PER_MESSAGE = 1024; the
        // sample blob needs exactly one chunk.
        assert!(blob.len() <= MAX_CONFIG_DATA_PER_MESSAGE);
        mock = mock
            .expect(|request_id, request| {
                assert!(matches!(request, Request::ConfigData { offset: 0, .. }));
                vec![empty_ok(request_id, Command::ConfigData)]
            })
            .expect(|request_id, request| {
                assert!(matches!(request, Request::ConfigCommit));
                vec![empty_ok(request_id, Command::ConfigCommit)]
            });
        let mut client = HostClient::new(Box::new(mock));
        apply_blob(&mut client, &blob).unwrap();
        let requests = requests.lock().unwrap();
        let Request::ConfigData { data, .. } = &requests[2] else { panic!() };
        assert_eq!(data.as_slice(), blob.as_slice());
    }

    #[test]
    fn gated_apply_and_read_back_preserve_the_gate() {
        let blob = file_to_blob(&parse_toml(
            "[[record]]\nactivation = \"always\"\ngate = { toggle = 7 }\n\
             cells = [{ keys = \"0\", color = \"red\" }]\n",
        )
        .unwrap())
        .unwrap();
        let returned = blob.clone();
        let total_len = returned.len() as u32;
        let mock = MockTransport::new()
            .expect(caps_handler(gated_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigCommit)])
            .expect(move |request_id, request| {
                assert!(matches!(request, Request::ConfigRead { offset: 0, .. }));
                vec![Response {
                    request_id,
                    command: Command::ConfigRead,
                    status: Status::Ok,
                    payload: ResponsePayload::ConfigData {
                        total_len,
                        data: heapless::Vec::from_slice(&returned).unwrap(),
                    },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        apply_blob(&mut client, &blob).unwrap();
        let read_back = client.read_config().unwrap();
        let decoded = decode_lighting_config(&read_back).unwrap();
        assert_eq!(decoded.records[0].gate, Some(ConfigGate::Toggle(7)));
    }

    #[test]
    fn gated_apply_requires_the_config_gates_feature() {
        let blob = file_to_blob(&parse_toml(
            "[[record]]\nactivation = \"always\"\ngate = \"usb\"\n",
        )
        .unwrap())
        .unwrap();
        let mock = MockTransport::new().expect(caps_handler(test_capabilities()));
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        assert!(
            format!("{error:#}").contains("conditional lighting config gates"),
            "{error:#}"
        );
    }

    #[test]
    fn layer_indicator_generator_validates_and_honors_gate_selection() {
        let text = generate_layer_indicators("10-12", true, "red,green,blue").unwrap();
        let config = file_to_config(&parse_toml(&text).unwrap()).unwrap();
        assert_eq!(config.records.len(), 3);
        for (layer, record) in config.records.iter().enumerate() {
            assert_eq!(record.activation, ConfigActivation::LayerActive(layer as u8));
            assert_eq!(record.gate, Some(ConfigGate::LayerActive(2)));
            assert_eq!(record.cells[0].key, 10 + layer as u8);
        }

        let text = generate_layer_indicators("4", false, "cyan").unwrap();
        let config = file_to_config(&parse_toml(&text).unwrap()).unwrap();
        assert_eq!(config.records[0].gate, None);
        assert!(generate_layer_indicators("0-1", true, "red").is_err());
        assert!(generate_layer_indicators("0", true, "not-a-color").is_err());
    }

    /// A blob larger than one chunk is split into strictly sequential
    /// offsets covering every byte.
    #[test]
    fn apply_chunks_sequentially() {
        let mut text = String::new();
        for record in 0..MAX_CONFIG_RECORDS {
            text.push_str(&format!(
                "[[record]]\nactivation = {{ layer = {} }}\ncells = [{{ keys = \"0-39\", \
                 color = \"#123456\" }}]\n",
                record % 8
            ));
        }
        let blob = file_to_blob(&parse_toml(&text).unwrap()).unwrap();
        assert!(blob.len() > 2 * MAX_CONFIG_DATA_PER_MESSAGE); // >= 3 chunks
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let mut mock = mock
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)]);
        let chunks = blob.len().div_ceil(MAX_CONFIG_DATA_PER_MESSAGE);
        for _ in 0..chunks {
            mock = mock.expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)]);
        }
        mock = mock.expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigCommit)]);
        let mut client = HostClient::new(Box::new(mock));
        apply_blob(&mut client, &blob).unwrap();

        let requests = requests.lock().unwrap();
        let mut reassembled = Vec::new();
        for request in requests.iter() {
            if let Request::ConfigData { offset, data } = request {
                assert_eq!(*offset as usize, reassembled.len(), "chunks must be sequential");
                assert!(data.len() <= MAX_CONFIG_DATA_PER_MESSAGE);
                reassembled.extend_from_slice(data);
            }
        }
        assert_eq!(reassembled, blob);
    }

    #[test]
    fn apply_reports_crc_mismatch_on_commit() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| {
                vec![empty_err(request_id, Command::ConfigCommit, Status::CrcMismatch)]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("CRC_MISMATCH"), "{chain}");
        assert!(chain.contains("previous lighting configuration untouched"), "{chain}");
    }

    #[test]
    fn apply_reports_invalid_config_on_commit() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| {
                vec![empty_err(request_id, Command::ConfigCommit, Status::InvalidConfig)]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("INVALID_CONFIG"), "{chain}");
        assert!(chain.contains("previous lighting configuration untouched"), "{chain}");
    }

    /// Session interrupted mid-transfer: the device answers BAD_OFFSET, the
    /// client sends a best-effort ABORT and surfaces the precise status.
    #[test]
    fn apply_aborts_on_interrupted_session() {
        let mut text = String::new();
        for record in 0..MAX_CONFIG_RECORDS {
            text.push_str(&format!(
                "[[record]]\nactivation = {{ layer = {} }}\ncells = [{{ keys = \"0-39\", \
                 color = \"#123456\" }}]\n",
                record % 8
            ));
        }
        let blob = file_to_blob(&parse_toml(&text).unwrap()).unwrap();
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let mock = mock
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| {
                // e.g. the session was replaced/interrupted device-side.
                vec![empty_err(request_id, Command::ConfigData, Status::BadOffset)]
            })
            .expect(|request_id, request| {
                assert!(matches!(request, Request::ConfigAbort));
                vec![empty_ok(request_id, Command::ConfigAbort)]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("BAD_OFFSET"), "{chain}");
        assert!(chain.contains("previous lighting configuration untouched"), "{chain}");
        // No COMMIT was ever sent; the last request is the ABORT.
        let requests = requests.lock().unwrap();
        assert!(matches!(requests.last(), Some(Request::ConfigAbort)));
        assert!(!requests.iter().any(|r| matches!(r, Request::ConfigCommit)));
    }

    #[test]
    fn apply_reports_config_incomplete() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| {
                vec![empty_err(request_id, Command::ConfigCommit, Status::ConfigIncomplete)]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        assert!(format!("{error:#}").contains("CONFIG_INCOMPLETE"), "{error:#}");
    }

    #[test]
    fn apply_requires_the_feature_bit_and_blob_ceiling() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();

        // Feature bit 6 absent: rejected before any CONFIG_* request.
        let capabilities = Capabilities {
            feature_bits: 0x3F, // no PERSISTENT_CONFIG
            max_config_blob_len: 0,
            ..test_capabilities()
        };
        let mock = MockTransport::new().expect(caps_handler(capabilities));
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        assert!(
            format!("{error:#}").contains("persistent configuration"),
            "{error:#}"
        );

        // Advertised blob ceiling smaller than the blob: rejected client-side.
        let capabilities =
            Capabilities { max_config_blob_len: 16, ..test_capabilities() };
        let mock = MockTransport::new().expect(caps_handler(capabilities));
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_blob(&mut client, &blob).unwrap_err();
        assert!(format!("{error:#}").contains("at most 16"), "{error:#}");
    }

    #[test]
    fn read_config_loops_until_complete() {
        let blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();
        let total = blob.len() as u32;
        let (first, rest) = blob.split_at(100);
        let first = first.to_vec();
        let rest = rest.to_vec();
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(move |request_id, request| {
                assert!(matches!(request, Request::ConfigRead { offset: 0, .. }));
                vec![Response {
                    request_id,
                    command: Command::ConfigRead,
                    status: Status::Ok,
                    payload: ResponsePayload::ConfigData {
                        total_len: total,
                        data: heapless::Vec::from_slice(&first).unwrap(),
                    },
                }]
            })
            .expect(move |request_id, request| {
                assert!(matches!(request, Request::ConfigRead { offset: 100, .. }));
                vec![Response {
                    request_id,
                    command: Command::ConfigRead,
                    status: Status::Ok,
                    payload: ResponsePayload::ConfigData {
                        total_len: total,
                        data: heapless::Vec::from_slice(&rest).unwrap(),
                    },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        assert_eq!(client.read_config().unwrap(), blob);
    }

    #[test]
    fn read_config_with_no_stored_config_is_a_clear_error() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| {
                vec![Response {
                    request_id,
                    command: Command::ConfigRead,
                    status: Status::Ok,
                    payload: ResponsePayload::ConfigData {
                        total_len: 0,
                        data: heapless::Vec::new(),
                    },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = read_active_config(&mut client).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("compiled-in defaults"), "{text}");
    }

    // -----------------------------------------------------------------------
    // Unified (keymap + lighting) canonical config
    // -----------------------------------------------------------------------

    use glove80_host_protocol::feature;

    use crate::keymapcfg::{self, LayerEntry, LayerRef, GRID_SIZE};

    /// Capabilities advertising both persistent config (bit 6) and keymap
    /// editing (bit 7) with the Glove80 grid.
    fn unified_capabilities() -> Capabilities {
        Capabilities {
            feature_bits: 0x7F | feature::KEYMAP,
            keymap_rows: 6,
            keymap_cols: 14,
            max_keymap_entries_per_op: 32,
            ..test_capabilities()
        }
    }

    fn grid_of(token: &str) -> String {
        vec![token; GRID_SIZE].join(" ")
    }

    fn layer(id: &str, keys: Option<String>) -> LayerEntry {
        LayerEntry { id: id.into(), name: None, keys }
    }

    /// A small unified file: two keymap layers plus a lighting record that
    /// references the second layer by stable id.
    fn unified_file() -> ConfigFile {
        ConfigFile {
            layers: vec![
                layer("base", Some(grid_of("KC_A"))),
                layer("lower", Some(grid_of("KC_B"))),
            ],
            toggles: Vec::new(),
            records: vec![RecordEntry {
                activation: ActivationSpec::Layer { layer: LayerRef::Id("lower".into()) },
                gate: None,
                cells: vec![CellSpec {
                    keys: "0-3".into(),
                    color: "#112233".into(),
                    ..CellSpec::default()
                }],
            }],
        }
    }

    fn load_file(file: &ConfigFile) -> Loaded {
        Loaded {
            blob: file_to_blob(file).unwrap(),
            plans: keymapcfg::build_layer_plans(&file.layers).unwrap(),
            apply_lighting: file.has_lighting(),
            source: "TOML",
        }
    }

    /// Mock handler answering one KEYMAP_WRITE by echoing the request
    /// (faithful storage).
    fn keymap_write_ok(request_id: u8, request: &Request) -> Vec<Response> {
        let Request::KeymapWrite { entries } = request else {
            panic!("expected KeymapWrite, got {request:?}");
        };
        vec![Response {
            request_id,
            command: Command::KeymapWrite,
            status: Status::Ok,
            payload: ResponsePayload::KeymapWritten {
                keycodes: entries.iter().map(|entry| entry.keycode).collect(),
            },
        }]
    }

    /// Stable-id resolution: `{ layer = "lower" }` becomes slot 1 on the
    /// wire; bare integers pass through; unknown ids are an error naming
    /// the record.
    #[test]
    fn layer_ids_resolve_to_slots_in_the_lighting_blob() {
        let config = file_to_config(&unified_file()).unwrap();
        assert_eq!(config.records[0].activation, ConfigActivation::LayerActive(1));

        let toml_text = r#"
[[layer]]
id = "base"

[[layer]]
id = "lower"

[[record]]
activation = { layer = "lower" }
cells = [{ keys = "0", color = "red" }]

[[record]]
activation = { layer = 7 }
"#;
        let config = file_to_config(&parse_toml(toml_text).unwrap()).unwrap();
        assert_eq!(config.records[0].activation, ConfigActivation::LayerActive(1));
        assert_eq!(config.records[1].activation, ConfigActivation::LayerActive(7));

        let mut file = unified_file();
        file.records[0].activation =
            ActivationSpec::Layer { layer: LayerRef::Id("upper".into()) };
        let error = file_to_config(&file).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("record 0 activation"), "{chain}");
        assert!(chain.contains("unknown layer id \"upper\""), "{chain}");
    }

    /// The full unified apply: keymap batches (with read-back) first, then
    /// the atomic lighting session.
    #[test]
    fn unified_apply_writes_keymap_then_lighting() {
        let loaded = load_file(&unified_file());
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        // 84 positions per layer at the CLI's 21-entry hardware-friendly
        // batch cap = 4 batches per layer.
        let mut mock = mock.expect(caps_handler(unified_capabilities()));
        for _ in 0..8 {
            mock = mock.expect(keymap_write_ok);
        }
        let mock = mock
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigCommit)]);
        let mut client = HostClient::new(Box::new(mock));
        apply_loaded(&mut client, &loaded).unwrap();

        let requests = requests.lock().unwrap();
        // Order: caps, 8 keymap batches, then the lighting session.
        assert_eq!(requests.len(), 12);
        let mut layer0 = Vec::new();
        let mut layer1 = Vec::new();
        for request in &requests[1..9] {
            let Request::KeymapWrite { entries } = request else {
                panic!("expected KeymapWrite, got {request:?}");
            };
            for entry in entries {
                match entry.layer {
                    0 => layer0.push((entry.key, entry.keycode)),
                    1 => layer1.push((entry.key, entry.keycode)),
                    other => panic!("unexpected layer {other}"),
                }
            }
        }
        // Every grid position of both layers written, in order.
        assert_eq!(layer0.len(), GRID_SIZE);
        assert_eq!(layer1.len(), GRID_SIZE);
        assert!(layer0.iter().enumerate().all(|(i, &(k, c))| k as usize == i && c == 0x0004));
        assert!(layer1.iter().enumerate().all(|(i, &(k, c))| k as usize == i && c == 0x0005));
        assert!(matches!(requests[9], Request::ConfigBegin { .. }));
        assert!(matches!(requests[11], Request::ConfigCommit));
    }

    /// A failed keymap batch aborts everything after it — the remaining
    /// batches AND the lighting session — and the error says exactly what
    /// was written and that nothing is rolled back.
    #[test]
    fn unified_apply_stops_on_keymap_batch_failure() {
        let loaded = load_file(&unified_file());
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let mock = mock
            .expect(caps_handler(unified_capabilities()))
            .expect(keymap_write_ok) // layer 0, keys 0-20
            .expect(keymap_write_ok) // layer 0, keys 21-41
            .expect(keymap_write_ok) // layer 0, keys 42-62
            .expect(keymap_write_ok) // layer 0, keys 63-83
            .expect(keymap_write_ok) // layer 1, keys 0-20
            .expect(|request_id, _| {
                vec![empty_err(request_id, Command::KeymapWrite, Status::Busy)]
            });
        let mut client = HostClient::new(Box::new(mock));
        let error = apply_loaded(&mut client, &loaded).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("keymap apply interrupted"), "{chain}");
        assert!(chain.contains("layer \"lower\" (slot 1)"), "{chain}");
        assert!(chain.contains("keys 0..21 written"), "{chain}");
        assert!(chain.contains("keys 21..84 untouched"), "{chain}");
        assert!(chain.contains("layer(s) base"), "{chain}");
        assert!(chain.contains("NOT rolled back"), "{chain}");
        assert!(chain.contains("BUSY"), "{chain}");
        // No lighting request was ever sent.
        let requests = requests.lock().unwrap();
        assert!(!requests.iter().any(|request| matches!(
            request,
            Request::ConfigBegin { .. } | Request::ConfigData { .. } | Request::ConfigCommit
        )));
    }

    /// Keymap-only file: no lighting session at all. Lighting-only file:
    /// no keymap writes (today's behavior unchanged).
    #[test]
    fn apply_skips_omitted_sections() {
        // Keymap-only: one layer, no toggles/records.
        let file = ConfigFile {
            layers: vec![layer("base", Some(grid_of("KC_C")))],
            ..ConfigFile::default()
        };
        let loaded = load_file(&file);
        assert!(!loaded.apply_lighting);
        let mock = MockTransport::new()
            .expect(caps_handler(unified_capabilities()))
            .expect(keymap_write_ok)
            .expect(keymap_write_ok)
            .expect(keymap_write_ok)
            .expect(keymap_write_ok);
        // No CONFIG_* handlers queued: any lighting request would panic.
        let mut client = HostClient::new(Box::new(mock));
        apply_loaded(&mut client, &loaded).unwrap();

        // Lighting-only (the long-standing sample file): no keymap writes.
        let loaded = load_file(&parse_toml(SAMPLE_TOML).unwrap());
        assert!(loaded.plans.is_empty());
        assert!(loaded.apply_lighting);
        let mock = MockTransport::new()
            .expect(caps_handler(unified_capabilities()))
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigBegin)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigData)])
            .expect(|request_id, _| vec![empty_ok(request_id, Command::ConfigCommit)]);
        let mut client = HostClient::new(Box::new(mock));
        apply_loaded(&mut client, &loaded).unwrap();
    }

    /// Export reads every layer + the lighting blob, synthesizes layer ids,
    /// and the result round-trips: parse(export) applies identically and a
    /// second export is textually identical.
    #[test]
    fn unified_export_round_trips() {
        let lighting_blob = file_to_blob(&parse_toml(SAMPLE_TOML).unwrap()).unwrap();

        // Device state: layer 0 all KC_A, layer 1 with one MO(2), layers
        // 2..7 empty (trailing layers must be dropped from the export).
        let device_layer = |layer: u8| -> Vec<u16> {
            match layer {
                0 => vec![0x0004; GRID_SIZE],
                1 => {
                    let mut codes = vec![0u16; GRID_SIZE];
                    codes[14] = 0x5222;
                    codes
                }
                _ => vec![0u16; GRID_SIZE],
            }
        };
        let mock_device = |lighting_blob: Vec<u8>| {
            let mut mock = MockTransport::new().expect(caps_handler(unified_capabilities()));
            for layer in 0u8..8 {
                // 84 positions at 32/op = 3 KEYMAP_READ chunks per layer.
                for _ in 0..3 {
                    let codes = device_layer(layer);
                    mock = mock.expect(move |request_id, request: &Request| {
                        let Request::KeymapRead { layer, start_key, max_count } = request
                        else {
                            panic!("expected KeymapRead, got {request:?}");
                        };
                        let start = usize::from(*start_key);
                        let end = (start + usize::from(*max_count)).min(GRID_SIZE);
                        vec![Response {
                            request_id,
                            command: Command::KeymapRead,
                            status: Status::Ok,
                            payload: ResponsePayload::KeymapActions {
                                layer: *layer,
                                start_key: *start_key,
                                keycodes: heapless::Vec::from_slice(&codes[start..end])
                                    .unwrap(),
                            },
                        }]
                    });
                }
            }
            mock.expect(move |request_id, request: &Request| {
                assert!(matches!(request, Request::ConfigRead { offset: 0, .. }));
                vec![Response {
                    request_id,
                    command: Command::ConfigRead,
                    status: Status::Ok,
                    payload: ResponsePayload::ConfigData {
                        total_len: lighting_blob.len() as u32,
                        data: heapless::Vec::from_slice(&lighting_blob).unwrap(),
                    },
                }]
            })
        };

        let mut client = HostClient::new(Box::new(mock_device(lighting_blob.clone())));
        let exported = export_file(&mut client).unwrap();
        assert_eq!(exported.layers.len(), 2, "trailing empty layers must be dropped");
        assert_eq!(exported.layers[0].id, "layer0");
        assert_eq!(exported.layers[1].id, "layer1");

        let text = to_toml_file(&exported).unwrap();
        let reparsed = parse_toml(&text).unwrap();
        // Parsed keymap plans reproduce the device grids exactly.
        let plans = keymapcfg::build_layer_plans(&reparsed.layers).unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].codes, device_layer(0));
        assert_eq!(plans[1].codes, device_layer(1));
        // The lighting blob is byte-stable through the text form.
        assert_eq!(file_to_blob(&reparsed).unwrap(), lighting_blob);
        // A second export of the same device state is textually identical.
        let mut client = HostClient::new(Box::new(mock_device(lighting_blob)));
        let text2 = to_toml_file(&export_file(&mut client).unwrap()).unwrap();
        assert_eq!(text2, text);
    }

    /// The shipped example files parse, validate, and (for the unified
    /// one) resolve their layer ids.
    #[test]
    fn shipped_examples_are_valid() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");

        // Lighting-only back-compat: the existing example keeps loading.
        let loaded = load_config(&dir.join("lighting-default.toml")).unwrap();
        assert!(loaded.plans.is_empty());
        assert!(loaded.apply_lighting);
        assert!(!loaded.blob.is_empty());

        // Full-keyboard example: five layers, lighting referencing ids.
        let loaded = load_config(&dir.join("glove80.toml")).unwrap();
        assert_eq!(loaded.plans.len(), 5);
        assert!(loaded.apply_lighting);
        let file = parse_toml(
            &std::fs::read_to_string(dir.join("glove80.toml")).unwrap(),
        )
        .unwrap();
        let config = file_to_config(&file).unwrap();
        // "base".."mac_hyper" resolved to slots 0..4; slots 5-7 literal.
        let layers: Vec<u8> = config
            .records
            .iter()
            .filter_map(|record| match record.activation {
                ConfigActivation::LayerActive(layer) => Some(layer),
                _ => None,
            })
            .collect();
        assert_eq!(layers, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        // Spot-check ported bindings: Base r3c0 = KC_LCTL, r0c6 layer-tap,
        // Magic bootloader keys, Mac Hyper's modifier chord.
        assert_eq!(loaded.plans[0].codes[42], 0x00E0);
        assert_eq!(loaded.plans[0].codes[6], 0x4129);
        assert_eq!(loaded.plans[2].codes[42], 0x7C00);
        assert_eq!(loaded.plans[4].codes[77], 0x0CE0);
    }
}
