//! Persistent lighting configuration apply (Phase 4 of
//! docs/implementation-plan.md): the layer between the opaque stored blob
//! (`config_store.rs`) and the live lighting state.
//!
//! Blob decoding and validation are entirely the shared protocol crate's
//! (`glove80_host_protocol::config`, protocol v1.1): storage below this
//! layer never interprets the bytes, and this layer never re-implements the
//! wire format. Applying a decoded config:
//!
//! - splits every record into its left-half cells (keys `0..40`, applied to
//!   the central's own compositor) and its right-half cells (keys `40..80`,
//!   remapped to local `0..40` and handed to the central's split state for
//!   transfer to the peripheral),
//! - swaps both record sets atomically (the compositor's `replace_records`
//!   is validate-first; the peripheral swaps only on a complete
//!   `ConfigCommit` — see `glove80_compositor::sync::ConfigStage`),
//! - applies the stored toggle state: non-persisted toggles take their
//!   `toggle_initial_state` bit; toggles in `toggle_persist_mask` keep the
//!   current runtime state ("toggle state is non-persistent unless the
//!   toggle opts in"). At boot the runtime state is all-off, so persisted
//!   toggles currently boot off until runtime write-back of toggle flips is
//!   implemented (a follow-up: it needs its own small flash record so a
//!   toggle keypress does not rewrite the whole blob).

use glove80_compositor::{Activation, Compositor, Condition, MAX_RECORDS, Record};
use glove80_host_protocol::config::{
    ConfigActivation, ConfigError as BlobError, ConfigGate, LightingConfig, decode_lighting_config,
};

use crate::lighting::NUM_LEDS;
use crate::split_lighting::SplitRole;

/// Keys per half; blob keys `0..40` are left/central, `40..80` right/remote.
const LEDS_PER_HALF: u8 = NUM_LEDS as u8;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ConfigError {
    /// The blob failed the shared decoder/validator; nothing was changed.
    Invalid(BlobError),
    /// The decoded config exceeds a compositor capacity. Unreachable for a
    /// validated blob (the protocol's capacities mirror the compositor's);
    /// kept as an explicit error rather than a panic path.
    Capacity,
}

/// Blob activation → compositor activation (persistable kinds only, by
/// construction of [`ConfigActivation`]).
fn activation(a: ConfigActivation) -> Activation {
    match a {
        ConfigActivation::Always => Activation::Always,
        ConfigActivation::LayerActive(layer) => Activation::LayerActive(layer),
        ConfigActivation::Toggle(id) => Activation::Toggle(id),
    }
}

/// Blob gate → compositor condition. The protocol validator has already
/// range-checked every argument before this conversion runs.
fn gate(g: ConfigGate) -> Condition {
    match g {
        ConfigGate::LayerActive(layer) => Condition::LayerActive(layer),
        ConfigGate::Toggle(id) => Condition::Toggle(id),
        ConfigGate::UsbConnected => Condition::UsbConnected,
        ConfigGate::Charging => Condition::Charging,
        ConfigGate::SplitLinkUp => Condition::SplitLinkUp,
    }
}

/// Build one half's record set from a decoded config into `out` (reused
/// scratch): for each blob record, the cells on this half (remapped to local
/// keys), same activation, same order. Records keep their identity on both
/// halves even when one half has no cells, so composition order stays
/// aligned across the board.
fn build_half(
    cfg: &LightingConfig,
    right_half: bool,
    out: &mut [Record; MAX_RECORDS],
) -> Result<usize, ConfigError> {
    for (slot, rec) in out.iter_mut().zip(cfg.records.iter()) {
        let mut r = Record::new(activation(rec.activation));
        r.set_gate(rec.gate.map(gate));
        for cell in rec.cells.iter() {
            let local = if right_half {
                if cell.key < LEDS_PER_HALF {
                    continue;
                }
                cell.key - LEDS_PER_HALF
            } else {
                if cell.key >= LEDS_PER_HALF {
                    continue;
                }
                cell.key
            };
            r.set(local, crate::host_proto::effect_to_cell(&cell.effect))
                .map_err(|_| ConfigError::Capacity)?;
        }
        *slot = r;
    }
    Ok(cfg.records.len())
}

/// Apply a decoded config to the live lighting state. Owned by the lighting
/// task (the compositor's single owner). Effects, in order:
///
/// 1. the central compositor's records are atomically replaced with the
///    left-half set (validate-first; on error nothing changes),
/// 2. the toggle bitmask is rebuilt per the persist/initial masks (then
///    mirrored to the peripheral via the state snapshot),
/// 3. the right-half set is handed to the central split state, which
///    streams it to the peripheral (and re-streams it on every link-up).
pub fn apply_decoded(
    comp: &mut Compositor<NUM_LEDS>,
    role: &mut SplitRole,
    cfg: &LightingConfig,
    now_ms: u64,
) -> Result<(), ConfigError> {
    // One reusable scratch set: build local, swap it in, rebuild as remote.
    let mut half = [Record::new(Activation::Always); MAX_RECORDS];
    let n = build_half(cfg, false, &mut half)?;
    comp.replace_records(&half[..n])
        .map_err(|_| ConfigError::Capacity)?;

    // Persisted toggles keep the runtime state; the rest take their stored
    // initial state (see the module docs for the boot caveat).
    let toggles = (comp.toggles_mask() & cfg.toggle_persist_mask)
        | (cfg.toggle_initial_state & !cfg.toggle_persist_mask);
    comp.set_toggles_mask(toggles);

    if let Some(central) = role.central_mut() {
        let n = build_half(cfg, true, &mut half)?;
        central.set_persistent_records(&half[..n], now_ms);
        central.notify_state(comp, now_ms);
    }
    Ok(())
}

/// Decode + apply in one step: the boot-load and session-commit entry point.
/// Any error leaves the previous lighting state in force.
pub fn apply_blob(
    comp: &mut Compositor<NUM_LEDS>,
    role: &mut SplitRole,
    blob: &[u8],
    now_ms: u64,
) -> Result<(), ConfigError> {
    let cfg = decode_lighting_config(blob).map_err(ConfigError::Invalid)?;
    apply_decoded(comp, role, &cfg, now_ms)
}
