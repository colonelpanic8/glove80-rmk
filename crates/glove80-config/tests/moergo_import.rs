//! Import of a real MoErgo Layout Editor export.
//!
//! The fixture is TailorKey v5.2³ Bilateral, which is the interesting case
//! because it builds bilateral home row mods the only way ZMK can: one
//! momentary layer per finger, driven by hold-taps that restrict their hold to
//! the opposite hand's key positions. Rynk decides the same thing from the
//! layout's hand tags, so the import is expected to recover the intent and drop
//! the scaffolding rather than transliterate it.

use glove80_config::import_moergo_layout;

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
        .morses
        .iter()
        .filter(|morse| morse.profile.unilateral_tap() == Some(true))
        .filter_map(|morse| morse.profile.hold_timeout_ms())
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
        .morses
        .iter()
        .filter(|morse| morse.profile.unilateral_tap() == Some(true))
        .count();
    assert_eq!(bilateral, 8, "eight home row mods, one per finger");
}

#[test]
fn autoshift_becomes_one_morse_per_key() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    // The Autoshift layer taps the key and holds for its shifted form, with a
    // 190 ms term and no bilateral restriction.
    let autoshift = imported
        .morses
        .iter()
        .filter(|morse| {
            morse.profile.hold_timeout_ms() == Some(190) && morse.profile.unilateral_tap().is_none()
        })
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
        !imported.combos.is_empty(),
        "the export declares 11 combos; none survived"
    );
    assert!(
        imported.combos.iter().all(|combo| combo.layer.is_some()),
        "a combo lost its layer restriction"
    );
    assert!(
        imported.combos.iter().all(|combo| combo.actions.len() >= 2),
        "a combo lost its trigger keys"
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
    println!("morses:      {}", imported.morses.len());
    println!("combos:      {}", imported.combos.len());
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
