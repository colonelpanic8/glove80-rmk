// VIA/Vial 16-bit keycode names: formatting, parsing, and search.
//
// The wire format is the VIA keycode encoding as implemented by the
// firmware's Vial conversion (`rmk/src/host/via/keycode_convert.rs`), so
// this table names exactly the ranges that firmware round-trips:
//
// - `0x0000..=0x00FF` basic HID keycodes (QMK `KC_*` names; includes the
//   0xA5+ system/consumer aliases like `KC_MPLY` and the mouse keys)
// - `0x0100..=0x1FFF` modified keys - `LCTL(kc)`, `MEH(kc)`, `HYPR(kc)`, ...
// - `0x2000..=0x3FFF` mod-taps - `LSFT_T(kc)`, `MEH_T(kc)`, `ALL_T(kc)`,
//   `MT(MOD_...|..., kc)`
// - `0x4000..=0x4FFF` layer-taps - `LT(layer, kc)` (layer 0-15)
// - `0x5000..=0x51FF` layer-with-modifier - `LM(layer, MOD_...)`
// - `0x5200..` layer verbs - `TO(n)`, `MO(n)`, `DF(n)`, `TG(n)`, `OSL(n)`,
//   `OSM(MOD_...)`, `TT(n)` (named, but the firmware stores it as `KC_NO`),
//   `PDF(n)`
// - `0x5700..=0x57FF` tap dance / morse - `TD(n)`
// - `0x7700..=0x771F` macros - `MACRO(n)`
// - `0x7C00..` QMK magic keys the firmware supports (`QK_BOOT`, `CW_TOGG`,
//   space cadet, ...)
// - `0x7E00..=0x7E1F` user/custom keys - `USER(n)`
//
// Anything else formats as bare hex (`0x1234`) and can be entered the same
// way; formatting never fails.
//
// This mirrors tools/glove80-control/src/keycodes.rs; keep the two in sync.

// Packed modifier bits, matching the firmware/QMK 5-bit encoding.
const MOD_CTRL = 0x01;
const MOD_SHIFT = 0x02;
const MOD_ALT = 0x04;
const MOD_GUI = 0x08;
// Set = the right-hand variants of every modifier bit below it.
const MOD_RIGHT = 0x10;
const MOD_MEH = MOD_CTRL | MOD_SHIFT | MOD_ALT;
const MOD_HYPR = MOD_MEH | MOD_GUI;

// An error raised while parsing a keycode name/expression.
export class KeycodeError extends Error {}

interface NameEntry {
  code: number;
  name: string;
  aliases: readonly string[];
}

