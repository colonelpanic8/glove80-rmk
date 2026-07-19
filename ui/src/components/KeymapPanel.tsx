// Keymap editor: the live keymap over KEYMAP_READ/KEYMAP_WRITE (protocol
// v1.2, feature bit 7). Bindings are VIA 16-bit keycodes — the same store
// Vial edits, so both editors always agree. Edits are staged locally and
// written in one batch; the firmware echoes what it actually stored, and any
// difference (a lossy mapping) is flagged rather than hidden.

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  GLOVE80_KEYS,
  GRID_TO_LED,
  KEYMAP_HOLES,
  LED_TO_GRID,
} from "../lib/glove80-layout";
import { FEATURE_KEYMAP, type Capabilities, type KeymapEntry } from "../lib/host-protocol";
import { formatKeycode, KeycodeError, parseKeycode, searchKeycodes } from "../lib/keycodes";
import type { ProtocolClient } from "../lib/protocol-client";
import { Board, type BoardCell } from "./Board";
import type { StatusUpdate } from "./OverlayPanel";

const NO_CELLS: ReadonlyMap<number, BoardCell> = new Map();
const MAX_SEARCH_RESULTS = 24;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function hex4(code: number): string {
  return `0x${code.toString(16).toUpperCase().padStart(4, "0")}`;
}

interface LossyWrite {
  requested: number;
  stored: number;
}

interface KeymapPanelProps {
  client: ProtocolClient | null;
  capabilities: Capabilities | null;
  onStatus: (status: StatusUpdate) => void;
}

