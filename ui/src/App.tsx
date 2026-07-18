import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  colorsByLed,
  GLOVE80_KEYS,
  mirrorLeftToRight,
  type KeySpec,
} from "./lib/glove80-layout";
import type { LightingClient } from "./lib/lighting-client";
import { connectLighting, transportSupported, type TransportKind } from "./lib/transports";

const STORAGE_KEY = "glove80-lightbench-scene-v1";
const FRAME_TIMEOUT_MS = 10_000;
const KEEPALIVE_MS = 5_000;
const EMPTY_SCENE = Array<number>(80).fill(0);
const PALETTE = [
  0xf05d3e, 0xf5a524, 0xf4d35e, 0x6ecb63, 0x48cae4, 0x4d7cff, 0x9b6cff, 0xe56bdb,
  0xffffff, 0x5d6460, 0x151716, 0x000000,
];

type Status = {
  tone: "idle" | "busy" | "ok" | "error";
  message: string;
};

function rgbToHex(rgb: number): string {
  return `#${rgb.toString(16).padStart(6, "0")}`;
}

function parseHex(value: string): number | undefined {
  const match = /^#?([0-9a-f]{6})$/i.exec(value.trim());
  return match ? Number.parseInt(match[1], 16) : undefined;
}

function withBrightness(rgb: number, brightness: number): number {
  const scale = brightness / 100;
  return (
    (Math.round(((rgb >>> 16) & 0xff) * scale) << 16) |
    (Math.round(((rgb >>> 8) & 0xff) * scale) << 8) |
    Math.round((rgb & 0xff) * scale)
  );
}

function loadScene(): number[] {
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (
      Array.isArray(stored) &&
      stored.length === 80 &&
      stored.every((value) => Number.isInteger(value) && value >= 0 && value <= 0xffffff)
    ) {
      return stored;
    }
  } catch {
    // A malformed local scene should never prevent the editor from opening.
  }
  return [...EMPTY_SCENE];
}

function connectionError(error: unknown): string {
  if (error instanceof DOMException && error.name === "NotFoundError") {
    return "Connection cancelled";
  }
  return error instanceof Error ? error.message : String(error);
}

