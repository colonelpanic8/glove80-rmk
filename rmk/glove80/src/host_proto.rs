//! Host protocol semantics and plumbing shared by both halves (Phase 2 of
//! docs/implementation-plan.md). The transport pumps themselves are
//! central-only and live in `host_pump.rs`; `lighting.rs` only ever sees
//! decoded [`HostRequest`]s and produces [`HostResponse`]s. The flow:
//!
//! ```text
//!  USB OUT reports ─┐ (rmk::host_proto_pipe, vendored GLOVE80 PATCH)
//!                   ├─> pump: reassemble -> decode ──> HOST_REQUESTS ─┐
//!  BLE ATT writes ──┘         (host_pump.rs)                         │
//!                                                    LightingProcessor
//!                                                (single owner of the
//!                                                 compositor: applies the
//!                                                 request via [`apply`])
//!  USB IN reports <─┐                                                │
//!                   ├── pump: encode -> frame <── response mailbox <─┘
//!  BLE notifies  <──┘
//! ```
//!
//! Split scope (Phase 2): overlay key space is `0..80` (left half `0..40`,
//! right half `40..80`). The central applies keys 0-39 locally; keys 40-79
//! are accepted and reported per the protocol's `PARTIAL_APPLY` ack, but the
//! cells themselves are dropped until Phase 3 adds split forwarding — they do
//! not appear in `READ_OVERLAY` and nothing is queued for the peripheral.

use embassy_sync::channel::Channel;
use glove80_compositor::{Cell, Compositor, Rgb};
use glove80_host_protocol::{
    BOOTLOADER_MAGIC, BootTarget, Capabilities, CellState, Effect, EffectKind,
    MAX_CELLS_PER_MESSAGE, MAX_MESSAGE_LEN, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR,
    Request, Response, ResponsePayload, Status, feature,
};
use rmk::RawMutex;

use crate::lighting::NUM_LEDS;

/// Keys per half on the wire (`led_count_left` / `led_count_right`).
const LEDS_PER_HALF: u8 = NUM_LEDS as u8;
/// Total overlay key space: left half `0..40`, right half `40..80`.
const TOTAL_KEYS: u8 = LEDS_PER_HALF * 2;
/// Keymap layers, matching `[layout] layers` in keyboard.toml.
const LAYER_CAPACITY: u8 = 8;
/// Toggle ids the compositor's toggle bitmask supports (`0..32`).
const TOGGLE_ID_LIMIT: u8 = 32;

/// Which transport a request arrived on (responses go back the same way).
// Only the central's pumps (host_pump.rs) construct these; the peripheral
// compiles this module for the shared request path but never feeds it.
#[allow(dead_code)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Transport {
    Usb,
    Ble,
}

/// A decoded request on its way to the lighting task.
pub struct HostRequest {
    pub transport: Transport,
    pub request_id: u8,
    pub request: Request,
}

/// The lighting task's answer, routed back to the requesting transport.
// Only the central's pumps read the fields; see the `Transport` note above.
#[allow(dead_code)]
pub struct HostResponse {
    pub response: Response,
    /// Reboot to the bootloader (best effort) after this response is flushed.
    pub enter_bootloader: bool,
}

/// Decoded requests from the transport pumps to the lighting task. On the
/// peripheral nothing sends here (no pumps), so the lighting loop's receive
/// arm simply never fires.
pub static HOST_REQUESTS: Channel<RawMutex, HostRequest, 2> = Channel::new();
/// Per-transport response mailboxes. Capacity 1 is exact: a pump has at most
/// one request outstanding and always drains the mailbox before the next.
pub static USB_RESPONSES: Channel<RawMutex, HostResponse, 1> = Channel::new();
pub static BLE_RESPONSES: Channel<RawMutex, HostResponse, 1> = Channel::new();

/// Route a response back to the transport its request came from. Called by
/// the lighting task; never blocks in practice (see mailbox sizing above).
pub async fn respond(transport: Transport, response: HostResponse) {
    match transport {
        Transport::Usb => USB_RESPONSES.send(response).await,
        Transport::Ble => BLE_RESPONSES.send(response).await,
    }
}

// --- Request semantics (called by the lighting task) -----------------------

