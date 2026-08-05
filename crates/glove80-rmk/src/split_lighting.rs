//! MoErgo semantic lighting replication over RMK's bounded split channel.
//!
//! The central remains the Rynk/Vial authority. It transfers declarative
//! standard-engine snapshots when engine state changes or the link reconnects,
//! and guarded context deltas otherwise. The peripheral applies a complete
//! staged snapshot atomically and renders every animation frame from its own
//! clock and compositor.

use core::num::NonZeroU32;

use rmk::lighting::compositor::{ExtensionLayerState, ExtensionState};
use rmk::lighting::standard::{EXTENSION_PARAM_CHUNK, ExtensionReplicaParams};
use rmk::lighting::{
    ActiveTransport, BackgroundMode, BackgroundState, BatteryCondition, BondedSlotCondition,
    BuiltinEffect, ChargeCondition, ConditionSet, ConnectionCondition, EffectsCondition,
    IndicatorState, LayerCondition, LayerPolicy, LayerState, LedSlot, LightingContext, OutputMode,
    OverlayBatch, OverlayCell, Rgb8, RuntimeConditionalSceneCell, RuntimeConditionalSceneTable,
    SceneTable, SceneTableCell, StandardMutableState, StandardReplicaState,
};
use rmk::split_app::{SPLIT_APP_MSG_MAX, SplitAppData};
use rmk::types::battery::{BatteryStatus, ChargeState};
use rmk::types::ble::BleState;
use rmk::types::connection::ConnectionStatus;

use crate::lighting::{BatteryPair, LEDS_PER_HALF, OVERLAY_CAPACITY, SCENE_CAPACITY, TOTAL_LEDS};

// Version 10 widens the conditional-scene extension packet with the effects,
// bonded-slot, and usb-connected predicates, and adds the bonded-slot bitmap
// to the context. A stale half would read those gates as absent, and an absent
// gate means "always matches" -- so both polarities of an indicator would light
// at once rather than the packet being rejected. Version 9 adds the
// conditional-scene connection extension packet (the cell packet itself is
// byte-for-byte full). Version 8 adds the optional second
// extension-effect packet. Version 7 widens the staged Extension packet with
// the active effect's tuning parameters; version 6 added that packet and the
// runtime conditional-scene packets. A stale half rejects the mismatched
// version and simply keeps its previous state until both halves are reflashed
// together.
const VERSION: u8 = 10;
const TAG_BEGIN: u8 = 1;
const TAG_CONTEXT: u8 = 2;
const TAG_CELL: u8 = 3;
const TAG_COMMIT: u8 = 4;
const TAG_ACK: u8 = 5;
const TAG_SCENE_CELL: u8 = 6;
const TAG_EXTENSION: u8 = 7;
const TAG_CONDITIONAL_SCENE_BEGIN: u8 = 8;
const TAG_CONDITIONAL_SCENE_CELL: u8 = 9;
const TAG_EXTENSION_OVERLAY: u8 = 10;
/// Optional transient traffic outside the atomic snapshot transaction. This
/// tag is additive, so it does not require changing the snapshot wire version:
/// an older peripheral simply ignores the unknown message.
const TAG_EFFECT_HIT: u8 = 11;
/// Standalone context traffic outside the atomic snapshot transaction. Like
/// effect hits, this is additive and does not change the snapshot wire version.
const TAG_CONTEXT_UPDATE: u8 = 12;
/// Connection condition for the immediately preceding conditional-scene cell,
/// sent only when that cell carries one. The cell packet is full, so the
/// condition rides in its own packet inside the same staged transaction.
const TAG_CONDITIONAL_SCENE_EXT: u8 = 13;

