#![cfg(test)]

use core::num::NonZeroU32;

use rmk::host::ReplicaDigests;
use rmk::lighting::{
    BackgroundMode, BackgroundState, BuiltinEffect, ConditionSet, LayerCondition, LayerPolicy,
    LayerState, LedSlot, LightingContext, OutputMode, OverlayBatch, OverlayCell, Rgb8,
    RuntimeConditionalSceneCell, RuntimeConditionalSceneTable, SceneTable, SceneTableCell,
    StandardMutableState, StandardReplicaState,
};
use rmk::split_app::{SPLIT_APP_MSG_MAX, SplitAppData};

mod lighting {
    use rmk::types::battery::BatteryStatus;

    pub const LEDS_PER_HALF: usize = 40;
    pub const TOTAL_LEDS: usize = 80;
    pub const OVERLAY_CAPACITY: usize = 64;
    pub const SCENE_CAPACITY: usize = 100;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct BatteryPair {
        pub left: BatteryStatus,
        pub right: BatteryStatus,
    }

    impl BatteryPair {
        pub const UNAVAILABLE: Self = Self {
            left: BatteryStatus::Unavailable,
            right: BatteryStatus::Unavailable,
        };
    }
}

#[path = "../../glove80-rmk/src/split_lighting.rs"]
mod split_lighting;

use lighting::BatteryPair;
use split_lighting::{
    AttestationDecision, AttestationRecovery, DecodeError, FrameChunkDecision, FramePageAssembly,
    Message, SnapshotStage,
};

fn mutable() -> StandardMutableState {
    StandardMutableState {
        output_enabled: true,
        output_brightness: 127,
        background: BackgroundState {
            enabled: true,
            hue: 1,
            saturation: 2,
            value: 3,
            speed: 4,
            mode: BackgroundMode::Breathe,
        },
    }
}

#[test]
fn existing_split_messages_round_trip() {
    let messages = [
        Message::Begin {
            generation: 7,
            revision: 0x1234_5678,
            cell_count: 1,
            scene_count: 2,
            scene_policy: LayerPolicy::ActiveStack,
            sample_time_ms: u64::MAX - 1,
            mutable: mutable(),
            output_mode: OutputMode::PoweredOnly,
        },
        Message::Context {
            generation: 7,
            revision: 0x1234_5678,
            context: LightingContext::default().into(),
            batteries: BatteryPair::UNAVAILABLE,
        },
        Message::Ack {
            generation: 7,
            revision: 0x1234_5678,
        },
        Message::EffectHit { slot: LedSlot(39) },
        Message::Attestation {
            generation: 7,
            digests: ReplicaDigests {
                schema: 1,
                revision: 0x1234_5678,
                settings: u32::MAX,
                overlay: 0x8000_0001,
                scenes: 0x7fff_ffff,
                conditional_scenes: 0x0102_0304,
            },
        },
        Message::ReplicaProbe { generation: 7 },
        Message::StatusRequest { request_id: 9 },
        Message::StatusReport {
            request_id: 9,
            applied_revision: Some(0x1234_5678),
            engine_revision: 11,
            layers: rmk::lighting::LayerState::new(2, 1, 0b111),
            powered: true,
            wake_active: false,
            effective_output_enabled: true,
        },
        Message::FrameChunkRequest {
            request_id: 10,
            offset: 24,
        },
        Message::FrameChunk {
            request_id: 10,
            revision: Some(0x1234_5678),
            total: 80,
            start: 24,
            len: 4,
            cells: [
                Rgb8::new(1, 2, 3),
                Rgb8::new(4, 5, 6),
                Rgb8::new(7, 8, 9),
                Rgb8::new(10, 11, 12),
            ],
        },
    ];

    for message in messages {
        assert_eq!(Message::decode(message.encode()), Ok(message));
    }
}

#[test]
fn semantic_context_packets_reserve_but_do_not_carry_layer_state() {
    let source = LightingContext {
        layers: LayerState::new(5, 1, 0b10_1010),
        ..LightingContext::default()
    };
    let message = Message::ContextUpdate {
        generation: 7,
        revision: 0x1234_5678,
        context: source.into(),
        batteries: BatteryPair::UNAVAILABLE,
    };

    let encoded = message.encode();
    assert_eq!(&encoded.payload()[7..17], &[0; 10]);
    assert_eq!(Message::decode(encoded), Ok(message));
}

