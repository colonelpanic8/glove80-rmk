//! Import of a real MoErgo Layout Editor export.
//!
//! The fixture is TailorKey v5.2³ Bilateral, which is the interesting case
//! because it builds bilateral home row mods the only way ZMK can: one
//! momentary layer per finger, driven by hold-taps that restrict their hold to
//! the opposite hand's key positions. Rynk decides the same thing from the
//! layout's hand tags, so the import is expected to recover the intent and drop
//! the scaffolding rather than transliterate it.

use glove80_config::{
    import_moergo_layout, runtime_config_from_moergo_json, RuntimeConfig, Severity,
};
use serde_json::json;

const TAILORKEY: &str = include_str!("fixtures/tailorkey-v52-bilateral.json");
/// sunaku's Glorious Engrammer. Where TailorKey exercises the bilateral home-row
/// idiom, this one exercises the tables TailorKey has none of: mod-morphs,
/// sticky keys, and ZMK's raw mouse-axis form.
const ENGRAMMER: &str = include_str!("fixtures/engrammer-v52a-no-code.json");

fn combo_export(extra_triggers: bool) -> String {
    let mut keys = vec![json!({ "value": "&none" }); 80];
    keys[0] = json!({ "value": "&kp", "params": [{ "value": "A" }] });
    keys[1] = json!({ "value": "&kp", "params": [{ "value": "B" }] });
    if extra_triggers {
        keys[2] = keys[0].clone();
        keys[3] = keys[1].clone();
    }

    json!({
        "keyboard": "glove80",
        "layer_names": ["Base"],
        "layers": [keys],
        "combos": [{
            "name": "copy-pair",
            "binding": { "value": "&kp", "params": [{ "value": "C" }] },
            "keyPositions": [0, 1],
            "layers": [0]
        }]
    })
    .to_string()
}

fn combo_ambiguity_diagnostics(imported: &glove80_config::ImportedLayout) -> Vec<&str> {
    imported
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Approximated
                && diagnostic.message.contains("extra key positions")
        })
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
}

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

#[test]
fn position_unique_combo_actions_are_not_reported_as_ambiguous() {
    let imported = import_moergo_layout(&combo_export(false)).expect("import");

    assert!(
        combo_ambiguity_diagnostics(&imported).is_empty(),
        "unique trigger positions were reported as ambiguous: {:?}",
        imported.diagnostics
    );
}

#[test]
fn combo_actions_repeated_at_other_positions_are_reported() {
    let imported = import_moergo_layout(&combo_export(true)).expect("import");
    let diagnostics = combo_ambiguity_diagnostics(&imported);

    assert_eq!(
        diagnostics.len(),
        1,
        "diagnostics: {:?}",
        imported.diagnostics
    );
    assert!(diagnostics[0].contains("copy-pair"), "{}", diagnostics[0]);
    assert!(diagnostics[0].contains("[2, 3]"), "{}", diagnostics[0]);
    assert_eq!(imported.runtime.combos.len(), 1, "the combo was dropped");
}

#[test]
fn tailorkey_import_has_no_dropped_diagnostics() {
    let imported = import_moergo_layout(TAILORKEY).expect("import");

    assert!(
        imported
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Dropped),
        "dropped diagnostics: {:?}",
        imported
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Dropped)
            .collect::<Vec<_>>()
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

/// The strict entry point refuses a layout it cannot represent — and names every
/// binding it choked on. Reporting only the first sends the reader back around
/// the import loop once per unportable key, and hides the answer they actually
/// want: how much of this layout is portable at all.
#[test]
fn refusing_a_layout_names_every_binding_it_cannot_import() {
    let mut root: serde_json::Value = serde_json::from_str(TAILORKEY).expect("fixture parses");
    let layers = root
        .pointer_mut("/layers")
        .and_then(serde_json::Value::as_array_mut)
        .expect("layers");
    // Two bindings the importer has no equivalent for, on the first layer.
    for key in [0usize, 1] {
        layers[0][key] = serde_json::json!({ "value": "&no_such_behavior", "params": [] });
    }
    let text = serde_json::to_string(&root).expect("reserialize");

    let error = format!(
        "{:#}",
        runtime_config_from_moergo_json(&text).expect_err("an unportable layout must be refused")
    );
    assert!(
        error.contains("2 bindings cannot be imported"),
        "the count of unportable bindings should lead: {error}"
    );
    assert_eq!(
        error.matches("no_such_behavior").count(),
        2,
        "both unportable bindings should be named, got: {error}"
    );

    // The permissive import sees the same two, and carries on with the rest.
    let imported = import_moergo_layout(&text).expect("the report form still succeeds");
    assert_eq!(
        imported
            .diagnostics
            .iter()
            .filter(|note| note.severity == Severity::Dropped)
            .count(),
        2
    );
}

/// A mod-morph is a fork: one key whose output swaps while a modifier is held.
#[test]
fn mod_morphs_become_forks() {
    let imported = import_moergo_layout(ENGRAMMER).expect("import");
    // `&parang_left` is `(` alone and `<` with right shift held.
    let forks = &imported.runtime.forks;
    assert!(
        forks.len() >= 2,
        "expected the two parenthesis/angle morphs to become forks, got {forks:?}"
    );
    let parang = forks
        .iter()
        .find(|fork| fork.trigger == "LSFT(KC_9)")
        .unwrap_or_else(|| panic!("no fork triggered by `(`: {forks:?}"));
    assert_eq!(parang.output, "LSFT(KC_COMM)");
    assert_eq!(parang.mods, ["RShift"]);
    // The unshifted output stays on the key, so it still types `(` even if the
    // fork table is empty.
    assert_eq!(
        imported.runtime.layers[0]
            .keys
            .matches("LSFT(KC_9)")
            .count(),
        1
    );
}

/// A hold-tap whose tap side is a sticky key: tap arms a one-shot modifier,
/// hold applies it directly.
#[test]
fn sticky_key_hold_taps_become_one_shot_morses() {
    let imported = import_moergo_layout(ENGRAMMER).expect("import");
    let armed = imported
        .runtime
        .morses
        .iter()
        .filter(|morse| morse.tap.as_deref() == Some("OSM(MOD_LSFT)"))
        .collect::<Vec<_>>();
    assert!(
        !armed.is_empty(),
        "expected a one-shot shift morse, got {:?}",
        imported.runtime.morses
    );
    assert_eq!(armed[0].hold.as_deref(), Some("KC_LSFT"));
}

/// The only bindings this export loses are the three display-brightness
/// keycodes RMK has no equivalent for. Everything else — 21 layers, 35 hold-taps,
/// 25 macros, 21 combos, 2 mod-morphs — survives.
#[test]
fn the_engrammer_loses_only_what_rmk_cannot_express() {
    let imported = import_moergo_layout(ENGRAMMER).expect("import");
    let dropped: Vec<&str> = imported
        .diagnostics
        .iter()
        .filter(|note| note.severity == Severity::Dropped)
        .map(|note| note.message.as_str())
        .collect();
    assert_eq!(dropped.len(), 3, "unexpected drops: {dropped:?}");
    for message in &dropped {
        assert!(
            message.contains("C_BRI_"),
            "only the brightness keycodes should be unmappable, got: {message}"
        );
    }
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
