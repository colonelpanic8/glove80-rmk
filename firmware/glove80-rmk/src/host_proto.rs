//! Host protocol semantics and plumbing shared by both halves (Phase 2 of
//! docs/implementation-plan.md). The transport pumps themselves are
//! central-only and live in `host_pump.rs`; `lighting.rs` only ever sees
//! decoded [`HostRequest`]s and produces [`HostResponse`]s. The flow:
//!
//! ```text
//!  USB OUT reports ─┐ (rmk::vendor_transport)
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
//! Split scope (Phase 3): overlay key space is `0..80` (left half `0..40`,
//! right half `40..80`). The central applies keys 0-39 locally; keys 40-79
//! land in the central's authoritative right-half store
//! ([`crate::split_lighting::CentralSplit`]) and are forwarded to the
//! peripheral over the split application channel. Writes answer `OK` when
//! the peripheral link is up and the deltas went out; `PARTIAL_APPLY`
//! (listing the right-half keys) only when the peripheral is genuinely
//! unavailable — the cells then land via the reconnect resync.
//! `READ_OVERLAY` always reports all 80 keys from the central's stores.

use embassy_sync::channel::Channel;
use glove80_compositor::{Cell, Compositor, Rgb};
use glove80_host_protocol::config::{
    ConfigError as BlobError, MAX_CONFIG_BLOB_LEN, crc32, validate_lighting_config,
};
use glove80_host_protocol::{
    BOOTLOADER_MAGIC, BootTarget, Capabilities, CellState, Effect, EffectKind, HalfVersion,
    MAX_CELLS_PER_MESSAGE, MAX_CONFIG_DATA_PER_MESSAGE, MAX_MESSAGE_LEN, PROTOCOL_VERSION_MAJOR,
    PROTOCOL_VERSION_MINOR, Request, Response, ResponsePayload, Status, VersionInfo, feature,
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
            | feature::PARTIAL_APPLY
            | feature::PERSISTENT_CONFIG
            | feature::VERSION_REPORT
            | feature::CONFIG_GATES,
        // The storage slots hold more (config_store::CONFIG_BLOB_MAX), so
        // the protocol's own maximum is the binding limit.
        max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
        // Keymap ownership has moved to Rynk. Keep these legacy capability
        // extension fields zero when the KEYMAP bit is absent.
        keymap_rows: 0,
        keymap_cols: 0,
        max_keymap_entries_per_op: 0,
    }
}
const _: () = assert!(MAX_CONFIG_BLOB_LEN <= crate::config_store::CONFIG_BLOB_MAX);

/// GET_VERSION payload (v1.3): this build for the central entry, the split
/// link's cached announcement for the peripheral entry. The peripheral keeps
/// its last-known fields with `present = false` while the link is down;
/// all-zero fields = never seen since boot (PROTOCOL.md "GET_VERSION").
fn version_info(role: &crate::split_lighting::SplitRole) -> VersionInfo {
    let own = crate::split_lighting::own_version();
    let central = HalfVersion {
        present: true,
        fw_major: own.major,
        fw_minor: own.minor,
        fw_patch: own.patch,
        git_hash: own.git_hash,
        dirty: own.dirty,
    };
    // The pumps only run on the central; answered defensively ("never seen")
    // if this were ever reached on the peripheral.
    let (last_seen, link_up) = role
        .as_central()
        .map(|c| c.peripheral_version())
        .unwrap_or((None, false));
    let peripheral = match last_seen {
        Some(v) => HalfVersion {
            present: link_up,
            fw_major: v.major,
            fw_minor: v.minor,
            fw_patch: v.patch,
            git_hash: v.git_hash,
            dirty: v.dirty,
        },
        None => HalfVersion::default(),
    };
    let halves_mismatch = central.present
        && peripheral.present
        && (central.git_hash != peripheral.git_hash
            || (central.fw_major, central.fw_minor, central.fw_patch)
                != (
                    peripheral.fw_major,
                    peripheral.fw_minor,
                    peripheral.fw_patch,
                ));
    VersionInfo {
        central,
        peripheral,
        halves_mismatch,
    }
}

