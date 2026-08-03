//! Import and export for the experimental JSON backup format used by the
//! MoErgo Glove80 Layout Editor.
//!
//! The editor stores ZMK behavior trees in physical-key order. Rynk stores a
//! typed action at every cell of the 6x14 matrix. This module deliberately
//! converts only the intersection of those models and reports the exact layer
//! and key for anything that cannot be represented without changing meaning.

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::{keycodes, LayerConfig, RuntimeConfig, Snapshot, COLS, LAYER_SIZE};

/// Editor key index -> row-major 6x14 Rynk matrix offset.
///
/// This is the walk in `config/firmware.toml`'s `[layout].map`: five top-row
/// keys per half, six keys per half on the next three rows, then the thumb
/// fans interleaved with the bottom two rows. The four matrix holes never
/// appear in an editor layer.
pub const MOERGO_TO_MATRIX: [usize; 80] = [
    0, 1, 2, 3, 4, 9, 10, 11, 12, 13, // top row
    14, 15, 16, 17, 18, 19, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 36, 37, 38, 39, 40, 41,
    42, 43, 44, 45, 46, 47, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 6, 20, 34, 35, 21, 7,
    64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 48, 62, 76, 77, 63, 49, 79, 80, 81, 82, 83,
];

/// Parse a MoErgo editor JSON backup into the managed runtime model.
/// Lighting is absent because the editor backup has no Rynk lighting state.
pub fn runtime_config_from_moergo_json(text: &str) -> Result<RuntimeConfig> {
    let root: Value = serde_json::from_str(text).context("invalid JSON")?;
    let layout = layout_object(&root)?;
    if let Some(keyboard) = layout.get("keyboard").and_then(Value::as_str) {
        if !keyboard.eq_ignore_ascii_case("glove80") {
            bail!("MoErgo JSON is for keyboard '{keyboard}', expected 'glove80'");
        }
    }

    let names = layout
        .get("layer_names")
        .and_then(Value::as_array)
        .context("MoErgo JSON has no layer_names array")?;
    let layers = layout
        .get("layers")
        .and_then(Value::as_array)
        .context("MoErgo JSON has no layers array")?;
    if names.len() != layers.len() {
        bail!(
            "MoErgo JSON has {} layer name(s) but {} layer array(s)",
            names.len(),
            layers.len()
        );
    }
    if layers.is_empty() {
        bail!("MoErgo JSON must contain at least one layer");
    }

    let layer_names = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            name.as_str()
                .map(str::to_owned)
                .with_context(|| format!("layer_names[{index}] must be a string"))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut ids = std::collections::BTreeSet::new();
    let mut converted = Vec::with_capacity(layers.len());
    for (layer_index, layer) in layers.iter().enumerate() {
        let keys = layer
            .as_array()
            .with_context(|| format!("layers[{layer_index}] must be an array"))?;
        if keys.len() != MOERGO_TO_MATRIX.len() {
            bail!(
                "layer {layer_index} ({}) has {} keys; a Glove80 editor layer must have 80",
                layer_names[layer_index],
                keys.len()
            );
        }
        let mut matrix = vec![0u16; LAYER_SIZE];
        for (editor_index, binding) in keys.iter().enumerate() {
            let offset = MOERGO_TO_MATRIX[editor_index];
            matrix[offset] =
                binding_to_via(binding, &layer_names, layer_index, editor_index, offset)?;
        }
        let name = layer_names[layer_index].clone();
        let base_id = slug(&name, layer_index);
        let mut id = base_id.clone();
        let mut suffix = 2;
        while !ids.insert(id.clone()) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        converted.push(LayerConfig {
            id,
            name,
            keys: crate::render_keys(&matrix),
        });
    }

    let config = RuntimeConfig {
        default_layer: 0,
        layers: converted,
        lighting: None,
    };
    config.snapshot().map(|_| config)
}

/// Render a runtime snapshot as a MoErgo editor JSON backup.
///
/// If `template` is supplied, every editor-owned field outside `layers` and
/// `layer_names` is retained, including a top-level `keymap` wrapper. This is
/// the lossless path for round-tripping metadata, combos, macros and custom
/// behaviors that Rynk does not manage.
pub fn snapshot_to_moergo_json(
    snapshot: &Snapshot,
    labels: Option<&RuntimeConfig>,
    template: Option<&str>,
) -> Result<String> {
    if snapshot.layers.is_empty() {
        bail!("configuration must contain at least one layer");
    }
    let mut root = match template {
        Some(text) => serde_json::from_str(text).context("invalid template JSON")?,
        None => default_document(),
    };
    let layout = layout_object_mut(&mut root)?;
    if let Some(keyboard) = layout.get("keyboard").and_then(Value::as_str) {
        if !keyboard.eq_ignore_ascii_case("glove80") {
            bail!("MoErgo JSON template is for keyboard '{keyboard}', expected 'glove80'");
        }
    }

    let names = snapshot
        .layers
        .iter()
        .enumerate()
        .map(|(index, _)| {
            Value::String(
                labels
                    .and_then(|config| config.layers.get(index))
                    .map_or_else(|| format!("Layer {index}"), |layer| layer.name.clone()),
            )
        })
        .collect();
    let mut layers = Vec::with_capacity(snapshot.layers.len());
    for (layer_index, matrix) in snapshot.layers.iter().enumerate() {
        if matrix.len() != LAYER_SIZE {
            bail!(
                "layer {layer_index} has {} matrix cells; expected {LAYER_SIZE}",
                matrix.len()
            );
        }
        let mut editor = Vec::with_capacity(MOERGO_TO_MATRIX.len());
        for (editor_index, offset) in MOERGO_TO_MATRIX.iter().copied().enumerate() {
            editor.push(via_to_binding(
                matrix[offset],
                layer_index,
                editor_index,
                offset,
            )?);
        }
        layers.push(Value::Array(editor));
    }
    layout.insert("keyboard".into(), Value::String("glove80".into()));
    layout.insert("layer_names".into(), Value::Array(names));
    layout.insert("layers".into(), Value::Array(layers));

    let mut text =
        serde_json::to_string_pretty(&root).context("could not serialize MoErgo JSON")?;
    text.push('\n');
    Ok(text)
}

