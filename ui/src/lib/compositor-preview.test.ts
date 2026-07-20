// Fixtures for the client-side mini-compositor, hand-computed against the
// contract in crates/glove80-compositor/src/lib.rs (and mirroring that crate's
// own test values where they apply: class ordering, transparent reveal,
// blink duty/phase edges, the breathe triangle wave, brightness scaling and
// the ceiling clamp).

import { describe, expect, it } from "vitest";

import {
  CHANNEL_CEILING,
  composePreview,
  effectAnimated,
  effectColorAt,
  previewAnimated,
  PREVIEW_LED_COUNT,
  type PreviewState,
  type Rgb,
} from "./compositor-preview";
import type { CellWrite, ConfigRecord, Effect } from "./host-protocol";

const rgb = (r: number, g: number, b: number): Rgb => ({ r, g, b });

// Composition fixtures use channel values below CHANNEL_CEILING (204) so the
// always-on safety clamp is a no-op for them; the ceiling tests exercise the
// clamp explicitly with full-scale white.
const RED = rgb(200, 0, 0);
const GREEN = rgb(0, 200, 0);
const BLUE = rgb(0, 0, 200);
const WHITE = rgb(200, 200, 200);

function solid(color: Rgb): Effect {
  return { kind: "solid", ...color, periodMs: 0, phaseMs: 0, dutyPercent: 0 };
}

function blink(color: Rgb, periodMs: number, phaseMs: number, dutyPercent: number): Effect {
  return { kind: "blink", ...color, periodMs, phaseMs, dutyPercent };
}

function breathe(color: Rgb, periodMs: number, phaseMs = 0): Effect {
  return { kind: "breathe", ...color, periodMs, phaseMs, dutyPercent: 0 };
}

function cells(entries: Array<[number, Effect]>): CellWrite[] {
  return entries.map(([key, effect]) => ({ key, effect }));
}

function record(activation: ConfigRecord["activation"], entries: Array<[number, Effect]>): ConfigRecord {
  return { activation, cells: cells(entries) };
}

function state(partial: Partial<PreviewState> & Pick<PreviewState, "records">): PreviewState {
  return { activeLayer: 0, togglesMask: 0, ...partial };
}