export function KeymapPanel({ client, capabilities, onStatus }: KeymapPanelProps) {
  const [layer, setLayer] = useState(0);
  /** layer → keycodes in flat grid order, as last read from the keyboard. */
  const [layers, setLayers] = useState<Map<number, number[]>>(new Map());
  /** Staged edits, keyed "layer:gridKey" → requested keycode. */
  const [pending, setPending] = useState<Map<string, number>>(new Map());
  /** Lossy write results, keyed "layer:gridKey". */
  const [lossy, setLossy] = useState<Map<string, LossyWrite>>(new Map());
  const [selectedGrid, setSelectedGrid] = useState<number | null>(null);
  const [entryText, setEntryText] = useState("");
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);

  const supported = !!capabilities && (capabilities.featureBits & FEATURE_KEYMAP) !== 0;
  const layerCount = capabilities?.layerCapacity ?? 8;
  const gridSize = capabilities ? capabilities.keymapRows * capabilities.keymapCols : 0;
  const cols = capabilities?.keymapCols ?? 14;

  // A new connection means a new keymap: drop everything cached or staged.
  useEffect(() => {
    setLayers(new Map());
    setPending(new Map());
    setLossy(new Map());
    setSelectedGrid(null);
    setLayer(0);
  }, [client]);

  const loadLayer = useCallback(
    async (target: number, announce: boolean) => {
      if (!client || !supported) return;
      setBusy(true);
      try {
        const keycodes = await client.readKeymapLayer(target);
        setLayers((current) => new Map(current).set(target, keycodes));
        setPending((current) => {
          const next = new Map(current);
          for (const key of next.keys()) {
            if (key.startsWith(`${target}:`)) next.delete(key);
          }
          return next;
        });
        setLossy((current) => {
          const next = new Map(current);
          for (const key of next.keys()) {
            if (key.startsWith(`${target}:`)) next.delete(key);
          }
          return next;
        });
        if (announce) {
          onStatus({ tone: "ok", message: `Layer ${target} reloaded from the keyboard` });
        }
      } catch (error) {
        onStatus({ tone: "error", message: errorMessage(error) });
      } finally {
        setBusy(false);
      }
    },
    [client, onStatus, supported],
  );

  // Lazily read each layer the first time it is shown.
  useEffect(() => {
    if (client && supported && !layers.has(layer)) void loadLayer(layer, false);
  }, [client, supported, layer, layers, loadLayer]);

  const stored = layers.get(layer);

  /** The keycode the board should show at a grid position: staged edit if
   * any, else the last value read from the keyboard. */
  const shownKeycode = useCallback(
    (grid: number): number | undefined => pending.get(`${layer}:${grid}`) ?? stored?.[grid],
    [layer, pending, stored],
  );

  const keyLabels = useMemo(() => {
    const labels = new Map<number, string>();
    for (const [grid, led] of GRID_TO_LED) {
      const code = shownKeycode(grid);
      labels.set(led, code === undefined ? "…" : formatKeycode(code));
    }
    return labels;
  }, [shownKeycode]);

  const pendingLeds = useMemo(() => {
    const leds = new Set<number>();
    for (const key of pending.keys()) {
      const [l, grid] = key.split(":").map(Number);
      if (l === layer) {
        const led = GRID_TO_LED.get(grid);
        if (led !== undefined) leds.add(led);
      }
    }
    return leds;
  }, [layer, pending]);

  const lossyLeds = useMemo(() => {
    const leds = new Set<number>();
    for (const key of lossy.keys()) {
      const [l, grid] = key.split(":").map(Number);
      if (l === layer) {
        const led = GRID_TO_LED.get(grid);
        if (led !== undefined) leds.add(led);
      }
    }
    return leds;
  }, [layer, lossy]);

  const selectKey = useCallback(
    (led: number) => {
      const grid = LED_TO_GRID.get(led);
      if (grid === undefined) return;
      setSelectedGrid(grid);
      const code = pending.get(`${layer}:${grid}`) ?? layers.get(layer)?.[grid];
      setEntryText(code === undefined ? "" : formatKeycode(code));
    },
    [layer, layers, pending],
  );

  const selectedLed = selectedGrid === null ? null : (GRID_TO_LED.get(selectedGrid) ?? null);
  const selectedSpec =
    selectedLed === null ? undefined : GLOVE80_KEYS.find((k) => k.ledIndex === selectedLed);
  const selectedStored = selectedGrid === null ? undefined : stored?.[selectedGrid];
  const selectedPending =
    selectedGrid === null ? undefined : pending.get(`${layer}:${selectedGrid}`);

  const parsedEntry = useMemo(() => {
    if (entryText.trim() === "") return null;
    try {
      return { code: parseKeycode(entryText), error: null };
    } catch (error) {
      return { code: null, error: error instanceof KeycodeError ? error.message : errorMessage(error) };
    }
  }, [entryText]);

  const searchResults = useMemo(
    () => (search.trim() === "" ? [] : searchKeycodes(search).slice(0, MAX_SEARCH_RESULTS)),
    [search],
  );

  const stageEntry = (code: number) => {
    if (selectedGrid === null) return;
    const key = `${layer}:${selectedGrid}`;
    setPending((current) => {
      const next = new Map(current);
      if (stored !== undefined && stored[selectedGrid] === code) {
        next.delete(key); // staging the current value = no edit
      } else {
        next.set(key, code);
      }
      return next;
    });
    setEntryText(formatKeycode(code));
  };

  const discardEdit = () => {
    if (selectedGrid === null) return;
    setPending((current) => {
      const next = new Map(current);
      next.delete(`${layer}:${selectedGrid}`);
      return next;
    });
    setEntryText(selectedStored === undefined ? "" : formatKeycode(selectedStored));
  };

  const writePending = async () => {
    if (!client || pending.size === 0) return;
    const entries: KeymapEntry[] = [...pending.entries()].map(([key, keycode]) => {
      const [entryLayer, entryKey] = key.split(":").map(Number);
      return { layer: entryLayer, key: entryKey, keycode };
    });
    setBusy(true);
    try {
      const readback = await client.writeKeymap(entries);
      const lossyWrites = new Map<string, LossyWrite>();
      setLayers((current) => {
        const next = new Map(current);
        entries.forEach((entry, index) => {
          const codes = next.get(entry.layer);
          if (codes) {
            const updated = [...codes];
            updated[entry.key] = readback[index];
            next.set(entry.layer, updated);
          }
        });
        return next;
      });
      entries.forEach((entry, index) => {
        if (readback[index] !== entry.keycode) {
          lossyWrites.set(`${entry.layer}:${entry.key}`, {
            requested: entry.keycode,
            stored: readback[index],
          });
        }
      });
      setPending(new Map());
      setLossy(lossyWrites);
      if (selectedGrid !== null) {
        const index = entries.findIndex((e) => e.layer === layer && e.key === selectedGrid);
        if (index >= 0) setEntryText(formatKeycode(readback[index]));
      }
      if (lossyWrites.size > 0) {
        onStatus({
          tone: "warn",
          message:
            `Wrote ${entries.length} key(s); ${lossyWrites.size} stored differently (LOSSY) — ` +
            "the firmware has no exact representation for those keycodes",
        });
      } else {
        onStatus({
          tone: "ok",
          message: `Wrote ${entries.length} key(s) — live on the keyboard now, and persisted`,
        });
      }
    } catch (error) {
      // KEYMAP_WRITE batches are all-or-nothing on the device; a failed
      // batch wrote nothing, so the staged edits stay staged.
      onStatus({ tone: "error", message: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  if (!client || !supported) {
    return (
      <section className="workspace">
        <div className="keymap-gate">
          {client
            ? "This keyboard does not advertise keymap editing (protocol feature bit 7). Update the firmware to a v1.2+ build."
            : "Connect a keyboard (or start demo mode) to read and edit its keymap."}
        </div>
      </section>
    );
  }

  const lossyEntries = [...lossy.entries()].filter(([key]) => key.startsWith(`${layer}:`));

  return (
    <section className="workspace">
      <aside className="tool-panel">
        <section>
          <div className="section-heading compact">
            <span className="step-number">01</span>
            <div>
              <h2>Layer</h2>
              <p>
                {capabilities.keymapRows}×{capabilities.keymapCols} grid · {layerCount} layers
              </p>
            </div>
          </div>
          <div className="layer-selector" role="tablist" aria-label="Keymap layers">
            {Array.from({ length: layerCount }, (_, index) => (
              <button
                key={index}
                role="tab"
                aria-selected={layer === index}
                className={layer === index ? "selected" : ""}
                onClick={() => {
                  setLayer(index);
                  setSelectedGrid(null);
                  setEntryText("");
                }}
              >
                {index}
              </button>
            ))}
          </div>
          <button
            className="button tool wide"
            disabled={busy}
            onClick={() => void loadLayer(layer, true)}
            title="KEYMAP_READ: re-read this layer from the live keymap (discards staged edits on it)"
          >
            Reload from keyboard
          </button>
        </section>

        <section>
          <div className="section-heading compact">
            <span className="step-number">02</span>
            <div>
              <h2>Binding</h2>
              <p>Click a key on the board to edit it</p>
            </div>
          </div>
          {selectedGrid === null || !selectedSpec ? (
            <p className="keymap-hint">No key selected.</p>
          ) : (
            <div className="binding-editor">
              <div className="binding-position">
                <strong>{selectedSpec.label}</strong>
                <small>
                  key {selectedGrid} · r{Math.floor(selectedGrid / cols)},c{selectedGrid % cols}
                  {KEYMAP_HOLES.includes(selectedGrid) ? " · hole" : ""}
                </small>
              </div>
              <div className="binding-current">
                <span>On keyboard</span>
                <strong>
                  {selectedStored === undefined
                    ? "…"
                    : `${formatKeycode(selectedStored)} (${hex4(selectedStored)})`}
                </strong>
              </div>
              {selectedPending !== undefined && (
                <div className="binding-current staged">
                  <span>Staged</span>
                  <strong>
                    {formatKeycode(selectedPending)} ({hex4(selectedPending)})
                  </strong>
                </div>
              )}
              <label className="binding-input">
                <span>Keycode — a name, MO(2), LT(1, KC_A), or hex like 0x0004</span>
                <input
                  value={entryText}
                  onChange={(event) => setEntryText(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && parsedEntry?.code !== null && parsedEntry) {
                      stageEntry(parsedEntry.code);
                    }
                  }}
                  placeholder="KC_A"
                  spellCheck={false}
                />
              </label>
              {parsedEntry && parsedEntry.error !== null && (
                <p className="binding-error">{parsedEntry.error}</p>
              )}
              <div className="tool-grid">
                <button
                  className="button tool"
                  disabled={!parsedEntry || parsedEntry.code === null}
                  onClick={() => parsedEntry?.code !== null && parsedEntry && stageEntry(parsedEntry.code)}
                >
                  Stage edit
                </button>
                <button
                  className="button tool"
                  disabled={selectedPending === undefined}
                  onClick={discardEdit}
                >
                  Discard
                </button>
              </div>
              <label className="binding-input">
                <span>Search keycodes</span>
                <input
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="play, shift, boot…"
                  spellCheck={false}
                />
              </label>
              {searchResults.length > 0 && (
                <ul className="keycode-results">
                  {searchResults.map((result) => (
                    <li key={result.code}>
                      <button onClick={() => stageEntry(result.code)}>
                        <strong>{result.name}</strong>
                        <small>
                          {hex4(result.code)}
                          {result.aliases.length > 0 ? ` · ${result.aliases.join(", ")}` : ""}
                        </small>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </section>

        <section className="scene-tools">
          <div className="section-heading compact">
            <span className="step-number">03</span>
            <div>
              <h2>Write</h2>
              <p>Batched KEYMAP_WRITE with canonical read-back</p>
            </div>
          </div>
          {lossyEntries.length > 0 && (
            <ul className="lossy-list">
              {lossyEntries.map(([key, write]) => (
                <li key={key}>
                  LOSSY · key {key.split(":")[1]}: wrote {formatKeycode(write.requested)}, stored{" "}
                  {formatKeycode(write.stored)} ({hex4(write.stored)})
                </li>
              ))}
            </ul>
          )}
          <button
            className="button apply"
            disabled={pending.size === 0 || busy}
            onClick={() => void writePending()}
            title="All-or-nothing per batch; the firmware echoes what it actually stored"
          >
            {pending.size === 0 ? "No staged edits" : `Write ${pending.size} change${pending.size === 1 ? "" : "s"}`}
          </button>
          <p className="keymap-hint">
            Writes change the live keymap immediately — no reboot — and persist. Same store as
            Vial: edits made here show up in Vial and vice versa.
          </p>
        </section>
      </aside>

      <section className="keyboard-stage" aria-label="Keymap editor">
        <Board
          cells={NO_CELLS}
          keyLabels={keyLabels}
          selectedKey={selectedLed}
          pendingKeys={pendingLeds}
          flaggedKeys={lossyLeds}
          onPaintKey={selectKey}
          caption={`Layer ${layer} — bindings as stored on the keyboard (VIA keycodes). Dashed = staged, red = lossy write. Grid holes are not shown; they always read KC_NO.`}
        />
      </section>
    </section>
  );
}
