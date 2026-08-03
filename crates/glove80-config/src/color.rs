//! Colour spellings accepted anywhere a config or command names a colour.

use anyhow::{bail, Result};

const NAMED_COLORS: &[(&str, (u8, u8, u8))] = &[
    ("red", (0xff, 0, 0)),
    ("green", (0, 0xff, 0)),
    ("blue", (0, 0, 0xff)),
    ("white", (0xff, 0xff, 0xff)),
    ("black", (0, 0, 0)),
    ("off", (0, 0, 0)),
    ("yellow", (0xff, 0xff, 0)),
    ("cyan", (0, 0xff, 0xff)),
    ("magenta", (0xff, 0, 0xff)),
    ("orange", (0xff, 0x80, 0)),
    ("purple", (0x80, 0, 0xff)),
    ("pink", (0xff, 0x69, 0xb4)),
];

pub fn parse_color(text: &str) -> Result<(u8, u8, u8)> {
    let lowered = text.to_ascii_lowercase();
    if let Some((_, rgb)) = NAMED_COLORS.iter().find(|(name, _)| *name == lowered) {
        return Ok(*rgb);
    }
    let hex = lowered
        .strip_prefix('#')
        .or_else(|| lowered.strip_prefix("0x"))
        .unwrap_or(&lowered);
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("color '{text}' must be #RRGGBB, RRGGBB, 0xRRGGBB, or a named color");
    }
    let value = u32::from_str_radix(hex, 16)?;
    Ok(((value >> 16) as u8, (value >> 8) as u8, value as u8))
}
