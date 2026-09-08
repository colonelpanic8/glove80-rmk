//! `keymap …` commands and their host-side parsing/rendering.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use rynk::rmk_types::protocol::rynk::LAYER_NAME_MAX_LEN;

use crate::transport::Selector;
use crate::{keycodes, rynk_keycode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeymapEntry {
    pub layer: u8,
    pub key: u16,
    pub keycode: u16,
}

/// One layer slot's persistent metadata, as the firmware holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerName {
    pub layer: u8,
    pub occupied: bool,
    pub name: String,
}

#[derive(Subcommand)]
pub enum KeymapCommand {
    /// Read one layer (the default) or every layer as a keycode grid.
    Read {
        #[arg(long, conflicts_with = "all")]
        layer: Option<u8>,
        #[arg(long)]
        all: bool,
        /// Print raw hexadecimal VIA keycodes.
        #[arg(long)]
        raw: bool,
    },
    /// Write LAYER KEY KEYCODE triples.
    Set {
        #[arg(required = true, value_name = "LAYER KEY KEYCODE")]
        entries: Vec<String>,
    },
    /// Read or set the persistent default layer.
    Default { layer: Option<u8> },
    /// List the persistent layer names, or rename one layer.
    Name {
        /// Layer to rename. Omit to list every layer.
        layer: Option<u8>,
        /// New name, at most 32 UTF-8 bytes. Omit to read the layer.
        name: Option<String>,
    },
    /// Report physical matrix presses for a short diagnostic window.
    Monitor {
        /// Number of seconds to monitor.
        #[arg(long, default_value_t = 15)]
        seconds: u64,
    },
    /// Search the keycode name table without connecting to a keyboard.
    Find { fragment: String },
}

pub fn check_grid(rows: u8, cols: u8, layers: u8) -> Result<()> {
    if rows == 0 || cols == 0 || layers == 0 {
        bail!("Rynk reports an empty keymap ({layers} layers of {rows}x{cols})");
    }
    Ok(())
}

pub fn check_action_count(
    actions: &[rynk::rmk_types::action::KeyAction],
    capabilities: &rynk::rmk_types::protocol::rynk::DeviceCapabilities,
) -> Result<()> {
    let expected = usize::from(capabilities.num_rows)
        * usize::from(capabilities.num_cols)
        * usize::from(capabilities.num_layers);
    if actions.len() != expected {
        bail!(
            "Rynk returned {} key actions; expected {expected}",
            actions.len()
        );
    }
    Ok(())
}

pub fn holes(rows: u8, cols: u8) -> &'static [u16] {
    match (rows, cols) {
        (6, 14) => &[5, 8, 75, 78],
        (5, 14) => &[48, 49, 56, 57, 61, 62, 63, 64, 68, 69],
        _ => &[],
    }
}

pub fn pressed_positions(bitmap: &[u8], rows: u8, cols: u8) -> Vec<(u8, u8)> {
    let bytes_per_row = usize::from(cols).div_ceil(8);
    let mut positions = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let byte = usize::from(row) * bytes_per_row + usize::from(col) / 8;
            if bitmap
                .get(byte)
                .is_some_and(|value| value & (1 << (col % 8)) != 0)
            {
                positions.push((row, col));
            }
        }
    }
    positions
}