/// Capabilities advertised to hosts. Every feature bit is backed by working
/// firmware; `PARTIAL_APPLY` covers the Phase 2 right-half pending semantics
/// documented at module level.
fn capabilities() -> Capabilities {
    Capabilities {
        protocol_major: PROTOCOL_VERSION_MAJOR,
        protocol_minor: PROTOCOL_VERSION_MINOR,
        led_count_left: LEDS_PER_HALF,
        led_count_right: LEDS_PER_HALF,
        layer_capacity: LAYER_CAPACITY,
        max_cells_per_op: MAX_CELLS_PER_MESSAGE as u8,
        // Bit n <=> EffectKind n: solid, blink, breathe.
        effect_mask: (1 << EffectKind::Solid as u16)
            | (1 << EffectKind::Blink as u16)
            | (1 << EffectKind::Breathe as u16),
        overlay_cell_capacity: TOTAL_KEYS as u16,
        max_message_len: MAX_MESSAGE_LEN as u16,
        feature_bits: feature::TTL
            | feature::TOGGLES
            | feature::BOOTLOADER_ENTRY
            | feature::ATOMIC_REPLACE
            | feature::OVERLAY_READBACK
            | feature::PARTIAL_APPLY,
    }
}

/// Wire effect -> compositor cell. Every wire kind is representable.
fn effect_to_cell(e: &Effect) -> Cell {
    let color = Rgb::new(e.r, e.g, e.b);
    match e.kind {
        EffectKind::Solid => Cell::Solid { color },
        EffectKind::Blink => Cell::Blink {
            color,
            period_ms: e.period_ms,
            phase_ms: e.phase_ms,
            duty_pct: e.duty_percent,
        },
        EffectKind::Breathe => Cell::Breathe { color, period_ms: e.period_ms, phase_ms: e.phase_ms },
    }
}

/// Compositor cell -> wire effect. `Transparent` has no wire form (the
/// protocol expresses transparency by *unsetting* a key), so it is `None`;
/// nothing writes transparent cells into the overlay today.
fn cell_to_effect(cell: &Cell) -> Option<Effect> {
    Some(match *cell {
        Cell::Transparent => return None,
        Cell::Solid { color } => Effect::solid(color.r, color.g, color.b),
        Cell::Blink { color, period_ms, phase_ms, duty_pct } => {
            Effect::blink(color.r, color.g, color.b, period_ms, phase_ms, duty_pct)
        }
        Cell::Breathe { color, period_ms, phase_ms } => {
            Effect::breathe(color.r, color.g, color.b, period_ms, phase_ms)
        }
    })
}

/// Wire TTL (0 = none) -> compositor TTL.
fn wire_ttl(ttl_ms: u32) -> Option<u32> {
    (ttl_ms != 0).then_some(ttl_ms)
}

/// Split a batch's keys: `Err(())` if any key is outside `0..TOTAL_KEYS`
/// (whole operation rejected, overlay untouched), otherwise the deduplicated
/// right-half keys that Phase 2 must report as pending.
fn pending_right_half_keys<'a, I: Iterator<Item = &'a u8>>(
    keys: I,
) -> Result<heapless::Vec<u8, MAX_CELLS_PER_MESSAGE>, ()> {
    let mut seen: u128 = 0;
    let mut pending = heapless::Vec::new();
    for &key in keys {
        if key >= TOTAL_KEYS {
            return Err(());
        }
        if key >= LEDS_PER_HALF && seen & (1 << key) == 0 {
            seen |= 1 << key;
            // Cannot overflow: at most TOTAL_KEYS unique keys fit easily.
            let _ = pending.push(key);
        }
    }
    Ok(pending)
}

/// Ack an overlay write: `OK` when fully applied, `PARTIAL_APPLY` listing the
/// right-half keys accepted for (Phase 3) forwarding but not yet applied.
fn overlay_ack(pending: heapless::Vec<u8, MAX_CELLS_PER_MESSAGE>) -> (Status, ResponsePayload) {
    let status = if pending.is_empty() { Status::Ok } else { Status::PartialApply };
    (status, ResponsePayload::OverlayAck { pending_keys: pending })
}

