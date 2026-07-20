// Client-side mini-compositor for the "Composed preview" board.
//
// Faithfully mirrors crates/glove80-compositor/src/lib.rs (the firmware's
// compositor) so the preview shows what the keyboard would render:
//
// - Composition is bottom-to-top by class — base (Always) < layer < toggle
//   < host — insertion order within a class. A defined cell replaces what is
//   below; an unpainted key is transparent and reveals it.
// - A blinking cell's dark phase is BLACK (it occludes), not transparent.
// - Blink/breathe phase math is the exact integer arithmetic of the Rust
//   crate (`phase_local`, `on_ms`, the breathe triangle wave).
// - Output is brightness-scaled (floor(c * level / 255)) then clamped to
//   min(runtime ceiling, CHANNEL_CEILING).
//
// This is a SIMULATION: it runs entirely in the browser and never asks the
// keyboard what it is actually showing. Status-class records (firmware
// state) are not persistable and not simulated; the host class here is an
// optional set of sample cells standing in for a live host overlay.

import type { CellWrite, ConfigRecord, Effect } from "./host-protocol";

/** Compile-time per-channel safety ceiling (80% of full scale) — MoErgo's
 * LED current / warranty limit, `CHANNEL_CEILING` in the compositor crate.
 * The firmware clamps every channel to this no matter what a host asks;
 * a runtime ceiling can only lower it. The host protocol (v1.3) has no
 * command to change the runtime ceiling yet, so the UI treats it as a
 * read-only constant. */
export const CHANNEL_CEILING = 204;

/** Both halves' LED chains: the full protocol key space `0..80`. */
export const PREVIEW_LED_COUNT = 80;

export interface Rgb {
  r: number;
  g: number;
  b: number;
}

export interface PreviewState {
  /** Config records in blob order (= composition order within a class). */
  records: readonly ConfigRecord[];
  /** The keymap layer currently active in the simulation. */
  activeLayer: number;
  /** Bit n set ⇔ toggle id n is on in the simulation. */
  togglesMask: number;
  /** Optional sample host-overlay cells (class above all config records). */
  hostCells?: readonly CellWrite[];
  /** Global brightness scalar 0..255 (default 255). */
  brightness?: number;
  /** Runtime ceiling; stored as min(value, CHANNEL_CEILING), like the
   * firmware's `set_ceiling`. Default CHANNEL_CEILING. */
  ceiling?: number;
  /** When set, compose ONLY this record (activation ignored, host cells
   * excluded) — the per-record solo preview. */
  soloRecord?: number | null;
}

/** Composition class of an activation (bottom to top), mirroring the Rust
 * `class()`: Always 0 < LayerActive 1 < Toggle 2 (< host 3 < status 4). */
function activationClass(record: ConfigRecord): number {
  switch (record.activation.kind) {
    case "always":
      return 0;
    case "layerActive":
      return 1;
    case "toggle":
      return 2;
  }
}

function recordActive(record: ConfigRecord, state: PreviewState): boolean {
  switch (record.activation.kind) {
    case "always":
      return true;
    case "layerActive":
      return record.activation.layer === state.activeLayer;
    case "toggle":
      return (
        record.activation.id < 32 &&
        (state.togglesMask & (1 << record.activation.id)) !== 0
      );
  }
}

/** Waveform-local time in `[0, periodMs)` — the Rust `phase_local`. */
function phaseLocal(nowMs: number, periodMs: number, phaseMs: number): number {
  return (nowMs + phaseMs) % periodMs;
}

/** ON span of a blink period in ms — the Rust `on_ms`. */
function onMs(periodMs: number, dutyPct: number): number {
  return Math.floor((periodMs * Math.min(dutyPct, 100)) / 100);
}

/** Scale every channel by `level / 255` (integer floor) — `Rgb::scaled`. */
function scaled(color: Rgb, level: number): Rgb {
  return {
    r: Math.floor((color.r * level) / 255),
    g: Math.floor((color.g * level) / 255),
    b: Math.floor((color.b * level) / 255),
  };
}

/** Clamp every channel to `ceiling` — `Rgb::clamped`. */
function clamped(color: Rgb, ceiling: number): Rgb {
  return {
    r: Math.min(color.r, ceiling),
    g: Math.min(color.g, ceiling),
    b: Math.min(color.b, ceiling),
  };
}