/// Wire effect -> compositor cell. Every wire kind is representable. Shared
/// with the persistent-config apply path (`lighting_config.rs`).
pub fn effect_to_cell(e: &Effect) -> Cell {
    let color = Rgb::new(e.r, e.g, e.b);
    match e.kind {
        EffectKind::Solid => Cell::Solid { color },
        EffectKind::Blink => Cell::Blink {
            color,
            period_ms: e.period_ms,
            phase_ms: e.phase_ms,
            duty_pct: e.duty_percent,
        },
        EffectKind::Breathe => Cell::Breathe {
            color,
            period_ms: e.period_ms,
            phase_ms: e.phase_ms,
        },
    }
}

/// Compositor cell -> wire effect. `Transparent` has no wire form (the
/// protocol expresses transparency by *unsetting* a key), so it is `None`;
/// nothing writes transparent cells into the overlay today.
fn cell_to_effect(cell: &Cell) -> Option<Effect> {
    Some(match *cell {
        Cell::Transparent => return None,
        Cell::Solid { color } => Effect::solid(color.r, color.g, color.b),
        Cell::Blink {
            color,
            period_ms,
            phase_ms,
            duty_pct,
        } => Effect::blink(color.r, color.g, color.b, period_ms, phase_ms, duty_pct),
        Cell::Breathe {
            color,
            period_ms,
            phase_ms,
        } => Effect::breathe(color.r, color.g, color.b, period_ms, phase_ms),
    })
}

/// Wire TTL (0 = none) -> compositor TTL.
fn wire_ttl(ttl_ms: u32) -> Option<u32> {
    (ttl_ms != 0).then_some(ttl_ms)
}

/// Split a batch's keys: `Err(())` if any key is outside `0..TOTAL_KEYS`
/// (whole operation rejected, overlay untouched), otherwise the deduplicated
/// right-half keys (protocol ids `40..80`) — reported as pending when the
/// peripheral is unavailable.
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
/// right-half keys accepted on the central but not yet applied on the
/// (unavailable) peripheral.
fn overlay_ack(pending: heapless::Vec<u8, MAX_CELLS_PER_MESSAGE>) -> (Status, ResponsePayload) {
    let status = if pending.is_empty() {
        Status::Ok
    } else {
        Status::PartialApply
    };
    (
        status,
        ResponsePayload::OverlayAck {
            pending_keys: pending,
        },
    )
}

/// Right-half entries of a cell batch, remapped to the peripheral's LOCAL
/// key space (protocol key − 40) with the wire effects converted.
fn right_local_cells(
    cells: &[glove80_host_protocol::CellWrite],
) -> heapless::Vec<(u8, Cell), MAX_CELLS_PER_MESSAGE> {
    let mut out = heapless::Vec::new();
    for c in cells.iter().filter(|c| c.key >= LEDS_PER_HALF) {
        // Capacity: right entries <= batch size <= the Vec's capacity.
        let _ = out.push((c.key - LEDS_PER_HALF, effect_to_cell(&c.effect)));
    }
    out
}