export function App() {
  const [colors, setColors] = useState(loadScene);
  const [paintColor, setPaintColor] = useState(0x48cae4);
  const [hexDraft, setHexDraft] = useState(rgbToHex(paintColor));
  const [brightness, setBrightness] = useState(100);
  const [client, setClient] = useState<LightingClient | null>(null);
  const [connecting, setConnecting] = useState<TransportKind | null>(null);
  const [status, setStatus] = useState<Status>({
    tone: "idle",
    message: "Design offline, then connect when you are ready",
  });
  const colorsRef = useRef(colors);
  const brightnessRef = useRef(brightness);
  const pendingPixels = useRef(new Map<number, number>());
  const pendingTimer = useRef<number | undefined>(undefined);
  const painting = useRef(false);

  useEffect(() => {
    colorsRef.current = colors;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(colors));
  }, [colors]);

  useEffect(() => {
    brightnessRef.current = brightness;
  }, [brightness]);

  const deviceColors = useCallback((source: readonly number[] = colorsRef.current) => {
    return colorsByLed(
      GLOVE80_KEYS,
      source.map((rgb) => withBrightness(rgb, brightnessRef.current)),
    );
  }, []);

  const flushPending = useCallback(async () => {
    pendingTimer.current = undefined;
    if (!client || pendingPixels.current.size === 0) return;
    const updates = [...pendingPixels.current].map(([index, rgb]) => ({
      index,
      rgb: withBrightness(rgb, brightnessRef.current),
    })).filter(({ index }) => index < client.capabilities.pixelCount);
    pendingPixels.current.clear();
    const chunkSize = client.capabilities.maxUpdatesPerRequest;
    try {
      for (let offset = 0; offset < updates.length; offset += chunkSize) {
        await client.setPixels(updates.slice(offset, offset + chunkSize), false, FRAME_TIMEOUT_MS);
      }
      setStatus({ tone: "ok", message: "Keyboard updated" });
    } catch (error) {
      setStatus({ tone: "error", message: connectionError(error) });
    }
  }, [client]);

  const queuePixel = useCallback(
    (keySpec: KeySpec, rgb: number) => {
      if (!client) return;
      pendingPixels.current.set(keySpec.ledIndex, rgb);
      if (pendingTimer.current === undefined) {
        pendingTimer.current = window.setTimeout(flushPending, 50);
      }
    },
    [client, flushPending],
  );

  const paintKey = useCallback(
    (keySpec: KeySpec) => {
      setColors((current) => {
        if (current[keySpec.logicalIndex] === paintColor) return current;
        const next = [...current];
        next[keySpec.logicalIndex] = paintColor;
        return next;
      });
      queuePixel(keySpec, paintColor);
    },
    [paintColor, queuePixel],
  );

  useEffect(() => {
    const stopPainting = () => {
      painting.current = false;
    };
    window.addEventListener("pointerup", stopPainting);
    window.addEventListener("pointercancel", stopPainting);
    return () => {
      window.removeEventListener("pointerup", stopPainting);
      window.removeEventListener("pointercancel", stopPainting);
    };
  }, []);

  useEffect(() => {
    if (!client) return;
    const interval = window.setInterval(() => {
      const frame = deviceColors();
      const keepalive = [{ index: 0, rgb: frame[0] }];
      if (client.capabilities.supportsSplit && client.capabilities.pixelCount > 40) {
        keepalive.push({ index: 40, rgb: frame[40] });
      }
      client
        .setPixels(keepalive, false, FRAME_TIMEOUT_MS)
        .catch((error) => setStatus({ tone: "error", message: connectionError(error) }));
    }, KEEPALIVE_MS);
    return () => window.clearInterval(interval);
  }, [client, deviceColors]);

  useEffect(() => {
    return () => {
      if (pendingTimer.current !== undefined) window.clearTimeout(pendingTimer.current);
      client?.close().catch(() => undefined);
    };
  }, [client]);

  const connect = async (kind: TransportKind) => {
    setConnecting(kind);
    setStatus({ tone: "busy", message: `Waiting for a ${kind.toUpperCase()} device…` });
    try {
      const connected = await connectLighting(kind);
      setClient(connected);
      setStatus({
        tone: "ok",
        message: `Connected · protocol v${connected.capabilities.protocolVersion}`,
      });
    } catch (error) {
      setStatus({ tone: "error", message: connectionError(error) });
    } finally {
      setConnecting(null);
    }
  };

  const disconnect = async () => {
    if (!client) return;
    try {
      await client.clear();
    } finally {
      await client.close();
      setClient(null);
      setStatus({ tone: "idle", message: "Disconnected · firmware lighting restored" });
    }
  };

  const applyScene = async (nextColors: readonly number[] = colors) => {
    if (!client) {
      setStatus({ tone: "idle", message: "Scene saved locally · connect to apply it" });
      return;
    }
    setStatus({ tone: "busy", message: "Sending complete scene…" });
    try {
      await client.applyFrame(deviceColors(nextColors), FRAME_TIMEOUT_MS);
      setStatus({ tone: "ok", message: "Complete scene applied" });
    } catch (error) {
      setStatus({ tone: "error", message: connectionError(error) });
    }
  };

  const updateAll = (rgb: number) => {
    const next = Array<number>(80).fill(rgb);
    setColors(next);
    void applyScene(next);
  };

  const mirror = () => {
    const next = mirrorLeftToRight(colors);
    setColors(next);
    void applyScene(next);
  };

  const releaseLighting = async () => {
    if (!client) return;
    try {
      await client.clear();
      setStatus({ tone: "idle", message: "Host override released · firmware lighting restored" });
    } catch (error) {
      setStatus({ tone: "error", message: connectionError(error) });
    }
  };

  const setSelectedColor = (rgb: number) => {
    setPaintColor(rgb);
    setHexDraft(rgbToHex(rgb));
  };

  const connectedLabel = useMemo(() => {
    if (!client) return "No keyboard";
    return client.label || "Glove80";
  }, [client]);

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-block">
          <span className="eyebrow">Glove80 tools</span>
          <h1>Lightbench</h1>
          <p>Paint the keyboard itself. No daemon required.</p>
        </div>
        <div className="connection-cluster">
          <div className={`connection-readout ${client ? "connected" : ""}`}>
            <span className="status-dot" aria-hidden="true" />
            <span>
              <strong>{connectedLabel}</strong>
              <small>{client ? "Live connection" : "Offline editor"}</small>
            </span>
          </div>
          {client ? (
            <button className="button subtle" onClick={() => void disconnect()}>
              Disconnect
            </button>
          ) : (
            <div className="connect-actions">
              <button
                className="button primary"
                disabled={!transportSupported("usb") || connecting !== null}
                onClick={() => void connect("usb")}
              >
                {connecting === "usb" ? "Connecting…" : "Connect USB"}
              </button>
              <button
                className="button subtle"
                disabled={!transportSupported("ble") || connecting !== null}
                onClick={() => void connect("ble")}
                title={transportSupported("ble") ? "Connect with Web Bluetooth" : "Web Bluetooth is unavailable in this browser"}
              >
                {connecting === "ble" ? "Connecting…" : "Connect BLE"}
              </button>
            </div>
          )}
        </div>
      </header>

      <section className="workspace">
        <aside className="tool-panel">
          <section>
            <div className="section-heading">
              <span className="step-number">01</span>
              <div>
                <h2>Paint color</h2>
                <p>Click or drag across keys</p>
              </div>
            </div>
            <div className="color-control">
              <label className="native-color" style={{ background: rgbToHex(paintColor) }}>
                <input
                  type="color"
                  value={rgbToHex(paintColor)}
                  onChange={(event) => setSelectedColor(Number.parseInt(event.target.value.slice(1), 16))}
                  aria-label="Choose paint color"
                />
              </label>
              <label className="hex-field">
                <span>HEX</span>
                <input
                  value={hexDraft}
                  spellCheck={false}
                  onChange={(event) => {
                    setHexDraft(event.target.value);
                    const parsed = parseHex(event.target.value);
                    if (parsed !== undefined) setPaintColor(parsed);
                  }}
                  onBlur={() => setHexDraft(rgbToHex(paintColor))}
                  aria-label="Paint color hexadecimal value"
                />
              </label>
            </div>
            <div className="palette" aria-label="Color palette">
              {PALETTE.map((rgb) => (
                <button
                  key={rgb}
                  className={`swatch ${rgb === paintColor ? "selected" : ""}`}
                  style={{ background: rgbToHex(rgb) }}
                  onClick={() => setSelectedColor(rgb)}
                  aria-label={`Use color ${rgbToHex(rgb)}`}
                  aria-pressed={rgb === paintColor}
                />
              ))}
            </div>
          </section>

          <section>
            <div className="section-heading compact">
              <span className="step-number">02</span>
              <div>
                <h2>Output</h2>
                <p>Within the firmware safety cap</p>
              </div>
            </div>
            <label className="range-control">
              <span>Brightness</span>
              <strong>{brightness}%</strong>
              <input
                type="range"
                min="0"
                max="100"
                value={brightness}
                onChange={(event) => setBrightness(Number(event.target.value))}
                onPointerUp={() => void applyScene()}
              />
            </label>
          </section>

          <section className="scene-tools">
            <div className="section-heading compact">
              <span className="step-number">03</span>
              <div>
                <h2>Scene tools</h2>
                <p>Canvas is saved in this browser</p>
              </div>
            </div>
            <div className="tool-grid">
              <button className="button tool" onClick={() => updateAll(paintColor)}>Fill all</button>
              <button className="button tool" onClick={mirror}>Mirror L → R</button>
              <button className="button tool" onClick={() => updateAll(0)}>Black out</button>
              <button className="button tool" disabled={!client} onClick={() => void releaseLighting()}>Release</button>
            </div>
            <button className="button apply" disabled={!client} onClick={() => void applyScene()}>
              Apply complete scene
            </button>
          </section>
        </aside>

        <section className="keyboard-stage" aria-label="Glove80 lighting canvas">
          <div className="stage-heading">
            <div>
              <span className="eyebrow">80 individually addressable keys</span>
              <h2>Lighting canvas</h2>
            </div>
            <div className={`operation-status ${status.tone}`} role="status" aria-live="polite">
              <span className="status-dot" aria-hidden="true" />
              {status.message}
            </div>
          </div>

          <div className="keyboard-scroll">
            <div className="keyboard-map" onDragStart={(event) => event.preventDefault()}>
              <div className="half-label left">Left</div>
              <div className="half-label right">Right</div>
              <div className="center-mark" aria-hidden="true"><span /></div>
              {GLOVE80_KEYS.map((keySpec) => {
                const rgb = colors[keySpec.logicalIndex];
                const color = rgbToHex(rgb);
                return (
                  <button
                    key={keySpec.logicalIndex}
                    className={`keycap ${keySpec.kind}`}
                    style={{
                      "--key-x": keySpec.x,
                      "--key-y": keySpec.y,
                      "--key-color": color,
                    } as React.CSSProperties}
                    onPointerDown={(event) => {
                      if (event.button !== 0) return;
                      painting.current = true;
                      paintKey(keySpec);
                      event.preventDefault();
                    }}
                    onPointerEnter={(event) => {
                      if (painting.current && (event.buttons & 1) === 1) paintKey(keySpec);
                    }}
                    title={`${keySpec.label} · LED ${keySpec.ledIndex} · ${color}`}
                    aria-label={`Set ${keySpec.label} to ${rgbToHex(paintColor)}`}
                  >
                    <span className="key-light" aria-hidden="true" />
                    <span className="key-label">{keySpec.label}</span>
                    <small>{keySpec.ledIndex}</small>
                  </button>
                );
              })}
            </div>
          </div>

          <footer className="stage-footer">
            <span>Live frames are temporary and never written to keyboard flash.</span>
            <span>Disconnecting restores normal firmware lighting automatically.</span>
          </footer>
        </section>
      </section>
    </main>
  );
}