// `(code, canonical QMK name, aliases)` for the basic (< 0x0100) range.
// Canonical names are the short QMK spellings VIA/Vial display.
const BASIC: readonly NameEntry[] = [
  { code: 0x0000, name: "KC_NO", aliases: ["XXXXXXX", "KC_NONE"] },
  { code: 0x0001, name: "KC_TRNS", aliases: ["KC_TRANSPARENT", "_______"] },
  { code: 0x0004, name: "KC_A", aliases: [] },
  { code: 0x0005, name: "KC_B", aliases: [] },
  { code: 0x0006, name: "KC_C", aliases: [] },
  { code: 0x0007, name: "KC_D", aliases: [] },
  { code: 0x0008, name: "KC_E", aliases: [] },
  { code: 0x0009, name: "KC_F", aliases: [] },
  { code: 0x000a, name: "KC_G", aliases: [] },
  { code: 0x000b, name: "KC_H", aliases: [] },
  { code: 0x000c, name: "KC_I", aliases: [] },
  { code: 0x000d, name: "KC_J", aliases: [] },
  { code: 0x000e, name: "KC_K", aliases: [] },
  { code: 0x000f, name: "KC_L", aliases: [] },
  { code: 0x0010, name: "KC_M", aliases: [] },
  { code: 0x0011, name: "KC_N", aliases: [] },
  { code: 0x0012, name: "KC_O", aliases: [] },
  { code: 0x0013, name: "KC_P", aliases: [] },
  { code: 0x0014, name: "KC_Q", aliases: [] },
  { code: 0x0015, name: "KC_R", aliases: [] },
  { code: 0x0016, name: "KC_S", aliases: [] },
  { code: 0x0017, name: "KC_T", aliases: [] },
  { code: 0x0018, name: "KC_U", aliases: [] },
  { code: 0x0019, name: "KC_V", aliases: [] },
  { code: 0x001a, name: "KC_W", aliases: [] },
  { code: 0x001b, name: "KC_X", aliases: [] },
  { code: 0x001c, name: "KC_Y", aliases: [] },
  { code: 0x001d, name: "KC_Z", aliases: [] },
  { code: 0x001e, name: "KC_1", aliases: [] },
  { code: 0x001f, name: "KC_2", aliases: [] },
  { code: 0x0020, name: "KC_3", aliases: [] },
  { code: 0x0021, name: "KC_4", aliases: [] },
  { code: 0x0022, name: "KC_5", aliases: [] },
  { code: 0x0023, name: "KC_6", aliases: [] },
  { code: 0x0024, name: "KC_7", aliases: [] },
  { code: 0x0025, name: "KC_8", aliases: [] },
  { code: 0x0026, name: "KC_9", aliases: [] },
  { code: 0x0027, name: "KC_0", aliases: [] },
  { code: 0x0028, name: "KC_ENT", aliases: ["KC_ENTER"] },
  { code: 0x0029, name: "KC_ESC", aliases: ["KC_ESCAPE"] },
  { code: 0x002a, name: "KC_BSPC", aliases: ["KC_BACKSPACE"] },
  { code: 0x002b, name: "KC_TAB", aliases: [] },
  { code: 0x002c, name: "KC_SPC", aliases: ["KC_SPACE"] },
  { code: 0x002d, name: "KC_MINS", aliases: ["KC_MINUS"] },
  { code: 0x002e, name: "KC_EQL", aliases: ["KC_EQUAL"] },
  { code: 0x002f, name: "KC_LBRC", aliases: ["KC_LEFT_BRACKET"] },
  { code: 0x0030, name: "KC_RBRC", aliases: ["KC_RIGHT_BRACKET"] },
  { code: 0x0031, name: "KC_BSLS", aliases: ["KC_BACKSLASH"] },
  { code: 0x0032, name: "KC_NUHS", aliases: ["KC_NONUS_HASH"] },
  { code: 0x0033, name: "KC_SCLN", aliases: ["KC_SEMICOLON"] },
  { code: 0x0034, name: "KC_QUOT", aliases: ["KC_QUOTE"] },
  { code: 0x0035, name: "KC_GRV", aliases: ["KC_GRAVE"] },
  { code: 0x0036, name: "KC_COMM", aliases: ["KC_COMMA"] },
  { code: 0x0037, name: "KC_DOT", aliases: [] },
  { code: 0x0038, name: "KC_SLSH", aliases: ["KC_SLASH"] },
  { code: 0x0039, name: "KC_CAPS", aliases: ["KC_CAPS_LOCK"] },
  { code: 0x003a, name: "KC_F1", aliases: [] },
  { code: 0x003b, name: "KC_F2", aliases: [] },
  { code: 0x003c, name: "KC_F3", aliases: [] },
  { code: 0x003d, name: "KC_F4", aliases: [] },
  { code: 0x003e, name: "KC_F5", aliases: [] },
  { code: 0x003f, name: "KC_F6", aliases: [] },
  { code: 0x0040, name: "KC_F7", aliases: [] },
  { code: 0x0041, name: "KC_F8", aliases: [] },
  { code: 0x0042, name: "KC_F9", aliases: [] },
  { code: 0x0043, name: "KC_F10", aliases: [] },
  { code: 0x0044, name: "KC_F11", aliases: [] },
  { code: 0x0045, name: "KC_F12", aliases: [] },
  { code: 0x0046, name: "KC_PSCR", aliases: ["KC_PRINT_SCREEN"] },
  { code: 0x0047, name: "KC_SCRL", aliases: ["KC_SCROLL_LOCK"] },
  { code: 0x0048, name: "KC_PAUS", aliases: ["KC_PAUSE"] },
  { code: 0x0049, name: "KC_INS", aliases: ["KC_INSERT"] },
  { code: 0x004a, name: "KC_HOME", aliases: [] },
  { code: 0x004b, name: "KC_PGUP", aliases: ["KC_PAGE_UP"] },
  { code: 0x004c, name: "KC_DEL", aliases: ["KC_DELETE"] },
  { code: 0x004d, name: "KC_END", aliases: [] },
  { code: 0x004e, name: "KC_PGDN", aliases: ["KC_PAGE_DOWN"] },
  { code: 0x004f, name: "KC_RGHT", aliases: ["KC_RIGHT"] },
  { code: 0x0050, name: "KC_LEFT", aliases: [] },
  { code: 0x0051, name: "KC_DOWN", aliases: [] },
  { code: 0x0052, name: "KC_UP", aliases: [] },
  { code: 0x0053, name: "KC_NUM", aliases: ["KC_NUM_LOCK"] },
  { code: 0x0054, name: "KC_PSLS", aliases: ["KC_KP_SLASH"] },
  { code: 0x0055, name: "KC_PAST", aliases: ["KC_KP_ASTERISK"] },
  { code: 0x0056, name: "KC_PMNS", aliases: ["KC_KP_MINUS"] },
  { code: 0x0057, name: "KC_PPLS", aliases: ["KC_KP_PLUS"] },
  { code: 0x0058, name: "KC_PENT", aliases: ["KC_KP_ENTER"] },
  { code: 0x0059, name: "KC_P1", aliases: ["KC_KP_1"] },
  { code: 0x005a, name: "KC_P2", aliases: ["KC_KP_2"] },
  { code: 0x005b, name: "KC_P3", aliases: ["KC_KP_3"] },
  { code: 0x005c, name: "KC_P4", aliases: ["KC_KP_4"] },
  { code: 0x005d, name: "KC_P5", aliases: ["KC_KP_5"] },
  { code: 0x005e, name: "KC_P6", aliases: ["KC_KP_6"] },
  { code: 0x005f, name: "KC_P7", aliases: ["KC_KP_7"] },
  { code: 0x0060, name: "KC_P8", aliases: ["KC_KP_8"] },
  { code: 0x0061, name: "KC_P9", aliases: ["KC_KP_9"] },
  { code: 0x0062, name: "KC_P0", aliases: ["KC_KP_0"] },
  { code: 0x0063, name: "KC_PDOT", aliases: ["KC_KP_DOT"] },
  { code: 0x0064, name: "KC_NUBS", aliases: ["KC_NONUS_BACKSLASH"] },
  { code: 0x0065, name: "KC_APP", aliases: ["KC_APPLICATION"] },
  { code: 0x0066, name: "KC_KB_POWER", aliases: [] },
  { code: 0x0067, name: "KC_PEQL", aliases: ["KC_KP_EQUAL"] },
  { code: 0x0068, name: "KC_F13", aliases: [] },
  { code: 0x0069, name: "KC_F14", aliases: [] },
  { code: 0x006a, name: "KC_F15", aliases: [] },
  { code: 0x006b, name: "KC_F16", aliases: [] },
  { code: 0x006c, name: "KC_F17", aliases: [] },
  { code: 0x006d, name: "KC_F18", aliases: [] },
  { code: 0x006e, name: "KC_F19", aliases: [] },
  { code: 0x006f, name: "KC_F20", aliases: [] },
  { code: 0x0070, name: "KC_F21", aliases: [] },
  { code: 0x0071, name: "KC_F22", aliases: [] },
  { code: 0x0072, name: "KC_F23", aliases: [] },
  { code: 0x0073, name: "KC_F24", aliases: [] },
  { code: 0x0074, name: "KC_EXEC", aliases: ["KC_EXECUTE"] },
  { code: 0x0075, name: "KC_HELP", aliases: [] },
  { code: 0x0076, name: "KC_MENU", aliases: [] },
  { code: 0x0077, name: "KC_SLCT", aliases: ["KC_SELECT"] },
  { code: 0x0078, name: "KC_STOP", aliases: [] },
  { code: 0x0079, name: "KC_AGIN", aliases: ["KC_AGAIN"] },
  { code: 0x007a, name: "KC_UNDO", aliases: [] },
  { code: 0x007b, name: "KC_CUT", aliases: [] },
  { code: 0x007c, name: "KC_COPY", aliases: [] },
  { code: 0x007d, name: "KC_PSTE", aliases: ["KC_PASTE"] },
  { code: 0x007e, name: "KC_FIND", aliases: [] },
  { code: 0x007f, name: "KC_KB_MUTE", aliases: [] },
  { code: 0x0080, name: "KC_KB_VOLUME_UP", aliases: [] },
  { code: 0x0081, name: "KC_KB_VOLUME_DOWN", aliases: [] },
  { code: 0x0085, name: "KC_PCMM", aliases: ["KC_KP_COMMA"] },
  { code: 0x0087, name: "KC_INT1", aliases: ["KC_INTERNATIONAL_1"] },
  { code: 0x0088, name: "KC_INT2", aliases: ["KC_INTERNATIONAL_2"] },
  { code: 0x0089, name: "KC_INT3", aliases: ["KC_INTERNATIONAL_3"] },
  { code: 0x008a, name: "KC_INT4", aliases: ["KC_INTERNATIONAL_4"] },
  { code: 0x008b, name: "KC_INT5", aliases: ["KC_INTERNATIONAL_5"] },
  { code: 0x008c, name: "KC_INT6", aliases: ["KC_INTERNATIONAL_6"] },
  { code: 0x008d, name: "KC_INT7", aliases: ["KC_INTERNATIONAL_7"] },
  { code: 0x008e, name: "KC_INT8", aliases: ["KC_INTERNATIONAL_8"] },
  { code: 0x008f, name: "KC_INT9", aliases: ["KC_INTERNATIONAL_9"] },
  { code: 0x0090, name: "KC_LNG1", aliases: ["KC_LANGUAGE_1"] },
  { code: 0x0091, name: "KC_LNG2", aliases: ["KC_LANGUAGE_2"] },
  { code: 0x0092, name: "KC_LNG3", aliases: ["KC_LANGUAGE_3"] },
  { code: 0x0093, name: "KC_LNG4", aliases: ["KC_LANGUAGE_4"] },
  { code: 0x0094, name: "KC_LNG5", aliases: ["KC_LANGUAGE_5"] },
  { code: 0x0095, name: "KC_LNG6", aliases: ["KC_LANGUAGE_6"] },
  { code: 0x0096, name: "KC_LNG7", aliases: ["KC_LANGUAGE_7"] },
  { code: 0x0097, name: "KC_LNG8", aliases: ["KC_LANGUAGE_8"] },
  { code: 0x0098, name: "KC_LNG9", aliases: ["KC_LANGUAGE_9"] },
  // System / consumer keys aliased into the basic range (the firmware
  // maps its Consumer/SystemControl actions onto these on the wire).
  { code: 0x00a5, name: "KC_PWR", aliases: ["KC_SYSTEM_POWER"] },
  { code: 0x00a6, name: "KC_SLEP", aliases: ["KC_SYSTEM_SLEEP"] },
  { code: 0x00a7, name: "KC_WAKE", aliases: ["KC_SYSTEM_WAKE"] },
  { code: 0x00a8, name: "KC_MUTE", aliases: ["KC_AUDIO_MUTE"] },
  { code: 0x00a9, name: "KC_VOLU", aliases: ["KC_AUDIO_VOL_UP"] },
  { code: 0x00aa, name: "KC_VOLD", aliases: ["KC_AUDIO_VOL_DOWN"] },
  { code: 0x00ab, name: "KC_MNXT", aliases: ["KC_MEDIA_NEXT_TRACK"] },
  { code: 0x00ac, name: "KC_MPRV", aliases: ["KC_MEDIA_PREV_TRACK"] },
  { code: 0x00ad, name: "KC_MSTP", aliases: ["KC_MEDIA_STOP"] },
  { code: 0x00ae, name: "KC_MPLY", aliases: ["KC_MEDIA_PLAY_PAUSE"] },
  { code: 0x00af, name: "KC_MSEL", aliases: ["KC_MEDIA_SELECT"] },
  { code: 0x00b0, name: "KC_EJCT", aliases: ["KC_MEDIA_EJECT"] },
  { code: 0x00b1, name: "KC_MAIL", aliases: [] },
  { code: 0x00b2, name: "KC_CALC", aliases: ["KC_CALCULATOR"] },
  { code: 0x00b3, name: "KC_MYCM", aliases: ["KC_MY_COMPUTER"] },
  { code: 0x00b4, name: "KC_WSCH", aliases: ["KC_WWW_SEARCH"] },
  { code: 0x00b5, name: "KC_WHOM", aliases: ["KC_WWW_HOME"] },
  { code: 0x00b6, name: "KC_WBAK", aliases: ["KC_WWW_BACK"] },
  { code: 0x00b7, name: "KC_WFWD", aliases: ["KC_WWW_FORWARD"] },
  { code: 0x00b8, name: "KC_WSTP", aliases: ["KC_WWW_STOP"] },
  { code: 0x00b9, name: "KC_WREF", aliases: ["KC_WWW_REFRESH"] },
  { code: 0x00ba, name: "KC_WFAV", aliases: ["KC_WWW_FAVORITES"] },
  { code: 0x00bb, name: "KC_MFFD", aliases: ["KC_MEDIA_FAST_FORWARD"] },
  { code: 0x00bc, name: "KC_MRWD", aliases: ["KC_MEDIA_REWIND"] },
  { code: 0x00bd, name: "KC_BRIU", aliases: ["KC_BRIGHTNESS_UP"] },
  { code: 0x00be, name: "KC_BRID", aliases: ["KC_BRIGHTNESS_DOWN"] },
  { code: 0x00bf, name: "KC_CPNL", aliases: ["KC_CONTROL_PANEL"] },
  { code: 0x00c0, name: "KC_ASST", aliases: ["KC_ASSISTANT"] },
  { code: 0x00c1, name: "KC_MCTL", aliases: ["KC_MISSION_CONTROL"] },
  { code: 0x00c2, name: "KC_LPAD", aliases: ["KC_LAUNCHPAD"] },
  { code: 0x00cd, name: "KC_MS_U", aliases: ["KC_MS_UP"] },
  { code: 0x00ce, name: "KC_MS_D", aliases: ["KC_MS_DOWN"] },
  { code: 0x00cf, name: "KC_MS_L", aliases: ["KC_MS_LEFT"] },
  { code: 0x00d0, name: "KC_MS_R", aliases: ["KC_MS_RIGHT"] },
  { code: 0x00d1, name: "KC_BTN1", aliases: ["KC_MS_BTN1"] },
  { code: 0x00d2, name: "KC_BTN2", aliases: ["KC_MS_BTN2"] },
  { code: 0x00d3, name: "KC_BTN3", aliases: ["KC_MS_BTN3"] },
  { code: 0x00d4, name: "KC_BTN4", aliases: ["KC_MS_BTN4"] },
  { code: 0x00d5, name: "KC_BTN5", aliases: ["KC_MS_BTN5"] },
  { code: 0x00d6, name: "KC_BTN6", aliases: ["KC_MS_BTN6"] },
  { code: 0x00d7, name: "KC_BTN7", aliases: ["KC_MS_BTN7"] },
  { code: 0x00d8, name: "KC_BTN8", aliases: ["KC_MS_BTN8"] },
  { code: 0x00d9, name: "KC_WH_U", aliases: ["KC_MS_WH_UP"] },
  { code: 0x00da, name: "KC_WH_D", aliases: ["KC_MS_WH_DOWN"] },
  { code: 0x00db, name: "KC_WH_L", aliases: ["KC_MS_WH_LEFT"] },
  { code: 0x00dc, name: "KC_WH_R", aliases: ["KC_MS_WH_RIGHT"] },
  { code: 0x00dd, name: "KC_ACL0", aliases: ["KC_MS_ACCEL0"] },
  { code: 0x00de, name: "KC_ACL1", aliases: ["KC_MS_ACCEL1"] },
  { code: 0x00df, name: "KC_ACL2", aliases: ["KC_MS_ACCEL2"] },
  { code: 0x00e0, name: "KC_LCTL", aliases: ["KC_LEFT_CTRL"] },
  { code: 0x00e1, name: "KC_LSFT", aliases: ["KC_LEFT_SHIFT"] },
  { code: 0x00e2, name: "KC_LALT", aliases: ["KC_LEFT_ALT", "KC_LOPT"] },
  { code: 0x00e3, name: "KC_LGUI", aliases: ["KC_LEFT_GUI", "KC_LCMD", "KC_LWIN"] },
  { code: 0x00e4, name: "KC_RCTL", aliases: ["KC_RIGHT_CTRL"] },
  { code: 0x00e5, name: "KC_RSFT", aliases: ["KC_RIGHT_SHIFT"] },
  { code: 0x00e6, name: "KC_RALT", aliases: ["KC_RIGHT_ALT", "KC_ROPT", "KC_ALGR"] },
  { code: 0x00e7, name: "KC_RGUI", aliases: ["KC_RIGHT_GUI", "KC_RCMD", "KC_RWIN"] },
];

