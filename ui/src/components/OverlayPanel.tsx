// Live host-overlay panel: paint RAM-only cells on the connected keyboard.
//
// Every edit goes straight to the device (SET_CELLS / UNSET_CELLS batched per
// drag). "Sync from keyboard" and "Push my state" are the explicit
// READ_OVERLAY / REPLACE_OVERLAY reconciliation primitives; PARTIAL_APPLY
// answers mark the affected keys as pending instead of pretending they lit.

import { useCallback, useEffect, useRef, useState } from "react";

import { brushToEffect, type Brush } from "../lib/brush";
import {
  FEATURE_OVERLAY_READBACK,
  FEATURE_TTL,
  type Capabilities,
  type CellState,
  type CellWrite,
  type Effect,
} from "../lib/host-protocol";
import type { OverlayWriteResult } from "../lib/protocol-client";
import { CHANNEL_CEILING } from "../lib/compositor-preview";
import { Board, type BoardCell } from "./Board";
import { BrushControls } from "./BrushControls";

const FLUSH_DELAY_MS = 40;

interface LocalCell {
  effect: Effect;
  /** Local clock time at which the firmware will expire the cell; null = no TTL. */
  expiresAt: number | null;
}

export interface StatusUpdate {
  tone: "idle" | "busy" | "ok" | "warn" | "error";
  message: string;
}

