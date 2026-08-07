//! Every physical key belongs to the split half that scans its matrix column.
//! This matters when a behavior enables `unilateral_tap`: unknown or bilateral
//! keys are deliberately excluded from same-hand roll detection.

use std::{collections::HashSet, path::Path};

fn assert_split_handedness(relative_path: &str, expected_keys: usize) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let text = std::fs::read_to_string(path).expect("read keyboard.toml");
    let config: toml::Value = toml::from_str(&text).expect("parse keyboard.toml");
    let map = config["layout"]["map"]
        .as_str()
        .expect("[layout].map string");
    let mut keys = HashSet::new();

    for tuple in map
        .split('(')
        .skip(1)
        .filter_map(|tail| tail.split_once(')').map(|x| x.0))
    {
        let fields: Vec<_> = tuple.split(',').map(str::trim).collect();
        if fields.first() == Some(&"e") {
            continue;
        }
        let Ok(row) = fields.first().expect("tuple first field").parse::<u8>() else {
            continue;
        };
        let col: u8 = fields
            .get(1)
            .expect("key column")
            .parse()
            .expect("numeric key column");
        let hand = fields.get(2).expect("every key has a hand marker");
        let expected = if col < 7 { 'L' } else { 'R' };
        assert_eq!(
            *hand,
            expected.to_string(),
            "key ({row},{col}) is on the {} split half",
            if expected == 'L' { "left" } else { "right" },
        );
        assert!(keys.insert((row, col)), "duplicate key ({row},{col})");
    }
    assert_eq!(keys.len(), expected_keys, "mapped key count");
}

#[test]
fn glove80_assigns_every_key_to_its_physical_half() {
    assert_split_handedness("../glove80-rmk/keyboard.toml", 80);
}

#[test]
fn go60_assigns_every_key_to_its_physical_half() {
    assert_split_handedness("../go60-rmk/keyboard.toml", 60);
}
