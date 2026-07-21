//! VIA/Vial 16-bit keycode names: formatting, parsing, and search.
//!
//! The wire format is the VIA keycode encoding as implemented by the
//! firmware's Vial conversion (`rmk/src/host/via/keycode_convert.rs`), so
//! this table names exactly the ranges that firmware round-trips:
//!
//! - `0x0000..=0x00FF` basic HID keycodes (QMK `KC_*` names; includes the
//!   0xA5+ system/consumer aliases like `KC_MPLY` and the mouse keys)
//! - `0x0100..=0x1FFF` modified keys — `LCTL(kc)`, `MEH(kc)`, `HYPR(kc)`, …
//! - `0x2000..=0x3FFF` mod-taps — `LSFT_T(kc)`, `MEH_T(kc)`, `ALL_T(kc)`,
//!   `MT(MOD_…|…, kc)`
//! - `0x4000..=0x4FFF` layer-taps — `LT(layer, kc)` (layer 0-15)
//! - `0x5000..=0x51FF` layer-with-modifier — `LM(layer, MOD_…)`
//! - `0x5200..` layer verbs — `TO(n)`, `MO(n)`, `DF(n)`, `TG(n)`, `OSL(n)`,
//!   `OSM(MOD_…)`, `TT(n)` (named, but the firmware stores it as `KC_NO`),
//!   `PDF(n)`
//! - `0x5700..=0x57FF` tap dance / morse — `TD(n)`
//! - `0x7700..=0x771F` macros — `MACRO(n)`
//! - `0x7C00..` QMK magic keys the firmware supports (`QK_BOOT`, `CW_TOGG`,
//!   space cadet, …)
//! - `0x7E00..=0x7E1F` user/custom keys — `USER(n)`
//!
//! Anything else formats as bare hex (`0x1234`) and can be entered the same
//! way; formatting never fails.

use anyhow::{anyhow, bail, Result};

/// Packed modifier bits, matching the firmware/QMK 5-bit encoding.
const MOD_CTRL: u8 = 0x01;
const MOD_SHIFT: u8 = 0x02;
const MOD_ALT: u8 = 0x04;
const MOD_GUI: u8 = 0x08;
/// Set = the right-hand variants of every modifier bit below it.
const MOD_RIGHT: u8 = 0x10;
const MOD_MEH: u8 = MOD_CTRL | MOD_SHIFT | MOD_ALT;
const MOD_HYPR: u8 = MOD_MEH | MOD_GUI;

