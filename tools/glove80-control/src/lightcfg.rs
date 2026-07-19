//! Persistent lighting configuration: the canonical human-editable TOML
//! representation of a [`LightingConfig`], and the `config
//! apply/export/show/validate` subcommands that move it over the v1.1
//! host-protocol session (CONFIG_BEGIN/DATA/COMMIT/READ).
//!
//! The TOML file is the editing surface; the config **blob** (encoded and
//! validated exclusively by the `glove80-host-protocol` crate) is the unit
//! of transfer and persistence. Round-trip guarantees:
//!
//! - text → config → blob → config → text is semantically stable (comments
//!   and toggle names, which never enter the blob, are lost on export);
//! - blob → config → blob is byte-stable (protocol-crate guarantee, tested
//!   end to end here).

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use glove80_host_protocol::{
    decode_lighting_config, encode_lighting_config, ConfigActivation, ConfigRecord, EffectKind,
    LightingConfig, CellWrite, Effect, CONFIG_MAGIC, CONFIG_TOGGLE_COUNT, MAX_CONFIG_BLOB_LEN,
};

use crate::hostproto::{ApplyStage, HostClient};
use crate::lighting::{build_effect, parse_color, parse_key_list};
use crate::transport::{self, Selector};

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

/// Top-level canonical lighting file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Per-toggle persistence/boot-state entries; unlisted toggles neither
    /// persist nor start on.
    #[serde(default, rename = "toggle", skip_serializing_if = "Vec::is_empty")]
    pub toggles: Vec<ToggleEntry>,
    /// Lighting records; order = composition order within each activation
    /// class.
    #[serde(default, rename = "record", skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<RecordEntry>,
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
    /// `"always"`, `{ layer = N }` (N < 8), or `{ toggle = N }` (N < 32).
    pub activation: ActivationSpec,
    /// Sparse key → effect entries; an unlisted key is transparent.
    #[serde(default, rename = "cells", skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<CellSpec>,
}

/// Activation predicate, `"always"` or a one-key table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActivationSpec {
    Named(NamedActivation),
    Layer { layer: u8 },
    Toggle { toggle: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NamedActivation {
    Always,
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
    fn to_config(self) -> ConfigActivation {
        match self {
            ActivationSpec::Named(NamedActivation::Always) => ConfigActivation::Always,
            ActivationSpec::Layer { layer } => ConfigActivation::LayerActive(layer),
            ActivationSpec::Toggle { toggle } => ConfigActivation::Toggle(toggle),
        }
    }

    fn from_config(activation: ConfigActivation) -> ActivationSpec {
        match activation {
            ConfigActivation::Always => ActivationSpec::Named(NamedActivation::Always),
            ConfigActivation::LayerActive(layer) => ActivationSpec::Layer { layer },
            ConfigActivation::Toggle(toggle) => ActivationSpec::Toggle { toggle },
        }
    }

    fn describe(self) -> String {
        match self {
            ActivationSpec::Named(NamedActivation::Always) => "always".into(),
            ActivationSpec::Layer { layer } => format!("layer {layer}"),
            ActivationSpec::Toggle { toggle } => format!("toggle {toggle}"),
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
        config
            .records
            .push(ConfigRecord {
                activation: record.activation.to_config(),
                cells,
            })
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
                cells,
            }
        })
        .collect();
    ConfigFile { toggles, records }
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
    toml::from_str(text).context("could not parse the lighting config TOML")
}

pub fn to_toml(config: &LightingConfig) -> Result<String> {
    let file = config_to_file(config);
    let body = toml::to_string_pretty(&file)
        .context("could not serialize the lighting config as TOML")?;
    Ok(format!(
        "# Glove80 persistent lighting configuration (canonical TOML form).\n\
         # Exported from a config blob; comments and toggle names do not\n\
         # survive a round trip through the device.\n\n{body}"
    ))
}

fn blob_magic_bytes() -> [u8; 4] {
    CONFIG_MAGIC.to_le_bytes()
}

/// Load `path` as a validated config blob. Raw blobs are detected by
/// content (the "G80L" magic) or a `.bin` extension; anything else is
/// parsed as canonical TOML.
pub fn load_blob(path: &Path) -> Result<(Vec<u8>, &'static str)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let is_bin_ext = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"));
    if bytes.starts_with(&blob_magic_bytes()) || is_bin_ext {
        decode_lighting_config(&bytes)
            .map_err(|error| anyhow!("raw config blob failed validation: {error}"))?;
        return Ok((bytes, "raw blob"));
    }
    let text = String::from_utf8(bytes)
        .context("file is neither a config blob (no G80L magic) nor UTF-8 TOML")?;
    let file = parse_toml(&text)?;
    Ok((file_to_blob(&file)?, "TOML"))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Human summary of a config: one row per record, plus toggle persistence.
pub fn render_summary(config: &LightingConfig, blob_len: usize) -> String {
    let mut rows = vec![[
        "REC".to_string(),
        "ACTIVATION".into(),
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
            record.cells.len().to_string(),
            if keys.is_empty() { "-".into() } else { format_key_list(&keys) },
            if kinds.is_empty() { "-".into() } else { kinds.join(", ") },
        ]);
    }
    let mut widths = [0usize; 5];
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

/// `config validate FILE`: offline parse + full protocol-crate validation.
pub fn run_validate(path: &Path) -> Result<()> {
    let (blob, source) = load_blob(path)?;
    let config = decode_lighting_config(&blob).expect("load_blob validated");
    println!("valid lighting config ({source})");
    println!("{}", render_summary(&config, blob.len()));
    Ok(())
}

/// `config apply FILE [--dry-run]`.
pub fn run_apply(selector: &Selector, path: &Path, dry_run: bool) -> Result<()> {
    let (blob, source) = load_blob(path)?;
    let config = decode_lighting_config(&blob).expect("load_blob validated");
    println!("parsed {} ({source}); client-side validation passed", path.display());
    println!("{}", render_summary(&config, blob.len()));
    if dry_run {
        println!("dry run: not touching the device");
        return Ok(());
    }
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
    apply_blob(&mut client, &blob)
}

/// Transport-independent apply (unit-tested against the mock transport).
pub fn apply_blob(client: &mut HostClient, blob: &[u8]) -> Result<()> {
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
            "the apply failed; the keyboard keeps its previous configuration untouched",
        )
    })
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

pub fn run_export(selector: &Selector, path: &Path, raw: bool) -> Result<()> {
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
    let blob = read_active_config(&mut client)?;
    if raw {
        std::fs::write(path, &blob)
            .with_context(|| format!("could not write {}", path.display()))?;
        println!("exported the active config blob ({} bytes) to {}", blob.len(), path.display());
        return Ok(());
    }
    let config = decode_lighting_config(&blob).expect("validated in read_active_config");
    std::fs::write(path, to_toml(&config)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!(
        "exported the active config ({} record(s), {}-byte blob) to {}",
        config.records.len(),
        blob.len(),
        path.display()
    );
    Ok(())
}

pub fn run_show(selector: &Selector) -> Result<()> {
    let transport = transport::connect(selector)?;
    let mut client = HostClient::new(transport);
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
        for toml_text in [SAMPLE_TOML, "", "[[record]]\nactivation = \"always\"\n"] {
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
        assert!(chain.contains("previous configuration untouched"), "{chain}");
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
        assert!(chain.contains("previous configuration untouched"), "{chain}");
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
        assert!(chain.contains("previous configuration untouched"), "{chain}");
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
}
