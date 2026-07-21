//! `lighting …` commands and their host-side parsing.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};

use crate::transport::Selector;

#[derive(Subcommand)]
pub enum LightingCommand {
    /// Round-trip a Rynk protocol query.
    Ping {
        /// Optional text retained for CLI compatibility; Rynk ignores it.
        #[arg(long)]
        data: Option<String>,
    },
    /// Show lighting capabilities and topology.
    Caps,
    /// Set one or more overlay cells.
    Set {
        /// LED indices as comma-separated values and ranges.
        keys: String,
        /// #RRGGBB, RRGGBB, or a named color.
        color: String,
        #[arg(long, value_enum, default_value_t = EffectArg::Solid)]
        effect: EffectArg,
        #[arg(long, value_name = "MS")]
        period: Option<u16>,
        #[arg(long, value_name = "MS")]
        phase: Option<u16>,
        #[arg(long, value_name = "PCT")]
        duty: Option<u8>,
        #[arg(long, value_name = "MS")]
        ttl: Option<u32>,
    },
    /// Remove one or more cells from the overlay.
    Unset {
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Clear the entire overlay.
    Clear,
    /// Read current lighting and split state.
    Read,
    /// Atomically replace the overlay from `KEY COLOR [EFFECT] [option=value]` lines.
    Replace {
        /// File to read; `-` or omission reads stdin.
        file: Option<PathBuf>,
        #[arg(long, value_name = "MS")]
        ttl: Option<u32>,
    },
    /// Read or set global brightness (0-255).
    Brightness { value: Option<u8> },
}

#[derive(Args, Clone, Copy)]
pub struct BootloaderArgs {
    /// Reboot the peripheral half instead of the central.
    #[arg(long)]
    pub peripheral: bool,
    /// Skip the confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum EffectArg {
    Solid,
    Blink,
    Breathe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSpec {
    pub kind: EffectArg,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub period_ms: u16,
    pub phase_ms: u16,
    pub duty_percent: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSpec {
    pub key: u8,
    pub effect: EffectSpec,
}

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

pub fn parse_key_list(text: &str) -> Result<Vec<u8>> {
    let mut keys = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("empty key in '{text}'");
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: u8 = start
                .parse()
                .with_context(|| format!("bad key '{start}'"))?;
            let end: u8 = end.parse().with_context(|| format!("bad key '{end}'"))?;
            if start > end {
                bail!("key range {start}-{end} is reversed");
            }
            keys.extend(start..=end);
        } else {
            keys.push(part.parse().with_context(|| format!("bad key '{part}'"))?);
        }
    }
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

pub fn build_effect(
    kind: EffectArg,
    rgb: (u8, u8, u8),
    period: Option<u16>,
    phase: Option<u16>,
    duty: Option<u8>,
) -> Result<EffectSpec> {
    let (period_ms, phase_ms, duty_percent) = match kind {
        EffectArg::Solid => {
            if period.is_some() || phase.is_some() || duty.is_some() {
                bail!("solid effects do not accept period, phase, or duty");
            }
            (0, 0, 0)
        }
        EffectArg::Blink => {
            let period = period.unwrap_or(1000);
            let duty = duty.unwrap_or(50);
            if period == 0 || duty > 100 {
                bail!("blink period must be positive and duty must be 0-100");
            }
            (period, phase.unwrap_or(0), duty)
        }
        EffectArg::Breathe => {
            if duty.is_some() {
                bail!("breathe effects do not accept duty");
            }
            let period = period.unwrap_or(1000);
            if period == 0 {
                bail!("breathe period must be positive");
            }
            (period, phase.unwrap_or(0), 0)
        }
    };
    Ok(EffectSpec {
        kind,
        red: rgb.0,
        green: rgb.1,
        blue: rgb.2,
        period_ms,
        phase_ms,
        duty_percent,
    })
}

pub fn parse_replace_spec(text: &str) -> Result<Vec<CellSpec>> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            (!line.is_empty() && !line.starts_with('#')).then_some((index + 1, line))
        })
        .map(|(line_number, line)| {
            parse_replace_line(line).with_context(|| format!("line {line_number}"))
        })
        .collect()
}

fn parse_replace_line(line: &str) -> Result<CellSpec> {
    let mut tokens = line.split_whitespace();
    let key = tokens.next().context("missing key")?.parse()?;
    let color = parse_color(tokens.next().context("missing color")?)?;
    let mut kind = EffectArg::Solid;
    let mut period = None;
    let mut phase = None;
    let mut duty = None;
    for token in tokens {
        if let Some(value) = token.strip_prefix("period=") {
            period = Some(value.parse()?);
        } else if let Some(value) = token.strip_prefix("phase=") {
            phase = Some(value.parse()?);
        } else if let Some(value) = token.strip_prefix("duty=") {
            duty = Some(value.parse()?);
        } else {
            kind = match token {
                "solid" => EffectArg::Solid,
                "blink" => EffectArg::Blink,
                "breathe" => EffectArg::Breathe,
                _ => bail!("unknown effect or option '{token}'"),
            };
        }
    }
    Ok(CellSpec {
        key,
        effect: build_effect(kind, color, period, phase, duty)?,
    })
}

pub fn run(selector: &Selector, command: &LightingCommand) -> Result<()> {
    crate::rynk_client::run_lighting(selector, command)
}

pub fn run_bootloader(selector: &Selector, peripheral: bool, yes: bool) -> Result<()> {
    let half = if peripheral { "peripheral" } else { "central" };
    if !yes {
        print!("Reboot the {half} half into its UF2 bootloader? [y/N] ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .context("could not read confirmation")?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }
    crate::rynk_client::run_bootloader(selector, peripheral)?;
    println!("{half} half accepted the Rynk bootloader request");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors_and_ranges() {
        assert_eq!(parse_color("#ff0066").unwrap(), (0xff, 0, 0x66));
        assert_eq!(parse_color("Orange").unwrap(), (0xff, 0x80, 0));
        assert_eq!(parse_key_list("0-3,12,3").unwrap(), vec![0, 1, 2, 3, 12]);
    }

    #[test]
    fn validates_effect_options() {
        assert!(build_effect(EffectArg::Solid, (1, 2, 3), Some(10), None, None).is_err());
        assert!(build_effect(EffectArg::Blink, (1, 2, 3), None, None, Some(101)).is_err());
        assert!(build_effect(EffectArg::Breathe, (1, 2, 3), None, None, Some(10)).is_err());
    }
}
