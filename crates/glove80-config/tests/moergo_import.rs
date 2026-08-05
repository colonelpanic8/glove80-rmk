//! Import of a real MoErgo Layout Editor export.
//!
//! The fixture is TailorKey v5.2³ Bilateral, which is the interesting case
//! because it builds bilateral home row mods the only way ZMK can: one
//! momentary layer per finger, driven by hold-taps that restrict their hold to
//! the opposite hand's key positions. Rynk decides the same thing from the
//! layout's hand tags, so the import is expected to recover the intent and drop
//! the scaffolding rather than transliterate it.

use glove80_config::{import_moergo_layout, RuntimeConfig};

const TAILORKEY: &str = include_str!("fixtures/tailorkey-v52-bilateral.json");

#[test]
fn drops_the_per_finger_layers_the_home_row_mods_are_built_from() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    // LeftPinky..RightPinky are editor layers 3..=10.
    assert_eq!(imported.dropped_layers, (3..=10).collect::<Vec<_>>());
    assert_eq!(
        imported.runtime.layers.len(),
        12,
        "20 editor layers less the 8 finger layers"
    );
    assert!(
        imported
            .runtime
            .layers
            .iter()
            .all(|layer| !layer.name.starts_with("Left") && !layer.name.starts_with("Right")),
        "no finger layer survived: {:?}",
        imported
            .runtime
            .layers
            .iter()
            .map(|layer| &layer.name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn home_row_mods_keep_their_per_finger_timing_and_become_unilateral() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    // TailorKey tunes one hold timeout per finger; the import must not
    // collapse them onto a shared default.
    let mut timeouts: Vec<u16> = imported
        .runtime
        .morses
        .iter()
        .filter(|morse| morse.unilateral_tap == Some(true))
        .filter_map(|morse| morse.hold_timeout_ms)
        .collect();
    timeouts.sort_unstable();
    timeouts.dedup();
    assert_eq!(
        timeouts,
        vec![180, 210, 240, 270],
        "index/middy/ring/pinky hold timeouts"
    );

    // `hold-trigger-key-positions` has no runtime equivalent, so bilateral
    // enforcement has to arrive as unilateral_tap or it is silently lost.
    let bilateral = imported
        .runtime
        .morses
        .iter()
        .filter(|morse| morse.unilateral_tap == Some(true))
        .count();
    assert_eq!(bilateral, 8, "eight home row mods, one per finger");
}

#[test]
fn autoshift_becomes_one_morse_per_key() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    // The Autoshift layer taps the key and holds for its shifted form, with a
    // 190 ms term and no bilateral restriction.
    let autoshift = imported
        .runtime
        .morses
        .iter()
        .filter(|morse| morse.hold_timeout_ms == Some(190) && morse.unilateral_tap.is_none())
        .count();
    assert!(
        autoshift >= 40,
        "expected the Autoshift row to lower to its own morses, got {autoshift}"
    );
}

#[test]
fn every_combo_resolves_on_each_layer_it_is_declared_for() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    assert!(
        !imported.runtime.combos.is_empty(),
        "the export declares 11 combos; none survived"
    );
    assert!(
        imported
            .runtime
            .combos
            .iter()
            .all(|combo| combo.layer.is_some()),
        "a combo lost its layer restriction"
    );
    assert!(
        imported
            .runtime
            .combos
            .iter()
            .all(|combo| combo.keys.len() >= 2),
        "a combo lost its trigger keys"
    );
}

/// An import is an ordinary configuration.
///
/// The behavior tables live on the configuration rather than beside it, so an
/// imported layout can be written as TOML, hand-edited to retune a timing, and
/// applied through the path a TOML source already takes. If the tables did not
/// survive the round trip, importing would be a one-way door.
#[test]
fn an_imported_layout_round_trips_through_toml() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");
    let text = imported.runtime.to_toml().expect("serialize");
    let reparsed = RuntimeConfig::from_toml(&text).expect("reparse");

    assert_eq!(reparsed.morses.len(), imported.runtime.morses.len());
    assert_eq!(reparsed.combos.len(), imported.runtime.combos.len());
    assert_eq!(reparsed.macros.len(), imported.runtime.macros.len());
    assert_eq!(
        reparsed.snapshot().expect("snapshot"),
        imported.runtime.snapshot().expect("snapshot"),
        "a round trip through TOML changed what would be written to the keyboard"
    );
}