const BEGIN_LEN: usize = 26;
const CONTEXT_LEN: usize = 24;
/// Selection (5 bytes after the header) plus the parameter block: one
/// length byte, the effect the values belong to, and the values themselves.
const EXTENSION_LEN: usize = 14 + EXTENSION_PARAM_CHUNK;
const EXTENSION_OVERLAY_LEN: usize = 11 + EXTENSION_PARAM_CHUNK;
const CELL_LEN: usize = 26;
const SCENE_CELL_LEN: usize = 23;
const CONDITIONAL_SCENE_BEGIN_LEN: usize = 8;
const CONDITIONAL_SCENE_CELL_LEN: usize = 26;
const CONDITIONAL_SCENE_EXT_LEN: usize = 11;
const COMMIT_LEN: usize = 9;
const ACK_LEN: usize = 7;
const EFFECT_HIT_LEN: usize = 3;
const CONTEXT_UPDATE_LEN: usize = 24;
const _: () = assert!(CELL_LEN <= SPLIT_APP_MSG_MAX);
const _: () = assert!(EXTENSION_LEN <= SPLIT_APP_MSG_MAX);
const _: () = assert!(EXTENSION_OVERLAY_LEN <= SPLIT_APP_MSG_MAX);
const _: () = assert!(CONDITIONAL_SCENE_CELL_LEN <= SPLIT_APP_MSG_MAX);
const _: () = assert!(CONDITIONAL_SCENE_EXT_LEN <= SPLIT_APP_MSG_MAX);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Message {
    Begin {
        generation: u8,
        revision: u32,
        cell_count: u8,
        scene_count: u8,
        scene_policy: LayerPolicy,
        sample_time_ms: u64,
        mutable: StandardMutableState,
        output_mode: OutputMode,
    },
    Context {
        generation: u8,
        revision: u32,
        context: LightingContext,
        batteries: BatteryPair,
    },
    ContextUpdate {
        generation: u8,
        revision: u32,
        context: LightingContext,
        batteries: BatteryPair,
    },
    /// Extension-source selection and the active effect's tuning at snapshot
    /// time. Always part of a staged snapshot (the Context packet has no
    /// spare bytes for it); `extension: None` means the authority has no
    /// selectable extension source, and `params: None` means the active
    /// effect exposes no parameters. Both travel together because the
    /// peripheral's effect pack cannot render the authority's Rain or Storm
    /// identically without the tuning that goes with the selection.
    Extension {
        generation: u8,
        revision: u32,
        extension: Option<ExtensionState>,
        params: Option<ExtensionReplicaParams>,
    },
    ExtensionOverlay {
        generation: u8,
        revision: u32,
        overlay: Option<u8>,
        params: Option<ExtensionReplicaParams>,
    },
    Cell {
        generation: u8,
        revision: u32,
        cell: OverlayCell,
    },
    SceneCell {
        generation: u8,
        revision: u32,
        cell: SceneTableCell,
    },
    ConditionalSceneBegin {
        generation: u8,
        revision: u32,
        cell_count: u8,
    },
    ConditionalSceneCell {
        generation: u8,
        revision: u32,
        cell: RuntimeConditionalSceneCell,
    },
    /// Amends the most recently staged conditional-scene cell with its
    /// connection condition; sent immediately after that cell's packet.
    ConditionalSceneExt {
        generation: u8,
        revision: u32,
        connection: Option<ConnectionCondition>,
        effects: Option<EffectsCondition>,
    },
    Commit {
        generation: u8,
        revision: u32,
        cell_count: u8,
        scene_count: u8,
    },
    Ack {
        generation: u8,
        revision: u32,
    },
    /// A central-half key hit mirrored to the peripheral effect engine. The
    /// slot remains board-wide so both engines sample the same geometry.
    EffectHit {
        slot: LedSlot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Version,
    Tag,
    Length,
    Value,
}

impl Message {
    pub fn encode(self) -> SplitAppData {
        let mut out = [0u8; SPLIT_APP_MSG_MAX];
        out[0] = VERSION;
        let len = match self {
            Message::Begin {
                generation,
                revision,
                cell_count,
                scene_count,
                scene_policy,
                sample_time_ms,
                mutable,
                output_mode,
            } => {
                out[1] = TAG_BEGIN;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = cell_count;
                put_u64(&mut out, 8, sample_time_ms);
                out[16] = mutable.output_enabled as u8;
                out[17] = mutable.output_brightness;
                out[18] = mutable.background.enabled as u8;
                out[19] = mutable.background.hue;
                out[20] = mutable.background.saturation;
                out[21] = mutable.background.value;
                out[22] = mutable.background.speed;
                // Pack the two-value mode into the high bit of speed's wire
                // byte; speed itself remains lossless in the context packet's
                // spare indicator byte below.
                if mutable.background.mode == BackgroundMode::Breathe {
                    out[18] |= 0x80;
                }
                out[23] = scene_count;
                out[24] = match scene_policy {
                    LayerPolicy::EffectiveOnly => 0,
                    LayerPolicy::ActiveStack => 1,
                };
                out[25] = match output_mode {
                    OutputMode::AlwaysOn => 0,
                    OutputMode::AlwaysOff => 1,
                    OutputMode::PoweredOnly => 2,
                };
                BEGIN_LEN
            }
            Message::Context {
                generation,
                revision,
                context,
                batteries,
            } => {
                out[1] = TAG_CONTEXT;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = context.layers.effective;
                out[8] = context.layers.default;
                put_u64(&mut out, 9, context.layers.active_bits());
                out[17] = indicators(context.indicators);
                put_battery(&mut out, 18, batteries.left);
                put_battery(&mut out, 20, batteries.right);
                out[22] = context.powered as u8;
                out[23] = context.bonded_slots;
                CONTEXT_LEN
            }
            Message::ContextUpdate {
                generation,
                revision,
                context,
                batteries,
            } => {
                out[1] = TAG_CONTEXT_UPDATE;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = context.layers.effective;
                out[8] = context.layers.default;
                put_u64(&mut out, 9, context.layers.active_bits());
                out[17] = indicators(context.indicators);
                put_battery(&mut out, 18, batteries.left);
                put_battery(&mut out, 20, batteries.right);
                out[22] = context.powered as u8;
                out[23] = context.bonded_slots;
                CONTEXT_UPDATE_LEN
            }
            Message::Extension {
                generation,
                revision,
                extension,
                params,
            } => {
                out[1] = TAG_EXTENSION;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                if let Some(extension) = extension {
                    out[7] = 1;
                    out[8] = extension.effect;
                    out[9] = extension.palette;
                    out[10] = extension.value;
                    out[11] = extension.speed;
                }
                // A zero length means "no parameters"; the values array is
                // fixed-size so the packet length never varies.
                if let Some(params) = params {
                    let values = params.values();
                    out[12] = values.len() as u8;
                    out[13] = params.effect;
                    out[14..14 + values.len()].copy_from_slice(values);
                }
                EXTENSION_LEN
            }
            Message::ExtensionOverlay {
                generation,
                revision,
                overlay,
                params,
            } => {
                out[1] = TAG_EXTENSION_OVERLAY;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                if let Some(effect) = overlay {
                    out[7] = 1;
                    out[8] = effect;
                }
                if let Some(params) = params {
                    let values = params.values();
                    out[9] = values.len() as u8;
                    out[10] = params.effect;
                    out[11..11 + values.len()].copy_from_slice(values);
                }
                EXTENSION_OVERLAY_LEN
            }
            Message::Cell {
                generation,
                revision,
                cell,
            } => {
                out[1] = TAG_CELL;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = cell.slot.0 as u8;
                let (kind, color, period_ms, phase_ms, auxiliary) = match cell.effect {
                    BuiltinEffect::Solid { color } => (0, color, 0, 0, 0),
                    BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty,
                    } => (1, color, period_ms, phase_ms, duty as u16),
                    BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms,
                    } => (2, color, period_ms, phase_ms, step_ms),
                };
                out[8] = kind;
                out[9..12].copy_from_slice(&[color.r, color.g, color.b]);
                put_u32(&mut out, 12, period_ms);
                put_u32(&mut out, 16, phase_ms);
                put_u16(&mut out, 20, auxiliary);
                put_u32(&mut out, 22, cell.ttl_ms.map(NonZeroU32::get).unwrap_or(0));
                CELL_LEN
            }
            Message::SceneCell {
                generation,
                revision,
                cell,
            } => {
                out[1] = TAG_SCENE_CELL;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = cell.layer;
                out[8] = cell.slot.0 as u8;
                let (kind, color, period_ms, phase_ms, auxiliary) = match cell.effect {
                    BuiltinEffect::Solid { color } => (0, color, 0, 0, 0),
                    BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty,
                    } => (1, color, period_ms, phase_ms, duty as u16),
                    BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms,
                    } => (2, color, period_ms, phase_ms, step_ms),
                };
                out[9] = kind;
                out[10..13].copy_from_slice(&[color.r, color.g, color.b]);
                put_u32(&mut out, 13, period_ms);
                put_u32(&mut out, 17, phase_ms);
                put_u16(&mut out, 21, auxiliary);
                SCENE_CELL_LEN
            }
            Message::ConditionalSceneBegin {
                generation,
                revision,
                cell_count,
            } => {
                out[1] = TAG_CONDITIONAL_SCENE_BEGIN;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = cell_count;
                CONDITIONAL_SCENE_BEGIN_LEN
            }
            Message::ConditionalSceneCell {
                generation,
                revision,
                cell,
            } => {
                out[1] = TAG_CONDITIONAL_SCENE_CELL;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                let (kind, color, period_ms, phase_ms, auxiliary) = match cell.effect {
                    BuiltinEffect::Solid { color } => (0, color, 0, 0, 0),
                    BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty,
                    } => (1, color, period_ms, phase_ms, duty as u16),
                    BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms,
                    } => (2, color, period_ms, phase_ms, step_ms),
                };
                out[7] = cell.slot.0 as u8 | (kind & 1) << 7;
                out[8] = cell
                    .conditions
                    .layer
                    .map(|condition| condition.layer & 0x3f | (condition.active as u8) << 6)
                    .unwrap_or(0x3f)
                    | (kind & 2) << 6;
                if let Some(condition) = cell.conditions.battery {
                    let charge = match condition.charge {
                        ChargeCondition::Any => 0,
                        ChargeCondition::Charging => 1,
                        ChargeCondition::Discharging => 2,
                        ChargeCondition::Unknown => 3,
                    };
                    out[9] = condition.node & 0x3f | charge << 6;
                    out[10] = condition.min_level.unwrap_or(u8::MAX);
                    out[11] = condition.max_level.unwrap_or(u8::MAX);
                } else {
                    out[9..12].fill(u8::MAX);
                }
                out[12..15].copy_from_slice(&[color.r, color.g, color.b]);
                put_u32(&mut out, 15, period_ms);
                put_u32(&mut out, 19, phase_ms);
                put_u16(&mut out, 23, auxiliary);
                // A rule gated on the output mode has to carry that gate across
                // the link. Without it the peripheral would see no condition,
                // which reads as "always matches" -- so every mode's rule would
                // fire at once and the last one written would win.
                out[25] = match cell.conditions.output_mode {
                    None => u8::MAX,
                    Some(OutputMode::AlwaysOn) => 0,
                    Some(OutputMode::AlwaysOff) => 1,
                    Some(OutputMode::PoweredOnly) => 2,
                };
                CONDITIONAL_SCENE_CELL_LEN
            }
            Message::ConditionalSceneExt {
                generation,
                revision,
                connection,
                effects,
            } => {
                out[1] = TAG_CONDITIONAL_SCENE_EXT;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                let mut gates = 0u8;
                let connection = connection.unwrap_or_default();
                if let Some(transport) = connection.transport {
                    gates |= 0x80
                        | match transport {
                            ActiveTransport::Usb => 0,
                            ActiveTransport::Ble => 1,
                            ActiveTransport::NoneActive => 2,
                        };
                }
                if let Some(state) = connection.ble_state {
                    gates |= 0x40
                        | (match state {
                            BleState::Advertising => 0,
                            BleState::Connected => 1,
                            BleState::Inactive => 2,
                        } << 2);
                }
                // The two bits this byte had left over. An effects gate that
                // did not cross the link would reach the peripheral as no
                // condition at all, which reads as "always matches" -- both
                // polarities of an effects indicator would light at once.
                if let Some(effects) = effects {
                    gates |= 0x20 | (effects.enabled as u8) << 4;
                }
                out[7] = gates;
                out[8] = connection
                    .profile
                    .map(|profile| 0x80 | (profile & 0x7f))
                    .unwrap_or(0);
                out[9] = connection
                    .bonded
                    .map(|bonded| 0x80 | (bonded.bonded as u8) << 6 | (bonded.slot & 0x3f))
                    .unwrap_or(0);
                out[10] = match connection.usb_connected {
                    None => u8::MAX,
                    Some(connected) => connected as u8,
                };
                CONDITIONAL_SCENE_EXT_LEN
            }
            Message::Commit {
                generation,
                revision,
                cell_count,
                scene_count,
            } => {
                out[1] = TAG_COMMIT;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                out[7] = cell_count;
                out[8] = scene_count;
                COMMIT_LEN
            }
            Message::Ack {
                generation,
                revision,
            } => {
                out[1] = TAG_ACK;
                out[2] = generation;
                put_u32(&mut out, 3, revision);
                ACK_LEN
            }
            Message::EffectHit { slot } => {
                out[1] = TAG_EFFECT_HIT;
                out[2] = slot.0 as u8;
                EFFECT_HIT_LEN
            }
        };
        SplitAppData::new(&out[..len]).expect("semantic lighting packet is bounded")
    }

    pub fn decode(data: SplitAppData) -> Result<Self, DecodeError> {
        let bytes = data.payload();
        if bytes.first() != Some(&VERSION) {
            return Err(DecodeError::Version);
        }
        let tag = *bytes.get(1).ok_or(DecodeError::Length)?;
        match tag {
            TAG_BEGIN if bytes.len() == BEGIN_LEN => {
                let enabled_and_mode = bytes[18];
                Ok(Message::Begin {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    cell_count: bytes[7],
                    scene_count: bytes[23],
                    scene_policy: match bytes[24] {
                        0 => LayerPolicy::EffectiveOnly,
                        1 => LayerPolicy::ActiveStack,
                        _ => return Err(DecodeError::Value),
                    },
                    sample_time_ms: get_u64(bytes, 8),
                    mutable: StandardMutableState {
                        output_enabled: flag(bytes[16])?,
                        output_brightness: bytes[17],
                        background: BackgroundState {
                            enabled: enabled_and_mode & 0x7f != 0,
                            hue: bytes[19],
                            saturation: bytes[20],
                            value: bytes[21],
                            speed: bytes[22],
                            mode: if enabled_and_mode & 0x80 == 0 {
                                BackgroundMode::Solid
                            } else {
                                BackgroundMode::Breathe
                            },
                        },
                    },
                    output_mode: match bytes[25] {
                        0 => OutputMode::AlwaysOn,
                        1 => OutputMode::AlwaysOff,
                        2 => OutputMode::PoweredOnly,
                        _ => return Err(DecodeError::Value),
                    },
                })
            }
            TAG_CONTEXT if bytes.len() == CONTEXT_LEN => Ok(Message::Context {
                generation: bytes[2],
                revision: get_u32(bytes, 3),
                context: LightingContext {
                    layers: LayerState::new(bytes[7], bytes[8], get_u64(bytes, 9)),
                    indicators: get_indicators(bytes[17]),
                    powered: flag(bytes[22])?,
                    // Only the central holds bonds, so unlike `connection`
                    // the peripheral cannot read this from its own state.
                    bonded_slots: bytes[23],
                    // Not carried on the wire: each half reads its own copy of
                    // the rmk-synced connection status at snapshot time.
                    connection: ConnectionStatus::new(),
                },
                batteries: BatteryPair {
                    left: get_battery(bytes, 18)?,
                    right: get_battery(bytes, 20)?,
                },
            }),
            TAG_CONTEXT_UPDATE if bytes.len() == CONTEXT_UPDATE_LEN => Ok(Message::ContextUpdate {
                generation: bytes[2],
                revision: get_u32(bytes, 3),
                context: LightingContext {
                    layers: LayerState::new(bytes[7], bytes[8], get_u64(bytes, 9)),
                    indicators: get_indicators(bytes[17]),
                    powered: flag(bytes[22])?,
                    // Only the central holds bonds, so unlike `connection`
                    // the peripheral cannot read this from its own state.
                    bonded_slots: bytes[23],
                    // Not carried on the wire: each half reads its own copy of
                    // the rmk-synced connection status at snapshot time.
                    connection: ConnectionStatus::new(),
                },
                batteries: BatteryPair {
                    left: get_battery(bytes, 18)?,
                    right: get_battery(bytes, 20)?,
                },
            }),
            TAG_EXTENSION if bytes.len() == EXTENSION_LEN => {
                let param_len = bytes[12] as usize;
                if param_len > EXTENSION_PARAM_CHUNK {
                    return Err(DecodeError::Value);
                }
                let mut values = [0u8; EXTENSION_PARAM_CHUNK];
                values[..param_len].copy_from_slice(&bytes[14..14 + param_len]);
                Ok(Message::Extension {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    extension: if flag(bytes[7])? {
                        Some(ExtensionState {
                            effect: bytes[8],
                            palette: bytes[9],
                            value: bytes[10],
                            speed: bytes[11],
                        })
                    } else {
                        None
                    },
                    params: (param_len != 0).then_some(ExtensionReplicaParams {
                        effect: bytes[13],
                        len: param_len as u8,
                        values,
                    }),
                })
            }
            TAG_EXTENSION_OVERLAY if bytes.len() == EXTENSION_OVERLAY_LEN => {
                let param_len = bytes[9] as usize;
                if param_len > EXTENSION_PARAM_CHUNK {
                    return Err(DecodeError::Value);
                }
                let mut values = [0u8; EXTENSION_PARAM_CHUNK];
                values[..param_len].copy_from_slice(&bytes[11..11 + param_len]);
                Ok(Message::ExtensionOverlay {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    overlay: flag(bytes[7])?.then_some(bytes[8]),
                    params: (param_len != 0).then_some(ExtensionReplicaParams {
                        effect: bytes[10],
                        len: param_len as u8,
                        values,
                    }),
                })
            }
            TAG_CELL if bytes.len() == CELL_LEN => {
                let slot = bytes[7] as usize;
                if !(LEDS_PER_HALF..TOTAL_LEDS).contains(&slot) {
                    return Err(DecodeError::Value);
                }
                let color = Rgb8::new(bytes[9], bytes[10], bytes[11]);
                let period_ms = get_u32(bytes, 12);
                let phase_ms = get_u32(bytes, 16);
                let auxiliary = get_u16(bytes, 20);
                let effect = match bytes[8] {
                    0 => BuiltinEffect::Solid { color },
                    1 if auxiliary <= 100 => BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty: auxiliary as u8,
                    },
                    2 => BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms: auxiliary,
                    },
                    _ => return Err(DecodeError::Value),
                };
                Ok(Message::Cell {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    cell: OverlayCell {
                        slot: LedSlot(slot as u16),
                        effect,
                        ttl_ms: NonZeroU32::new(get_u32(bytes, 22)),
                    },
                })
            }
            TAG_SCENE_CELL if bytes.len() == SCENE_CELL_LEN => {
                let slot = bytes[8] as usize;
                if !(LEDS_PER_HALF..TOTAL_LEDS).contains(&slot) {
                    return Err(DecodeError::Value);
                }
                let color = Rgb8::new(bytes[10], bytes[11], bytes[12]);
                let period_ms = get_u32(bytes, 13);
                let phase_ms = get_u32(bytes, 17);
                let auxiliary = get_u16(bytes, 21);
                let effect = match bytes[9] {
                    0 => BuiltinEffect::Solid { color },
                    1 if auxiliary <= 100 => BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty: auxiliary as u8,
                    },
                    2 => BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms: auxiliary,
                    },
                    _ => return Err(DecodeError::Value),
                };
                Ok(Message::SceneCell {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    cell: SceneTableCell {
                        layer: bytes[7],
                        slot: LedSlot(slot as u16),
                        effect,
                    },
                })
            }
            TAG_CONDITIONAL_SCENE_BEGIN if bytes.len() == CONDITIONAL_SCENE_BEGIN_LEN => {
                Ok(Message::ConditionalSceneBegin {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    cell_count: bytes[7],
                })
            }
            TAG_CONDITIONAL_SCENE_CELL if bytes.len() == CONDITIONAL_SCENE_CELL_LEN => {
                let slot = (bytes[7] & 0x7f) as usize;
                if !(LEDS_PER_HALF..TOTAL_LEDS).contains(&slot) {
                    return Err(DecodeError::Value);
                }
                let layer_id = bytes[8] & 0x3f;
                let layer = if layer_id == 0x3f {
                    None
                } else {
                    Some(LayerCondition {
                        layer: layer_id,
                        active: bytes[8] & 0x40 != 0,
                    })
                };
                let battery = if bytes[9] == u8::MAX {
                    None
                } else {
                    Some(BatteryCondition {
                        node: bytes[9] & 0x3f,
                        min_level: (bytes[10] != u8::MAX).then_some(bytes[10]),
                        max_level: (bytes[11] != u8::MAX).then_some(bytes[11]),
                        charge: match bytes[9] >> 6 {
                            0 => ChargeCondition::Any,
                            1 => ChargeCondition::Charging,
                            2 => ChargeCondition::Discharging,
                            3 => ChargeCondition::Unknown,
                            _ => return Err(DecodeError::Value),
                        },
                    })
                };
                let color = Rgb8::new(bytes[12], bytes[13], bytes[14]);
                let period_ms = get_u32(bytes, 15);
                let phase_ms = get_u32(bytes, 19);
                let auxiliary = get_u16(bytes, 23);
                let kind = bytes[7] >> 7 | (bytes[8] >> 7) << 1;
                let effect = match kind {
                    0 => BuiltinEffect::Solid { color },
                    1 if auxiliary <= 100 => BuiltinEffect::Blink {
                        color,
                        period_ms,
                        phase_ms,
                        duty: auxiliary as u8,
                    },
                    2 => BuiltinEffect::Breathe {
                        color,
                        period_ms,
                        phase_ms,
                        step_ms: auxiliary,
                    },
                    _ => return Err(DecodeError::Value),
                };
                Ok(Message::ConditionalSceneCell {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    cell: RuntimeConditionalSceneCell {
                        conditions: ConditionSet {
                            layer,
                            battery,
                            output_mode: match bytes[25] {
                                0 => Some(OutputMode::AlwaysOn),
                                1 => Some(OutputMode::AlwaysOff),
                                2 => Some(OutputMode::PoweredOnly),
                                u8::MAX => None,
                                _ => return Err(DecodeError::Value),
                            },
                            // Both arrive in the trailing ConditionalSceneExt
                            // packet, which is sent when either is present.
                            connection: None,
                            effects: None,
                        },
                        slot: LedSlot(slot as u16),
                        effect,
                    },
                })
            }
            TAG_CONDITIONAL_SCENE_EXT if bytes.len() == CONDITIONAL_SCENE_EXT_LEN => {
                let gates = bytes[7];
                let transport = if gates & 0x80 != 0 {
                    Some(match gates & 0x03 {
                        0 => ActiveTransport::Usb,
                        1 => ActiveTransport::Ble,
                        2 => ActiveTransport::NoneActive,
                        _ => return Err(DecodeError::Value),
                    })
                } else {
                    None
                };
                let ble_state = if gates & 0x40 != 0 {
                    Some(match (gates >> 2) & 0x03 {
                        0 => BleState::Advertising,
                        1 => BleState::Connected,
                        2 => BleState::Inactive,
                        _ => return Err(DecodeError::Value),
                    })
                } else {
                    None
                };
                let profile = if bytes[8] & 0x80 != 0 {
                    Some(bytes[8] & 0x7f)
                } else {
                    None
                };
                let bonded = (bytes[9] & 0x80 != 0).then(|| BondedSlotCondition {
                    slot: bytes[9] & 0x3f,
                    bonded: bytes[9] & 0x40 != 0,
                });
                let usb_connected = match bytes[10] {
                    0 => Some(false),
                    1 => Some(true),
                    u8::MAX => None,
                    _ => return Err(DecodeError::Value),
                };
                let connection = (transport.is_some()
                    || profile.is_some()
                    || ble_state.is_some()
                    || bonded.is_some()
                    || usb_connected.is_some())
                .then_some(ConnectionCondition {
                    transport,
                    profile,
                    ble_state,
                    bonded,
                    usb_connected,
                });
                Ok(Message::ConditionalSceneExt {
                    generation: bytes[2],
                    revision: get_u32(bytes, 3),
                    connection,
                    effects: (gates & 0x20 != 0).then_some(EffectsCondition {
                        enabled: gates & 0x10 != 0,
                    }),
                })
            }
            TAG_COMMIT if bytes.len() == COMMIT_LEN => Ok(Message::Commit {
                generation: bytes[2],
                revision: get_u32(bytes, 3),
                cell_count: bytes[7],
                scene_count: bytes[8],
            }),
            TAG_ACK if bytes.len() == ACK_LEN => Ok(Message::Ack {
                generation: bytes[2],
                revision: get_u32(bytes, 3),
            }),
            TAG_EFFECT_HIT if bytes.len() == EFFECT_HIT_LEN && bytes[2] < TOTAL_LEDS as u8 => {
                Ok(Message::EffectHit {
                    slot: LedSlot(bytes[2] as u16),
                })
            }
            TAG_BEGIN
            | TAG_CONTEXT
            | TAG_CELL
            | TAG_COMMIT
            | TAG_ACK
            | TAG_SCENE_CELL
            | TAG_EXTENSION
            | TAG_EXTENSION_OVERLAY
            | TAG_CONDITIONAL_SCENE_BEGIN
            | TAG_CONDITIONAL_SCENE_CELL
            | TAG_CONDITIONAL_SCENE_EXT
            | TAG_EFFECT_HIT
            | TAG_CONTEXT_UPDATE => Err(DecodeError::Length),
            _ => Err(DecodeError::Tag),
        }
    }
}