// Named non-basic keycodes the firmware understands (QMK "magic" range).
const EXTRA: readonly NameEntry[] = [
  { code: 0x7c00, name: "QK_BOOT", aliases: ["QK_BOOTLOADER", "RESET"] },
  { code: 0x7c01, name: "QK_RBT", aliases: ["QK_REBOOT"] },
  { code: 0x7c03, name: "EE_CLR", aliases: ["QK_CLEAR_EEPROM"] },
  { code: 0x7c16, name: "QK_GESC", aliases: ["QK_GRAVE_ESCAPE", "GRAVE_ESC"] },
  { code: 0x7c18, name: "SC_LCPO", aliases: ["KC_LCPO"] },
  { code: 0x7c19, name: "SC_RCPC", aliases: ["KC_RCPC"] },
  { code: 0x7c1a, name: "SC_LSPO", aliases: ["KC_LSPO"] },
  { code: 0x7c1b, name: "SC_RSPC", aliases: ["KC_RSPC"] },
  { code: 0x7c1c, name: "SC_LAPO", aliases: ["KC_LAPO"] },
  { code: 0x7c1d, name: "SC_RAPC", aliases: ["KC_RAPC"] },
  { code: 0x7c1e, name: "SC_SENT", aliases: ["KC_SFTENT"] },
  { code: 0x7c50, name: "CM_ON", aliases: ["QK_COMBO_ON"] },
  { code: 0x7c51, name: "CM_OFF", aliases: ["QK_COMBO_OFF"] },
  { code: 0x7c52, name: "CM_TOGG", aliases: ["QK_COMBO_TOGGLE"] },
  { code: 0x7c73, name: "CW_TOGG", aliases: ["QK_CAPS_WORD_TOGGLE", "CAPS_WORD"] },
  { code: 0x7c77, name: "TL_LOWR", aliases: ["QK_TRI_LAYER_LOWER"] },
  { code: 0x7c78, name: "TL_UPPR", aliases: ["QK_TRI_LAYER_UPPER"] },
  { code: 0x7c79, name: "QK_REP", aliases: ["QK_REPEAT_KEY", "REPEAT"] },
];