describe("composePreview composition", () => {
  it("base fills its keys and unset keys stay null", () => {
    const frame = composePreview(
      state({ records: [record({ kind: "always" }, [[0, solid(RED)], [3, solid(GREEN)]])] }),
      0,
    );
    expect(frame).toHaveLength(PREVIEW_LED_COUNT);
    expect(frame[0]).toEqual(RED);
    expect(frame[3]).toEqual(GREEN);
    for (const key of [1, 2, 4, 5, 79]) expect(frame[key]).toBeNull();
  });

  it("class order is base < layer < toggle < host, regardless of list order", () => {
    // Records listed in REVERSE class order: class, not list position, wins.
    const frame = composePreview(
      state({
        records: [
          record({ kind: "toggle", id: 1 }, [[0, solid(BLUE)], [1, solid(BLUE)], [2, solid(BLUE)]]),
          record({ kind: "layerActive", layer: 0 }, [
            [0, solid(GREEN)], [1, solid(GREEN)], [2, solid(GREEN)], [3, solid(GREEN)],
          ]),
          record({ kind: "always" }, [
            [0, solid(RED)], [1, solid(RED)], [2, solid(RED)], [3, solid(RED)], [4, solid(RED)],
          ]),
        ],
        togglesMask: 1 << 1,
        hostCells: cells([[0, solid(rgb(9, 9, 9))]]),
      }),
      0,
    );
    expect(frame[0]).toEqual(rgb(9, 9, 9)); // host beats toggle
    expect(frame[1]).toEqual(BLUE); // toggle beats layer
    expect(frame[2]).toEqual(BLUE);
    expect(frame[3]).toEqual(GREEN); // layer beats base
    expect(frame[4]).toEqual(RED); // base shows where nothing is above
  });

  it("an unpainted key is transparent and reveals the record below", () => {
    const frame = composePreview(
      state({
        records: [
          record({ kind: "always" }, [[0, solid(RED)], [1, solid(RED)]]),
          record({ kind: "layerActive", layer: 0 }, [[1, solid(GREEN)]]), // key 0 unpainted
        ],
      }),
      0,
    );
    expect(frame[0]).toEqual(RED); // revealed
    expect(frame[1]).toEqual(GREEN); // replaced
  });

  it("list order breaks ties within a class (later records win)", () => {
    const frame = composePreview(
      state({
        records: [
          record({ kind: "always" }, [[0, solid(RED)]]),
          record({ kind: "always" }, [[0, solid(GREEN)]]),
        ],
      }),
      0,
    );
    expect(frame[0]).toEqual(GREEN);
  });

  it("inactive layer and toggle records do not compose", () => {
    const records = [
      record({ kind: "layerActive", layer: 2 }, [[0, solid(GREEN)]]),
      record({ kind: "toggle", id: 3 }, [[1, solid(BLUE)]]),
    ];
    const off = composePreview(state({ records }), 0);
    expect(off[0]).toBeNull();
    expect(off[1]).toBeNull();

    const on = composePreview(state({ records, activeLayer: 2, togglesMask: 1 << 3 }), 0);
    expect(on[0]).toEqual(GREEN);
    expect(on[1]).toEqual(BLUE);
  });

  it("solo composes only the chosen record, forcing it active and dropping host cells", () => {
    const st = state({
      records: [
        record({ kind: "always" }, [[0, solid(RED)], [1, solid(RED)]]),
        record({ kind: "toggle", id: 7 }, [[1, solid(BLUE)], [2, solid(BLUE)]]),
      ],
      togglesMask: 0, // toggle 7 is OFF — solo still shows it
      hostCells: cells([[3, solid(WHITE)]]),
      soloRecord: 1,
    });
    const frame = composePreview(st, 0);
    expect(frame[0]).toBeNull(); // base excluded
    expect(frame[1]).toEqual(BLUE);
    expect(frame[2]).toEqual(BLUE);
    expect(frame[3]).toBeNull(); // host sample excluded
  });
});

describe("blink", () => {
  it("duty and phase hit the exact edges of the Rust fixture", () => {
    const cell = blink(RED, 1000, 0, 25);
    expect(effectColorAt(cell, 0)).toEqual(RED);
    expect(effectColorAt(cell, 249)).toEqual(RED);
    expect(effectColorAt(cell, 250)).toEqual(rgb(0, 0, 0)); // dark phase is black
    expect(effectColorAt(cell, 999)).toEqual(rgb(0, 0, 0));
    expect(effectColorAt(cell, 1000)).toEqual(RED); // wraps to the next period

    // phase shifts the waveform: with phase 250 the cell is dark at t=0.
    const shifted = blink(RED, 1000, 250, 25);
    expect(effectColorAt(shifted, 0)).toEqual(rgb(0, 0, 0));
    expect(effectColorAt(shifted, 750)).toEqual(RED); // 750 + 250 wraps into ON
  });

  it("dark phase occludes the record below (black, not transparent)", () => {
    const st = state({
      records: [
        record({ kind: "always" }, [[0, solid(GREEN)]]),
        record({ kind: "layerActive", layer: 0 }, [[0, blink(RED, 100, 0, 50)]]),
      ],
    });
    expect(composePreview(st, 0)[0]).toEqual(RED);
    expect(composePreview(st, 50)[0]).toEqual(rgb(0, 0, 0)); // NOT green
  });

  it("degenerate blinks are static", () => {
    expect(effectColorAt(blink(RED, 0, 0, 50), 12345)).toEqual(RED); // period 0 = on
    expect(effectColorAt(blink(RED, 100, 0, 100), 12345)).toEqual(RED); // duty 100 = on
    expect(effectColorAt(blink(RED, 100, 0, 0), 12345)).toEqual(rgb(0, 0, 0)); // duty 0 = black
    for (const cell of [blink(RED, 0, 0, 50), blink(RED, 100, 0, 100), blink(RED, 100, 0, 0)]) {
      expect(effectAnimated(cell)).toBe(false);
    }
  });
});

