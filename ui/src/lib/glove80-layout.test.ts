import { describe, expect, it } from "vitest";

import {
  colorsByLed,
  GLOVE80_KEYS,
  GRID_TO_LED,
  KEYMAP_COLS,
  KEYMAP_HOLES,
  KEYMAP_KEY_COUNT,
  LED_TO_GRID,
  mirrorLeftToRight,
} from "./glove80-layout";

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

describe("Glove80 keymap grid", () => {
  it("is a bijection between the 80 non-hole grid positions and the 80 LEDs", () => {
    expect(GRID_TO_LED.size).toBe(KEYMAP_KEY_COUNT - KEYMAP_HOLES.length);
    expect(new Set(GRID_TO_LED.values())).toEqual(
      new Set(Array.from({ length: 80 }, (_, index) => index)),
    );
    for (const [grid, led] of GRID_TO_LED) {
      expect(LED_TO_GRID.get(led)).toBe(grid);
    }
    for (const hole of KEYMAP_HOLES) expect(GRID_TO_LED.has(hole)).toBe(false);
  });

  it("puts the holes at rows 0/5, columns 5/8 (per PROTOCOL.md)", () => {
    expect(KEYMAP_HOLES).toEqual([5, 8, 75, 78]);
    for (const hole of KEYMAP_HOLES) {
      expect([0, 5]).toContain(Math.floor(hole / KEYMAP_COLS));
      expect([5, 8]).toContain(hole % KEYMAP_COLS);
    }
  });

  it("maps spot-checked matrix positions to the expected physical keys", () => {
    const labelOf = (grid: number) => {
      const led = GRID_TO_LED.get(grid);
      return GLOVE80_KEYS.find((key) => key.ledIndex === led)?.label;
    };
    expect(labelOf(0)).toBe("F1"); // r0,c0
    expect(labelOf(13)).toBe("F10"); // r0,c13
    expect(labelOf(6)).toBe("Esc"); // r0,c6 — left thumb top
    expect(labelOf(2 * KEYMAP_COLS + 1)).toBe("Q"); // r2,c1
    expect(labelOf(3 * KEYMAP_COLS + 1)).toBe("A"); // r3,c1 home row
    expect(labelOf(5 * KEYMAP_COLS + 13)).toBe("L3"); // r5,c13
    expect(labelOf(5 * KEYMAP_COLS + 7)).toBe("⌘"); // r5,c7 — right thumb low
  });
});