function tableName(table: readonly NameEntry[], code: number): string | undefined {
  return table.find((entry) => entry.code === code)?.name;
}

function tableCode(table: readonly NameEntry[], name: string): number | undefined {
  const upper = name.toUpperCase();
  return table.find(
    (entry) =>
      entry.name.toUpperCase() === upper ||
      entry.aliases.some((alias) => alias.toUpperCase() === upper),
  )?.code;
}

// Modifier wrapper names in packed-bit order: `[bit, left, right]`.
const MOD_WRAPPERS: ReadonlyArray<readonly [number, string, string]> = [
  [MOD_CTRL, "LCTL", "RCTL"],
  [MOD_SHIFT, "LSFT", "RSFT"],
  [MOD_ALT, "LALT", "RALT"],
  [MOD_GUI, "LGUI", "RGUI"],
];

// Render packed modifier bits as a `MOD_LCTL|MOD_LSFT` style list.
function formatModList(bits: number): string {
  const right = (bits & MOD_RIGHT) !== 0;
  const names = MOD_WRAPPERS.filter(([bit]) => (bits & bit) !== 0).map(
    ([, left, rightName]) => `MOD_${right ? rightName : left}`,
  );
  return names.length === 0 ? "MOD_NONE" : names.join("|");
}

function hex4(code: number): string {
  return `0x${code.toString(16).toUpperCase().padStart(4, "0")}`;
}