/// `(code, canonical QMK name, aliases)` for the basic (< 0x0100) range.
/// Canonical names are the short QMK spellings VIA/Vial display.
const BASIC: &[(u16, &str, &[&str])] = &[
    (0x0000, "KC_NO", &["XXXXXXX", "KC_NONE"]),
    (0x0001, "KC_TRNS", &["KC_TRANSPARENT", "_______"]),
    (0x0004, "KC_A", &[]),
    (0x0005, "KC_B", &[]),
    (0x0006, "KC_C", &[]),
    (0x0007, "KC_D", &[]),
    (0x0008, "KC_E", &[]),
    (0x0009, "KC_F", &[]),
    (0x000A, "KC_G", &[]),
    (0x000B, "KC_H", &[]),
    (0x000C, "KC_I", &[]),
    (0x000D, "KC_J", &[]),
    (0x000E, "KC_K", &[]),
    (0x000F, "KC_L", &[]),
    (0x0010, "KC_M", &[]),
    (0x0011, "KC_N", &[]),
    (0x0012, "KC_O", &[]),
    (0x0013, "KC_P", &[]),
    (0x0014, "KC_Q", &[]),
    (0x0015, "KC_R", &[]),
    (0x0016, "KC_S", &[]),
    (0x0017, "KC_T", &[]),
    (0x0018, "KC_U", &[]),
    (0x0019, "KC_V", &[]),
    (0x001A, "KC_W", &[]),
    (0x001B, "KC_X", &[]),
    (0x001C, "KC_Y", &[]),
    (0x001D, "KC_Z", &[]),
    (0x001E, "KC_1", &[]),
    (0x001F, "KC_2", &[]),
    (0x0020, "KC_3", &[]),
    (0x0021, "KC_4", &[]),
    (0x0022, "KC_5", &[]),
    (0x0023, "KC_6", &[]),
    (0x0024, "KC_7", &[]),
    (0x0025, "KC_8", &[]),
    (0x0026, "KC_9", &[]),
    (0x0027, "KC_0", &[]),
    (0x0028, "KC_ENT", &["KC_ENTER"]),
    (0x0029, "KC_ESC", &["KC_ESCAPE"]),
    (0x002A, "KC_BSPC", &["KC_BACKSPACE"]),
    (0x002B, "KC_TAB", &[]),
    (0x002C, "KC_SPC", &["KC_SPACE"]),
    (0x002D, "KC_MINS", &["KC_MINUS"]),
    (0x002E, "KC_EQL", &["KC_EQUAL"]),
    (0x002F, "KC_LBRC", &["KC_LEFT_BRACKET"]),
    (0x0030, "KC_RBRC", &["KC_RIGHT_BRACKET"]),
    (0x0031, "KC_BSLS", &["KC_BACKSLASH"]),
    (0x0032, "KC_NUHS", &["KC_NONUS_HASH"]),
    (0x0033, "KC_SCLN", &["KC_SEMICOLON"]),
    (0x0034, "KC_QUOT", &["KC_QUOTE"]),
    (0x0035, "KC_GRV", &["KC_GRAVE"]),
    (0x0036, "KC_COMM", &["KC_COMMA"]),
    (0x0037, "KC_DOT", &[]),
    (0x0038, "KC_SLSH", &["KC_SLASH"]),
    (0x0039, "KC_CAPS", &["KC_CAPS_LOCK"]),
    (0x003A, "KC_F1", &[]),
    (0x003B, "KC_F2", &[]),
    (0x003C, "KC_F3", &[]),
    (0x003D, "KC_F4", &[]),
    (0x003E, "KC_F5", &[]),
    (0x003F, "KC_F6", &[]),
    (0x0040, "KC_F7", &[]),
    (0x0041, "KC_F8", &[]),
    (0x0042, "KC_F9", &[]),
    (0x0043, "KC_F10", &[]),
    (0x0044, "KC_F11", &[]),
    (0x0045, "KC_F12", &[]),
    (0x0046, "KC_PSCR", &["KC_PRINT_SCREEN"]),
    (0x0047, "KC_SCRL", &["KC_SCROLL_LOCK"]),
    (0x0048, "KC_PAUS", &["KC_PAUSE"]),
    (0x0049, "KC_INS", &["KC_INSERT"]),
    (0x004A, "KC_HOME", &[]),
    (0x004B, "KC_PGUP", &["KC_PAGE_UP"]),
    (0x004C, "KC_DEL", &["KC_DELETE"]),
    (0x004D, "KC_END", &[]),
    (0x004E, "KC_PGDN", &["KC_PAGE_DOWN"]),
    (0x004F, "KC_RGHT", &["KC_RIGHT"]),
    (0x0050, "KC_LEFT", &[]),
    (0x0051, "KC_DOWN", &[]),
    (0x0052, "KC_UP", &[]),
    (0x0053, "KC_NUM", &["KC_NUM_LOCK"]),
    (0x0054, "KC_PSLS", &["KC_KP_SLASH"]),
    (0x0055, "KC_PAST", &["KC_KP_ASTERISK"]),
    (0x0056, "KC_PMNS", &["KC_KP_MINUS"]),
    (0x0057, "KC_PPLS", &["KC_KP_PLUS"]),
    (0x0058, "KC_PENT", &["KC_KP_ENTER"]),
    (0x0059, "KC_P1", &["KC_KP_1"]),
    (0x005A, "KC_P2", &["KC_KP_2"]),
    (0x005B, "KC_P3", &["KC_KP_3"]),
    (0x005C, "KC_P4", &["KC_KP_4"]),
    (0x005D, "KC_P5", &["KC_KP_5"]),
    (0x005E, "KC_P6", &["KC_KP_6"]),
    (0x005F, "KC_P7", &["KC_KP_7"]),
    (0x0060, "KC_P8", &["KC_KP_8"]),
    (0x0061, "KC_P9", &["KC_KP_9"]),
    (0x0062, "KC_P0", &["KC_KP_0"]),
    (0x0063, "KC_PDOT", &["KC_KP_DOT"]),
    (0x0064, "KC_NUBS", &["KC_NONUS_BACKSLASH"]),
    (0x0065, "KC_APP", &["KC_APPLICATION"]),
    (0x0066, "KC_KB_POWER", &[]),
    (0x0067, "KC_PEQL", &["KC_KP_EQUAL"]),
    (0x0068, "KC_F13", &[]),
    (0x0069, "KC_F14", &[]),
    (0x006A, "KC_F15", &[]),
    (0x006B, "KC_F16", &[]),
    (0x006C, "KC_F17", &[]),
    (0x006D, "KC_F18", &[]),
    (0x006E, "KC_F19", &[]),
    (0x006F, "KC_F20", &[]),
    (0x0070, "KC_F21", &[]),
    (0x0071, "KC_F22", &[]),
    (0x0072, "KC_F23", &[]),
    (0x0073, "KC_F24", &[]),
    (0x0074, "KC_EXEC", &["KC_EXECUTE"]),
    (0x0075, "KC_HELP", &[]),
    (0x0076, "KC_MENU", &[]),
    (0x0077, "KC_SLCT", &["KC_SELECT"]),
    (0x0078, "KC_STOP", &[]),
    (0x0079, "KC_AGIN", &["KC_AGAIN"]),
    (0x007A, "KC_UNDO", &[]),
    (0x007B, "KC_CUT", &[]),
    (0x007C, "KC_COPY", &[]),
    (0x007D, "KC_PSTE", &["KC_PASTE"]),
    (0x007E, "KC_FIND", &[]),
    (0x007F, "KC_KB_MUTE", &[]),
    (0x0080, "KC_KB_VOLUME_UP", &[]),
    (0x0081, "KC_KB_VOLUME_DOWN", &[]),
    (0x0085, "KC_PCMM", &["KC_KP_COMMA"]),
    (0x0087, "KC_INT1", &["KC_INTERNATIONAL_1"]),
    (0x0088, "KC_INT2", &["KC_INTERNATIONAL_2"]),
    (0x0089, "KC_INT3", &["KC_INTERNATIONAL_3"]),
    (0x008A, "KC_INT4", &["KC_INTERNATIONAL_4"]),
    (0x008B, "KC_INT5", &["KC_INTERNATIONAL_5"]),
    (0x008C, "KC_INT6", &["KC_INTERNATIONAL_6"]),
    (0x008D, "KC_INT7", &["KC_INTERNATIONAL_7"]),
    (0x008E, "KC_INT8", &["KC_INTERNATIONAL_8"]),
    (0x008F, "KC_INT9", &["KC_INTERNATIONAL_9"]),
    (0x0090, "KC_LNG1", &["KC_LANGUAGE_1"]),
    (0x0091, "KC_LNG2", &["KC_LANGUAGE_2"]),
    (0x0092, "KC_LNG3", &["KC_LANGUAGE_3"]),
    (0x0093, "KC_LNG4", &["KC_LANGUAGE_4"]),
    (0x0094, "KC_LNG5", &["KC_LANGUAGE_5"]),
    (0x0095, "KC_LNG6", &["KC_LANGUAGE_6"]),
    (0x0096, "KC_LNG7", &["KC_LANGUAGE_7"]),
    (0x0097, "KC_LNG8", &["KC_LANGUAGE_8"]),
    (0x0098, "KC_LNG9", &["KC_LANGUAGE_9"]),
    // System / consumer keys aliased into the basic range (the firmware
    // maps its Consumer/SystemControl actions onto these on the wire).
    (0x00A5, "KC_PWR", &["KC_SYSTEM_POWER"]),
    (0x00A6, "KC_SLEP", &["KC_SYSTEM_SLEEP"]),
    (0x00A7, "KC_WAKE", &["KC_SYSTEM_WAKE"]),
    (0x00A8, "KC_MUTE", &["KC_AUDIO_MUTE"]),
    (0x00A9, "KC_VOLU", &["KC_AUDIO_VOL_UP"]),
    (0x00AA, "KC_VOLD", &["KC_AUDIO_VOL_DOWN"]),
    (0x00AB, "KC_MNXT", &["KC_MEDIA_NEXT_TRACK"]),
    (0x00AC, "KC_MPRV", &["KC_MEDIA_PREV_TRACK"]),
    (0x00AD, "KC_MSTP", &["KC_MEDIA_STOP"]),
    (0x00AE, "KC_MPLY", &["KC_MEDIA_PLAY_PAUSE"]),
    (0x00AF, "KC_MSEL", &["KC_MEDIA_SELECT"]),
    (0x00B0, "KC_EJCT", &["KC_MEDIA_EJECT"]),
    (0x00B1, "KC_MAIL", &[]),
    (0x00B2, "KC_CALC", &["KC_CALCULATOR"]),
    (0x00B3, "KC_MYCM", &["KC_MY_COMPUTER"]),
    (0x00B4, "KC_WSCH", &["KC_WWW_SEARCH"]),
    (0x00B5, "KC_WHOM", &["KC_WWW_HOME"]),
    (0x00B6, "KC_WBAK", &["KC_WWW_BACK"]),
    (0x00B7, "KC_WFWD", &["KC_WWW_FORWARD"]),
    (0x00B8, "KC_WSTP", &["KC_WWW_STOP"]),
    (0x00B9, "KC_WREF", &["KC_WWW_REFRESH"]),
    (0x00BA, "KC_WFAV", &["KC_WWW_FAVORITES"]),
    (0x00BB, "KC_MFFD", &["KC_MEDIA_FAST_FORWARD"]),
    (0x00BC, "KC_MRWD", &["KC_MEDIA_REWIND"]),
    (0x00BD, "KC_BRIU", &["KC_BRIGHTNESS_UP"]),
    (0x00BE, "KC_BRID", &["KC_BRIGHTNESS_DOWN"]),
    (0x00BF, "KC_CPNL", &["KC_CONTROL_PANEL"]),
    (0x00C0, "KC_ASST", &["KC_ASSISTANT"]),
    (0x00C1, "KC_MCTL", &["KC_MISSION_CONTROL"]),
    (0x00C2, "KC_LPAD", &["KC_LAUNCHPAD"]),
    (0x00CD, "KC_MS_U", &["KC_MS_UP"]),
    (0x00CE, "KC_MS_D", &["KC_MS_DOWN"]),
    (0x00CF, "KC_MS_L", &["KC_MS_LEFT"]),
    (0x00D0, "KC_MS_R", &["KC_MS_RIGHT"]),
    (0x00D1, "KC_BTN1", &["KC_MS_BTN1"]),
    (0x00D2, "KC_BTN2", &["KC_MS_BTN2"]),
    (0x00D3, "KC_BTN3", &["KC_MS_BTN3"]),
    (0x00D4, "KC_BTN4", &["KC_MS_BTN4"]),
    (0x00D5, "KC_BTN5", &["KC_MS_BTN5"]),
    (0x00D6, "KC_BTN6", &["KC_MS_BTN6"]),
    (0x00D7, "KC_BTN7", &["KC_MS_BTN7"]),
    (0x00D8, "KC_BTN8", &["KC_MS_BTN8"]),
    (0x00D9, "KC_WH_U", &["KC_MS_WH_UP"]),
    (0x00DA, "KC_WH_D", &["KC_MS_WH_DOWN"]),
    (0x00DB, "KC_WH_L", &["KC_MS_WH_LEFT"]),
    (0x00DC, "KC_WH_R", &["KC_MS_WH_RIGHT"]),
    (0x00DD, "KC_ACL0", &["KC_MS_ACCEL0"]),
    (0x00DE, "KC_ACL1", &["KC_MS_ACCEL1"]),
    (0x00DF, "KC_ACL2", &["KC_MS_ACCEL2"]),
    (0x00E0, "KC_LCTL", &["KC_LEFT_CTRL"]),
    (0x00E1, "KC_LSFT", &["KC_LEFT_SHIFT"]),
    (0x00E2, "KC_LALT", &["KC_LEFT_ALT", "KC_LOPT"]),
    (0x00E3, "KC_LGUI", &["KC_LEFT_GUI", "KC_LCMD", "KC_LWIN"]),
    (0x00E4, "KC_RCTL", &["KC_RIGHT_CTRL"]),
    (0x00E5, "KC_RSFT", &["KC_RIGHT_SHIFT"]),
    (0x00E6, "KC_RALT", &["KC_RIGHT_ALT", "KC_ROPT", "KC_ALGR"]),
    (0x00E7, "KC_RGUI", &["KC_RIGHT_GUI", "KC_RCMD", "KC_RWIN"]),
];

