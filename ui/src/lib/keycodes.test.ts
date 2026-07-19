import { describe, expect, it } from "vitest";

import { formatKeycode, KeycodeError, parseKeycode, searchKeycodes } from "./keycodes";

describe("formatKeycode", () => {
  it("formats basic and unknown codes", () => {
    expect(formatKeycode(0x0000)).toBe("KC_NO");
    expect(formatKeycode(0x0001)).toBe("KC_TRNS");
    expect(formatKeycode(0x0004)).toBe("KC_A");
    expect(formatKeycode(0x00e5)).toBe("KC_RSFT");
    expect(formatKeycode(0x00ae)).toBe("KC_MPLY");
    // Unnamed codes never fail; they render as hex.
    expect(formatKeycode(0x0083)).toBe("0x0083");
    expect(formatKeycode(0x6fff)).toBe("0x6FFF");
  });

  it("formats composites matching firmware encoding", () => {
    // Values cross-checked against keycode_convert.rs tests.
    expect(formatKeycode(0x5223)).toBe("MO(3)");
    expect(formatKeycode(0x5283)).toBe("OSL(3)");
    expect(formatKeycode(0x5243)).toBe("DF(3)");
    expect(formatKeycode(0x52e3)).toBe("PDF(3)");
    expect(formatKeycode(0x0104)).toBe("LCTL(KC_A)");
    expect(formatKeycode(0x1104)).toBe("RCTL(KC_A)");
    expect(formatKeycode(0x0704)).toBe("MEH(KC_A)");
    expect(formatKeycode(0x0f04)).toBe("HYPR(KC_A)");
    expect(formatKeycode(0x0304)).toBe("LCTL(LSFT(KC_A))");
    expect(formatKeycode(0x4304)).toBe("LT(3, KC_A)");
    expect(formatKeycode(0x2204)).toBe("LSFT_T(KC_A)");
    expect(formatKeycode(0x3228)).toBe("RSFT_T(KC_ENT)");
    expect(formatKeycode(0x2704)).toBe("MEH_T(KC_A)");
    expect(formatKeycode(0x2f04)).toBe("ALL_T(KC_A)");
    expect(formatKeycode(0x2604)).toBe("MT(MOD_LSFT|MOD_LALT, KC_A)");
    expect(formatKeycode(0x5022)).toBe("LM(1, MOD_LSFT)");
    expect(formatKeycode(0x52a2)).toBe("OSM(MOD_LSFT)");
    expect(formatKeycode(0x52b1)).toBe("OSM(MOD_RCTL)");
    expect(formatKeycode(0x5705)).toBe("TD(5)");
    expect(formatKeycode(0x7705)).toBe("MACRO(5)");
    expect(formatKeycode(0x7e10)).toBe("USER(16)");
    expect(formatKeycode(0x7c00)).toBe("QK_BOOT");
    expect(formatKeycode(0x7c73)).toBe("CW_TOGG");
  });
});