describe("breathe", () => {
  it("rises from black to the peak at half period, then falls back", () => {
    const full = rgb(255, 255, 255);
    const level = (t: number) => effectColorAt(breathe(full, 1000), t).r;
    expect(level(0)).toBe(0);
    expect(level(500)).toBe(255);
    let prev = level(0);
    for (let t = 0; t <= 500; t += 20) {
      expect(level(t)).toBeGreaterThanOrEqual(prev);
      prev = level(t);
    }
    prev = level(500);
    for (let t = 500; t < 1000; t += 20) {
      expect(level(t)).toBeLessThanOrEqual(prev);
      prev = level(t);
    }
    expect(level(1000)).toBe(0); // wraps to black
    // phase shift moves the peak to t=0.
    expect(effectColorAt(breathe(full, 1000, 500), 0).r).toBe(255);
  });

  it("scales all channels with the exact integer math", () => {
    // t=250 of period 1000: level = floor(250*255/500) = 127.
    const c = effectColorAt(breathe(rgb(200, 100, 0), 1000), 250);
    expect(c).toEqual(rgb(Math.floor((200 * 127) / 255), Math.floor((100 * 127) / 255), 0));
    expect(c).toEqual(rgb(99, 49, 0));
  });

  it("period < 2 renders as static color", () => {
    expect(effectColorAt(breathe(RED, 1), 777)).toEqual(RED);
    expect(effectAnimated(breathe(RED, 1))).toBe(false);
    expect(effectAnimated(breathe(RED, 2))).toBe(true);
  });
});

describe("brightness and ceiling", () => {
  it("brightness 255 is identity, otherwise floor(c * level / 255)", () => {
    const records = [record({ kind: "always" }, [[0, solid(rgb(200, 100, 50))]])];
    expect(composePreview(state({ records }), 0)[0]).toEqual(rgb(200, 100, 50));
    expect(composePreview(state({ records, brightness: 128 }), 0)[0]).toEqual(
      rgb(Math.floor((200 * 128) / 255), Math.floor((100 * 128) / 255), Math.floor((50 * 128) / 255)),
    );
    expect(composePreview(state({ records, brightness: 0 }), 0)[0]).toEqual(rgb(0, 0, 0));
  });

  it("clamps to CHANNEL_CEILING by default and a runtime ceiling only lowers", () => {
    const records = [record({ kind: "always" }, [[0, solid(rgb(255, 255, 255))]])];
    expect(composePreview(state({ records }), 0)[0]).toEqual(
      rgb(CHANNEL_CEILING, CHANNEL_CEILING, CHANNEL_CEILING),
    );
    expect(composePreview(state({ records, ceiling: 100 }), 0)[0]).toEqual(rgb(100, 100, 100));
    // Attempting to raise above the compiled safety value is ignored.
    expect(composePreview(state({ records, ceiling: 255 }), 0)[0]).toEqual(
      rgb(CHANNEL_CEILING, CHANNEL_CEILING, CHANNEL_CEILING),
    );
  });
});

describe("previewAnimated", () => {
  it("is false for a fully static composition", () => {
    expect(
      previewAnimated(state({ records: [record({ kind: "always" }, [[0, solid(RED)]])] })),
    ).toBe(false);
  });

  it("tracks whether animated records are currently active", () => {
    const records = [
      record({ kind: "always" }, [[0, solid(GREEN)]]),
      record({ kind: "layerActive", layer: 1 }, [[0, breathe(RED, 1000)]]),
    ];
    expect(previewAnimated(state({ records }))).toBe(false);
    expect(previewAnimated(state({ records, activeLayer: 1 }))).toBe(true);
  });

  it("counts sample host cells except under solo", () => {
    const st = state({
      records: [record({ kind: "always" }, [[0, solid(RED)]])],
      hostCells: cells([[1, blink(WHITE, 500, 0, 50)]]),
    });
    expect(previewAnimated(st)).toBe(true);
    expect(previewAnimated({ ...st, soloRecord: 0 })).toBe(false);
  });
});
