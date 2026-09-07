use rmk::lighting::service::{LightingEngine, RenderInput};
use rmk::lighting::*;

use crate::borrowed_engine::BorrowedEngine;

fn full_capacity_transaction<const LEDS: usize, const CAP: usize>() {
    type Engine<const N: usize, const C: usize> =
        StandardLightingEngine<'static, EmptySource, EmptySource, N, 64, C>;
    let inner: Engine<LEDS, CAP> = StandardLightingEngine::new(
        BackgroundState {
            enabled: false,
            ..BackgroundState::default()
        },
        LayerScenes {
            scenes: &[],
            policy: LayerPolicy::ActiveStack,
        },
        EmptySource,
        EmptySource,
    )
    .with_controls(LightingControls {
        wake_layers: 1 << 2,
        wake_linger_ms: 20_000,
        ..LightingControls::default()
    });
    let mut engine = BorrowedEngine(Box::leak(Box::new(inner)));
    let mut snapshot = LightingContext {
        layers: LayerState::new(2, 0, 0b101),
        indicators: IndicatorState {
            caps_lock: true,
            ..IndicatorState::default()
        },
        ..LightingContext::default()
    };
    let reply = engine
        .handle_command(
            0,
            StandardCommand::BeginRuntimeConditionalSceneReplace {
                expected_revision: 0,
                cell_count: CAP as u16,
            },
            &snapshot,
        )
        .unwrap()
        .reply;
    let StandardReply::RuntimeConditionalSceneTransaction { id, .. } = reply else {
        panic!("expected transaction");
    };
    let expected: Vec<_> = (0..CAP)
        .map(|i| RuntimeConditionalSceneCell {
            slot: LedSlot((i % LEDS) as u16),
            conditions: ConditionSet {
                layers: Some(LayersCondition {
                    active: 1 << (2 + i / LEDS),
                    inactive: 0,
                }),
                indicators: Some(IndicatorCondition {
                    caps_lock: Some(true),
                    ..IndicatorCondition::default()
                }),
                ..ConditionSet::default()
            },
            effect: BuiltinEffect::solid(Rgb8::new(200, 0, 0)),
        })
        .collect();
    let chunk_size = rmk::lighting::standard::CONDITIONAL_SCENE_CHUNK_SIZE;
    for (page, cells) in expected.chunks(chunk_size).enumerate() {
        let mut chunk = RuntimeConditionalSceneChunk::new();
        for cell in cells {
            chunk.push(*cell).unwrap();
        }
        engine
            .handle_command(
                1,
                StandardCommand::PutRuntimeConditionalSceneChunk {
                    transaction_id: id,
                    offset: (page * chunk_size) as u16,
                    cells: chunk,
                },
                &snapshot,
            )
            .unwrap();
        assert_eq!(engine.0.runtime_conditional_scenes().len(), 0);
    }
    engine
        .handle_command(
            2,
            StandardCommand::CommitRuntimeConditionalSceneReplace { transaction_id: id },
            &snapshot,
        )
        .unwrap();
    assert!(engine.0.runtime_conditional_scenes().iter().eq(expected));

    let mut frame = LogicalFrame::new(Rgb8::BLACK);
    let mut render =
        |engine: &mut BorrowedEngine<Engine<LEDS, CAP>>, now_ms, snapshot: &LightingContext| {
            engine
                .render(RenderInput { now_ms, snapshot }, &mut frame)
                .unwrap();
            <BorrowedEngine<Engine<LEDS, CAP>> as LightingEngine<LightingContext>>::on_presented(
                engine, &frame,
            );
            frame.as_slice().to_vec()
        };
    assert_eq!(
        render(&mut engine, 2, &snapshot),
        vec![Rgb8::new(200, 0, 0); LEDS]
    );
    snapshot.indicators.caps_lock = false;
    assert_eq!(render(&mut engine, 3, &snapshot), vec![Rgb8::BLACK; LEDS]);
    snapshot.indicators.caps_lock = true;
    engine
        .handle_command(4, StandardCommand::SetOutputBrightness(128), &snapshot)
        .unwrap();
    let dimmed = render(&mut engine, 4, &snapshot);
    assert!(
        dimmed
            .iter()
            .all(|rgb| rgb.r > 0 && rgb.r < 200 && rgb.g == 0 && rgb.b == 0)
    );
    snapshot.layers = LayerState::new(0, 0, 1);
    assert_eq!(render(&mut engine, 10, &snapshot), dimmed);
    assert_eq!(render(&mut engine, 20_009, &snapshot), dimmed);
    assert_eq!(
        render(&mut engine, 20_010, &snapshot),
        vec![Rgb8::BLACK; LEDS]
    );
    engine
        .handle_command(20_011, StandardCommand::SetOutputEnabled(false), &snapshot)
        .unwrap();
    assert!(!engine.0.state().output_enabled);
    engine
        .handle_command(20_012, StandardCommand::SetOutputEnabled(true), &snapshot)
        .unwrap();
    assert!(engine.0.state().output_enabled);
}

#[test]
fn glove80_borrowed_engine_preserves_full_capacity_magic_transactions() {
    full_capacity_transaction::<80, 100>();
}

#[test]
fn go60_borrowed_engine_preserves_full_capacity_magic_transactions() {
    full_capacity_transaction::<60, 80>();
}