// Format any VIA keycode as a human-readable name; never fails
// (unknown codes render as `0xXXXX`).
export function formatKeycode(code: number): string {
  const basicName = tableName(BASIC, code);
  if (basicName !== undefined) {
    return basicName;
  }
  const extraName = tableName(EXTRA, code);
  if (extraName !== undefined) {
    return extraName;
  }
  const basic = (kc: number): string => tableName(BASIC, kc) ?? hex4(kc);

  if (code >= 0x0100 && code <= 0x1fff) {
    const bits = (code >> 8) & 0xff;
    if ((bits & ~MOD_RIGHT) === 0) {
      // Only the hand flag set: no nameable modifier.
      return hex4(code);
    }
    const inner = basic(code & 0xff);
    const right = (bits & MOD_RIGHT) !== 0;
    const plainBits = bits & ~MOD_RIGHT;
    if (plainBits === MOD_MEH && !right) {
      return `MEH(${inner})`;
    }
    if (plainBits === MOD_HYPR && !right) {
      return `HYPR(${inner})`;
    }
    let text = inner;
    // Innermost-first so the rendered nesting reads
    // LCTL(LSFT(kc)) for ctrl|shift.
    for (let i = MOD_WRAPPERS.length - 1; i >= 0; i--) {
      const [bit, left, rightName] = MOD_WRAPPERS[i];
      if ((bits & bit) !== 0) {
        const name = right ? rightName : left;
        text = `${name}(${text})`;
      }
    }
    return text;
  }

  if (code >= 0x2000 && code <= 0x3fff) {
    const bits = (code >> 8) & 0x1f;
    if ((bits & ~MOD_RIGHT) === 0) {
      return hex4(code);
    }
    const inner = basic(code & 0xff);
    const plainBits = bits & ~MOD_RIGHT;
    const right = (bits & MOD_RIGHT) !== 0;
    const single = MOD_WRAPPERS.find(([bit]) => bit === plainBits);
    if (plainBits === MOD_MEH && !right) {
      return `MEH_T(${inner})`;
    }
    if (plainBits === MOD_HYPR && !right) {
      return `ALL_T(${inner})`;
    }
    if (single !== undefined) {
      const [, left, rightName] = single;
      return `${right ? rightName : left}_T(${inner})`;
    }
    return `MT(${formatModList(bits)}, ${inner})`;
  }

  if (code >= 0x4000 && code <= 0x4fff) {
    return `LT(${(code >> 8) & 0xf}, ${basic(code & 0xff)})`;
  }

  if (code >= 0x5000 && code <= 0x51ff && (code & 0x0f) !== 0) {
    return `LM(${(code >> 5) & 0xf}, ${formatModList(code & 0x1f)})`;
  }

  if (code >= 0x5200 && code <= 0x521f) {
    return `TO(${code & 0xf})`;
  }
  if (code >= 0x5220 && code <= 0x523f) {
    return `MO(${code & 0xf})`;
  }
  if (code >= 0x5240 && code <= 0x525f) {
    return `DF(${code & 0xf})`;
  }
  if (code >= 0x5260 && code <= 0x527f) {
    return `TG(${code & 0xf})`;
  }
  if (code >= 0x5280 && code <= 0x529f) {
    return `OSL(${code & 0xf})`;
  }
  if (code >= 0x52a0 && code <= 0x52bf && (code & 0x0f) !== 0) {
    return `OSM(${formatModList(code & 0x1f)})`;
  }
  if (code >= 0x52c0 && code <= 0x52df) {
    return `TT(${code & 0xf})`;
  }
  if (code >= 0x52e0 && code <= 0x52ff) {
    return `PDF(${code & 0xf})`;
  }
  if (code >= 0x5700 && code <= 0x57ff) {
    return `TD(${code & 0xff})`;
  }
  if (code >= 0x7700 && code <= 0x771f) {
    return `MACRO(${code & 0x1f})`;
  }
  if (code >= 0x7e00 && code <= 0x7e1f) {
    return `USER(${code & 0x1f})`;
  }
  return hex4(code);
}

