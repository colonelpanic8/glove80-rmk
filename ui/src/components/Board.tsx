import { useEffect, useRef } from "react";

import { GLOVE80_KEYS } from "../lib/glove80-layout";
import type { Effect } from "../lib/host-protocol";

export function effectColor(effect: Effect): string {
  return `#${((effect.r << 16) | (effect.g << 8) | effect.b).toString(16).padStart(6, "0")}`;
}

function effectTitle(effect: Effect): string {
  switch (effect.kind) {
    case "solid":
      return "solid";
    case "blink":
      return `blink ${(effect.periodMs / 1000).toFixed(2)}s · ${effect.dutyPercent}% on`;
    case "breathe":
      return `breathe ${(effect.periodMs / 1000).toFixed(2)}s`;
  }
}

export interface BoardCell {
  effect: Effect;
  /** Extra line for the key tooltip (e.g. a TTL countdown). */
  note?: string;
}

interface BoardProps {
  /** Lit cells by protocol key index (= LED chain index, right half 40–79). */
  cells: ReadonlyMap<number, BoardCell>;
  /** Keys accepted on the central but pending on the offline right half —
   * or, on the keymap panel, staged edits not yet written. */
  pendingKeys?: ReadonlySet<number>;
  /** Called on click and drag-over. Absent = board is read-only. */
  onPaintKey?: (ledIndex: number) => void;
  caption?: string;
  /** Replace the printed key legends (e.g. keymap bindings), by LED index. */
  keyLabels?: ReadonlyMap<number, string>;
  /** Highlight one key as the current selection. */
  selectedKey?: number | null;
  /** Mark keys whose last write was lossy (stored ≠ requested). */
  flaggedKeys?: ReadonlySet<number>;
}

/**
 * The shared Glove80 visualization. Both panels paint through this board;
 * the protocol key space is the LED chain order from glove80-layout.ts.
 */
export function Board({
  cells,
  pendingKeys,
  onPaintKey,
  caption,
  keyLabels,
  selectedKey,
  flaggedKeys,
}: BoardProps) {
  const painting = useRef(false);

  useEffect(() => {
    const stop = () => {
      painting.current = false;
    };
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
    return () => {
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
  }, []);

  return (
    <div className="keyboard-scroll">
      <div className="keyboard-map" onDragStart={(event) => event.preventDefault()}>
        <div className="half-label left">Left · keys 0–39</div>
        <div className="half-label right">Right · keys 40–79</div>
        <div className="center-mark" aria-hidden="true">
          <span />
        </div>
        {GLOVE80_KEYS.map((keySpec) => {
          const cell = cells.get(keySpec.ledIndex);
          const pending = pendingKeys?.has(keySpec.ledIndex) ?? false;
          const flagged = flaggedKeys?.has(keySpec.ledIndex) ?? false;
          const customLabel = keyLabels?.get(keySpec.ledIndex);
          const color = cell ? effectColor(cell.effect) : "#000000";
          const classes = [
            "keycap",
            keySpec.kind,
            cell ? `effect-${cell.effect.kind}` : "unlit",
            pending ? "pending" : "",
            flagged ? "flagged" : "",
            selectedKey === keySpec.ledIndex ? "selected" : "",
            onPaintKey ? "" : "readonly",
          ]
            .filter(Boolean)
            .join(" ");
          const title = [
            `${keySpec.label} · key ${keySpec.ledIndex}`,
            customLabel,
            keyLabels ? undefined : cell ? `${color} · ${effectTitle(cell.effect)}` : "transparent",
            cell?.note,
            pending ? (keyLabels ? "staged — not written yet" : "pending on the right half") : undefined,
            flagged ? "LOSSY — the firmware stored a different keycode" : undefined,
          ]
            .filter(Boolean)
            .join("\n");
          return (
            <button
              key={keySpec.ledIndex}
              className={classes}
              style={
                {
                  "--key-x": keySpec.x,
                  "--key-y": keySpec.y,
                  "--key-color": color,
                  "--effect-period": cell ? `${Math.max(cell.effect.periodMs, 1)}ms` : undefined,
                  "--effect-delay": cell ? `${-cell.effect.phaseMs}ms` : undefined,
                } as React.CSSProperties
              }
              onPointerDown={(event) => {
                if (event.button !== 0 || !onPaintKey) return;
                painting.current = true;
                onPaintKey(keySpec.ledIndex);
                event.preventDefault();
              }}
              onPointerEnter={(event) => {
                if (onPaintKey && painting.current && (event.buttons & 1) === 1) {
                  onPaintKey(keySpec.ledIndex);
                }
              }}
              title={title}
              aria-label={title.replaceAll("\n", ", ")}
            >
              <span className="key-light" aria-hidden="true" />
              <span className="key-label">{customLabel ?? keySpec.label}</span>
              <small>{keySpec.ledIndex}</small>
            </button>
          );
        })}
      </div>
      {caption && <p className="board-caption">{caption}</p>}
    </div>
  );
}