fn default_document() -> Value {
    json!({
        "keyboard": "glove80",
        "firmware_api_version": "1",
        "locale": "en-US",
        "uuid": "",
        "parent_uuid": "",
        "unlisted": true,
        "date": 0,
        "creator": "",
        "title": "Rynkbench export",
        "notes": "",
        "tags": [],
        "custom_defined_behaviors": "",
        "custom_devicetree": "",
        "config_parameters": [],
        "layout_parameters": {},
        "macros": [],
        "holdTaps": [],
        "combos": [],
        "inputListeners": [],
        "layer_names": [],
        "layers": []
    })
}

fn layout_object(root: &Value) -> Result<&Map<String, Value>> {
    let object = root
        .as_object()
        .context("MoErgo JSON root must be an object")?;
    match object.get("keymap") {
        Some(value) => value
            .as_object()
            .context("MoErgo JSON keymap wrapper must contain an object"),
        None => Ok(object),
    }
}

fn layout_object_mut(root: &mut Value) -> Result<&mut Map<String, Value>> {
    let object = root
        .as_object_mut()
        .context("MoErgo JSON root must be an object")?;
    if object.contains_key("keymap") {
        return object
            .get_mut("keymap")
            .and_then(Value::as_object_mut)
            .context("MoErgo JSON keymap wrapper must contain an object");
    }
    Ok(object)
}

fn slug(name: &str, index: usize) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if result.is_empty() {
        format!("layer{index}")
    } else {
        result
    }
}

fn location(layer: usize, editor_index: usize, offset: usize) -> String {
    format!(
        "layer {layer}, editor key {editor_index} (r{},c{})",
        offset / usize::from(COLS),
        offset % usize::from(COLS)
    )
}

