use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn config() -> toml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../go60-rmk/keyboard.toml");
    toml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn go60_hardware_identity_and_matrix_match_the_vendor_board() {
    let config = config();
    assert_eq!(config["keyboard"]["name"].as_str(), Some("Go60"));
    assert_eq!(config["keyboard"]["vendor_id"].as_integer(), Some(0x16c0));
    assert_eq!(config["keyboard"]["product_id"].as_integer(), Some(0x27db));
    assert_eq!(config["layout"]["rows"].as_integer(), Some(5));
    assert_eq!(config["layout"]["cols"].as_integer(), Some(14));

    let central = &config["split"]["central"]["matrix"];
    let peripheral = &config["split"]["peripheral"][0]["matrix"];
    assert_eq!(
        strings(&central["row_pins"]),
        ["P1_01", "P1_03", "P1_05", "P1_07", "P1_06"]
    );
    assert_eq!(
        strings(&central["col_pins"]),
        [
            "P0_24", "P0_20", "P0_17", "P0_15", "P0_16", "P0_13", "P0_14"
        ]
    );
    assert_eq!(
        strings(&peripheral["col_pins"]),
        [
            "P0_14", "P0_13", "P0_16", "P0_15", "P0_17", "P0_20", "P0_24"
        ]
    );
}

#[test]
fn go60_lighting_routes_every_physical_key_once_per_half() {
    let config = config();
    let emitters = config["lighting"]["emitter"].as_array().unwrap();
    assert_eq!(emitters.len(), 60);

    let expected_keys = physical_keys();
    let actual_keys = emitters
        .iter()
        .map(|emitter| {
            let key = emitter["key"].as_array().unwrap();
            (key[0].as_integer().unwrap(), key[1].as_integer().unwrap())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_keys, expected_keys);

    for node in 0..=1 {
        let indices = emitters
            .iter()
            .filter(|emitter| emitter["node"].as_integer() == Some(node))
            .map(|emitter| emitter["physical_index"].as_integer().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(indices, (0..30).collect());
    }

    let outputs = config["lighting"]["output"].as_array().unwrap();
    assert_eq!(outputs.len(), 2);
    assert!(
        outputs
            .iter()
            .all(|output| output["pixel_count"].as_integer() == Some(30))
    );
}

/// Too few subscriber slots is a boot-time panic inside RMK's pubsub, not a
/// build error, so nothing else catches a shortfall before a flash.
#[test]
fn go60_layer_change_has_a_slot_for_every_subscriber() {
    // The split driver, the Rynk layer topic, the lighting state, and the
    // trackpads' LayerModes processor.
    const SUBSCRIBERS: i64 = 4;
    assert!(
        config()["event"]["layer_change"]["subs"]
            .as_integer()
            .unwrap()
            >= SUBSCRIBERS
    );
}

fn strings(value: &toml::Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect()
}

fn physical_keys() -> BTreeSet<(i64, i64)> {
    let mut keys = BTreeSet::new();
    for row in 0..4 {
        for col in 0..6 {
            keys.insert((row, col));
        }
        for col in 8..14 {
            keys.insert((row, col));
        }
    }
    for col in 2..=4 {
        keys.insert((4, col));
    }
    for col in 9..=11 {
        keys.insert((4, col));
    }
    for row in 0..=2 {
        keys.insert((row, 6));
        keys.insert((row, 7));
    }
    keys
}
