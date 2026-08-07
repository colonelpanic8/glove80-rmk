//! `lighting …` commands and their host-side parsing.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use rynk::rmk_types::protocol::rynk::{LightingReplicaStatus, LightingReplicationHealth};

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
    /// Read the frame most recently presented by one lighting node.
    Frame {
        /// Lighting node id (0 is the central/left half, 1 the peripheral/right half).
        #[arg(default_value_t = 0)]
        node: u8,
        /// Emit a machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Diagnose split lighting replication and digest agreement.
    ReplicaStatus {
        /// Emit a machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Atomically replace the overlay from `KEY COLOR [EFFECT] [option=value]` lines.
    Replace {
        /// File to read; `-` or omission reads stdin.
        file: Option<PathBuf>,
        #[arg(long, value_name = "MS")]
        ttl: Option<u32>,
    },
    /// Read or set global brightness (0-255).
    Brightness { value: Option<u8> },
    /// Read durable per-layer scene cells.
    SceneRead,
    /// Set one or more durable scene cells on a layer.
    SceneSet {
        layer: u8,
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
    },
    /// Remove one or more durable scene cells from a layer.
    SceneUnset {
        layer: u8,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// List extension effect parameters, or set one of them.
    ///
    /// `params` lists every effect that advertises parameters, `params EFFECT`
    /// lists one effect's, and `params EFFECT NAME VALUE` writes one.
    Params {
        /// Effect name as advertised by the extension descriptor.
        effect: Option<String>,
        /// Parameter name as advertised by the effect.
        name: Option<String>,
        /// New value, within the parameter's advertised range.
        value: Option<u8>,
    },
    /// Read or set how active layer scenes are composed.
    ScenePolicy {
        #[arg(value_enum)]
        policy: Option<LayerPolicyArg>,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LayerPolicyArg {
    EffectiveOnly,
    ActiveStack,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicaVerdict {
    InSync,
    Resyncing,
    Stale,
    Diverged,
    Halted,
    Unavailable,
    Unattested,
}

impl ReplicaVerdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "IN SYNC",
            Self::Resyncing => "RESYNCING",
            Self::Stale => "STALE",
            Self::Diverged => "DIVERGED",
            Self::Halted => "HALTED",
            Self::Unavailable => "UNAVAILABLE",
            Self::Unattested => "UNATTESTED",
        }
    }
}

pub fn replica_verdict(status: &LightingReplicaStatus, freshness_ms: u32) -> ReplicaVerdict {
    let Some(machine) = status.replication.as_ref() else {
        return ReplicaVerdict::Unavailable;
    };
    if !machine.link_up || status.peripheral.is_none() {
        return ReplicaVerdict::Unavailable;
    }
    if machine.health == LightingReplicationHealth::Halted {
        return ReplicaVerdict::Halted;
    }
    if machine.awaiting_ack
        || machine.durable_dirty
        || machine.context_dirty
        || machine.health == LightingReplicationHealth::Resynchronizing
    {
        return ReplicaVerdict::Resyncing;
    }
    if machine.health == LightingReplicationHealth::Diverged {
        return ReplicaVerdict::Diverged;
    }
    let peripheral = status.peripheral.as_ref().expect("checked above");
    if peripheral.age_ms > freshness_ms || machine.health == LightingReplicationHealth::Stale {
        return ReplicaVerdict::Stale;
    }
    let (Some(expected), Some(observed)) = (
        machine.expected_digests.as_ref(),
        peripheral.digests.as_ref(),
    ) else {
        return ReplicaVerdict::Unattested;
    };
    if expected.revision == observed.revision && expected != observed {
        return ReplicaVerdict::Diverged;
    }
    let context_matches = status.central.effective_layer == peripheral.effective_layer
        && status.central.default_layer == peripheral.default_layer
        && status.central.active_bits == peripheral.active_bits
        && status.central.powered == peripheral.powered
        && status.central.wake_active == peripheral.wake_active
        && status.central.effective_output_enabled == peripheral.effective_output_enabled;
    if expected == observed
        && peripheral.applied_revision == machine.last_acked_revision
        && context_matches
    {
        ReplicaVerdict::InSync
    } else {
        ReplicaVerdict::Stale
    }
}

pub use glove80_config::parse_color;

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
    use rynk::rmk_types::protocol::rynk::{
        LightingCentralReplicaState, LightingNodeId, LightingPeripheralReplicaState,
        LightingReplicaDigests, LightingReplicationMachine,
    };

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

    fn status(health: LightingReplicationHealth) -> LightingReplicaStatus {
        let digests = LightingReplicaDigests {
            schema: 1,
            revision: 7,
            settings: 1,
            overlay: 2,
            scenes: 3,
            conditional_scenes: 4,
        };
        LightingReplicaStatus {
            central: LightingCentralReplicaState {
                revision: 7,
                presented_revision: Some(7),
                effective_layer: 2,
                default_layer: 0,
                active_bits: 5,
                powered: true,
                wake_active: false,
                effective_output_enabled: true,
            },
            replication: Some(LightingReplicationMachine {
                last_acked_revision: Some(7),
                awaiting_ack: false,
                generation: 1,
                link_up: true,
                durable_dirty: false,
                context_dirty: false,
                health,
                expected_digests: Some(digests),
                last_attested_age_ms: Some(10),
                mismatch_count: 0,
            }),
            peripheral: Some(LightingPeripheralReplicaState {
                node: LightingNodeId(1),
                applied_revision: Some(7),
                engine_revision: 9,
                effective_layer: 2,
                default_layer: 0,
                active_bits: 5,
                powered: true,
                wake_active: false,
                effective_output_enabled: true,
                age_ms: 10,
                digests: Some(digests),
            }),
        }
    }

    #[test]
    fn classifies_replica_status_without_presentation_heuristics() {
        let healthy = status(LightingReplicationHealth::Healthy);
        assert_eq!(replica_verdict(&healthy, 30_000), ReplicaVerdict::InSync);

        let mut resyncing = healthy.clone();
        resyncing.replication.as_mut().unwrap().awaiting_ack = true;
        assert_eq!(
            replica_verdict(&resyncing, 30_000),
            ReplicaVerdict::Resyncing
        );

        let mut unattested = healthy.clone();
        unattested.peripheral.as_mut().unwrap().digests = None;
        assert_eq!(
            replica_verdict(&unattested, 30_000),
            ReplicaVerdict::Unattested
        );

        let mut stale = healthy.clone();
        stale.peripheral.as_mut().unwrap().age_ms = 30_001;
        assert_eq!(replica_verdict(&stale, 30_000), ReplicaVerdict::Stale);

        let mut divergent = healthy.clone();
        divergent
            .peripheral
            .as_mut()
            .unwrap()
            .digests
            .as_mut()
            .unwrap()
            .overlay = 99;
        assert_eq!(
            replica_verdict(&divergent, 30_000),
            ReplicaVerdict::Diverged
        );

        let halted = status(LightingReplicationHealth::Halted);
        assert_eq!(replica_verdict(&halted, 30_000), ReplicaVerdict::Halted);

        let mut unavailable = healthy;
        unavailable.replication.as_mut().unwrap().link_up = false;
        assert_eq!(
            replica_verdict(&unavailable, 30_000),
            ReplicaVerdict::Unavailable
        );
    }
}