/// Best-effort delivery for a central-half key hit. Effect hits are ephemeral:
/// dropping one while the split queue is saturated is preferable to delaying
/// matrix-event processing or an atomic lighting snapshot.
pub fn try_queue_effect_hit(slot: LedSlot) -> bool {
    slot.index() < LEDS_PER_HALF
        && rmk::split_app::SPLIT_APP_TX
            .try_send(Message::EffectHit { slot }.encode())
            .is_ok()
}

/// Queue one complete snapshot. The staged peripheral state remains invisible
/// unless every packet lands and the final commit is applied.
pub fn try_queue_snapshot(
    generation: u8,
    snapshot: &StandardReplicaState<OVERLAY_CAPACITY, SCENE_CAPACITY>,
    batteries: BatteryPair,
) -> bool {
    let cell_count = snapshot
        .overlay
        .as_slice()
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
        .count();
    if cell_count > LEDS_PER_HALF {
        return false;
    }
    let scene_count = snapshot
        .scenes
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
        .count();
    if scene_count > SCENE_CAPACITY {
        return false;
    }
    let conditional_scene_count = snapshot
        .runtime_conditional_scenes
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
        .count();
    if conditional_scene_count > SCENE_CAPACITY {
        return false;
    }
    let queue = |message: Message| {
        rmk::split_app::SPLIT_APP_TX
            .try_send(message.encode())
            .is_ok()
    };
    if !queue(Message::Begin {
        generation,
        revision: snapshot.revision,
        cell_count: cell_count as u8,
        scene_count: scene_count as u8,
        scene_policy: snapshot.scenes.policy(),
        sample_time_ms: snapshot.sample_time_ms,
        mutable: snapshot.mutable,
        output_mode: snapshot.output_mode,
    }) || !queue(Message::Context {
        generation,
        revision: snapshot.revision,
        context: snapshot.context,
        batteries,
    }) || !queue(Message::Extension {
        generation,
        revision: snapshot.revision,
        extension: snapshot.extension,
        params: snapshot.extension_params,
    }) || !queue(Message::ExtensionOverlay {
        generation,
        revision: snapshot.revision,
        overlay: snapshot.extension_layers.and_then(|state| state.overlay),
        params: snapshot.extension_overlay_params,
    }) {
        return false;
    }
    for &cell in snapshot
        .overlay
        .as_slice()
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
    {
        if !queue(Message::Cell {
            generation,
            revision: snapshot.revision,
            cell,
        }) {
            return false;
        }
    }
    for cell in snapshot
        .scenes
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
    {
        if !queue(Message::SceneCell {
            generation,
            revision: snapshot.revision,
            cell,
        }) {
            return false;
        }
    }
    // These packets are part of the versioned replica snapshot. Both halves
    // must run the same protocol version before the transaction is accepted.
    if !queue(Message::ConditionalSceneBegin {
        generation,
        revision: snapshot.revision,
        cell_count: conditional_scene_count as u8,
    }) {
        return false;
    }
    for cell in snapshot
        .runtime_conditional_scenes
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
    {
        if !queue(Message::ConditionalSceneCell {
            generation,
            revision: snapshot.revision,
            cell,
        }) {
            return false;
        }
        let conditions = cell.conditions;
        if (conditions.connection.is_some() || conditions.effects.is_some())
            && !queue(Message::ConditionalSceneExt {
                generation,
                revision: snapshot.revision,
                connection: conditions.connection,
                effects: conditions.effects,
            })
        {
            return false;
        }
    }
    queue(Message::Commit {
        generation,
        revision: snapshot.revision,
        cell_count: cell_count as u8,
        scene_count: scene_count as u8,
    })
}