/// Apply one decoded request to the compositor and produce its response.
///
/// Pure with respect to everything but `comp`; MUST only be called by the
/// compositor's single owner (the lighting task). A bootloader-entry OK sets
/// [`HostResponse::enter_bootloader`]; the transport pump reboots after the
/// response is flushed.
pub fn apply(
    comp: &mut Compositor<NUM_LEDS>,
    request_id: u8,
    req: &Request,
    now_ms: u64,
) -> HostResponse {
    let command = req.command();
    let mut enter_bootloader = false;
    let (status, payload) = match req {
        Request::GetCapabilities { client_major, client_minor: _ } => {
            if *client_major != PROTOCOL_VERSION_MAJOR {
                (Status::UnsupportedVersion, ResponsePayload::Empty)
            } else {
                (Status::Ok, ResponsePayload::Capabilities(capabilities()))
            }
        }
        Request::Ping { data } => (Status::Ok, ResponsePayload::Echo { data: data.clone() }),
        Request::SetCells { ttl_ms, cells } => {
            match pending_right_half_keys(cells.iter().map(|c| &c.key)) {
                Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
                Ok(pending) => {
                    let mut ok = true;
                    for cell in cells.iter().filter(|c| c.key < LEDS_PER_HALF) {
                        // Cannot fail while keys < LEDS_PER_HALF <= overlay
                        // capacity (one slot per key); guarded anyway.
                        if comp
                            .host_set(cell.key, effect_to_cell(&cell.effect), wire_ttl(*ttl_ms), now_ms)
                            .is_err()
                        {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        overlay_ack(pending)
                    } else {
                        (Status::CapacityExceeded, ResponsePayload::Empty)
                    }
                }
            }
        }
        Request::UnsetCells { keys } => match pending_right_half_keys(keys.iter()) {
            Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
            Ok(pending) => {
                for &key in keys.iter().filter(|&&k| k < LEDS_PER_HALF) {
                    comp.host_unset(key);
                }
                overlay_ack(pending)
            }
        },
        Request::ClearOverlay => {
            // Phase 2: the right half can hold no host cells (nothing is ever
            // forwarded), so a clear is fully effective -> plain OK.
            comp.host_clear();
            overlay_ack(heapless::Vec::new())
        }
        Request::ReadOverlay => {
            let mut cells: heapless::Vec<CellState, MAX_CELLS_PER_MESSAGE> = heapless::Vec::new();
            for (key, cell, expires_at) in comp.host_cells() {
                // Entries past their expiry linger until the next render;
                // don't report them (their remaining TTL would be 0 = "no
                // TTL" on the wire).
                let remaining_ttl_ms = match expires_at {
                    Some(at) if at <= now_ms => continue,
                    Some(at) => (at - now_ms).min(u32::MAX as u64) as u32,
                    None => 0,
                };
                if let Some(effect) = cell_to_effect(&cell) {
                    // Capacity: at most 40 overlay cells << 80.
                    let _ = cells.push(CellState { key, effect, remaining_ttl_ms });
                }
            }
            (Status::Ok, ResponsePayload::OverlayState { cells })
        }
        Request::ReplaceOverlay { ttl_ms, cells } => {
            match pending_right_half_keys(cells.iter().map(|c| &c.key)) {
                Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
                Ok(pending) => {
                    let mut left: heapless::Vec<(u8, Cell, Option<u32>), MAX_CELLS_PER_MESSAGE> =
                        heapless::Vec::new();
                    for c in cells.iter().filter(|c| c.key < LEDS_PER_HALF) {
                        // Capacity: left cells <= batch size <= 80.
                        let _ = left.push((c.key, effect_to_cell(&c.effect), wire_ttl(*ttl_ms)));
                    }
                    // Validate-first and atomic inside the compositor.
                    match comp.host_replace(&left, now_ms) {
                        Ok(()) => overlay_ack(pending),
                        Err(_) => (Status::CapacityExceeded, ResponsePayload::Empty),
                    }
                }
            }
        }
        Request::GetBrightness => {
            (Status::Ok, ResponsePayload::Brightness { level: comp.brightness() })
        }
        Request::SetBrightness { level } => {
            comp.set_brightness(*level);
            (Status::Ok, ResponsePayload::Brightness { level: comp.brightness() })
        }
        Request::GetToggle { id } => {
            if *id >= TOGGLE_ID_LIMIT {
                (Status::UnknownToggle, ResponsePayload::Empty)
            } else {
                (Status::Ok, ResponsePayload::Toggle { id: *id, state: comp.toggle(*id) })
            }
        }
        Request::SetToggle { id, state } => {
            if *id >= TOGGLE_ID_LIMIT {
                (Status::UnknownToggle, ResponsePayload::Empty)
            } else {
                comp.set_toggle(*id, *state);
                (Status::Ok, ResponsePayload::Toggle { id: *id, state: comp.toggle(*id) })
            }
        }
        Request::EnterBootloader { magic, target } => {
            if *magic != BOOTLOADER_MAGIC {
                (Status::BadMagic, ResponsePayload::Empty)
            } else {
                match target {
                    BootTarget::Central => {
                        enter_bootloader = true;
                        (Status::Ok, ResponsePayload::Empty)
                    }
                    // Peripheral bootloader entry needs the Phase 3 split
                    // channel; unsupported for now. The protocol has no
                    // dedicated "unsupported" status, so this reports the
                    // target as out of range (documented in PROTOCOL.md's
                    // transport addendum and the firmware README).
                    BootTarget::Peripheral => (Status::OutOfRange, ResponsePayload::Empty),
                }
            }
        }
    };
    HostResponse {
        response: Response { request_id, command, status, payload },
        enter_bootloader,
    }
}