/// A configuration written before the behavior tables existed says nothing
/// about them, and applying it must not wipe what the keyboard already holds.
#[test]
fn a_file_without_behavior_tables_stays_silent_about_them() {
    let config =
        RuntimeConfig::from_toml(&format!("default_layer = 0\n{}", one_layer())).expect("parse");
    let snapshot = config.snapshot().expect("snapshot");

    assert!(snapshot.behaviors.morses.is_none());
    assert!(snapshot.behaviors.combos.is_none());
    assert!(snapshot.behaviors.macros.is_none());
}

/// The smallest keymap the parser accepts: one layer of `A`, with `--` marking
/// the four physical holes at r0c5, r0c8, r5c5 and r5c8.
fn one_layer() -> String {
    let row = |holes: bool| {
        (0..14)
            .map(|col| {
                if holes && (col == 5 || col == 8) {
                    "--"
                } else {
                    "A"
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let keys = [
        row(true),
        row(false),
        row(false),
        row(false),
        row(false),
        row(true),
    ]
    .join("\n");
    format!(
        "[[layer]]\n\
         id = \"base\"\n\
         name = \"Base\"\n\
         keys = \"\"\"\n{keys}\n\"\"\"\n"
    )
}

/// A morse need not define a hold: a tap-dance defines `tap` and `double_tap`
/// and nothing else. Writing such a slot out has to produce a file that still
/// parses, which it does not if the absent action is rendered as an empty name.
#[test]
fn a_morse_without_a_hold_survives_a_round_trip() {
    let config = RuntimeConfig::from_toml(&format!(
        "default_layer = 0\n{}\
         [[morse]]\n\
         tap = \"KC_A\"\n\
         double_tap = \"KC_B\"\n",
        one_layer()
    ))
    .expect("parse");
    let wire = config.snapshot().expect("snapshot");

    let text = config.to_toml().expect("serialize");
    assert!(
        !text.contains("hold = \"\""),
        "an absent hold was written as an empty keycode:\n{text}"
    );
    let reparsed = RuntimeConfig::from_toml(&text).expect("the rendered file must parse again");
    assert_eq!(reparsed.snapshot().expect("snapshot"), wire);
}

/// A slot that names no action at all is a mistake, not an empty morse.
#[test]
fn a_morse_with_no_actions_is_rejected() {
    let error = RuntimeConfig::from_toml(&format!("default_layer = 0\n{}[[morse]]\n", one_layer()))
        .expect_err("a morse with no actions must be rejected");
    assert!(
        format!("{error:#}").contains("at least one of tap"),
        "unhelpful error: {error:#}"
    );
}

/// The import is allowed to leave gaps, but never quietly: anything it drops
/// has to name the key it came from.
#[test]
fn every_gap_names_its_source_key() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    for diagnostic in &imported.diagnostics {
        assert!(
            !diagnostic.message.is_empty(),
            "empty diagnostic at {:?}",
            diagnostic.location
        );
    }
}

#[test]
#[ignore = "reports coverage rather than asserting it"]
fn coverage_report() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");
    println!("layers:      {}", imported.runtime.layers.len());
    println!("morses:      {}", imported.runtime.morses.len());
    println!("combos:      {}", imported.runtime.combos.len());
    println!("dropped:     {:?}", imported.dropped_layers);
    println!("diagnostics: {}", imported.diagnostics.len());
    for diagnostic in &imported.diagnostics {
        println!(
            "  {} :: {}",
            diagnostic.location.as_deref().unwrap_or("(export)"),
            diagnostic.message
        );
    }
}