const BLACK: Rgb = { r: 0, g: 0, b: 0 };

/**
 * The color a defined cell shows at `nowMs` — `Cell::color_at`, minus the
 * transparent case (config cells are always defined; an unpainted key is
 * simply absent). A blink's dark phase is black, never transparent.
 */
export function effectColorAt(effect: Effect, nowMs: number): Rgb {
  const color: Rgb = { r: effect.r, g: effect.g, b: effect.b };
  switch (effect.kind) {
    case "solid":
      return color;
    case "blink": {
      // Degenerate params are static: period 0 or duty >= 100 is always-on,
      // duty 0 is always-black.
      if (effect.periodMs === 0 || effect.dutyPercent >= 100) return color;
      if (effect.dutyPercent === 0) return BLACK;
      const t = phaseLocal(nowMs, effect.periodMs, effect.phaseMs);
      return t < onMs(effect.periodMs, effect.dutyPercent) ? color : BLACK;
    }
    case "breathe": {
      if (effect.periodMs < 2) return color;
      const t = phaseLocal(nowMs, effect.periodMs, effect.phaseMs);
      const half = Math.floor(effect.periodMs / 2);
      const level =
        t < half
          ? Math.floor((t * 255) / half)
          : Math.floor(((effect.periodMs - t) * 255) / (effect.periodMs - half));
      return scaled(color, level);
    }
  }
}

/** Whether this effect's output can change over time (`next_change_after`
 * would be non-None): a non-degenerate blink or a breathing cell. */
export function effectAnimated(effect: Effect): boolean {
  switch (effect.kind) {
    case "solid":
      return false;
    case "blink":
      return effect.periodMs > 0 && effect.dutyPercent > 0 && effect.dutyPercent < 100;
    case "breathe":
      return effect.periodMs >= 2;
  }
}

function activeRecords(state: PreviewState): readonly ConfigRecord[] {
  const solo = state.soloRecord;
  if (solo !== null && solo !== undefined) {
    const record = state.records[solo];
    return record ? [record] : [];
  }
  // Bottom-to-top by class, insertion (list) order within a class —
  // mirrors the Rust render loop `for cls { for record { … } }`.
  const ordered: ConfigRecord[] = [];
  for (let cls = 0; cls <= 2; cls++) {
    for (const record of state.records) {
      if (activationClass(record) === cls && recordActive(record, state)) {
        ordered.push(record);
      }
    }
  }
  return ordered;
}

/**
 * Compose everything visible at `nowMs` into an 80-entry frame.
 * `null` = nothing composed on that key (the board key stays unlit);
 * a defined entry is the final brightness-scaled, ceiling-clamped color.
 */
export function composePreview(state: PreviewState, nowMs: number): (Rgb | null)[] {
  const effective: (Effect | null)[] = Array.from({ length: PREVIEW_LED_COUNT }, () => null);
  const place = (cell: CellWrite) => {
    if (cell.key >= 0 && cell.key < PREVIEW_LED_COUNT) {
      effective[cell.key] = cell.effect; // a defined cell replaces what is below
    }
  };
  for (const record of activeRecords(state)) {
    for (const cell of record.cells) place(cell);
  }
  if (state.soloRecord === null || state.soloRecord === undefined) {
    for (const cell of state.hostCells ?? []) place(cell);
  }

  const brightness = state.brightness ?? 255;
  const ceiling = Math.min(state.ceiling ?? CHANNEL_CEILING, CHANNEL_CEILING);
  return effective.map((effect) =>
    effect === null ? null : clamped(scaled(effectColorAt(effect, nowMs), brightness), ceiling),
  );
}

/** True when any composed cell can change on its own — drives the preview's
 * requestAnimationFrame loop. (Coarser than the firmware's next-wake: an
 * occluded animated cell still counts here; the composition stays correct,
 * the loop just runs when it strictly need not.) */
export function previewAnimated(state: PreviewState): boolean {
  for (const record of activeRecords(state)) {
    if (record.cells.some((cell) => effectAnimated(cell.effect))) return true;
  }
  if (state.soloRecord === null || state.soloRecord === undefined) {
    if ((state.hostCells ?? []).some((cell) => effectAnimated(cell.effect))) return true;
  }
  return false;
}
