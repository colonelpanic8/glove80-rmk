//! `keymap …` commands and their host-side parsing/rendering.

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::keycodes;
use crate::transport::Selector;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeymapEntry {
    pub layer: u8,
    pub key: u8,
    pub keycode: u16,
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
    /// Search the keycode name table without connecting to a keyboard.
    Find { fragment: String },
}

pub fn parse_key_position(text: &str, rows: u8, cols: u8) -> Result<u8> {
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
    Ok(key as u8)
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
            Ok(KeymapEntry {
                layer: triple[0]
                    .parse()
                    .with_context(|| format!("bad layer '{}'", triple[0]))?,
                key: parse_key_position(&triple[1], rows, cols)?,
                keycode: keycodes::parse_keycode(&triple[2])?,
            })
        })
        .collect()
}

pub fn render_layer(
    layer: u8,
    keycodes_flat: &[u16],
    rows: u8,
    cols: u8,
    holes: &[u8],
    raw: bool,
) -> String {
    let columns = usize::from(cols);
    let mut output = format!("layer {layer}\n");
    for row in 0..usize::from(rows) {
        let cells = (0..columns)
            .map(|column| {
                let index = row * columns + column;
                let code = keycodes_flat[index];
                if holes.contains(&(index as u8)) && code == 0 {
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
                entry.key / cols,
                entry.key % cols,
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
    fn renders_lossy_writes() {
        let entries = [KeymapEntry {
            layer: 0,
            key: 28,
            keycode: 0x0004,
        }];
        assert!(render_write_outcome(&entries, &[0], 14).contains("LOSSY"));
    }
}
