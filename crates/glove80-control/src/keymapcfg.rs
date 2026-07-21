//! Keymap section of the canonical configuration file: `[[layer]]` entries
//! with stable IDs, display names, and a keycode grid, plus the apply and
//! export paths. Production device I/O uses Rynk; the older host-protocol
//! backend remains here for isolated compatibility tests.
//!
//! Layer names and stable IDs are **host-side only**: the firmware knows
//! plain slot numbers 0..layer_capacity. A layer's position in the file is
//! its firmware slot; lighting records reference layers by ID and the CLI
//! resolves the ID to the slot index at encode time.
//!
//! Unlike the lighting blob (one atomic CONFIG session), keymap writes are
//! **best-effort per batch**: each KEYMAP_WRITE batch is all-or-nothing,
//! but a multi-batch apply interrupted midway leaves earlier batches
//! written. The apply path reports exactly how far it got.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use glove80_host_protocol::KeymapEntry;

#[cfg(test)]
use crate::hostproto::HostClient;
use crate::keycodes;

/// The Glove80 grid the canonical file targets. Offline validation uses
/// these; at apply/export time the device's advertised dimensions are
/// checked against them.
pub const GRID_ROWS: u8 = 6;
pub const GRID_COLS: u8 = 14;
pub const GRID_SIZE: usize = GRID_ROWS as usize * GRID_COLS as usize;
/// Grid positions with no physical switch on the Glove80.
pub const GRID_HOLES: [u8; 4] = [5, 8, 75, 78];
/// Compile-time layer capacity of the firmware (design-goals.md).
pub const LAYER_CAPACITY: usize = 8;

/// Placeholder token for a hole (or any `KC_NO`) in the keys grid.
const HOLE_TOKEN: &str = "--";

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

/// One `[[layer]]` entry. Position in the file = firmware layer slot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerEntry {
    /// Stable layer ID, unique within the file, host-side only. Lighting
    /// records reference layers by this ID. Must not be purely numeric
    /// (bare integers in activations mean literal slot numbers).
    pub id: String,
    /// Display name, host-side only (the firmware stores no names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The 6x14 keycode grid as whitespace-separated tokens, row-major,
    /// one row per line by convention. Omit to declare the layer for ID
    /// reference only — apply then leaves its bindings untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
}

/// A layer literal slot number or a stable layer ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LayerRef {
    Index(u8),
    Id(String),
}

impl LayerRef {
    pub fn describe(&self) -> String {
        match self {
            LayerRef::Index(index) => format!("layer {index}"),
            LayerRef::Id(id) => format!("layer \"{id}\""),
        }
    }
}

/// A layer with bindings, resolved and ready to write: `slot` is the
/// firmware layer, `codes` is the full row-major grid.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPlan {
    pub slot: u8,
    pub id: String,
    pub name: Option<String>,
    pub codes: Vec<u16>,
}

/// Validate `[[layer]]` entries: unique non-numeric IDs, bounded count,
/// parseable grids. Returns the plans for every layer that has keys.
pub fn build_layer_plans(layers: &[LayerEntry]) -> Result<Vec<LayerPlan>> {
    if layers.len() > LAYER_CAPACITY {
        bail!(
            "{} [[layer]] entries exceed the firmware's layer capacity of {LAYER_CAPACITY}",
            layers.len()
        );
    }
    let mut plans = Vec::new();
    for (slot, layer) in layers.iter().enumerate() {
        let id = layer.id.trim();
        if id.is_empty() {
            bail!("layer {slot}: id must not be empty");
        }
        if id.chars().all(|c| c.is_ascii_digit()) {
            bail!(
                "layer {slot}: id '{id}' is purely numeric; numeric layer references \
                 mean literal slot numbers, so IDs must contain a non-digit"
            );
        }
        if layers[..slot].iter().any(|other| other.id.trim() == id) {
            bail!("layer {slot}: id '{id}' is used by an earlier layer");
        }
        if let Some(keys) = &layer.keys {
            let codes = parse_grid(keys)
                .with_context(|| format!("layer {slot} (\"{id}\"): bad keys grid"))?;
            plans.push(LayerPlan {
                slot: slot as u8,
                id: id.to_string(),
                name: layer.name.clone(),
                codes,
            });
        }
    }
    Ok(plans)
}