fn binding_to_via(
    binding: &Value,
    layer_names: &[String],
    layer: usize,
    editor_index: usize,
    offset: usize,
) -> Result<u16> {
    let object = binding.as_object().with_context(|| {
        format!(
            "{} must be a binding object",
            location(layer, editor_index, offset)
        )
    })?;
    let behavior = object
        .get("value")
        .and_then(Value::as_str)
        .with_context(|| {
            format!(
                "{} binding has no string value",
                location(layer, editor_index, offset)
            )
        })?;
    let params = object
        .get("params")
        .map(|value| {
            value.as_array().with_context(|| {
                format!(
                    "{} params must be an array",
                    location(layer, editor_index, offset)
                )
            })
        })
        .transpose()?
        .map(Vec::as_slice)
        .unwrap_or_default();
    let here = || location(layer, editor_index, offset);
    let param = |index: usize| -> Result<&Value> {
        params
            .get(index)
            .and_then(|value| {
                value
                    .as_object()
                    .and_then(|value| value.get("value"))
                    .or(Some(value))
            })
            .with_context(|| format!("{} {behavior} is missing parameter {}", here(), index + 1))
    };
    let no_params = || -> Result<()> {
        if params.is_empty() {
            Ok(())
        } else {
            bail!("{} {behavior} expects no parameters", here())
        }
    };
    let one_layer = |verb: &str| -> Result<u16> {
        if params.len() != 1 {
            bail!("{} {behavior} expects one layer parameter", here());
        }
        keycodes::parse_keycode(&format!("{verb}({})", layer_param(param(0)?, &here())?))
    };

    let result = match behavior {
        "&none" => {
            no_params()?;
            0
        }
        "&trans" => {
            no_params()?;
            1
        }
        "&kp" => {
            if params.len() != 1 {
                bail!("{} &kp expects one keycode parameter", here());
            }
            zmk_keycode_to_via(string_param(param(0)?, &here())?)?
        }
        "&mt" => {
            if params.len() != 2 {
                bail!("{} &mt expects hold and tap keycodes", here());
            }
            let modifier = zmk_modifier(string_param(param(0)?, &here())?)
                .with_context(|| format!("{} &mt hold action", here()))?;
            let tap = zmk_keycode_to_via(string_param(param(1)?, &here())?)?;
            if tap > 0xff {
                bail!("{} &mt tap action must be an unmodified HID key", here());
            }
            keycodes::parse_keycode(&format!(
                "MT({modifier}, {})",
                keycodes::format_keycode(tap)
            ))?
        }
        "&lt" => {
            if params.len() != 2 {
                bail!("{} &lt expects layer and tap keycode parameters", here());
            }
            let target = layer_param(param(0)?, &here())?;
            let tap = zmk_keycode_to_via(string_param(param(1)?, &here())?)?;
            if tap > 0xff {
                bail!("{} &lt tap action must be an unmodified HID key", here());
            }
            keycodes::parse_keycode(&format!("LT({target}, {})", keycodes::format_keycode(tap)))?
        }
        "&mo" => one_layer("MO")?,
        "&to" => one_layer("TO")?,
        "&tog" => one_layer("TG")?,
        "&sl" => one_layer("OSL")?,
        "&layer" => one_layer("MO")?,
        "&sk" => {
            if params.len() != 1 {
                bail!("{} &sk expects one modifier parameter", here());
            }
            keycodes::parse_keycode(&format!(
                "OSM({})",
                zmk_modifier(string_param(param(0)?, &here())?)?
            ))?
        }
        "&caps_word" => {
            no_params()?;
            keycodes::parse_keycode("CW_TOGG")?
        }
        "&key_repeat" => {
            no_params()?;
            keycodes::parse_keycode("QK_REP")?
        }
        "&bootloader" => {
            no_params()?;
            if offset % usize::from(COLS) >= 7 {
                keycodes::parse_keycode("USER(12)")?
            } else {
                keycodes::parse_keycode("QK_BOOT")?
            }
        }
        "&reset" | "&sys_reset" => {
            no_params()?;
            keycodes::parse_keycode("QK_RBT")?
        }
        "&magic" | "&lower" => {
            no_params()?;
            let wanted = if behavior == "&magic" {
                "Magic"
            } else {
                "Lower"
            };
            let target = layer_names
                .iter()
                .position(|name| name == wanted)
                .with_context(|| {
                    format!(
                        "{} {behavior} needs a layer named exactly '{wanted}'",
                        here()
                    )
                })?;
            keycodes::parse_keycode(&format!("MO({target})"))?
        }
        "&bt_0" | "&bt_1" | "&bt_2" | "&bt_3" => {
            no_params()?;
            keycodes::parse_keycode(&format!("USER({})", &behavior[4..]))?
        }
        "&bt" => {
            let command = string_param(param(0)?, &here())?;
            match command {
                "BT_CLR" => keycodes::parse_keycode("USER(10)")?,
                "BT_CLR_ALL" => keycodes::parse_keycode("USER(11)")?,
                _ => bail!("{} unsupported Bluetooth command '{command}'", here()),
            }
        }
        "&out" => {
            let command = string_param(param(0)?, &here())?;
            match command {
                "OUT_USB" => keycodes::parse_keycode("QK_OUTPUT_USB")?,
                "OUT_BLE" => keycodes::parse_keycode("QK_OUTPUT_BLUETOOTH")?,
                _ => bail!("{} unsupported output command '{command}'", here()),
            }
        }
        "&rgb_ug" => {
            let command = string_param(param(0)?, &here())?;
            let via = match command {
                "RGB_TOG" => "RGB_TOG",
                "RGB_HUI" => "RGB_HUI",
                "RGB_HUD" => "RGB_HUD",
                "RGB_SAI" => "RGB_SAI",
                "RGB_SAD" => "RGB_SAD",
                "RGB_BRI" => "RGB_VAI",
                "RGB_BRD" => "RGB_VAD",
                "RGB_SPI" => "RGB_SPI",
                "RGB_SPD" => "RGB_SPD",
                "RGB_EFF" => "RGB_MOD",
                "RGB_EFR" => "RGB_RMOD",
                _ => bail!("{} unsupported RGB command '{command}'", here()),
            };
            keycodes::parse_keycode(via)?
        }
        "&mkp" | "&mmv" | "&msc" => {
            if params.len() != 1 {
                bail!("{} {behavior} expects one mouse action parameter", here());
            }
            let action = string_param(param(0)?, &here())?;
            let via = match (behavior, action) {
                ("&mkp", "LCLK" | "MB1") => "KC_BTN1",
                ("&mkp", "RCLK" | "MB2") => "KC_BTN2",
                ("&mkp", "MCLK" | "MB3") => "KC_BTN3",
                ("&mkp", "MB4") => "KC_BTN4",
                ("&mkp", "MB5") => "KC_BTN5",
                ("&mmv", "MOVE_UP") => "KC_MS_U",
                ("&mmv", "MOVE_DOWN") => "KC_MS_D",
                ("&mmv", "MOVE_LEFT") => "KC_MS_L",
                ("&mmv", "MOVE_RIGHT") => "KC_MS_R",
                ("&msc", "SCRL_UP") => "KC_WH_U",
                ("&msc", "SCRL_DOWN") => "KC_WH_D",
                ("&msc", "SCRL_LEFT") => "KC_WH_L",
                ("&msc", "SCRL_RIGHT") => "KC_WH_R",
                _ => bail!("{} unsupported {behavior} action '{action}'", here()),
            };
            keycodes::parse_keycode(via)?
        }
        _ => bail!(
            "{} behavior '{behavior}' cannot be represented by the Rynk runtime keymap",
            here()
        ),
    };
    Ok(result)
}