struct Stage {
    generation: u8,
    snapshot: StandardReplicaState<OVERLAY_CAPACITY, SCENE_CAPACITY>,
    expected_overlay_cells: usize,
    expected_scene_cells: usize,
    expected_conditional_scene_cells: Option<usize>,
    context_received: bool,
    extension_received: bool,
    extension_overlay_received: bool,
    batteries: BatteryPair,
}

pub struct SnapshotStage {
    stage: Option<Stage>,
}

impl SnapshotStage {
    pub const fn new() -> Self {
        Self { stage: None }
    }

    pub fn apply(
        &mut self,
        message: Message,
    ) -> Option<(
        u8,
        StandardReplicaState<OVERLAY_CAPACITY, SCENE_CAPACITY>,
        BatteryPair,
    )> {
        match message {
            Message::Begin {
                generation,
                revision,
                cell_count,
                scene_count,
                scene_policy,
                sample_time_ms,
                mutable,
                output_mode,
            } if cell_count as usize <= LEDS_PER_HALF && scene_count as usize <= SCENE_CAPACITY => {
                let mut scenes = SceneTable::new();
                scenes.set_policy(scene_policy);
                self.stage = Some(Stage {
                    generation,
                    snapshot: StandardReplicaState {
                        revision,
                        mutable,
                        output_mode,
                        overlay: OverlayBatch::new(),
                        scenes,
                        runtime_conditional_scenes: RuntimeConditionalSceneTable::new(),
                        context: LightingContext::default(),
                        sample_time_ms,
                        extension: None,
                        extension_layers: None,
                        extension_params: None,
                        extension_overlay_params: None,
                    },
                    expected_overlay_cells: cell_count as usize,
                    expected_scene_cells: scene_count as usize,
                    expected_conditional_scene_cells: None,
                    context_received: false,
                    extension_received: false,
                    extension_overlay_received: false,
                    batteries: BatteryPair::UNAVAILABLE,
                });
                None
            }
            Message::Context {
                generation,
                revision,
                context,
                batteries,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation || stage.snapshot.revision != revision {
                    self.stage = None;
                    return None;
                }
                stage.snapshot.context = context;
                stage.batteries = batteries;
                stage.context_received = true;
                None
            }
            Message::Extension {
                generation,
                revision,
                extension,
                params,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation || stage.snapshot.revision != revision {
                    self.stage = None;
                    return None;
                }
                stage.snapshot.extension = extension;
                stage.snapshot.extension_params = params;
                stage.extension_received = true;
                None
            }
            Message::ExtensionOverlay {
                generation,
                revision,
                overlay,
                params,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation || stage.snapshot.revision != revision {
                    self.stage = None;
                    return None;
                }
                stage.snapshot.extension_layers = Some(ExtensionLayerState { overlay });
                stage.snapshot.extension_overlay_params = params;
                stage.extension_overlay_received = true;
                None
            }
            Message::Cell {
                generation,
                revision,
                cell,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation
                    || stage.snapshot.revision != revision
                    || stage.snapshot.overlay.as_slice().len() >= stage.expected_overlay_cells
                    || stage
                        .snapshot
                        .overlay
                        .as_slice()
                        .iter()
                        .any(|existing| existing.slot == cell.slot)
                    || stage.snapshot.overlay.push(cell).is_err()
                {
                    self.stage = None;
                }
                None
            }
            Message::SceneCell {
                generation,
                revision,
                cell,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation
                    || stage.snapshot.revision != revision
                    || stage.snapshot.scenes.len() >= stage.expected_scene_cells
                    || stage
                        .snapshot
                        .scenes
                        .iter()
                        .any(|existing| existing.layer == cell.layer && existing.slot == cell.slot)
                    || stage.snapshot.scenes.set(cell).is_err()
                {
                    self.stage = None;
                }
                None
            }
            Message::ConditionalSceneBegin {
                generation,
                revision,
                cell_count,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation
                    || stage.snapshot.revision != revision
                    || cell_count as usize > SCENE_CAPACITY
                {
                    self.stage = None;
                } else {
                    stage.expected_conditional_scene_cells = Some(cell_count as usize);
                    stage.snapshot.runtime_conditional_scenes.clear();
                }
                None
            }
            Message::ConditionalSceneCell {
                generation,
                revision,
                cell,
            } => {
                let stage = self.stage.as_mut()?;
                if stage.generation != generation
                    || stage.snapshot.revision != revision
                    || stage
                        .expected_conditional_scene_cells
                        .is_none_or(|expected| {
                            stage.snapshot.runtime_conditional_scenes.len() >= expected
                        })
                    || stage
                        .snapshot
                        .runtime_conditional_scenes
                        .push(cell)
                        .is_err()
                {
                    self.stage = None;
                }
                None
            }
            Message::ConditionalSceneExt {
                generation,
                revision,
                connection,
                effects,
            } => {
                let stage = self.stage.as_mut()?;
                let amended = stage.generation == generation
                    && stage.snapshot.revision == revision
                    && stage
                        .snapshot
                        .runtime_conditional_scenes
                        .last_conditions_mut()
                        .map(|conditions| {
                            conditions.connection = connection;
                            conditions.effects = effects;
                        })
                        .is_some();
                if !amended {
                    self.stage = None;
                }
                None
            }
            Message::Commit {
                generation,
                revision,
                cell_count,
                scene_count,
            } => {
                let valid = self.stage.as_ref().is_some_and(|stage| {
                    stage.generation == generation
                        && stage.snapshot.revision == revision
                        && stage.context_received
                        && stage.extension_received
                        && stage.extension_overlay_received
                        && stage.expected_overlay_cells == cell_count as usize
                        && stage.expected_scene_cells == scene_count as usize
                        && stage.snapshot.overlay.as_slice().len() == stage.expected_overlay_cells
                        && stage.snapshot.scenes.len() == stage.expected_scene_cells
                        && stage
                            .expected_conditional_scene_cells
                            .is_none_or(|expected| {
                                stage.snapshot.runtime_conditional_scenes.len() == expected
                            })
                });
                if valid {
                    self.stage
                        .take()
                        .map(|stage| (stage.generation, stage.snapshot, stage.batteries))
                } else {
                    self.stage = None;
                    None
                }
            }
            Message::Ack { .. }
            | Message::EffectHit { .. }
            | Message::ContextUpdate { .. }
            | Message::Begin { .. } => None,
        }
    }