/// Resolve a layer reference against the file's `[[layer]]` list. Bare
/// integers pass through unchanged (back-compat with lighting-only files).
pub fn resolve_layer_ref(reference: &LayerRef, layers: &[LayerEntry]) -> Result<u8> {
    match reference {
        LayerRef::Index(index) => Ok(*index),
        LayerRef::Id(id) => layers
            .iter()
            .position(|layer| layer.id.trim() == id.trim())
            .map(|slot| slot as u8)
            .ok_or_else(|| {
                anyhow!(
                    "unknown layer id \"{id}\" (defined layers: {})",
                    if layers.is_empty() {
                        "none".to_string()
                    } else {
                        layers
                            .iter()
                            .map(|layer| format!("\"{}\"", layer.id.trim()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )
            }),
    }
}

// ---------------------------------------------------------------------------
// Grid text <-> keycodes
// ---------------------------------------------------------------------------

/// Split a keys grid into keycode tokens. Whitespace separates tokens
/// except inside parentheses (so `LT(1, KC_A)` is one token); `#` starts a
/// comment running to the end of the line.
fn tokenize_grid(text: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut in_comment = false;
    for character in text.chars() {
        if in_comment {
            if character == '\n' {
                in_comment = false;
            }
            continue;
        }
        match character {
            '#' if depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                in_comment = true;
            }
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("unbalanced ')' in the keys grid"))?;
                current.push(character);
            }
            c if c.is_whitespace() => {
                // Whitespace inside parentheses is dropped so `LT(1, KC_A)`
                // normalizes to the single token `LT(1,KC_A)`.
                if depth == 0 && !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if depth != 0 {
        bail!("unbalanced '(' in the keys grid");
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Parse a keys grid into the full row-major keycode vector. Requires
/// exactly `GRID_SIZE` tokens; `--` means `KC_NO` (used for the four
/// physical holes, but accepted anywhere).
pub fn parse_grid(text: &str) -> Result<Vec<u16>> {
    let tokens = tokenize_grid(text)?;
    if tokens.len() != GRID_SIZE {
        bail!(
            "keys grid has {} token(s) but the {GRID_ROWS}x{GRID_COLS} grid needs exactly \
             {GRID_SIZE} (write `--` for the four holes and for unbound keys)",
            tokens.len()
        );
    }
    tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            if token == HOLE_TOKEN {
                return Ok(0x0000);
            }
            keycodes::parse_keycode(token).with_context(|| {
                format!(
                    "grid position {index} (r{},c{})",
                    index / usize::from(GRID_COLS),
                    index % usize::from(GRID_COLS)
                )
            })
        })
        .collect()
}

/// One grid cell as canonical text: `--` for `KC_NO` at a hole, otherwise
/// the keycode name with internal spaces removed so each cell is a single
/// whitespace-free token.
fn grid_token(code: u16) -> String {
    if code == 0x0000 {
        return HOLE_TOKEN.into();
    }
    keycodes::format_keycode(code).replace(", ", ",")
}

/// Render a full layer as the canonical keys grid: one row per line,
/// columns aligned with two-space gutters, `--` for `KC_NO`. Deterministic,
/// so export -> apply -> export is textually stable.
pub fn format_grid(codes: &[u16]) -> String {
    let cols = usize::from(GRID_COLS);
    let cells: Vec<String> = codes.iter().map(|&code| grid_token(code)).collect();
    let mut widths = vec![0usize; cols];
    for (index, cell) in cells.iter().enumerate() {
        let column = index % cols;
        widths[column] = widths[column].max(cell.len());
    }
    cells
        .chunks(cols)
        .map(|row| {
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Apply / export over the host protocol
// ---------------------------------------------------------------------------

/// Progress milestones of a keymap apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapStage {
    /// Starting to write one layer's grid.
    LayerBegun { slot: u8, id: String },
    /// One KEYMAP_WRITE batch acknowledged; `written` of `total` positions
    /// of this layer are now stored.
    Batch {
        slot: u8,
        written: usize,
        total: usize,
    },
    /// A layer's grid is fully written.
    LayerDone { slot: u8, lossy: usize },
}

/// Outcome of a completed keymap apply.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeymapReport {
    /// Grid positions written across all layers.
    pub entries_written: usize,
    /// Entries whose firmware read-back differed from the request:
    /// `(layer slot, key, requested, stored)`.
    pub lossy: Vec<(u8, u8, u16, u16)>,
}

/// Check the device's advertised keymap shape against the canonical grid
/// and the file's layer usage.
#[cfg(test)]
pub fn check_device_grid(
    capabilities: &glove80_host_protocol::Capabilities,
    plans: &[LayerPlan],
) -> Result<()> {
    if capabilities.keymap_rows != GRID_ROWS || capabilities.keymap_cols != GRID_COLS {
        bail!(
            "the canonical keys grid is {GRID_ROWS}x{GRID_COLS} but the keyboard \
             advertises a {}x{} keymap",
            capabilities.keymap_rows,
            capabilities.keymap_cols
        );
    }
    if let Some(plan) = plans
        .iter()
        .find(|plan| plan.slot >= capabilities.layer_capacity)
    {
        bail!(
            "layer \"{}\" occupies slot {} but the keyboard advertises only \
             {} layer(s)",
            plan.id,
            plan.slot,
            capabilities.layer_capacity
        );
    }
    Ok(())
}

/// Write every planned layer, chunked by the advertised batch limit.
///
/// **Not transactional across batches**: each batch is all-or-nothing
/// device-side, but on a mid-apply failure earlier batches stay written.
/// The error then states exactly which layers/positions were stored;
/// nothing after the failed batch is attempted.
#[cfg(test)]
pub fn apply_keymap(
    client: &mut HostClient,
    plans: &[LayerPlan],
    mut stage: impl FnMut(KeymapStage),
) -> Result<KeymapReport> {
    let capabilities = client.keymap_capabilities()?;
    check_device_grid(&capabilities, plans)?;
    // Cap batches well below the advertised per-op limit: every entry costs
    // the firmware a per-key flash persist before it answers, and a full
    // 84-entry layer reliably blows past the response timeout on hardware
    // (single writes are fine). 21 entries = a quarter layer per exchange.
    const MAX_HW_FRIENDLY_BATCH: usize = 21;
    let batch_size = usize::from(capabilities.max_keymap_entries_per_op)
        .min(glove80_host_protocol::MAX_KEYMAP_ENTRIES_PER_MESSAGE)
        .min(MAX_HW_FRIENDLY_BATCH);
    let mut report = KeymapReport::default();
    let mut done: Vec<&str> = Vec::new();
    for plan in plans {
        stage(KeymapStage::LayerBegun {
            slot: plan.slot,
            id: plan.id.clone(),
        });
        let entries: Vec<KeymapEntry> = plan
            .codes
            .iter()
            .enumerate()
            .map(|(key, &keycode)| KeymapEntry {
                layer: plan.slot,
                key: key as u8,
                keycode,
            })
            .collect();
        let mut written_in_layer = 0usize;
        let mut layer_lossy = 0usize;
        for batch in entries.chunks(batch_size) {
            let readback = client.write_keymap(batch).map_err(|error| {
                error.context(format!(
                    "keymap apply interrupted: layer \"{}\" (slot {}) has keys \
                     0..{} written and keys {}..{} untouched; {} keymap writes are \
                     NOT rolled back",
                    plan.id,
                    plan.slot,
                    written_in_layer,
                    written_in_layer,
                    entries.len(),
                    if done.is_empty() {
                        "earlier".to_string()
                    } else {
                        format!("the fully-written layer(s) {} and earlier", done.join(", "))
                    },
                ))
            })?;
            for (entry, &stored) in batch.iter().zip(&readback) {
                if stored != entry.keycode {
                    layer_lossy += 1;
                    report
                        .lossy
                        .push((entry.layer, entry.key, entry.keycode, stored));
                }
            }
            written_in_layer += batch.len();
            report.entries_written += batch.len();
            stage(KeymapStage::Batch {
                slot: plan.slot,
                written: written_in_layer,
                total: entries.len(),
            });
        }
        stage(KeymapStage::LayerDone {
            slot: plan.slot,
            lossy: layer_lossy,
        });
        done.push(&plan.id);
    }
    Ok(report)
}

/// Read every layer the device advertises, for export. Returns
/// `(slot, codes)` pairs; the caller synthesizes IDs.
#[cfg(test)]
pub fn read_all_layers(client: &mut HostClient) -> Result<Vec<Vec<u16>>> {
    let capabilities = client.keymap_capabilities()?;
    check_device_grid(&capabilities, &[])?;
    (0..capabilities.layer_capacity)
        .map(|layer| {
            client
                .read_keymap_layer(layer)
                .with_context(|| format!("could not read keymap layer {layer}"))
        })
        .collect()
}

/// Turn device layers into `[[layer]]` entries with synthesized IDs
/// `layer0..layerN` (the device stores no IDs or names). Trailing layers
/// that are entirely `KC_NO` are omitted; interior empty layers are kept
/// so file position keeps matching the firmware slot.
pub fn layers_to_entries(layers: &[Vec<u16>]) -> Vec<LayerEntry> {
    let last_used = layers
        .iter()
        .rposition(|codes| codes.iter().any(|&code| code != 0))
        .map_or(0, |index| index + 1);
    layers[..last_used]
        .iter()
        .enumerate()
        .map(|(slot, codes)| LayerEntry {
            id: format!("layer{slot}"),
            name: None,
            keys: Some(format!("\n{}\n", format_grid(codes))),
        })
        .collect()
}

/// Human summary of the keymap section: one line per layer.
pub fn render_keymap_summary(plans: &[LayerPlan]) -> String {
    let mut out = String::new();
    for plan in plans {
        let bound = plan
            .codes
            .iter()
            .enumerate()
            .filter(|(index, &code)| code != 0 && !GRID_HOLES.contains(&(*index as u8)))
            .count();
        out.push_str(&format!(
            "layer {} \"{}\"{}: {bound} bound key(s)\n",
            plan.slot,
            plan.id,
            plan.name
                .as_deref()
                .map(|name| format!(" ({name})"))
                .unwrap_or_default(),
        ));
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trns_grid_with(overrides: &[(usize, &str)]) -> String {
        let mut tokens = vec!["_______".to_string(); GRID_SIZE];
        for &hole in &GRID_HOLES {
            tokens[usize::from(hole)] = HOLE_TOKEN.into();
        }
        for &(index, token) in overrides {
            tokens[index] = token.into();
        }
        tokens
            .chunks(usize::from(GRID_COLS))
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn tokenizer_handles_parens_comments_and_whitespace() {
        let tokens = tokenize_grid(
            "KC_A LT(1, KC_ESC)  MT(MOD_LSFT|MOD_LALT, KC_A) # trailing comment\n\
             # whole-line comment\n  --\tKC_B\n",
        )
        .unwrap();
        assert_eq!(
            tokens,
            [
                "KC_A",
                "LT(1,KC_ESC)",
                "MT(MOD_LSFT|MOD_LALT,KC_A)",
                "--",
                "KC_B"
            ]
            .map(str::to_string)
        );
        assert!(tokenize_grid("LT(1, KC_A").is_err());
        assert!(tokenize_grid("LT 1)").is_err());
    }

    #[test]
    fn grid_round_trips_through_text() {
        let text = trns_grid_with(&[
            (0, "KC_F1"),
            (6, "LT(1,KC_ESC)"),
            (20, "MT(MOD_LSFT|MOD_LALT,KC_A)"),
            (83, "TG(3)"),
        ]);
        let codes = parse_grid(&text).unwrap();
        assert_eq!(codes.len(), GRID_SIZE);
        assert_eq!(codes[0], 0x003A);
        assert_eq!(codes[5], 0x0000); // hole
        assert_eq!(codes[6], 0x4129);
        assert_eq!(codes[20], 0x2604);
        assert_eq!(codes[83], 0x5263);
        let formatted = format_grid(&codes);
        assert_eq!(parse_grid(&formatted).unwrap(), codes);
        // Formatting is deterministic (a second round trip is textual).
        assert_eq!(format_grid(&parse_grid(&formatted).unwrap()), formatted);
        // One row per line, aligned.
        assert_eq!(formatted.lines().count(), usize::from(GRID_ROWS));
    }

    #[test]
    fn grid_rejects_wrong_token_counts_and_bad_codes() {
        let error = parse_grid("KC_A KC_B").unwrap_err();
        assert!(error.to_string().contains("needs exactly 84"), "{error}");
        let text = trns_grid_with(&[(30, "KC_NOPE")]);
        let error = parse_grid(&text).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("grid position 30 (r2,c2)"), "{chain}");
    }

    #[test]
    fn layer_plans_enforce_ids_and_capacity() {
        let layer = |id: &str, keys: Option<&str>| LayerEntry {
            id: id.into(),
            name: None,
            keys: keys.map(str::to_string),
        };
        let grid = trns_grid_with(&[]);
        let plans =
            build_layer_plans(&[layer("base", Some(&grid)), layer("ref-only", None)]).unwrap();
        assert_eq!(plans.len(), 1); // keys-less layers produce no plan
        assert_eq!(plans[0].slot, 0);

        let error = build_layer_plans(&[layer("a", None), layer("a", None)]).unwrap_err();
        assert!(error.to_string().contains("earlier layer"), "{error}");
        let error = build_layer_plans(&[layer("3", None)]).unwrap_err();
        assert!(error.to_string().contains("purely numeric"), "{error}");
        let error = build_layer_plans(&[layer("", None)]).unwrap_err();
        assert!(error.to_string().contains("empty"), "{error}");
        let nine: Vec<LayerEntry> = (0..9)
            .map(|index| layer(&format!("l{index}"), None))
            .collect();
        let error = build_layer_plans(&nine).unwrap_err();
        assert!(error.to_string().contains("capacity"), "{error}");
    }

    #[test]
    fn layer_refs_resolve_ids_and_pass_integers() {
        let layers = vec![
            LayerEntry {
                id: "base".into(),
                name: None,
                keys: None,
            },
            LayerEntry {
                id: "lower".into(),
                name: None,
                keys: None,
            },
        ];
        assert_eq!(
            resolve_layer_ref(&LayerRef::Id("lower".into()), &layers).unwrap(),
            1
        );
        assert_eq!(resolve_layer_ref(&LayerRef::Index(7), &layers).unwrap(), 7);
        let error = resolve_layer_ref(&LayerRef::Id("upper".into()), &layers).unwrap_err();
        assert!(
            error.to_string().contains("unknown layer id \"upper\""),
            "{error}"
        );
        assert!(error.to_string().contains("\"base\", \"lower\""), "{error}");
        let error = resolve_layer_ref(&LayerRef::Id("x".into()), &[]).unwrap_err();
        assert!(error.to_string().contains("none"), "{error}");
    }

    #[test]
    fn export_synthesizes_ids_and_drops_trailing_empty_layers() {
        let mut used = vec![0u16; GRID_SIZE];
        used[10] = 0x0004;
        let empty = vec![0u16; GRID_SIZE];
        let entries = layers_to_entries(&[used.clone(), empty.clone(), used, empty.clone(), empty]);
        // Interior empty layer 1 kept (position = slot), trailing dropped.
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "layer0");
        assert_eq!(entries[1].id, "layer1");
        assert_eq!(entries[2].id, "layer2");
        let codes = parse_grid(entries[2].keys.as_ref().unwrap()).unwrap();
        assert_eq!(codes[10], 0x0004);
    }
}