// Parse packed modifier bits from a `MOD_LCTL|MOD_RSFT` style list. Bare
// names without the `MOD_` prefix are accepted. Any right-hand modifier
// sets the shared right-hand flag (the encoding cannot mix hands).
function parseModList(text: string): number {
  let bits = 0;
  for (const rawToken of text.split("|")) {
    const token = rawToken.trim().toUpperCase();
    const name = token.startsWith("MOD_") ? token.slice(4) : token;
    const matched = MOD_WRAPPERS.find(([, left, right]) => name === left || name === right);
    if (matched !== undefined) {
      const [bit, , right] = matched;
      bits |= name === right ? bit | MOD_RIGHT : bit;
    } else if (name === "MEH") {
      bits |= MOD_MEH;
    } else if (name === "HYPR") {
      bits |= MOD_HYPR;
    } else {
      throw new KeycodeError(
        `unknown modifier '${token}' (use MOD_LCTL, MOD_LSFT, MOD_LALT, MOD_LGUI, ` +
          `their R variants, MEH, or HYPR, joined with '|')`,
      );
    }
  }
  if (bits === 0) {
    throw new KeycodeError("empty modifier list");
  }
  return bits;
}

function parseLayer(text: string, what: string, max: number): number {
  const trimmed = text.trim();
  if (!/^[0-9]+$/.test(trimmed)) {
    throw new KeycodeError(`${what} must be an integer, got '${trimmed}'`);
  }
  const value = Number.parseInt(trimmed, 10);
  if (value > max) {
    throw new KeycodeError(`${what} must be at most ${max}, got ${value}`);
  }
  return value;
}

