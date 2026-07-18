import { describe, expect, it } from "vitest";

import { colorsByLed, GLOVE80_KEYS, mirrorLeftToRight } from "./glove80-layout";

describe("Glove80 LED layout", () => {
  it("maps every logical key and every physical LED exactly once", () => {
    expect(GLOVE80_KEYS).toHaveLength(80);
    expect(new Set(GLOVE80_KEYS.map((key) => key.logicalIndex))).toEqual(
      new Set(Array.from({ length: 80 }, (_, index) => index)),
    );
    expect(new Set(GLOVE80_KEYS.map((key) => key.ledIndex))).toEqual(
      new Set(Array.from({ length: 80 }, (_, index) => index)),
    );
  });

  it("converts logical colors to LED-chain order", () => {
    const colors = Array.from({ length: 80 }, (_, index) => index + 100);
    const byLed = colorsByLed(GLOVE80_KEYS, colors);
    for (const key of GLOVE80_KEYS) expect(byLed[key.ledIndex]).toBe(colors[key.logicalIndex]);
  });

  it("mirrors every left LED onto its physical right-side partner", () => {
    const colors = Array.from({ length: 80 }, (_, index) => index);
    const mirrored = mirrorLeftToRight(colors);
    const logicalByLed = new Map(GLOVE80_KEYS.map((key) => [key.ledIndex, key.logicalIndex]));
    for (const left of GLOVE80_KEYS.filter((key) => key.ledIndex < 40)) {
      expect(mirrored[logicalByLed.get(left.mirrorLedIndex)!]).toBe(colors[left.logicalIndex]);
    }
  });
});