#[test]
fn unknown_or_wrong_version_packets_are_ignored_safely() {
    let unknown = SplitAppData::new(&[10, 0xff]).unwrap();
    assert_eq!(Message::decode(unknown), Err(DecodeError::Tag));

    let wrong_version = SplitAppData::new(&[9, 5, 1, 0, 0, 0, 0]).unwrap();
    assert_eq!(Message::decode(wrong_version), Err(DecodeError::Version));

    assert!(SPLIT_APP_MSG_MAX >= 26);
}

#[test]
fn staging_aborts_on_a_mismatched_generation() {
    let mut stage = SnapshotStage::new();
    assert!(
        stage
            .apply(Message::Begin {
                generation: 1,
                revision: 7,
                cell_count: 1,
                scene_count: 0,
                scene_policy: LayerPolicy::EffectiveOnly,
                sample_time_ms: 0,
                mutable: mutable(),
                output_mode: OutputMode::AlwaysOn,
            })
            .is_none()
    );
    assert!(
        stage
            .apply(Message::Cell {
                generation: 2,
                revision: 7,
                cell: OverlayCell {
                    slot: LedSlot(40),
                    effect: BuiltinEffect::Solid {
                        color: Rgb8::new(1, 2, 3),
                    },
                    ttl_ms: NonZeroU32::new(100),
                },
            })
            .is_none()
    );
    assert!(
        stage
            .apply(Message::Commit {
                generation: 1,
                revision: 7,
                cell_count: 1,
                scene_count: 0,
            })
            .is_none()
    );
}

fn digests(revision: u32, settings: u32) -> ReplicaDigests {
    ReplicaDigests {
        schema: 1,
        revision,
        settings,
        overlay: 2,
        scenes: 3,
        conditional_scenes: 4,
    }
}

#[test]
fn attestation_recovery_halts_on_the_post_resync_mismatch() {
    let expected = digests(7, 1);
    let divergent = digests(7, 9);
    let mut recovery = AttestationRecovery::new();

    assert_eq!(
        recovery.observe(Some(expected), divergent),
        AttestationDecision::Recover
    );
    assert_eq!(recovery.mismatch_count(), 1);
    assert_eq!(
        recovery.observe(Some(expected), divergent),
        AttestationDecision::Recover
    );
    assert_eq!(recovery.mismatch_count(), 1);

    recovery.snapshot_acked(7);
    assert_eq!(
        recovery.observe(Some(expected), divergent),
        AttestationDecision::Halt
    );
    assert_eq!(recovery.mismatch_count(), 2);
}

#[test]
fn attestation_recovery_resets_on_match_new_revision_and_link_session() {
    let mut recovery = AttestationRecovery::new();
    let expected = digests(7, 1);
    assert_eq!(
        recovery.observe(Some(expected), digests(7, 9)),
        AttestationDecision::Recover
    );
    assert_eq!(
        recovery.observe(Some(expected), expected),
        AttestationDecision::Matching
    );
    assert_eq!(recovery.mismatch_count(), 0);

    assert_eq!(
        recovery.observe(Some(digests(8, 1)), digests(7, 9)),
        AttestationDecision::Stale
    );
    assert_eq!(recovery.mismatch_count(), 0);

    recovery.observe(Some(expected), digests(7, 9));
    recovery.reset();
    assert_eq!(recovery.mismatch_count(), 0);
}

fn solid(slot: u16, red: u8, ttl: Option<u32>) -> OverlayCell {
    OverlayCell {
        slot: LedSlot(slot),
        effect: BuiltinEffect::Solid {
            color: Rgb8::new(red, 2, 3),
        },
        ttl_ms: ttl.and_then(NonZeroU32::new),
    }
}

fn replica(
    overlay: OverlayBatch<64>,
    scenes: SceneTable<100>,
    conditional: RuntimeConditionalSceneTable<100>,
) -> StandardReplicaState<64, 100> {
    StandardReplicaState {
        revision: 7,
        mutable: mutable(),
        output_mode: OutputMode::AlwaysOn,
        overlay,
        scenes,
        runtime_conditional_scenes: conditional,
        context: LightingContext::default(),
        sample_time_ms: 123,
        extension: None,
        extension_layers: None,
        extension_params: None,
        extension_overlay_params: None,
    }
}