    pub fn reset(&mut self) {
        self.stage = None;
    }
}

fn flag(value: u8) -> Result<bool, DecodeError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::Value),
    }
}

fn indicators(value: IndicatorState) -> u8 {
    value.num_lock as u8
        | (value.caps_lock as u8) << 1
        | (value.scroll_lock as u8) << 2
        | (value.compose as u8) << 3
        | (value.kana as u8) << 4
}

fn get_indicators(value: u8) -> IndicatorState {
    IndicatorState {
        num_lock: value & 1 != 0,
        caps_lock: value & 2 != 0,
        scroll_lock: value & 4 != 0,
        compose: value & 8 != 0,
        kana: value & 16 != 0,
    }
}

fn put_battery(out: &mut [u8], at: usize, status: BatteryStatus) {
    let (state, level) = match status {
        BatteryStatus::Unavailable => (0, None),
        BatteryStatus::Available {
            charge_state: ChargeState::Charging,
            level,
        } => (1, level),
        BatteryStatus::Available {
            charge_state: ChargeState::Discharging,
            level,
        } => (2, level),
        BatteryStatus::Available {
            charge_state: ChargeState::Unknown,
            level,
        } => (3, level),
    };
    out[at] = state;
    out[at + 1] = level.unwrap_or(u8::MAX);
}

fn get_battery(bytes: &[u8], at: usize) -> Result<BatteryStatus, DecodeError> {
    let level = match bytes[at + 1] {
        u8::MAX => None,
        level if level <= 100 => Some(level),
        _ => return Err(DecodeError::Value),
    };
    Ok(match bytes[at] {
        0 if level.is_none() => BatteryStatus::Unavailable,
        1 => BatteryStatus::Available {
            charge_state: ChargeState::Charging,
            level,
        },
        2 => BatteryStatus::Available {
            charge_state: ChargeState::Discharging,
            level,
        },
        3 => BatteryStatus::Available {
            charge_state: ChargeState::Unknown,
            level,
        },
        _ => return Err(DecodeError::Value),
    })
}

fn put_u16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn get_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn put_u32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn get_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn put_u64(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