fn string_param<'a>(value: &'a Value, location: &str) -> Result<&'a str> {
    value
        .as_str()
        .with_context(|| format!("{location} parameter must be a string"))
}

fn layer_param(value: &Value, location: &str) -> Result<u8> {
    let number = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .with_context(|| format!("{location} layer parameter must be an integer"))?;
    u8::try_from(number)
        .ok()
        .filter(|layer| *layer < 16)
        .with_context(|| format!("{location} layer {number} is outside Rynk's 0..15 range"))
}

fn zmk_modifier(text: &str) -> Result<String> {
    let upper = text.trim().to_ascii_uppercase();
    let modifier = match upper.as_str() {
        "LCTRL" | "LEFT_CONTROL" => "MOD_LCTL",
        "LSHFT" | "LSHIFT" | "LEFT_SHIFT" => "MOD_LSFT",
        "LALT" | "LEFT_ALT" => "MOD_LALT",
        "LGUI" | "LEFT_GUI" => "MOD_LGUI",
        "RCTRL" | "RIGHT_CONTROL" => "MOD_RCTL",
        "RSHFT" | "RSHIFT" | "RIGHT_SHIFT" => "MOD_RSFT",
        "RALT" | "RIGHT_ALT" => "MOD_RALT",
        "RGUI" | "RIGHT_GUI" => "MOD_RGUI",
        _ => bail!("'{text}' is not a modifier key"),
    };
    Ok(modifier.into())
}

fn zmk_keycode_to_via(text: &str) -> Result<u16> {
    let text = text.trim();
    if let Some((function, argument)) = split_call(text) {
        let wrapper = match function.to_ascii_uppercase().as_str() {
            "LC" => "LCTL",
            "LS" => "LSFT",
            "LA" => "LALT",
            "LG" => "LGUI",
            "RC" => "RCTL",
            "RS" => "RSFT",
            "RA" => "RALT",
            "RG" => "RGUI",
            _ => bail!("unsupported ZMK keycode function '{function}'"),
        };
        let inner = zmk_keycode_to_via(argument)?;
        return keycodes::parse_keycode(&format!("{wrapper}({})", keycodes::format_keycode(inner)));
    }
    let upper = text.to_ascii_uppercase();
    if let Some(base) = shifted_base(&upper) {
        return keycodes::parse_keycode(&format!("LSFT({})", keycodes::format_keycode(base)));
    }
    let qmk = match upper.as_str() {
        "N1" | "NUMBER_1" => "KC_1",
        "N2" | "NUMBER_2" => "KC_2",
        "N3" | "NUMBER_3" => "KC_3",
        "N4" | "NUMBER_4" => "KC_4",
        "N5" | "NUMBER_5" => "KC_5",
        "N6" | "NUMBER_6" => "KC_6",
        "N7" | "NUMBER_7" => "KC_7",
        "N8" | "NUMBER_8" => "KC_8",
        "N9" | "NUMBER_9" => "KC_9",
        "N0" | "NUMBER_0" => "KC_0",
        "RET" | "RETURN" => "KC_ENT",
        "ESC" | "ESCAPE" => "KC_ESC",
        "BSPC" | "BACKSPACE" => "KC_BSPC",
        "SPACE" => "KC_SPC",
        "MINUS" => "KC_MINS",
        "EQUAL" => "KC_EQL",
        "LBKT" | "LEFT_BRACKET" => "KC_LBRC",
        "RBKT" | "RIGHT_BRACKET" => "KC_RBRC",
        "BSLH" | "BACKSLASH" => "KC_BSLS",
        "SEMI" | "SEMICOLON" => "KC_SCLN",
        "SQT" | "SINGLE_QUOTE" => "KC_QUOT",
        "GRAVE" => "KC_GRV",
        "COMMA" => "KC_COMM",
        "FSLH" | "SLASH" => "KC_SLSH",
        "CAPS" | "CAPSLOCK" => "KC_CAPS",
        "PRINTSCREEN" => "KC_PSCR",
        "SCROLLLOCK" => "KC_SCRL",
        "PAUSE_BREAK" => "KC_PAUS",
        "INS" | "INSERT" => "KC_INS",
        "PG_UP" | "PAGE_UP" => "KC_PGUP",
        "DEL" | "DELETE" => "KC_DEL",
        "PG_DN" | "PAGE_DOWN" => "KC_PGDN",
        "RIGHT" | "RIGHT_ARROW" => "KC_RGHT",
        "LEFT" | "LEFT_ARROW" => "KC_LEFT",
        "DOWN" | "DOWN_ARROW" => "KC_DOWN",
        "UP" | "UP_ARROW" => "KC_UP",
        "KP_NUM" | "KP_NUMLOCK" => "KC_NUM",
        "KP_SLASH" | "KP_DIVIDE" => "KC_PSLS",
        "KP_MULTIPLY" => "KC_PAST",
        "KP_MINUS" => "KC_PMNS",
        "KP_PLUS" => "KC_PPLS",
        "KP_ENTER" => "KC_PENT",
        "KP_N1" | "KP_NUMBER_1" => "KC_P1",
        "KP_N2" | "KP_NUMBER_2" => "KC_P2",
        "KP_N3" | "KP_NUMBER_3" => "KC_P3",
        "KP_N4" | "KP_NUMBER_4" => "KC_P4",
        "KP_N5" | "KP_NUMBER_5" => "KC_P5",
        "KP_N6" | "KP_NUMBER_6" => "KC_P6",
        "KP_N7" | "KP_NUMBER_7" => "KC_P7",
        "KP_N8" | "KP_NUMBER_8" => "KC_P8",
        "KP_N9" | "KP_NUMBER_9" => "KC_P9",
        "KP_N0" | "KP_NUMBER_0" => "KC_P0",
        "KP_DOT" => "KC_PDOT",
        "KP_EQUAL" => "KC_PEQL",
        "K_APP" | "K_APPLICATION" => "KC_APP",
        "LCTRL" | "LEFT_CONTROL" => "KC_LCTL",
        "LSHFT" | "LSHIFT" | "LEFT_SHIFT" => "KC_LSFT",
        "LALT" | "LEFT_ALT" => "KC_LALT",
        "LGUI" | "LEFT_GUI" => "KC_LGUI",
        "RCTRL" | "RIGHT_CONTROL" => "KC_RCTL",
        "RSHFT" | "RSHIFT" | "RIGHT_SHIFT" => "KC_RSFT",
        "RALT" | "RIGHT_ALT" => "KC_RALT",
        "RGUI" | "RIGHT_GUI" => "KC_RGUI",
        "C_POWER" => "KC_PWR",
        "C_SLEEP" => "KC_SLEP",
        "C_MUTE" => "KC_MUTE",
        "C_VOL_UP" => "KC_VOLU",
        "C_VOL_DN" => "KC_VOLD",
        "C_NEXT" => "KC_MNXT",
        "C_PREV" => "KC_MPRV",
        "C_STOP" => "KC_MSTP",
        "C_PP" | "K_PLAY_PAUSE" => "KC_MPLY",
        "C_EJECT" | "K_EJECT" => "KC_EJCT",
        "C_BRI_UP" => "KC_BRIU",
        "C_BRI_DN" => "KC_BRID",
        _ => return keycodes::parse_keycode(&upper),
    };
    keycodes::parse_keycode(qmk)
}