describe("parseKeycode", () => {
  it("parses names, hex, and decimal", () => {
    expect(parseKeycode("KC_A")).toBe(0x0004);
    expect(parseKeycode("kc_a")).toBe(0x0004);
    expect(parseKeycode("A")).toBe(0x0004);
    expect(parseKeycode("KC_ENTER")).toBe(0x0028);
    expect(parseKeycode("ENT")).toBe(0x0028);
    expect(parseKeycode("0x0004")).toBe(0x0004);
    expect(parseKeycode("0X1104")).toBe(0x1104);
    expect(parseKeycode("4")).toBe(4);
    expect(parseKeycode("QK_BOOT")).toBe(0x7c00);
    expect(parseKeycode("_______")).toBe(0x0001);
    expect(() => parseKeycode("KC_NOPE")).toThrow(KeycodeError);
    expect(() => parseKeycode("")).toThrow(KeycodeError);
  });

  it("parses composites", () => {
    expect(parseKeycode("MO(3)")).toBe(0x5223);
    expect(parseKeycode("mo(3)")).toBe(0x5223);
    expect(parseKeycode("TG(2)")).toBe(0x5262);
    expect(parseKeycode("TO(1)")).toBe(0x5201);
    expect(parseKeycode("OSL(3)")).toBe(0x5283);
    expect(parseKeycode("PDF(15)")).toBe(0x52ef);
    expect(parseKeycode("LT(3, KC_A)")).toBe(0x4304);
    expect(parseKeycode("LT(3,a)")).toBe(0x4304);
    expect(parseKeycode("LSFT(KC_1)")).toBe(0x021e);
    expect(parseKeycode("S(KC_1)")).toBe(0x021e);
    expect(parseKeycode("RCTL(KC_A)")).toBe(0x1104);
    expect(parseKeycode("MEH(KC_A)")).toBe(0x0704);
    expect(parseKeycode("HYPR(KC_A)")).toBe(0x0f04);
    expect(parseKeycode("LCTL(LSFT(KC_A))")).toBe(0x0304);
    expect(parseKeycode("LSFT_T(KC_A)")).toBe(0x2204);
    expect(parseKeycode("RSFT_T(KC_ENT)")).toBe(0x3228);
    expect(parseKeycode("MEH_T(KC_A)")).toBe(0x2704);
    expect(parseKeycode("ALL_T(KC_A)")).toBe(0x2f04);
    expect(parseKeycode("MT(MOD_LSFT|MOD_LALT, KC_A)")).toBe(0x2604);
    expect(parseKeycode("LM(1, MOD_LSFT)")).toBe(0x5022);
    expect(parseKeycode("OSM(MOD_RCTL)")).toBe(0x52b1);
    expect(parseKeycode("TD(5)")).toBe(0x5705);
    expect(parseKeycode("MACRO(5)")).toBe(0x7705);
    expect(parseKeycode("M(5)")).toBe(0x7705);
    expect(parseKeycode("USER(16)")).toBe(0x7e10);

    // layer beyond 4 bits
    expect(() => parseKeycode("MO(16)")).toThrow(KeycodeError);
    // non-basic tap keycode
    expect(() => parseKeycode("LT(3, MO(2))")).toThrow(KeycodeError);
    // wrapping a non-basic code
    expect(() => parseKeycode("LSFT(MO(2))")).toThrow(KeycodeError);
    // mixed hands
    expect(() => parseKeycode("LCTL(RSFT(KC_A))")).toThrow(KeycodeError);
    // unbalanced parenthesis
    expect(() => parseKeycode("MO(2")).toThrow(KeycodeError);
    // unknown composite
    expect(() => parseKeycode("WAT(2)")).toThrow(KeycodeError);
    // unknown modifier
    expect(() => parseKeycode("OSM(MOD_NOPE)")).toThrow(KeycodeError);
  });

  it("round-trips format and parse", () => {
    // Every named/structured code must re-parse to itself.
    const codes = [
      0x0000, 0x0001, 0x0004, 0x00e7, 0x00ae, 0x0104, 0x1104, 0x0704, 0x0f04,
      0x0304, 0x2204, 0x3228, 0x2704, 0x2f04, 0x2604, 0x4304, 0x5022, 0x5201,
      0x5223, 0x5243, 0x5262, 0x5283, 0x52a2, 0x52b1, 0x52e3, 0x5705, 0x7705,
      0x7c00, 0x7c73, 0x7e10,
      // Unknown codes round-trip through their hex rendering.
      0x0083, 0x6fff,
    ];
    for (const code of codes) {
      const name = formatKeycode(code);
      expect(parseKeycode(name), `round-trip of ${name}`).toBe(code);
    }
  });
});

describe("searchKeycodes", () => {
  it("searches names and aliases", () => {
    let hits = searchKeycodes("play");
    expect(hits.some((hit) => hit.code === 0x00ae && hit.name === "KC_MPLY")).toBe(true);
    hits = searchKeycodes("mply");
    expect(hits.some((hit) => hit.code === 0x00ae)).toBe(true);
    hits = searchKeycodes("boot");
    expect(hits.some((hit) => hit.code === 0x7c00)).toBe(true);
    expect(searchKeycodes("zzzznothing")).toEqual([]);
  });
});