pub fn render_pressed(positions: &[(u8, u8)]) -> String {
    if positions.is_empty() {
        return "released".to_owned();
    }
    format!(
        "pressed: {}",
        positions
            .iter()
            .map(|(row, col)| format!("r{row},c{col}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

pub fn parse_key_position(text: &str, rows: u8, cols: u8) -> Result<u16> {
    check_grid(rows, cols, 1)?;
    let total = u16::from(rows) * u16::from(cols);
    let key = if let Some((row, col)) = text.split_once(',') {
        let row: u16 = row
            .trim()
            .parse()
            .with_context(|| format!("bad row in '{text}'"))?;
        let col: u16 = col
            .trim()
            .parse()
            .with_context(|| format!("bad column in '{text}'"))?;
        if row >= u16::from(rows) || col >= u16::from(cols) {
            bail!("position '{text}' is outside the {rows}x{cols} grid");
        }
        row * u16::from(cols) + col
    } else {
        text.trim()
            .parse()
            .with_context(|| format!("key '{text}' must be a flat index or row,col"))?
    };
    if key >= total {
        bail!(
            "key {key} is out of range (grid has positions 0..{})",
            total - 1
        );
    }
    Ok(key)
}

pub fn parse_set_entries(arguments: &[String], rows: u8, cols: u8) -> Result<Vec<KeymapEntry>> {
    if !arguments.len().is_multiple_of(3) {
        bail!(
            "expected LAYER KEY KEYCODE triples, got {} argument(s); e.g. \
             `keymap set 0 28 KC_A 0 2,3 MO(2)`",
            arguments.len()
        );
    }
    arguments
        .chunks(3)
        .map(|triple| {
            let keycode = keycodes::parse_keycode(&triple[2])?;
            if rynk_keycode::to_via_keycode(rynk_keycode::from_via_keycode(keycode)) != keycode {
                bail!(
                    "keycode '{}' cannot be represented faithfully by Rynk",
                    triple[2]
                );
            }
            Ok(KeymapEntry {
                layer: triple[0]
                    .parse()
                    .with_context(|| format!("bad layer '{}'", triple[0]))?,
                key: parse_key_position(&triple[1], rows, cols)?,
                keycode,
            })
        })
        .collect()
}

pub fn render_layer(
    layer: u8,
    keycodes_flat: &[u16],
    rows: u8,
    cols: u8,
    holes: &[u16],
    raw: bool,
) -> String {
    let columns = usize::from(cols);
    let mut output = format!("layer {layer}\n");
    for row in 0..usize::from(rows) {
        let cells = (0..columns)
            .map(|column| {
                let index = row * columns + column;
                let code = keycodes_flat[index];
                if holes.contains(&(index as u16)) && code == 0 {
                    "--".to_owned()
                } else if raw {
                    format!("0x{code:04X}")
                } else {
                    keycodes::format_keycode(code)
                }
            })
            .collect::<Vec<_>>();
        output.push_str(&cells.join("  "));
        output.push('\n');
    }
    output
}

/// Check a requested layer name against the fixed-capacity firmware slot.
pub fn parse_layer_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("a layer name must not be empty");
    }
    if trimmed.len() > LAYER_NAME_MAX_LEN {
        bail!(
            "layer name is {} bytes; the maximum is {LAYER_NAME_MAX_LEN}",
            trimmed.len()
        );
    }
    Ok(trimmed.to_owned())
}

pub fn render_layer_names(slots: &[LayerName]) -> String {
    slots
        .iter()
        .map(|slot| {
            if slot.occupied {
                format!("layer {}: {}", slot.layer, slot.name)
            } else {
                format!("layer {}: (vacant)", slot.layer)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_write_outcome(entries: &[KeymapEntry], readback: &[u16], cols: u8) -> String {
    let mut output = String::new();
    let mut lossy = 0;
    for (entry, stored) in entries.iter().zip(readback) {
        if entry.keycode != *stored {
            lossy += 1;
            output.push_str(&format!(
                "LOSSY layer {} key {} (r{},c{}): requested {}, stored {}\n",
                entry.layer,
                entry.key,
                entry.key / u16::from(cols),
                entry.key % u16::from(cols),
                keycodes::format_keycode(entry.keycode),
                keycodes::format_keycode(*stored),
            ));
        }
    }
    if lossy > 0 {
        output.push_str(&format!(
            "{lossy} of {} entries were stored with a different representation",
            entries.len()
        ));
    } else {
        output.push_str(&format!(
            "wrote {} entr{} (read-back matches; changes are live and persisted)",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" },
        ));
    }
    output
}

pub fn run(selector: &Selector, command: &KeymapCommand) -> Result<()> {
    if let KeymapCommand::Find { fragment } = command {
        println!("{}", render_find(fragment));
        return Ok(());
    }
    crate::rynk_client::run_keymap(selector, command)
}

pub fn render_find(fragment: &str) -> String {
    let hits = keycodes::search(fragment);
    if hits.is_empty() {
        return format!("no keycode matches '{fragment}'");
    }
    hits.into_iter()
        .map(|(code, canonical, aliases)| {
            let aliases = if aliases.is_empty() {
                String::new()
            } else {
                format!("  ({})", aliases.join(", "))
            };
            format!("0x{code:04X}  {canonical}{aliases}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positions_and_entries() {
        assert_eq!(parse_key_position("2,0", 6, 14).unwrap(), 28);
        assert!(parse_key_position("84", 6, 14).is_err());
        let entries = parse_set_entries(&["0".into(), "28".into(), "KC_A".into()], 6, 14).unwrap();
        assert_eq!(entries[0].keycode, 0x0004);
    }

    #[test]
    fn rejects_lossy_actions_before_writing_any_entries() {
        let arguments = ["0", "0", "KC_A", "0", "1", "0x52c0"].map(String::from);
        let error = parse_set_entries(&arguments, 5, 14).unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot be represented faithfully"));
        for code in ["KC_NO", "MO(3)", "LCTL(KC_C)"] {
            assert!(parse_set_entries(&["0".into(), "0".into(), code.into()], 5, 14).is_ok());
        }
    }

    #[test]
    fn rejects_removed_keyboard_actions() {
        for code in ["0x7780", "0x7c02", "QK_OUTPUT_AUTO", "OUT_AUTO"] {
            let arguments = ["0", "0", "KC_A", "0", "1", code].map(String::from);
            assert!(parse_set_entries(&arguments, 6, 14).is_err(), "{code}");
        }
        for code in ["QK_OUTPUT_USB", "QK_OUTPUT_BLUETOOTH", "QK_BOOT"] {
            let arguments = ["0", "0", code].map(String::from);
            assert!(parse_set_entries(&arguments, 6, 14).is_ok(), "{code}");
        }
    }

    #[test]
    fn supports_both_boards_and_large_matrices() {
        assert!(check_grid(6, 14, 16).is_ok());
        assert!(check_grid(5, 14, 16).is_ok());
        assert!(check_grid(0, 14, 16).is_err());
        assert!(check_grid(5, 0, 16).is_err());
        assert!(check_grid(5, 14, 0).is_err());
        assert!(parse_key_position("0", 0, 0).is_err());
        assert_eq!(parse_key_position("19,19", 20, 20).unwrap(), 399);
        assert!(parse_key_position("70", 5, 14).is_err());
        assert_eq!(parse_key_position("4,13", 5, 14).unwrap(), 69);
    }

    #[test]
    fn rejects_truncated_and_surplus_bulk_keymaps() {
        use rynk::rmk_types::{action::KeyAction, protocol::rynk::DeviceCapabilities};
        let capabilities = DeviceCapabilities {
            num_rows: 5,
            num_cols: 14,
            num_layers: 2,
            ..Default::default()
        };
        assert!(check_action_count(&vec![KeyAction::No; 140], &capabilities).is_ok());
        for length in [0, 70, 139, 141] {
            assert!(check_action_count(&vec![KeyAction::No; length], &capabilities).is_err());
        }
    }

    #[test]
    fn holes_match_stock_board_layouts() {
        for (board, rows) in [("glove80", 6), ("go60", 5)] {
            let path = format!(
                "{}/../{board}-rmk/keyboard.toml",
                env!("CARGO_MANIFEST_DIR")
            );
            let text = std::fs::read_to_string(path).unwrap();
            let layout = rynk_kle::decode_layout(&text).unwrap();
            let keys = &layout.variants[layout.default_variant as usize].keys;
            let missing: Vec<_> = (0..u16::from(rows) * 14)
                .filter(|index| {
                    !keys
                        .iter()
                        .any(|key| u16::from(key.row) * 14 + u16::from(key.col) == *index)
                })
                .collect();
            assert_eq!(holes(rows, 14), missing);
            let rendered = render_layer(
                0,
                &vec![0; usize::from(rows) * 14],
                rows,
                14,
                holes(rows, 14),
                false,
            );
            assert_eq!(rendered.matches("--").count(), missing.len());
        }
    }

    #[test]
    fn renders_lossy_writes() {
        let entries = [KeymapEntry {
            layer: 0,
            key: 28,
            keycode: 0x0004,
        }];
        assert!(render_write_outcome(&entries, &[0], 14).contains("LOSSY"));
    }

    #[test]
    fn decodes_row_major_pressed_bitmap() {
        assert_eq!(
            pressed_positions(&[0x01, 0x02, 0x40, 0x20], 2, 14),
            vec![(0, 0), (0, 9), (1, 6), (1, 13)]
        );
        assert_eq!(render_pressed(&[]), "released");
        assert_eq!(render_pressed(&[(2, 3), (4, 5)]), "pressed: r2,c3 r4,c5");
    }
}