fn split_call(text: &str) -> Option<(&str, &str)> {
    let open = text.find('(')?;
    let argument = text.get(open + 1..text.len().checked_sub(1)?)?;
    text.ends_with(')').then_some((&text[..open], argument))
}

fn shifted_base(name: &str) -> Option<u16> {
    let base = match name {
        "EXCL" | "EXCLAMATION" => "KC_1",
        "AT" | "AT_SIGN" => "KC_2",
        "HASH" => "KC_3",
        "DLLR" | "DOLLAR" => "KC_4",
        "PRCNT" | "PERCENT" => "KC_5",
        "CARET" => "KC_6",
        "AMPS" | "AMPERSAND" => "KC_7",
        "STAR" | "ASTERISK" => "KC_8",
        "LPAR" | "LEFT_PARENTHESIS" => "KC_9",
        "RPAR" | "RIGHT_PARENTHESIS" => "KC_0",
        "UNDER" | "UNDERSCORE" => "KC_MINS",
        "PLUS" => "KC_EQL",
        "LBRC" | "LEFT_BRACE" => "KC_LBRC",
        "RBRC" | "RIGHT_BRACE" => "KC_RBRC",
        "PIPE" => "KC_BSLS",
        "COLON" => "KC_SCLN",
        "DQT" | "DOUBLE_QUOTES" => "KC_QUOT",
        "TILDE" => "KC_GRV",
        "LT" | "LESS_THAN" => "KC_COMM",
        "GT" | "GREATER_THAN" => "KC_DOT",
        "QMARK" | "QUESTION" => "KC_SLSH",
        _ => return None,
    };
    keycodes::parse_keycode(base).ok()
}