/// Apply one decoded request to the compositor and produce its response.
///
/// Mutates only `comp` and `role` (the central's right-half store + split
/// delta queue); MUST only be called by the compositor's single owner (the
/// lighting task), which owns both. A bootloader-entry OK sets
/// [`HostResponse::enter_bootloader`]; the transport pump reboots after the
/// response is flushed.
pub fn apply(
    comp: &mut Compositor<NUM_LEDS>,
    role: &mut crate::split_lighting::SplitRole,
    request_id: u8,
    req: &Request,
    now_ms: u64,
) -> HostResponse {
    let command = req.command();
    let mut enter_bootloader = false;
    let (status, payload) = match req {
        Request::GetCapabilities {
            client_major,
            client_minor: _,
        } => {
            if *client_major != PROTOCOL_VERSION_MAJOR {
                (Status::UnsupportedVersion, ResponsePayload::Empty)
            } else {
                (Status::Ok, ResponsePayload::Capabilities(capabilities()))
            }
        }
        Request::Ping { data } => (Status::Ok, ResponsePayload::Echo { data: data.clone() }),
        Request::GetVersion => (Status::Ok, ResponsePayload::Version(version_info(role))),
        Request::SetCells { ttl_ms, cells } => {
            match pending_right_half_keys(cells.iter().map(|c| &c.key)) {
                Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
                Ok(right_keys) => {
                    let mut ok = true;
                    for cell in cells.iter().filter(|c| c.key < LEDS_PER_HALF) {
                        // Cannot fail while keys < LEDS_PER_HALF <= overlay
                        // capacity (one slot per key); guarded anyway.
                        if comp
                            .host_set(
                                cell.key,
                                effect_to_cell(&cell.effect),
                                wire_ttl(*ttl_ms),
                                now_ms,
                            )
                            .is_err()
                        {
                            ok = false;
                            break;
                        }
                    }
                    if !ok {
                        (Status::CapacityExceeded, ResponsePayload::Empty)
                    } else {
                        let delivered = if right_keys.is_empty() {
                            true
                        } else {
                            match role.central_mut() {
                                Some(split) => split.write_cells(
                                    &right_local_cells(cells),
                                    wire_ttl(*ttl_ms),
                                    now_ms,
                                ),
                                None => false,
                            }
                        };
                        overlay_ack(if delivered {
                            heapless::Vec::new()
                        } else {
                            right_keys
                        })
                    }
                }
            }
        }
        Request::UnsetCells { keys } => match pending_right_half_keys(keys.iter()) {
            Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
            Ok(right_keys) => {
                for &key in keys.iter().filter(|&&k| k < LEDS_PER_HALF) {
                    comp.host_unset(key);
                }
                let delivered = if right_keys.is_empty() {
                    true
                } else {
                    match role.central_mut() {
                        Some(split) => {
                            let mut local: heapless::Vec<u8, MAX_CELLS_PER_MESSAGE> =
                                heapless::Vec::new();
                            for &key in &right_keys {
                                // Capacity: same length as right_keys.
                                let _ = local.push(key - LEDS_PER_HALF);
                            }
                            split.unset_keys(&local, now_ms)
                        }
                        None => false,
                    }
                };
                overlay_ack(if delivered {
                    heapless::Vec::new()
                } else {
                    right_keys
                })
            }
        },
        Request::ClearOverlay => {
            comp.host_clear();
            // While the peripheral is unreachable its overlay state is
            // unknowable (it self-clears only after the link-loss grace), so
            // an offline clear is reported as PARTIAL_APPLY with an empty
            // pending list, exactly as PROTOCOL.md specifies.
            let delivered = match role.central_mut() {
                Some(split) => split.clear(now_ms),
                None => true,
            };
            let status = if delivered {
                Status::Ok
            } else {
                Status::PartialApply
            };
            (
                status,
                ResponsePayload::OverlayAck {
                    pending_keys: heapless::Vec::new(),
                },
            )
        }
        Request::ReadOverlay => {
            let mut cells: heapless::Vec<CellState, MAX_CELLS_PER_MESSAGE> = heapless::Vec::new();
            // Left half from the compositor, right half from the central's
            // authoritative remote store — all 80 keys, TTLs included.
            let left = comp.host_cells();
            let right = role
                .as_central()
                .into_iter()
                .flat_map(|split| split.remote_cells())
                .map(|(key, cell, expires_at)| (key + LEDS_PER_HALF, cell, expires_at));
            for (key, cell, expires_at) in left.chain(right) {
                // Entries past their expiry linger until the next render /
                // expiry sweep; don't report them (their remaining TTL would
                // be 0 = "no TTL" on the wire).
                let remaining_ttl_ms = match expires_at {
                    Some(at) if at <= now_ms => continue,
                    Some(at) => (at - now_ms).min(u32::MAX as u64) as u32,
                    None => 0,
                };
                if let Some(effect) = cell_to_effect(&cell) {
                    // Capacity: at most 40 + 40 overlay cells == 80.
                    let _ = cells.push(CellState {
                        key,
                        effect,
                        remaining_ttl_ms,
                    });
                }
            }
            (Status::Ok, ResponsePayload::OverlayState { cells })
        }
        Request::ReplaceOverlay { ttl_ms, cells } => {
            match pending_right_half_keys(cells.iter().map(|c| &c.key)) {
                Err(()) => (Status::OutOfRange, ResponsePayload::Empty),
                Ok(right_keys) => {
                    let mut left: heapless::Vec<(u8, Cell, Option<u32>), MAX_CELLS_PER_MESSAGE> =
                        heapless::Vec::new();
                    for c in cells.iter().filter(|c| c.key < LEDS_PER_HALF) {
                        // Capacity: left cells <= batch size <= 80.
                        let _ = left.push((c.key, effect_to_cell(&c.effect), wire_ttl(*ttl_ms)));
                    }
                    // Validate-first and atomic inside the compositor; on
                    // failure nothing (local or remote) has changed.
                    match comp.host_replace(&left, now_ms) {
                        Ok(()) => {
                            // Replace covers the whole 80-key space: keys not
                            // listed become transparent on the right half too,
                            // so the remote store is always replaced (possibly
                            // with nothing).
                            let delivered = match role.central_mut() {
                                Some(split) => split.replace_cells(
                                    &right_local_cells(cells),
                                    wire_ttl(*ttl_ms),
                                    now_ms,
                                ),
                                None => right_keys.is_empty(),
                            };
                            if delivered {
                                overlay_ack(heapless::Vec::new())
                            } else {
                                // Offline: listed right-half keys are pending;
                                // an implicit right-half clear alone reports
                                // PARTIAL_APPLY with an empty list, like
                                // CLEAR_OVERLAY.
                                (
                                    Status::PartialApply,
                                    ResponsePayload::OverlayAck {
                                        pending_keys: right_keys,
                                    },
                                )
                            }
                        }
                        Err(_) => (Status::CapacityExceeded, ResponsePayload::Empty),
                    }
                }
            }
        }
        Request::GetBrightness => (
            Status::Ok,
            ResponsePayload::Brightness {
                level: comp.brightness(),
            },
        ),
        Request::SetBrightness { level } => {
            comp.set_brightness(*level);
            // Mirror shared state to the peripheral (best effort; the
            // reconnect resync also carries it). Not an overlay write, so the
            // ack is always plain OK with the value now in effect.
            if let Some(split) = role.central_mut() {
                split.notify_state(comp, now_ms);
            }
            (
                Status::Ok,
                ResponsePayload::Brightness {
                    level: comp.brightness(),
                },
            )
        }
        Request::GetToggle { id } => {
            if *id >= TOGGLE_ID_LIMIT {
                (Status::UnknownToggle, ResponsePayload::Empty)
            } else {
                (
                    Status::Ok,
                    ResponsePayload::Toggle {
                        id: *id,
                        state: comp.toggle(*id),
                    },
                )
            }
        }
        Request::SetToggle { id, state } => {
            if *id >= TOGGLE_ID_LIMIT {
                (Status::UnknownToggle, ResponsePayload::Empty)
            } else {
                comp.set_toggle(*id, *state);
                // See SetBrightness: shared state mirrors best-effort.
                if let Some(split) = role.central_mut() {
                    split.notify_state(comp, now_ms);
                }
                (
                    Status::Ok,
                    ResponsePayload::Toggle {
                        id: *id,
                        state: comp.toggle(*id),
                    },
                )
            }
        }
        // Persistent-config commands are routed to [`apply_config`] and
        // keymap commands to [`apply_keymap`] by the lighting task (they
        // need async access to state this function does not own); these
        // arms are unreachable there.
        Request::ConfigBegin { .. }
        | Request::ConfigData { .. }
        | Request::ConfigCommit
        | Request::ConfigAbort
        | Request::ConfigRead { .. }
        | Request::KeymapRead { .. }
        | Request::KeymapWrite { .. } => (Status::Busy, ResponsePayload::Empty),
        Request::EnterBootloader { magic, target } => {
            if *magic != BOOTLOADER_MAGIC {
                (Status::BadMagic, ResponsePayload::Empty)
            } else {
                match target {
                    BootTarget::Central => {
                        enter_bootloader = true;
                        (Status::Ok, ResponsePayload::Empty)
                    }
                    // Forwarded over the split application channel (magic-
                    // guarded on the wire too); the peripheral reboots via
                    // the same Adafruit bootloader GPREGRET mechanism as the
                    // central. OK = the request was dispatched to a
                    // connected peripheral; BUSY = peripheral offline (or
                    // queue momentarily full) — nothing happened, retry.
                    BootTarget::Peripheral => {
                        let dispatched = role
                            .central_mut()
                            .is_some_and(|split| split.request_peripheral_bootloader());
                        if dispatched {
                            (Status::Ok, ResponsePayload::Empty)
                        } else {
                            (Status::Busy, ResponsePayload::Empty)
                        }
                    }
                }
            }
        }
    };
    HostResponse {
        response: Response {
            request_id,
            command,
            status,
            payload,
        },
        enter_bootloader,
    }
}