export interface OverlayClient {
  readonly supportsOverlayReadback?: boolean;
  readOverlay?(): Promise<CellState[]>;
  getBrightness(): Promise<number>;
  setBrightness(level: number): Promise<number>;
  setCells(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult>;
  unsetCells(keys: number[]): Promise<OverlayWriteResult>;
  clearOverlay(): Promise<OverlayWriteResult>;
  replaceOverlay(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult>;
}

interface OverlayPanelProps {
  client: OverlayClient | null;
  capabilities: Capabilities | null;
  brush: Brush;
  onBrushChange: (brush: Brush) => void;
  onStatus: (status: StatusUpdate) => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function OverlayPanel({ client, capabilities, brush, onBrushChange, onStatus }: OverlayPanelProps) {
  const [cells, setCells] = useState(new Map<number, LocalCell>());
  const [pending, setPending] = useState(new Set<number>());
  const [ttlSeconds, setTtlSeconds] = useState(0);
  const [brightness, setBrightness] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const queue = useRef(new Map<number, CellWrite | null>()); // null = erase
  const flushTimer = useRef<number | undefined>(undefined);
  const flushing = useRef(false);
  const ttlRef = useRef(ttlSeconds);
  ttlRef.current = ttlSeconds;

  const supportsTtl = !capabilities || (capabilities.featureBits & FEATURE_TTL) !== 0;
  const supportsReadback =
    !!client?.readOverlay &&
    client.supportsOverlayReadback !== false &&
    (!capabilities || (capabilities.featureBits & FEATURE_OVERLAY_READBACK) !== 0);
  const maxPerOp = capabilities?.maxCellsPerOp ?? 40;

  const mergeAck = useCallback((result: OverlayWriteResult, writtenKeys: number[]) => {
    setPending((current) => {
      const next = new Set(current);
      for (const key of writtenKeys) next.delete(key);
      for (const key of result.pendingKeys) next.add(key);
      return next;
    });
    if (result.partial) {
      onStatus({
        tone: "warn",
        message:
          "Applied on the left half; the right half is offline — marked keys apply when it reconnects",
      });
    }
  }, [onStatus]);

  const syncFromKeyboard = useCallback(async (announce: boolean) => {
    if (!client) return;
    try {
      const level = await client.getBrightness();
      const overlay = client.readOverlay ? await client.readOverlay() : null;
      setBrightness(level);
      if (!overlay) {
        if (announce) {
          onStatus({
            tone: "ok",
            message: "Authoritative Rynk lighting state refreshed · overlay contents stay local",
          });
        }
        return;
      }
      const now = Date.now();
      setCells(
        new Map(
          overlay.map((cell) => [
            cell.key,
            { effect: cell.effect, expiresAt: cell.remainingTtlMs > 0 ? now + cell.remainingTtlMs : null },
          ]),
        ),
      );
      setPending(new Set());
      if (announce) {
        onStatus({ tone: "ok", message: `Overlay synced from the keyboard · ${overlay.length} lit keys` });
      }
    } catch (error) {
      onStatus({ tone: "error", message: errorMessage(error) });
    }
  }, [client, onStatus]);

  // Hydrate from the device on connect; forget device state on disconnect.
  useEffect(() => {
    queue.current.clear();
    if (flushTimer.current !== undefined) {
      window.clearTimeout(flushTimer.current);
      flushTimer.current = undefined;
    }
    if (!client) {
      setCells(new Map());
      setPending(new Set());
      setBrightness(null);
      return;
    }
    void syncFromKeyboard(false);
  }, [client, supportsReadback, syncFromKeyboard]);

  // Tick down TTL cells so the board matches what the firmware will do.
  useEffect(() => {
    if (![...cells.values()].some((cell) => cell.expiresAt !== null)) return;
    const interval = window.setInterval(() => {
      setCells((current) => {
        const now = Date.now();
        if (![...current.values()].some((cell) => cell.expiresAt !== null && cell.expiresAt <= now)) {
          return new Map(current); // re-render for the countdown notes
        }
        const next = new Map<number, LocalCell>();
        for (const [key, cell] of current) {
          if (cell.expiresAt === null || cell.expiresAt > now) next.set(key, cell);
        }
        return next;
      });
    }, 1000);
    return () => window.clearInterval(interval);
  }, [cells]);

  const flush = useCallback(async () => {
    flushTimer.current = undefined;
    if (!client || flushing.current || queue.current.size === 0) return;
    flushing.current = true;
    const batch = [...queue.current];
    queue.current.clear();
    const writes = batch.filter((entry): entry is [number, CellWrite] => entry[1] !== null).map(([, w]) => w);
    const erases = batch.filter(([, w]) => w === null).map(([key]) => key);
    try {
      for (let offset = 0; offset < writes.length; offset += maxPerOp) {
        const chunk = writes.slice(offset, offset + maxPerOp);
        mergeAck(await client.setCells(Math.round(ttlRef.current * 1000), chunk), chunk.map((c) => c.key));
      }
      for (let offset = 0; offset < erases.length; offset += maxPerOp) {
        const chunk = erases.slice(offset, offset + maxPerOp);
        mergeAck(await client.unsetCells(chunk), chunk);
      }
    } catch (error) {
      onStatus({ tone: "error", message: errorMessage(error) });
    } finally {
      flushing.current = false;
      if (queue.current.size > 0 && flushTimer.current === undefined) {
        flushTimer.current = window.setTimeout(() => void flush(), FLUSH_DELAY_MS);
      }
    }
  }, [client, maxPerOp, mergeAck, onStatus]);

  const paintKey = useCallback(
    (key: number) => {
      if (!client) return;
      if (brush.mode === "erase") {
        setCells((current) => {
          if (!current.has(key)) return current;
          const next = new Map(current);
          next.delete(key);
          return next;
        });
        queue.current.set(key, null);
      } else {
        const effect = brushToEffect(brush);
        const expiresAt = supportsTtl && ttlRef.current > 0 ? Date.now() + ttlRef.current * 1000 : null;
        setCells((current) => new Map(current).set(key, { effect, expiresAt }));
        queue.current.set(key, { key, effect });
      }
      if (flushTimer.current === undefined) {
        flushTimer.current = window.setTimeout(() => void flush(), FLUSH_DELAY_MS);
      }
    },
    [brush, client, flush, supportsTtl],
  );

  const pushMyState = async () => {
    if (!client) return;
    setBusy(true);
    onStatus({ tone: "busy", message: "Replacing the keyboard overlay with this canvas…" });
    try {
      const writes: CellWrite[] = [...cells.entries()]
        .sort(([a], [b]) => a - b)
        .map(([key, cell]) => ({ key, effect: cell.effect }));
      const ttlMs = supportsTtl ? Math.round(ttlSeconds * 1000) : 0;
      // REPLACE_OVERLAY is atomic but bounded by max_cells_per_op; a canvas
      // beyond that is replaced with the first chunk and merged up with
      // SET_CELLS for the rest.
      const first = writes.slice(0, maxPerOp);
      mergeAck(await client.replaceOverlay(ttlMs, first), first.map((c) => c.key));
      for (let offset = maxPerOp; offset < writes.length; offset += maxPerOp) {
        const chunk = writes.slice(offset, offset + maxPerOp);
        mergeAck(await client.setCells(ttlMs, chunk), chunk.map((c) => c.key));
      }
      const expiresAt = ttlMs > 0 ? Date.now() + ttlMs : null;
      setCells(new Map(writes.map((w) => [w.key, { effect: w.effect, expiresAt }])));
      onStatus({ tone: "ok", message: `Keyboard overlay replaced · ${writes.length} lit keys` });
    } catch (error) {
      onStatus({ tone: "error", message: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  const clearAll = async () => {
    if (!client) return;
    setBusy(true);
    try {
      const result = await client.clearOverlay();
      setCells(new Map());
      setPending(new Set());
      if (result.partial) {
        onStatus({ tone: "warn", message: "Cleared on the left half; the right half clears when it reconnects" });
      } else {
        onStatus({ tone: "ok", message: "Overlay cleared · firmware lighting shows through" });
      }
    } catch (error) {
      onStatus({ tone: "error", message: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  const applyBrightness = async (level: number) => {
    if (!client) return;
    try {
      const applied = await client.setBrightness(level);
      setBrightness(applied);
      onStatus({ tone: "ok", message: `Brightness set to ${applied}/255` });
    } catch (error) {
      onStatus({ tone: "error", message: errorMessage(error) });
    }
  };

  const boardCells = new Map<number, BoardCell>();
  const now = Date.now();
  for (const [key, cell] of cells) {
    boardCells.set(key, {
      effect: cell.effect,
      note: cell.expiresAt === null ? undefined : `TTL ${Math.max(0, Math.ceil((cell.expiresAt - now) / 1000))}s`,
    });
  }

  return (
    <section className="workspace">
      <aside className="tool-panel">
        <BrushControls brush={brush} onChange={onBrushChange} capabilities={capabilities} />

        <section>
          <div className="section-heading compact">
            <span className="step-number">03</span>
            <div>
              <h2>Time to live</h2>
              <p>{supportsTtl ? "Firmware reverts the cells when it expires" : "Not supported by this keyboard"}</p>
            </div>
          </div>
          <label className="range-control">
            <span>TTL</span>
            <strong>{ttlSeconds === 0 ? "none" : `${ttlSeconds}s`}</strong>
            <input
              type="range"
              min="0"
              max="600"
              step="5"
              value={ttlSeconds}
              disabled={!supportsTtl}
              onChange={(event) => setTtlSeconds(Number(event.target.value))}
            />
          </label>
        </section>

        <section>
          <div className="section-heading compact">
            <span className="step-number">04</span>
            <div>
              <h2>Brightness</h2>
              <p>Global scalar under the firmware safety cap</p>
            </div>
          </div>
          <label className="range-control">
            <span>Level</span>
            <strong>{brightness === null ? "—" : `${brightness}/255`}</strong>
            <input
              type="range"
              min="0"
              max="255"
              value={brightness ?? 255}
              disabled={!client}
              onChange={(event) => setBrightness(Number(event.target.value))}
              onPointerUp={(event) => void applyBrightness(Number((event.target as HTMLInputElement).value))}
            />
          </label>
          <p className="ceiling-note">
            Effective ceiling: <strong>{CHANNEL_CEILING}/255</strong> (80%) — the compile-time
            safety cap (MoErgo's LED current limit). The firmware can lower it at runtime, but the
            protocol exposes no ceiling command yet, so runtime ceiling control is pending protocol
            support.
          </p>
        </section>

        <section className="scene-tools">
          <div className="section-heading compact">
            <span className="step-number">05</span>
            <div>
              <h2>Reconcile</h2>
              <p>Explicit sync in either direction</p>
            </div>
          </div>
          <div className="tool-grid">
            <button
              className="button tool"
              disabled={!client || !supportsReadback || busy}
              onClick={() => void syncFromKeyboard(true)}
              title="READ_OVERLAY: adopt whatever the keyboard is showing"
            >
              Sync from keyboard
            </button>
            <button
              className="button tool"
              disabled={!client || busy}
              onClick={() => void pushMyState()}
              title="REPLACE_OVERLAY: make the keyboard match this canvas exactly"
            >
              Push my state
            </button>
          </div>
          <button className="button apply" disabled={!client || busy} onClick={() => void clearAll()}>
            Clear overlay
          </button>
        </section>
      </aside>

      <section className="keyboard-stage" aria-label="Live overlay canvas">
        <Board
          cells={boardCells}
          pendingKeys={pending}
          onPaintKey={client ? paintKey : undefined}
          caption={
            client
              ? "Every stroke is written to the keyboard immediately. RAM-only: a reboot or Clear removes it."
              : "Connect a keyboard (or the demo) to paint the live overlay."
          }
        />
      </section>
    </section>
  );
}