fn via_to_binding(code: u16, layer: usize, editor_index: usize, offset: usize) -> Result<Value> {
    let here = || location(layer, editor_index, offset);
    match code {
        0 => return Ok(zero_binding("&none")),
        1 => return Ok(zero_binding("&trans")),
        0xcd => return Ok(one_binding("&mmv", "MOVE_UP")),
        0xce => return Ok(one_binding("&mmv", "MOVE_DOWN")),
        0xcf => return Ok(one_binding("&mmv", "MOVE_LEFT")),
        0xd0 => return Ok(one_binding("&mmv", "MOVE_RIGHT")),
        0xd1 => return Ok(one_binding("&mkp", "LCLK")),
        0xd2 => return Ok(one_binding("&mkp", "RCLK")),
        0xd3 => return Ok(one_binding("&mkp", "MCLK")),
        0xd4 => return Ok(one_binding("&mkp", "MB4")),
        0xd5 => return Ok(one_binding("&mkp", "MB5")),
        0xd9 => return Ok(one_binding("&msc", "SCRL_UP")),
        0xda => return Ok(one_binding("&msc", "SCRL_DOWN")),
        0xdb => return Ok(one_binding("&msc", "SCRL_LEFT")),
        0xdc => return Ok(one_binding("&msc", "SCRL_RIGHT")),
        0x0002..=0x00ff => return Ok(one_binding("&kp", via_basic_to_zmk(code)?)),
        0x0100..=0x1fff => return Ok(one_binding("&kp", modified_to_zmk(code)?)),
        0x2000..=0x3fff => {
            let modifiers = ((code >> 8) & 0x1f) as u8;
            let tap = via_basic_to_zmk(code & 0xff)?;
            return Ok(two_binding("&mt", modifier_bits_to_zmk(modifiers)?, tap));
        }
        0x4000..=0x4fff => {
            return Ok(two_binding(
                "&lt",
                Value::from((code >> 8) & 0xf),
                via_basic_to_zmk(code & 0xff)?,
            ));
        }
        0x5200..=0x521f => return Ok(layer_binding("&to", code & 0xf)),
        0x5220..=0x523f => return Ok(layer_binding("&mo", code & 0xf)),
        0x5260..=0x527f => return Ok(layer_binding("&tog", code & 0xf)),
        0x5280..=0x529f => return Ok(layer_binding("&sl", code & 0xf)),
        0x52a0..=0x52bf => {
            return Ok(one_binding(
                "&sk",
                modifier_bits_to_zmk((code & 0x1f) as u8)?,
            ));
        }
        0x7000..=0x701f => {
            return Ok(one_binding(
                "&kp",
                modifier_bits_to_zmk((code & 0x1f) as u8)?,
            ));
        }
        0x7784 => return Ok(one_binding("&out", "OUT_USB")),
        0x7786 => return Ok(one_binding("&out", "OUT_BLE")),
        0x7820 => return Ok(one_binding("&rgb_ug", "RGB_TOG")),
        0x7821 => return Ok(one_binding("&rgb_ug", "RGB_EFF")),
        0x7822 => return Ok(one_binding("&rgb_ug", "RGB_EFR")),
        0x7823 => return Ok(one_binding("&rgb_ug", "RGB_HUI")),
        0x7824 => return Ok(one_binding("&rgb_ug", "RGB_HUD")),
        0x7825 => return Ok(one_binding("&rgb_ug", "RGB_SAI")),
        0x7826 => return Ok(one_binding("&rgb_ug", "RGB_SAD")),
        0x7827 => return Ok(one_binding("&rgb_ug", "RGB_BRI")),
        0x7828 => return Ok(one_binding("&rgb_ug", "RGB_BRD")),
        0x7829 => return Ok(one_binding("&rgb_ug", "RGB_SPI")),
        0x782a => return Ok(one_binding("&rgb_ug", "RGB_SPD")),
        0x7c00 => return Ok(zero_binding("&bootloader")),
        0x7c01 => return Ok(zero_binding("&reset")),
        0x7c73 => return Ok(zero_binding("&caps_word")),
        0x7c79 => return Ok(zero_binding("&key_repeat")),
        0x7e00..=0x7e03 => return Ok(zero_binding(&format!("&bt_{}", code & 0x1f))),
        0x7e0a => return Ok(one_binding("&bt", "BT_CLR")),
        0x7e0b => return Ok(one_binding("&bt", "BT_CLR_ALL")),
        0x7e0c if offset % usize::from(COLS) >= 7 => return Ok(zero_binding("&bootloader")),
        _ => {}
    }
    bail!(
        "{} runtime action {} cannot be represented by the MoErgo editor JSON format",
        here(),
        keycodes::format_keycode(code)
    )
}