// Parse a basic (< 0x0100) keycode argument used inside composites.
function parseBasic(text: string): number {
  const code = parseKeycode(text);
  if (code > 0x00ff) {
    throw new KeycodeError(
      `'${text.trim()}' (${formatKeycode(code)}) cannot be nested here; only basic ` +
        `keycodes (below 0x0100) fit inside this composite`,
    );
  }
  return code;
}

// Parse a keycode from any accepted spelling: `0x`-prefixed hex, bare
// decimal, a `KC_*`/magic name or alias (case-insensitive, `KC_` optional),
// or a composite like `MO(2)`, `LT(1, KC_A)`, `LSFT(KC_1)`, `LCTL_T(KC_A)`,
// `MT(MOD_LCTL|MOD_LSFT, KC_A)`, `OSM(MOD_LSFT)`, `LM(1, MOD_LALT)`.
export function parseKeycode(rawText: string): number {
  const text = rawText.trim();
  if (text.length === 0) {
    throw new KeycodeError("empty keycode");
  }
  if (text.startsWith("0x") || text.startsWith("0X")) {
    const hex = text.slice(2);
    if (!/^[0-9a-fA-F]+$/.test(hex)) {
      throw new KeycodeError(`'${text}' is not a valid hex keycode`);
    }
    return Number.parseInt(hex, 16);
  }
  if (/^[0-9]+$/.test(text)) {
    const value = Number.parseInt(text, 10);
    if (value > 0xffff) {
      throw new KeycodeError(`'${text}' does not fit a 16-bit keycode`);
    }
    return value;
  }

  // Composite syntax NAME(args).
  const openIndex = text.indexOf("(");
  if (openIndex !== -1) {
    const name = text.slice(0, openIndex);
    const rest = text.slice(openIndex + 1);
    if (!rest.endsWith(")")) {
      throw new KeycodeError(`'${text}' is missing its closing parenthesis`);
    }
    const args = rest.slice(0, -1);
    return parseComposite(name.trim().toUpperCase(), args, text);
  }

  const upper = text.toUpperCase();
  const named = tableCode(BASIC, upper) ?? tableCode(EXTRA, upper);
  if (named !== undefined) {
    return named;
  }
  const prefixed = `KC_${upper}`;
  const prefixedCode = tableCode(BASIC, prefixed);
  if (prefixedCode !== undefined) {
    return prefixedCode;
  }
  throw new KeycodeError(
    `unknown keycode '${text}'; try \`keymap find ${text}\`, a composite like MO(2) or ` +
      `LT(1, KC_A), or a raw hex value like 0x0004`,
  );
}