#[test]
fn digest_is_canonical_for_maps_and_excludes_overlay_ttl() {
    let mut overlay_a = OverlayBatch::new();
    overlay_a.push(solid(41, 10, Some(50))).unwrap();
    overlay_a.push(solid(40, 20, None)).unwrap();
    let mut overlay_b = OverlayBatch::new();
    overlay_b.push(solid(40, 20, Some(999))).unwrap();
    overlay_b.push(solid(41, 10, None)).unwrap();

    let mut scenes_a = SceneTable::new();
    scenes_a
        .set(SceneTableCell {
            layer: 2,
            slot: LedSlot(41),
            effect: solid(41, 30, None).effect,
        })
        .unwrap();
    scenes_a
        .set(SceneTableCell {
            layer: 1,
            slot: LedSlot(40),
            effect: solid(40, 40, None).effect,
        })
        .unwrap();
    let mut scenes_b = SceneTable::new();
    scenes_b
        .set(SceneTableCell {
            layer: 1,
            slot: LedSlot(40),
            effect: solid(40, 40, None).effect,
        })
        .unwrap();
    scenes_b
        .set(SceneTableCell {
            layer: 2,
            slot: LedSlot(41),
            effect: solid(41, 30, None).effect,
        })
        .unwrap();

    let a = split_lighting::replica_digests(&replica(
        overlay_a,
        scenes_a,
        RuntimeConditionalSceneTable::new(),
    ));
    let b = split_lighting::replica_digests(&replica(
        overlay_b,
        scenes_b,
        RuntimeConditionalSceneTable::new(),
    ));
    assert_eq!(a.overlay, b.overlay);
    assert_eq!(a.scenes, b.scenes);
}

#[test]
fn digest_preserves_conditional_scene_order() {
    let first = RuntimeConditionalSceneCell {
        conditions: ConditionSet {
            layer: Some(LayerCondition {
                layer: 1,
                active: true,
            }),
            ..ConditionSet::default()
        },
        slot: LedSlot(40),
        effect: solid(40, 10, None).effect,
    };
    let second = RuntimeConditionalSceneCell {
        conditions: ConditionSet {
            layer: Some(LayerCondition {
                layer: 2,
                active: true,
            }),
            ..ConditionSet::default()
        },
        slot: LedSlot(40),
        effect: solid(40, 20, None).effect,
    };
    let mut a = RuntimeConditionalSceneTable::new();
    a.push(first).unwrap();
    a.push(second).unwrap();
    let mut b = RuntimeConditionalSceneTable::new();
    b.push(second).unwrap();
    b.push(first).unwrap();

    let a = split_lighting::replica_digests(&replica(OverlayBatch::new(), SceneTable::new(), a));
    let b = split_lighting::replica_digests(&replica(OverlayBatch::new(), SceneTable::new(), b));
    assert_ne!(a.conditional_scenes, b.conditional_scenes);
}

fn frame_chunk(request_id: u8, revision: u32, total: u16, start: u16, len: u8) -> Message {
    Message::FrameChunk {
        request_id,
        revision: Some(revision),
        total,
        start,
        len,
        cells: [
            Rgb8::new(start as u8, 1, 2),
            Rgb8::new(start as u8 + 1, 1, 2),
            Rgb8::new(start as u8 + 2, 1, 2),
            Rgb8::new(start as u8 + 3, 1, 2),
        ],
    }
}

#[test]
fn frame_page_assembly_accepts_only_contiguous_consistent_chunks() {
    let mut assembly = FramePageAssembly::new(0);
    assert_eq!(
        assembly.accept(frame_chunk(1, 7, 6, 0, 4)),
        FrameChunkDecision::Accepted
    );
    assert_eq!(
        assembly.accept(frame_chunk(2, 7, 6, 4, 2)),
        FrameChunkDecision::Complete
    );
    assert_eq!(assembly.revision(), Some(7));
    assert_eq!(assembly.len(), 6);

    let mut wrong_start = FramePageAssembly::new(0);
    assert_eq!(
        wrong_start.accept(frame_chunk(1, 7, 8, 4, 4)),
        FrameChunkDecision::Invalid
    );

    let mut torn = FramePageAssembly::new(0);
    assert_eq!(
        torn.accept(frame_chunk(1, 7, 8, 0, 4)),
        FrameChunkDecision::Accepted
    );
    assert_eq!(
        torn.accept(frame_chunk(2, 8, 8, 4, 4)),
        FrameChunkDecision::Restart
    );
}

#[test]
fn diagnostics_only_enter_an_empty_depth_two_response_queue() {
    assert!(split_lighting::diagnostic_may_enqueue(2));
    assert!(!split_lighting::diagnostic_may_enqueue(1));
    assert!(!split_lighting::diagnostic_may_enqueue(0));
}