/// Named non-basic keycodes the firmware understands (QMK "magic" range).
const EXTRA: &[(u16, &str, &[&str])] = &[
    (0x7C00, "QK_BOOT", &["QK_BOOTLOADER", "RESET"]),
    (0x7C01, "QK_RBT", &["QK_REBOOT"]),
    (0x7C03, "EE_CLR", &["QK_CLEAR_EEPROM"]),
    (0x7C16, "QK_GESC", &["QK_GRAVE_ESCAPE", "GRAVE_ESC"]),
    (0x7C18, "SC_LCPO", &["KC_LCPO"]),
    (0x7C19, "SC_RCPC", &["KC_RCPC"]),
    (0x7C1A, "SC_LSPO", &["KC_LSPO"]),
    (0x7C1B, "SC_RSPC", &["KC_RSPC"]),
    (0x7C1C, "SC_LAPO", &["KC_LAPO"]),
    (0x7C1D, "SC_RAPC", &["KC_RAPC"]),
    (0x7C1E, "SC_SENT", &["KC_SFTENT"]),
    (0x7C50, "CM_ON", &["QK_COMBO_ON"]),
    (0x7C51, "CM_OFF", &["QK_COMBO_OFF"]),
    (0x7C52, "CM_TOGG", &["QK_COMBO_TOGGLE"]),
    (0x7C73, "CW_TOGG", &["QK_CAPS_WORD_TOGGLE", "CAPS_WORD"]),
    (0x7C77, "TL_LOWR", &["QK_TRI_LAYER_LOWER"]),
    (0x7C78, "TL_UPPR", &["QK_TRI_LAYER_UPPER"]),
    (0x7C79, "QK_REP", &["QK_REPEAT_KEY", "REPEAT"]),
];