fn via_basic_to_zmk(code: u16) -> Result<&'static str> {
    let name = match code {
        0x04 => "A",
        0x05 => "B",
        0x06 => "C",
        0x07 => "D",
        0x08 => "E",
        0x09 => "F",
        0x0a => "G",
        0x0b => "H",
        0x0c => "I",
        0x0d => "J",
        0x0e => "K",
        0x0f => "L",
        0x10 => "M",
        0x11 => "N",
        0x12 => "O",
        0x13 => "P",
        0x14 => "Q",
        0x15 => "R",
        0x16 => "S",
        0x17 => "T",
        0x18 => "U",
        0x19 => "V",
        0x1a => "W",
        0x1b => "X",
        0x1c => "Y",
        0x1d => "Z",
        0x1e => "N1",
        0x1f => "N2",
        0x20 => "N3",
        0x21 => "N4",
        0x22 => "N5",
        0x23 => "N6",
        0x24 => "N7",
        0x25 => "N8",
        0x26 => "N9",
        0x27 => "N0",
        0x28 => "RET",
        0x29 => "ESC",
        0x2a => "BSPC",
        0x2b => "TAB",
        0x2c => "SPACE",
        0x2d => "MINUS",
        0x2e => "EQUAL",
        0x2f => "LBKT",
        0x30 => "RBKT",
        0x31 => "BSLH",
        0x33 => "SEMI",
        0x34 => "SQT",
        0x35 => "GRAVE",
        0x36 => "COMMA",
        0x37 => "DOT",
        0x38 => "FSLH",
        0x39 => "CAPS",
        0x3a => "F1",
        0x3b => "F2",
        0x3c => "F3",
        0x3d => "F4",
        0x3e => "F5",
        0x3f => "F6",
        0x40 => "F7",
        0x41 => "F8",
        0x42 => "F9",
        0x43 => "F10",
        0x44 => "F11",
        0x45 => "F12",
        0x46 => "PRINTSCREEN",
        0x47 => "SCROLLLOCK",
        0x48 => "PAUSE_BREAK",
        0x49 => "INS",
        0x4a => "HOME",
        0x4b => "PG_UP",
        0x4c => "DEL",
        0x4d => "END",
        0x4e => "PG_DN",
        0x4f => "RIGHT",
        0x50 => "LEFT",
        0x51 => "DOWN",
        0x52 => "UP",
        0x53 => "KP_NUM",
        0x54 => "KP_SLASH",
        0x55 => "KP_MULTIPLY",
        0x56 => "KP_MINUS",
        0x57 => "KP_PLUS",
        0x58 => "KP_ENTER",
        0x59 => "KP_N1",
        0x5a => "KP_N2",
        0x5b => "KP_N3",
        0x5c => "KP_N4",
        0x5d => "KP_N5",
        0x5e => "KP_N6",
        0x5f => "KP_N7",
        0x60 => "KP_N8",
        0x61 => "KP_N9",
        0x62 => "KP_N0",
        0x63 => "KP_DOT",
        0x65 => "K_APP",
        0x67 => "KP_EQUAL",
        0x68 => "F13",
        0x69 => "F14",
        0x6a => "F15",
        0x6b => "F16",
        0x6c => "F17",
        0x6d => "F18",
        0x6e => "F19",
        0x6f => "F20",
        0x70 => "F21",
        0x71 => "F22",
        0x72 => "F23",
        0x73 => "F24",
        0xa5 => "C_POWER",
        0xa6 => "C_SLEEP",
        0xa8 => "C_MUTE",
        0xa9 => "C_VOL_UP",
        0xaa => "C_VOL_DN",
        0xab => "C_NEXT",
        0xac => "C_PREV",
        0xad => "C_STOP",
        0xae => "C_PP",
        0xb0 => "C_EJECT",
        0xbd => "C_BRI_UP",
        0xbe => "C_BRI_DN",
        0xe0 => "LCTRL",
        0xe1 => "LSHFT",
        0xe2 => "LALT",
        0xe3 => "LGUI",
        0xe4 => "RCTRL",
        0xe5 => "RSHFT",
        0xe6 => "RALT",
        0xe7 => "RGUI",
        _ => bail!(
            "basic runtime keycode {} has no MoErgo name",
            keycodes::format_keycode(code)
        ),
    };
    Ok(name)
}

fn modified_to_zmk(code: u16) -> Result<String> {
    let base = via_basic_to_zmk(code & 0xff)?.to_owned();
    apply_modifier_wrappers(base, (code >> 8) as u8)
}

fn modifier_bits_to_zmk(bits: u8) -> Result<String> {
    let right = bits & 0x10 != 0;
    let mods = bits & 0x0f;
    let last = [
        (0x08, if right { "RGUI" } else { "LGUI" }),
        (0x04, if right { "RALT" } else { "LALT" }),
        (0x02, if right { "RSHFT" } else { "LSHFT" }),
        (0x01, if right { "RCTRL" } else { "LCTRL" }),
    ]
    .into_iter()
    .find(|(bit, _)| mods & bit != 0)
    .context("empty modifier combination")?;
    let remaining = mods & !last.0;
    apply_modifier_wrappers(last.1.to_owned(), remaining | if right { 0x10 } else { 0 })
}

fn apply_modifier_wrappers(mut inner: String, bits: u8) -> Result<String> {
    let right = bits & 0x10 != 0;
    let mods = bits & 0x0f;
    if mods == 0 {
        return Ok(inner);
    }
    for (bit, left, right_name) in [
        (0x08, "LG", "RG"),
        (0x04, "LA", "RA"),
        (0x02, "LS", "RS"),
        (0x01, "LC", "RC"),
    ] {
        if mods & bit != 0 {
            inner = format!("{}({inner})", if right { right_name } else { left });
        }
    }
    Ok(inner)
}

fn zero_binding(behavior: &str) -> Value {
    json!({ "value": behavior })
}

fn one_binding(behavior: &str, param: impl Into<Value>) -> Value {
    json!({ "value": behavior, "params": [{ "value": param.into(), "params": [] }] })
}