function parseComposite(name: string, args: string, original: string): number {
  // Single-integer layer verbs.
  const layerVerb = (base: number): number => base | parseLayer(args, "layer", 15);

  switch (name) {
    case "TO":
      return layerVerb(0x5200);
    case "MO":
      return layerVerb(0x5220);
    case "DF":
      return layerVerb(0x5240);
    case "TG":
      return layerVerb(0x5260);
    case "OSL":
      return layerVerb(0x5280);
    case "TT":
      return layerVerb(0x52c0);
    case "PDF":
      return layerVerb(0x52e0);
    case "TD":
      return 0x5700 | parseLayer(args, "tap-dance index", 255);
    case "MACRO":
    case "M":
      return 0x7700 | parseLayer(args, "macro index", 31);
    case "USER":
    case "CUSTOM":
      return 0x7e00 | parseLayer(args, "user-key index", 31);
    case "OSM":
      return 0x52a0 | parseModList(args);
    default:
      break;
  }

  // Two-argument composites: LT(layer, kc), LM(layer, mods), MT(mods, kc).
  if (name === "LT" || name === "LM" || name === "MT") {
    const commaIndex = args.indexOf(",");
    if (commaIndex === -1) {
      throw new KeycodeError(`'${original}' needs two comma-separated arguments`);
    }
    const first = args.slice(0, commaIndex);
    const second = args.slice(commaIndex + 1);
    if (name === "LT") {
      return 0x4000 | (parseLayer(first, "layer", 15) << 8) | parseBasic(second);
    }
    if (name === "LM") {
      return 0x5000 | (parseLayer(first, "layer", 15) << 5) | parseModList(second);
    }
    // MT
    return 0x2000 | (parseModList(first) << 8) | parseBasic(second);
  }

  // Mod-tap shorthands: LCTL_T(kc), MEH_T(kc), ALL_T(kc), ...
  if (name.endsWith("_T")) {
    const prefix = name.slice(0, -2);
    let mods: number | undefined;
    if (prefix === "MEH") {
      mods = MOD_MEH;
    } else if (prefix === "ALL" || prefix === "HYPR") {
      mods = MOD_HYPR;
    } else {
      const wrapper = MOD_WRAPPERS.find(([, left, right]) => prefix === left || prefix === right);
      if (wrapper !== undefined) {
        const [bit, , right] = wrapper;
        mods = prefix === right ? bit | MOD_RIGHT : bit;
      }
    }
    if (mods !== undefined) {
      return 0x2000 | (mods << 8) | parseBasic(args);
    }
  }

  // Modifier wrappers: LSFT(kc), C(kc), MEH(kc), ... possibly nested.
  let wrapperBits: number | undefined;
  if (name === "MEH") {
    wrapperBits = MOD_MEH;
  } else if (name === "HYPR") {
    wrapperBits = MOD_HYPR;
  } else if (name === "C") {
    wrapperBits = MOD_CTRL;
  } else if (name === "S") {
    wrapperBits = MOD_SHIFT;
  } else if (name === "A") {
    wrapperBits = MOD_ALT;
  } else if (name === "G") {
    wrapperBits = MOD_GUI;
  } else {
    const wrapper = MOD_WRAPPERS.find(([, left, right]) => name === left || name === right);
    if (wrapper !== undefined) {
      const [bit, , right] = wrapper;
      wrapperBits = name === right ? bit | MOD_RIGHT : bit;
    }
  }

  if (wrapperBits !== undefined) {
    const bits = wrapperBits;
    const inner = parseKeycode(args);
    if (inner <= 0x00ff) {
      return (bits << 8) | inner;
    }
    if (inner >= 0x0100 && inner <= 0x1fff) {
      // Nested wrappers: merge modifier bits (hands must agree).
      const innerBits = (inner >> 8) & 0xff;
      if (
        (bits & MOD_RIGHT) !== (innerBits & MOD_RIGHT) &&
        (innerBits & ~MOD_RIGHT) !== 0 &&
        (bits & ~MOD_RIGHT) !== 0
      ) {
        throw new KeycodeError(
          `'${original}' mixes left- and right-hand modifiers; the encoding ` +
            `has a single hand flag for all of them`,
        );
      }
      return ((bits | innerBits) << 8) | (inner & 0xff);
    }
    throw new KeycodeError(
      `'${original}': modifier wrappers only apply to basic keycodes, not ${formatKeycode(inner)}`,
    );
  }

  throw new KeycodeError(
    `unknown composite '${name}' in '${original}' (supported: TO, MO, DF, TG, OSL, OSM, ` +
      `TT, PDF, LT, LM, MT, TD, MACRO, USER, modifier wrappers like LSFT()/MEH()/HYPR(), ` +
      `and mod-taps like LCTL_T())`,
  );
}

// Search the name tables for a fragment (case-insensitive substring over
// canonical names and aliases). Returns entries in BASIC then EXTRA order.
export function searchKeycodes(
  fragment: string,
): Array<{ code: number; name: string; aliases: readonly string[] }> {
  const needle = fragment.toUpperCase();
  const matches = (entry: NameEntry): boolean =>
    entry.name.toUpperCase().includes(needle) ||
    entry.aliases.some((alias) => alias.toUpperCase().includes(needle));
  return [...BASIC, ...EXTRA]
    .filter(matches)
    .map(({ code, name, aliases }) => ({ code, name, aliases }));
}