type NameTable = [(u16, &'static str, &'static [&'static str])];

fn table_name(table: &'static NameTable, code: u16) -> Option<&'static str> {
    table
        .iter()
        .find(|(value, _, _)| *value == code)
        .map(|(_, name, _)| *name)
}

fn table_code(table: &'static NameTable, name: &str) -> Option<u16> {
    table
        .iter()
        .find(|(_, canonical, aliases)| {
            canonical.eq_ignore_ascii_case(name)
                || aliases.iter().any(|alias| alias.eq_ignore_ascii_case(name))
        })
        .map(|(code, _, _)| *code)
}

/// Modifier wrapper names in packed-bit order: `(bit, left, right)`.
const MOD_WRAPPERS: &[(u8, &str, &str)] = &[
    (MOD_CTRL, "LCTL", "RCTL"),
    (MOD_SHIFT, "LSFT", "RSFT"),
    (MOD_ALT, "LALT", "RALT"),
    (MOD_GUI, "LGUI", "RGUI"),
];

/// Render packed modifier bits as a `MOD_LCTL|MOD_LSFT` style list.
fn format_mod_list(bits: u8) -> String {
    let right = bits & MOD_RIGHT != 0;
    let names: Vec<String> = MOD_WRAPPERS
        .iter()
        .filter(|(bit, _, _)| bits & bit != 0)
        .map(|(_, left, right_name)| format!("MOD_{}", if right { right_name } else { *left }))
        .collect();
    if names.is_empty() {
        "MOD_NONE".into()
    } else {
        names.join("|")
    }
}

/// Format any VIA keycode as a human-readable name; never fails
/// (unknown codes render as `0xXXXX`).
pub fn format_keycode(code: u16) -> String {
    if let Some(name) = table_name(BASIC, code) {
        return name.into();
    }
    if let Some(name) = table_name(EXTRA, code) {
        return name.into();
    }
    let basic = |kc: u16| -> String {
        table_name(BASIC, kc)
            .map(String::from)
            .unwrap_or_else(|| format!("0x{kc:04X}"))
    };
    match code {
        0x0100..=0x1FFF => {
            let bits = (code >> 8) as u8;
            if bits & !MOD_RIGHT == 0 {
                // Only the hand flag set: no nameable modifier.
                return format!("0x{code:04X}");
            }
            let inner = basic(code & 0xFF);
            match bits & !MOD_RIGHT {
                MOD_MEH if bits & MOD_RIGHT == 0 => format!("MEH({inner})"),
                MOD_HYPR if bits & MOD_RIGHT == 0 => format!("HYPR({inner})"),
                _ => {
                    let right = bits & MOD_RIGHT != 0;
                    let mut text = inner;
                    // Innermost-first so the rendered nesting reads
                    // LCTL(LSFT(kc)) for ctrl|shift.
                    for (bit, left, right_name) in MOD_WRAPPERS.iter().rev() {
                        if bits & bit != 0 {
                            let name = if right { right_name } else { left };
                            text = format!("{name}({text})");
                        }
                    }
                    text
                }
            }
        }
        0x2000..=0x3FFF => {
            let bits = ((code >> 8) & 0x1F) as u8;
            if bits & !MOD_RIGHT == 0 {
                return format!("0x{code:04X}");
            }
            let inner = basic(code & 0xFF);
            let single = MOD_WRAPPERS
                .iter()
                .find(|(bit, _, _)| bits & !MOD_RIGHT == *bit)
                .map(|(_, left, right)| if bits & MOD_RIGHT != 0 { *right } else { *left });
            match (bits & !MOD_RIGHT, single) {
                (MOD_MEH, _) if bits & MOD_RIGHT == 0 => format!("MEH_T({inner})"),
                (MOD_HYPR, _) if bits & MOD_RIGHT == 0 => format!("ALL_T({inner})"),
                (_, Some(name)) => format!("{name}_T({inner})"),
                _ => format!("MT({}, {inner})", format_mod_list(bits)),
            }
        }
        0x4000..=0x4FFF => format!("LT({}, {})", (code >> 8) & 0xF, basic(code & 0xFF)),
        0x5000..=0x51FF if code & 0x0F != 0 => {
            format!(
                "LM({}, {})",
                (code >> 5) & 0xF,
                format_mod_list((code & 0x1F) as u8)
            )
        }
        0x5200..=0x521F => format!("TO({})", code & 0xF),
        0x5220..=0x523F => format!("MO({})", code & 0xF),
        0x5240..=0x525F => format!("DF({})", code & 0xF),
        0x5260..=0x527F => format!("TG({})", code & 0xF),
        0x5280..=0x529F => format!("OSL({})", code & 0xF),
        0x52A0..=0x52BF if code & 0x0F != 0 => {
            format!("OSM({})", format_mod_list((code & 0x1F) as u8))
        }
        0x52C0..=0x52DF => format!("TT({})", code & 0xF),
        0x52E0..=0x52FF => format!("PDF({})", code & 0xF),
        0x5700..=0x57FF => format!("TD({})", code & 0xFF),
        0x7700..=0x771F => format!("MACRO({})", code & 0x1F),
        0x7E00..=0x7E1F => format!("USER({})", code & 0x1F),
        _ => format!("0x{code:04X}"),
    }
}

/// Parse packed modifier bits from a `MOD_LCTL|MOD_RSFT` style list. Bare
/// names without the `MOD_` prefix are accepted. Any right-hand modifier
/// sets the shared right-hand flag (the encoding cannot mix hands).
fn parse_mod_list(text: &str) -> Result<u8> {
    let mut bits = 0u8;
    for token in text.split('|') {
        let token = token.trim().to_ascii_uppercase();
        let name = token.strip_prefix("MOD_").unwrap_or(&token);
        let matched = MOD_WRAPPERS.iter().find_map(|(bit, left, right)| {
            if name == *left {
                Some(*bit)
            } else if name == *right {
                Some(bit | MOD_RIGHT)
            } else {
                None
            }
        });
        match (matched, name) {
            (Some(bit), _) => bits |= bit,
            (None, "MEH") => bits |= MOD_MEH,
            (None, "HYPR") => bits |= MOD_HYPR,
            (None, _) => bail!(
                "unknown modifier '{token}' (use MOD_LCTL, MOD_LSFT, MOD_LALT, MOD_LGUI, \
                 their R variants, MEH, or HYPR, joined with '|')"
            ),
        }
    }
    if bits == 0 {
        bail!("empty modifier list");
    }
    Ok(bits)
}

fn parse_layer(text: &str, what: &str, max: u16) -> Result<u16> {
    let value: u16 = text
        .trim()
        .parse()
        .map_err(|_| anyhow!("{what} must be an integer, got '{}'", text.trim()))?;
    if value > max {
        bail!("{what} must be at most {max}, got {value}");
    }
    Ok(value)
}

/// Parse a basic (< 0x0100) keycode argument used inside composites.
fn parse_basic(text: &str) -> Result<u16> {
    let code = parse_keycode(text)?;
    if code > 0x00FF {
        bail!(
            "'{}' ({}) cannot be nested here; only basic keycodes (below 0x0100) fit \
             inside this composite",
            text.trim(),
            format_keycode(code)
        );
    }
    Ok(code)
}

/// Parse a keycode from any accepted spelling: `0x`-prefixed hex, bare
/// decimal, a `KC_*`/magic name or alias (case-insensitive, `KC_` optional),
/// or a composite like `MO(2)`, `LT(1, KC_A)`, `LSFT(KC_1)`, `LCTL_T(KC_A)`,
/// `MT(MOD_LCTL|MOD_LSFT, KC_A)`, `OSM(MOD_LSFT)`, `LM(1, MOD_LALT)`.
pub fn parse_keycode(text: &str) -> Result<u16> {
    let text = text.trim();
    if text.is_empty() {
        bail!("empty keycode");
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16)
            .map_err(|_| anyhow!("'{text}' is not a valid hex keycode"));
    }
    if text.chars().all(|c| c.is_ascii_digit()) {
        return text
            .parse()
            .map_err(|_| anyhow!("'{text}' does not fit a 16-bit keycode"));
    }

    // Composite syntax NAME(args).
    if let Some((name, rest)) = text.split_once('(') {
        let args = rest
            .strip_suffix(')')
            .ok_or_else(|| anyhow!("'{text}' is missing its closing parenthesis"))?;
        return parse_composite(&name.trim().to_ascii_uppercase(), args, text);
    }

    let upper = text.to_ascii_uppercase();
    if let Some(code) = table_code(BASIC, &upper).or_else(|| table_code(EXTRA, &upper)) {
        return Ok(code);
    }
    let prefixed = format!("KC_{upper}");
    if let Some(code) = table_code(BASIC, &prefixed) {
        return Ok(code);
    }
    bail!(
        "unknown keycode '{text}'; try `keymap find {text}`, a composite like MO(2) or \
         LT(1, KC_A), or a raw hex value like 0x0004"
    )
}

fn parse_composite(name: &str, args: &str, original: &str) -> Result<u16> {
    // Single-integer layer verbs.
    let layer_verb = |base: u16| -> Result<u16> { Ok(base | parse_layer(args, "layer", 15)?) };
    match name {
        "TO" => return layer_verb(0x5200),
        "MO" => return layer_verb(0x5220),
        "DF" => return layer_verb(0x5240),
        "TG" => return layer_verb(0x5260),
        "OSL" => return layer_verb(0x5280),
        "TT" => return layer_verb(0x52C0),
        "PDF" => return layer_verb(0x52E0),
        "TD" => return Ok(0x5700 | parse_layer(args, "tap-dance index", 255)?),
        "MACRO" | "M" => return Ok(0x7700 | parse_layer(args, "macro index", 31)?),
        "USER" | "CUSTOM" => return Ok(0x7E00 | parse_layer(args, "user-key index", 31)?),
        "OSM" => return Ok(0x52A0 | u16::from(parse_mod_list(args)?)),
        _ => {}
    }

    // Two-argument composites: LT(layer, kc), LM(layer, mods), MT(mods, kc).
    if let "LT" | "LM" | "MT" = name {
        let (first, second) = args
            .split_once(',')
            .ok_or_else(|| anyhow!("'{original}' needs two comma-separated arguments"))?;
        return match name {
            "LT" => Ok(0x4000 | (parse_layer(first, "layer", 15)? << 8) | parse_basic(second)?),
            "LM" => Ok(0x5000
                | (parse_layer(first, "layer", 15)? << 5)
                | u16::from(parse_mod_list(second)?)),
            "MT" => Ok(0x2000 | (u16::from(parse_mod_list(first)?) << 8) | parse_basic(second)?),
            _ => unreachable!(),
        };
    }

    // Mod-tap shorthands: LCTL_T(kc), MEH_T(kc), ALL_T(kc), …
    if let Some(mods) = name.strip_suffix("_T").and_then(|prefix| match prefix {
        "MEH" => Some(MOD_MEH),
        "ALL" | "HYPR" => Some(MOD_HYPR),
        other => MOD_WRAPPERS.iter().find_map(|(bit, left, right)| {
            if other == *left {
                Some(*bit)
            } else if other == *right {
                Some(bit | MOD_RIGHT)
            } else {
                None
            }
        }),
    }) {
        return Ok(0x2000 | (u16::from(mods) << 8) | parse_basic(args)?);
    }

    // Modifier wrappers: LSFT(kc), C(kc), MEH(kc), … possibly nested.
    let wrapper_bits = match name {
        "MEH" => Some(MOD_MEH),
        "HYPR" => Some(MOD_HYPR),
        "C" => Some(MOD_CTRL),
        "S" => Some(MOD_SHIFT),
        "A" => Some(MOD_ALT),
        "G" => Some(MOD_GUI),
        other => MOD_WRAPPERS.iter().find_map(|(bit, left, right)| {
            if other == *left {
                Some(*bit)
            } else if other == *right {
                Some(bit | MOD_RIGHT)
            } else {
                None
            }
        }),
    };
    if let Some(bits) = wrapper_bits {
        let inner = parse_keycode(args)?;
        return match inner {
            0x0000..=0x00FF => Ok((u16::from(bits) << 8) | inner),
            // Nested wrappers: merge modifier bits (hands must agree).
            0x0100..=0x1FFF => {
                let inner_bits = (inner >> 8) as u8;
                if (bits & MOD_RIGHT) != (inner_bits & MOD_RIGHT)
                    && inner_bits & !MOD_RIGHT != 0
                    && bits & !MOD_RIGHT != 0
                {
                    bail!(
                        "'{original}' mixes left- and right-hand modifiers; the encoding \
                         has a single hand flag for all of them"
                    );
                }
                Ok((u16::from(bits | inner_bits) << 8) | (inner & 0xFF))
            }
            _ => bail!(
                "'{original}': modifier wrappers only apply to basic keycodes, \
                 not {}",
                format_keycode(inner)
            ),
        };
    }

    bail!(
        "unknown composite '{name}' in '{original}' (supported: TO, MO, DF, TG, OSL, OSM, \
         TT, PDF, LT, LM, MT, TD, MACRO, USER, modifier wrappers like LSFT()/MEH()/HYPR(), \
         and mod-taps like LCTL_T())"
    )
}

/// Search the name tables for a fragment (case-insensitive substring over
/// canonical names and aliases). Returns `(code, canonical, aliases)`.
pub fn search(fragment: &str) -> Vec<(u16, &'static str, &'static [&'static str])> {
    let needle = fragment.to_ascii_uppercase();
    BASIC
        .iter()
        .chain(EXTRA.iter())
        .filter(|(_, canonical, aliases)| {
            canonical.to_ascii_uppercase().contains(&needle)
                || aliases
                    .iter()
                    .any(|alias| alias.to_ascii_uppercase().contains(&needle))
        })
        .map(|(code, canonical, aliases)| (*code, *canonical, *aliases))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_basic_and_unknown_codes() {
        assert_eq!(format_keycode(0x0000), "KC_NO");
        assert_eq!(format_keycode(0x0001), "KC_TRNS");
        assert_eq!(format_keycode(0x0004), "KC_A");
        assert_eq!(format_keycode(0x00E5), "KC_RSFT");
        assert_eq!(format_keycode(0x00AE), "KC_MPLY");
        // Unnamed codes never fail; they render as hex.
        assert_eq!(format_keycode(0x0083), "0x0083");
        assert_eq!(format_keycode(0x6FFF), "0x6FFF");
    }

    #[test]
    fn formats_composites_matching_firmware_encoding() {
        // Values cross-checked against keycode_convert.rs tests.
        assert_eq!(format_keycode(0x5223), "MO(3)");
        assert_eq!(format_keycode(0x5283), "OSL(3)");
        assert_eq!(format_keycode(0x5243), "DF(3)");
        assert_eq!(format_keycode(0x52E3), "PDF(3)");
        assert_eq!(format_keycode(0x0104), "LCTL(KC_A)");
        assert_eq!(format_keycode(0x1104), "RCTL(KC_A)");
        assert_eq!(format_keycode(0x0704), "MEH(KC_A)");
        assert_eq!(format_keycode(0x0F04), "HYPR(KC_A)");
        assert_eq!(format_keycode(0x0304), "LCTL(LSFT(KC_A))");
        assert_eq!(format_keycode(0x4304), "LT(3, KC_A)");
        assert_eq!(format_keycode(0x2204), "LSFT_T(KC_A)");
        assert_eq!(format_keycode(0x3228), "RSFT_T(KC_ENT)");
        assert_eq!(format_keycode(0x2704), "MEH_T(KC_A)");
        assert_eq!(format_keycode(0x2F04), "ALL_T(KC_A)");
        assert_eq!(format_keycode(0x2604), "MT(MOD_LSFT|MOD_LALT, KC_A)");
        assert_eq!(format_keycode(0x5022), "LM(1, MOD_LSFT)");
        assert_eq!(format_keycode(0x52A2), "OSM(MOD_LSFT)");
        assert_eq!(format_keycode(0x52B1), "OSM(MOD_RCTL)");
        assert_eq!(format_keycode(0x5705), "TD(5)");
        assert_eq!(format_keycode(0x7705), "MACRO(5)");
        assert_eq!(format_keycode(0x7E10), "USER(16)");
        assert_eq!(format_keycode(0x7C00), "QK_BOOT");
        assert_eq!(format_keycode(0x7C73), "CW_TOGG");
    }

    #[test]
    fn parses_names_hex_and_decimal() {
        assert_eq!(parse_keycode("KC_A").unwrap(), 0x0004);
        assert_eq!(parse_keycode("kc_a").unwrap(), 0x0004);
        assert_eq!(parse_keycode("A").unwrap(), 0x0004);
        assert_eq!(parse_keycode("KC_ENTER").unwrap(), 0x0028);
        assert_eq!(parse_keycode("ENT").unwrap(), 0x0028);
        assert_eq!(parse_keycode("0x0004").unwrap(), 0x0004);
        assert_eq!(parse_keycode("0X1104").unwrap(), 0x1104);
        assert_eq!(parse_keycode("4").unwrap(), 4);
        assert_eq!(parse_keycode("QK_BOOT").unwrap(), 0x7C00);
        assert_eq!(parse_keycode("_______").unwrap(), 0x0001);
        assert!(parse_keycode("KC_NOPE").is_err());
        assert!(parse_keycode("").is_err());
    }

    #[test]
    fn parses_composites() {
        assert_eq!(parse_keycode("MO(3)").unwrap(), 0x5223);
        assert_eq!(parse_keycode("mo(3)").unwrap(), 0x5223);
        assert_eq!(parse_keycode("TG(2)").unwrap(), 0x5262);
        assert_eq!(parse_keycode("TO(1)").unwrap(), 0x5201);
        assert_eq!(parse_keycode("OSL(3)").unwrap(), 0x5283);
        assert_eq!(parse_keycode("PDF(15)").unwrap(), 0x52EF);
        assert_eq!(parse_keycode("LT(3, KC_A)").unwrap(), 0x4304);
        assert_eq!(parse_keycode("LT(3,a)").unwrap(), 0x4304);
        assert_eq!(parse_keycode("LSFT(KC_1)").unwrap(), 0x021E);
        assert_eq!(parse_keycode("S(KC_1)").unwrap(), 0x021E);
        assert_eq!(parse_keycode("RCTL(KC_A)").unwrap(), 0x1104);
        assert_eq!(parse_keycode("MEH(KC_A)").unwrap(), 0x0704);
        assert_eq!(parse_keycode("HYPR(KC_A)").unwrap(), 0x0F04);
        assert_eq!(parse_keycode("LCTL(LSFT(KC_A))").unwrap(), 0x0304);
        assert_eq!(parse_keycode("LSFT_T(KC_A)").unwrap(), 0x2204);
        assert_eq!(parse_keycode("RSFT_T(KC_ENT)").unwrap(), 0x3228);
        assert_eq!(parse_keycode("MEH_T(KC_A)").unwrap(), 0x2704);
        assert_eq!(parse_keycode("ALL_T(KC_A)").unwrap(), 0x2F04);
        assert_eq!(
            parse_keycode("MT(MOD_LSFT|MOD_LALT, KC_A)").unwrap(),
            0x2604
        );
        assert_eq!(parse_keycode("LM(1, MOD_LSFT)").unwrap(), 0x5022);
        assert_eq!(parse_keycode("OSM(MOD_RCTL)").unwrap(), 0x52B1);
        assert_eq!(parse_keycode("TD(5)").unwrap(), 0x5705);
        assert_eq!(parse_keycode("MACRO(5)").unwrap(), 0x7705);
        assert_eq!(parse_keycode("M(5)").unwrap(), 0x7705);
        assert_eq!(parse_keycode("USER(16)").unwrap(), 0x7E10);

        assert!(parse_keycode("MO(16)").is_err(), "layer beyond 4 bits");
        assert!(
            parse_keycode("LT(3, MO(2))").is_err(),
            "non-basic tap keycode"
        );
        assert!(
            parse_keycode("LSFT(MO(2))").is_err(),
            "wrapping a non-basic code"
        );
        assert!(parse_keycode("LCTL(RSFT(KC_A))").is_err(), "mixed hands");
        assert!(parse_keycode("MO(2").is_err(), "unbalanced parenthesis");
        assert!(parse_keycode("WAT(2)").is_err(), "unknown composite");
        assert!(parse_keycode("OSM(MOD_NOPE)").is_err(), "unknown modifier");
    }

    #[test]
    fn format_parse_round_trip() {
        // Every named/structured code must re-parse to itself.
        for code in [
            0x0000u16, 0x0001, 0x0004, 0x00E7, 0x00AE, 0x0104, 0x1104, 0x0704, 0x0F04, 0x0304,
            0x2204, 0x3228, 0x2704, 0x2F04, 0x2604, 0x4304, 0x5022, 0x5201, 0x5223, 0x5243, 0x5262,
            0x5283, 0x52A2, 0x52B1, 0x52E3, 0x5705, 0x7705, 0x7C00, 0x7C73, 0x7E10,
            // Unknown codes round-trip through their hex rendering.
            0x0083, 0x6FFF,
        ] {
            let name = format_keycode(code);
            assert_eq!(parse_keycode(&name).unwrap(), code, "round-trip of {name}");
        }
    }

    #[test]
    fn searches_names_and_aliases() {
        let hits = search("play");
        assert!(hits
            .iter()
            .any(|(code, name, _)| *code == 0x00AE && *name == "KC_MPLY"));
        let hits = search("mply");
        assert!(hits.iter().any(|(code, _, _)| *code == 0x00AE));
        let hits = search("boot");
        assert!(hits.iter().any(|(code, _, _)| *code == 0x7C00));
        assert!(search("zzzznothing").is_empty());
    }
}