fn two_binding(behavior: &str, first: impl Into<Value>, second: impl Into<Value>) -> Value {
    json!({
        "value": behavior,
        "params": [
            { "value": first.into(), "params": [] },
            { "value": second.into(), "params": [] }
        ]
    })
}

fn layer_binding(behavior: &str, layer: u16) -> Value {
    one_binding(behavior, Value::from(layer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn physical_order_covers_every_non_hole_matrix_cell_once() {
        let mut offsets = MOERGO_TO_MATRIX.to_vec();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), 80);
        assert_eq!(
            offsets,
            (0..LAYER_SIZE)
                .filter(|n| ![5, 8, 75, 78].contains(n))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parses_editor_bindings_and_preserves_matrix_holes() {
        let mut keys = vec![json!({ "value": "&trans" }); 80];
        keys[0] = json!({ "value": "&kp", "params": [{ "value": "F1", "params": [] }] });
        keys[52] = json!({ "value": "&lt", "params": [{ "value": 1 }, { "value": "ESC" }] });
        keys[79] = json!({ "value": "&rgb_ug", "params": [{ "value": "RGB_TOG" }] });
        let text = json!({
            "keymap": {
                "keyboard": "glove80",
                "layer_names": ["Base", "Lower"],
                "layers": [keys, vec![json!({ "value": "&none" }); 80]]
            }
        })
        .to_string();
        let config = runtime_config_from_moergo_json(&text).unwrap();
        let snapshot = config.snapshot().unwrap();
        assert_eq!(
            snapshot.layers[0][0],
            keycodes::parse_keycode("KC_F1").unwrap()
        );
        assert_eq!(
            snapshot.layers[0][6],
            keycodes::parse_keycode("LT(1,KC_ESC)").unwrap()
        );
        assert_eq!(
            snapshot.layers[0][83],
            keycodes::parse_keycode("RGB_TOG").unwrap()
        );
        for hole in [5, 8, 75, 78] {
            assert_eq!(snapshot.layers[0][hole], 0);
        }
    }

    #[test]
    fn json_round_trip_preserves_template_metadata_and_wrapper() {
        let keys = (0..80)
            .map(|index| {
                if index == 54 {
                    json!({ "value": "&magic" })
                } else {
                    json!({ "value": "&trans" })
                }
            })
            .collect::<Vec<_>>();
        let text = json!({
            "untouched": 42,
            "keymap": {
                "keyboard": "glove80",
                "title": "Keep me",
                "layer_names": ["Base", "Magic"],
                "layers": [keys, vec![json!({ "value": "&none" }); 80]],
                "combos": [{ "name": "kept" }]
            }
        })
        .to_string();
        let config = runtime_config_from_moergo_json(&text).unwrap();
        let rendered =
            snapshot_to_moergo_json(&config.snapshot().unwrap(), Some(&config), Some(&text))
                .unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["untouched"], 42);
        assert_eq!(value["keymap"]["title"], "Keep me");
        assert_eq!(value["keymap"]["combos"][0]["name"], "kept");
        assert_eq!(value["keymap"]["layers"][0].as_array().unwrap().len(), 80);
    }

    #[test]
    fn unsupported_custom_behavior_names_its_exact_key() {
        let err = binding_to_via(
            &json!({ "value": "&my_macro" }),
            &["Base".into()],
            0,
            52,
            MOERGO_TO_MATRIX[52],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("editor key 52 (r0,c6)"), "{err}");
        assert!(err.contains("&my_macro"), "{err}");
    }

    #[test]
    fn shifted_and_modified_keycodes_round_trip() {
        for binding_value in [
            json!({ "value": "&kp", "params": [{ "value": "EXCL" }] }),
            json!({ "value": "&kp", "params": [{ "value": "LC(LS(A))" }] }),
            json!({ "value": "&mt", "params": [{ "value": "LSHFT" }, { "value": "A" }] }),
        ] {
            let code = binding_to_via(&binding_value, &["Base".into()], 0, 0, 0).unwrap();
            let rendered = via_to_binding(code, 0, 0, 0).unwrap();
            let reparsed = binding_to_via(&rendered, &["Base".into()], 0, 0, 0).unwrap();
            assert_eq!(
                reparsed,
                code,
                "{} -> {}",
                binding(binding_value),
                binding(rendered)
            );
        }
    }

    #[test]
    fn mouse_bindings_round_trip() {
        for binding_value in [
            json!({ "value": "&mkp", "params": [{ "value": "LCLK" }] }),
            json!({ "value": "&mmv", "params": [{ "value": "MOVE_LEFT" }] }),
            json!({ "value": "&msc", "params": [{ "value": "SCRL_DOWN" }] }),
        ] {
            let code = binding_to_via(&binding_value, &["Base".into()], 0, 0, 0).unwrap();
            let rendered = via_to_binding(code, 0, 0, 0).unwrap();
            let reparsed = binding_to_via(&rendered, &["Base".into()], 0, 0, 0).unwrap();
            assert_eq!(reparsed, code);
        }
    }
}
