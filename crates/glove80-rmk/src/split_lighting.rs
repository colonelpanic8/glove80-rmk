//! Glove80 semantic lighting replication over RMK's bounded split channel.
//!
//! The central remains the Rynk/Vial authority. It transfers declarative
//! standard-engine snapshots only when state changes or the link reconnects;
//! the peripheral applies a complete staged snapshot atomically and renders
//! every animation frame from its own clock and compositor.

use core::num::NonZeroU32;

use rmk::lighting::{
    BackgroundMode, BackgroundState, BuiltinEffect, IndicatorState, LayerPolicy, LayerState,
    LedSlot, LightingContext, OutputMode, OverlayBatch, OverlayCell, Rgb8, SceneTable,
    SceneTableCell, StandardMutableState, StandardReplicaState,
};
use rmk::split_app::{SPLIT_APP_MSG_MAX, SplitAppData};
use rmk::types::battery::{BatteryStatus, ChargeState};

use crate::lighting::{BatteryPair, LEDS_PER_HALF, OVERLAY_CAPACITY, SCENE_CAPACITY, TOTAL_LEDS};

const VERSION: u8 = 4;
const TAG_BEGIN: u8 = 1;
const TAG_CONTEXT: u8 = 2;
const TAG_CELL: u8 = 3;
const TAG_COMMIT: u8 = 4;
const TAG_ACK: u8 = 5;
const TAG_SCENE_CELL: u8 = 6;

const BEGIN_LEN: usize = 26;
const CONTEXT_LEN: usize = 23;
const CELL_LEN: usize = 26;
const SCENE_CELL_LEN: usize = 23;
const COMMIT_LEN: usize = 9;
const ACK_LEN: usize = 7;
const _: () = assert!(CELL_LEN <= SPLIT_APP_MSG_MAX);

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
                CONTEXT_LEN
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
                },
                batteries: BatteryPair {
                    left: get_battery(bytes, 18)?,
                    right: get_battery(bytes, 20)?,
                },
            }),
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
            TAG_BEGIN | TAG_CONTEXT | TAG_CELL | TAG_COMMIT | TAG_ACK | TAG_SCENE_CELL => {
                Err(DecodeError::Length)
            }
            _ => Err(DecodeError::Tag),
        }
    }
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
        .as_slice()
        .iter()
        .filter(|cell| cell.slot.index() >= LEDS_PER_HALF)
        .count();
    if scene_count > SCENE_CAPACITY {
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
    for &cell in snapshot
        .scenes
        .as_slice()
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
    context_received: bool,
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
                        context: LightingContext::default(),
                        sample_time_ms,
                    },
                    expected_overlay_cells: cell_count as usize,
                    expected_scene_cells: scene_count as usize,
                    context_received: false,
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
                    || stage.snapshot.scenes.as_slice().len() >= stage.expected_scene_cells
                    || stage
                        .snapshot
                        .scenes
                        .as_slice()
                        .iter()
                        .any(|existing| existing.layer == cell.layer && existing.slot == cell.slot)
                    || stage.snapshot.scenes.set(cell).is_err()
                {
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
                        && stage.expected_overlay_cells == cell_count as usize
                        && stage.expected_scene_cells == scene_count as usize
                        && stage.snapshot.overlay.as_slice().len() == stage.expected_overlay_cells
                        && stage.snapshot.scenes.as_slice().len() == stage.expected_scene_cells
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
            Message::Ack { .. } | Message::Begin { .. } => None,
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