// --- Persistent-config session (protocol v1.1, Phase 4) ---------------------

/// One open CONFIG_BEGIN → DATA → COMMIT transfer session. Exactly one
/// exists, on the central, shared across both transports (a BEGIN on either
/// replaces it); the assembled bytes accumulate in the central's RAM blob
/// buffer (`CentralConfig::blob_buf`).
pub struct ConfigSession {
    total_len: u32,
    blob_crc32: u32,
    received: u32,
}

/// Whether `req` is a persistent-config command (routed to [`apply_config`]
/// instead of [`apply`]).
pub fn is_config_request(req: &Request) -> bool {
    matches!(
        req,
        Request::ConfigBegin { .. }
            | Request::ConfigData { .. }
            | Request::ConfigCommit
            | Request::ConfigAbort
            | Request::ConfigRead { .. }
    )
}

/// Apply one persistent-config request (PROTOCOL.md "Persistent
/// configuration"). Called by the lighting task on the central, which owns
/// the compositor, the split state, and the config store/session — so the
/// whole commit (persist, activate, split push) happens under the single
/// owner. Flash traffic awaits the shared-flash service; key scanning is
/// unaffected.
pub async fn apply_config(
    comp: &mut Compositor<NUM_LEDS>,
    role: &mut crate::split_lighting::SplitRole,
    cfg: &mut crate::lighting::CentralConfig,
    request_id: u8,
    req: &Request,
    now_ms: u64,
) -> HostResponse {
    let command = req.command();
    let (status, payload) = match req {
        Request::ConfigBegin {
            total_len,
            blob_crc32,
        } => {
            // A new BEGIN always replaces any open session (even when it
            // itself is then rejected).
            cfg.session = None;
            if *total_len as usize > MAX_CONFIG_BLOB_LEN {
                (Status::CapacityExceeded, ResponsePayload::Empty)
            } else {
                cfg.session = Some(ConfigSession {
                    total_len: *total_len,
                    blob_crc32: *blob_crc32,
                    received: 0,
                });
                (Status::Ok, ResponsePayload::Empty)
            }
        }
        Request::ConfigData { offset, data } => match &mut cfg.session {
            None => (Status::NoSession, ResponsePayload::Empty),
            Some(s) => {
                let contiguous = *offset == s.received
                    && s.received as usize + data.len() <= s.total_len as usize;
                if !contiguous {
                    // Any DATA error aborts the session (restart with BEGIN).
                    cfg.session = None;
                    (Status::BadOffset, ResponsePayload::Empty)
                } else {
                    cfg.blob_buf[s.received as usize..][..data.len()].copy_from_slice(data);
                    s.received += data.len() as u32;
                    (Status::Ok, ResponsePayload::Empty)
                }
            }
        },
        Request::ConfigCommit => match cfg.session.take() {
            // Every COMMIT, success or failure, ends the session.
            None => (Status::NoSession, ResponsePayload::Empty),
            Some(s) if s.received < s.total_len => {
                (Status::ConfigIncomplete, ResponsePayload::Empty)
            }
            Some(s) => {
                let blob = &cfg.blob_buf[..s.total_len as usize];
                if crc32(blob) != s.blob_crc32 {
                    (Status::CrcMismatch, ResponsePayload::Empty)
                } else {
                    match validate_lighting_config(blob) {
                        Err(BlobError::CrcMismatch { .. }) => {
                            (Status::CrcMismatch, ResponsePayload::Empty)
                        }
                        Err(e) => {
                            defmt::warn!(
                                "host-proto: config rejected: {}",
                                defmt::Debug2Format(&e)
                            );
                            (Status::InvalidConfig, ResponsePayload::Empty)
                        }
                        Ok(()) => {
                            // Persist first (transactional: the previous
                            // config survives any failure or power loss),
                            // then activate live + stream to the peripheral.
                            match cfg.store.save(blob).await {
                                Err(e) => {
                                    defmt::error!(
                                        "host-proto: config store failed: {}",
                                        defmt::Debug2Format(&e)
                                    );
                                    (Status::Busy, ResponsePayload::Empty)
                                }
                                Ok(()) => match crate::lighting_config::apply_blob(
                                    comp, role, blob, now_ms,
                                ) {
                                    Ok(()) => (Status::Ok, ResponsePayload::Empty),
                                    // Unreachable for a validated blob.
                                    Err(_) => (Status::InvalidConfig, ResponsePayload::Empty),
                                },
                            }
                        }
                    }
                }
            }
        },
        Request::ConfigAbort => {
            cfg.session = None; // idempotent
            (Status::Ok, ResponsePayload::Empty)
        }
        Request::ConfigRead { offset, max_len } => match cfg.store.active_len() {
            // No stored config: total_len = 0, no bytes.
            None => (
                Status::Ok,
                ResponsePayload::ConfigData {
                    total_len: 0,
                    data: heapless::Vec::new(),
                },
            ),
            Some(total) => {
                if *offset as usize > total {
                    (Status::OutOfRange, ResponsePayload::Empty)
                } else {
                    let want = (*max_len as usize)
                        .min(MAX_CONFIG_DATA_PER_MESSAGE)
                        .min(total - *offset as usize);
                    let mut data: heapless::Vec<u8, MAX_CONFIG_DATA_PER_MESSAGE> =
                        heapless::Vec::new();
                    // Cannot fail: want <= the Vec's capacity.
                    let _ = data.resize(want, 0);
                    match cfg.store.read_active_at(*offset as usize, &mut data).await {
                        Ok(_) => (
                            Status::Ok,
                            ResponsePayload::ConfigData {
                                total_len: total as u32,
                                data,
                            },
                        ),
                        Err(e) => {
                            defmt::error!(
                                "host-proto: config read failed: {}",
                                defmt::Debug2Format(&e)
                            );
                            (Status::Busy, ResponsePayload::Empty)
                        }
                    }
                }
            }
        },
        // Non-config requests never reach here (see [`is_config_request`]).
        _ => (Status::UnknownCommand, ResponsePayload::Empty),
    };
    HostResponse {
        response: Response {
            request_id,
            command,
            status,
            payload,
        },
        enter_bootloader: false,
    }
}

// --- Legacy keymap commands -------------------------------------------------

/// Keymap ownership has moved to Rynk. Legacy requests remain decodable for
/// protocol compatibility but are no longer routed to an RMK-side bridge.
pub fn is_keymap_request(_req: &Request) -> bool {
    false
}

/// Defensive response for callers that bypass capability negotiation. New
/// clients must use Rynk's typed keymap endpoints.
pub async fn apply_keymap(request_id: u8, req: &Request) -> HostResponse {
    let command = req.command();
    HostResponse {
        response: Response {
            request_id,
            command,
            status: Status::UnknownCommand,
            payload: ResponsePayload::Empty,
        },
        enter_bootloader: false,
    }
}
