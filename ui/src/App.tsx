// Lightbench: the browser workbench for the Glove80 host protocol.
//
// Two panels over one connection: the live RAM-only host overlay and the
// persistent boot config, both painted on the same board rendering. The
// connection bar speaks WebHID (USB) and Web Bluetooth, and a mock keyboard
// provides a full demo mode when no hardware is around.

import { useEffect, useState } from "react";

import { ConfigPanel } from "./components/ConfigPanel";
import { KeymapPanel } from "./components/KeymapPanel";
import { OverlayPanel, type StatusUpdate } from "./components/OverlayPanel";
import { DEFAULT_BRUSH, type Brush } from "./lib/brush";
import {
  FEATURE_ATOMIC_REPLACE,
  FEATURE_BOOTLOADER_ENTRY,
  FEATURE_KEYMAP,
  FEATURE_OVERLAY_READBACK,
  FEATURE_PARTIAL_APPLY,
  FEATURE_PERSISTENT_CONFIG,
  FEATURE_TOGGLES,
  FEATURE_TTL,
  FEATURE_VERSION,
  gitHashText,
  type Capabilities,
  type HalfVersion,
  type VersionInfo,
} from "./lib/host-protocol";
import { createDemoKeyboard, MockTransport } from "./lib/mock-device";
import { ProtocolClient } from "./lib/protocol-client";
import type { Transport, TransportKind } from "./lib/transport";
import { connectWebBluetooth, webBluetoothSupported } from "./lib/webbluetooth-transport";
import { connectWebHid, webHidSupported } from "./lib/webhid-transport";

type PanelName = "overlay" | "config" | "keymap";

const FEATURE_NAMES: Array<[number, string]> = [
  [FEATURE_TTL, "TTL"],
  [FEATURE_TOGGLES, "toggles"],
  [FEATURE_BOOTLOADER_ENTRY, "bootloader"],
  [FEATURE_ATOMIC_REPLACE, "atomic replace"],
  [FEATURE_OVERLAY_READBACK, "read-back"],
  [FEATURE_PARTIAL_APPLY, "partial-apply"],
  [FEATURE_PERSISTENT_CONFIG, "persistent config"],
  [FEATURE_KEYMAP, "keymap"],
  [FEATURE_VERSION, "version"],
];

function describeHalf(half: HalfVersion): string {
  const hash = gitHashText(half.gitHashHex) || "????????";
  return `v${half.fwMajor}.${half.fwMinor}.${half.fwPatch} @${hash}${half.dirty ? "+dirty" : ""}`;
}

/** The peripheral entry with present=false and all-zero fields means the
 * central has not heard from it since boot (a real state — the split link
 * may simply not have synced a version yet). */
function peripheralNeverSeen(half: HalfVersion): boolean {
  return (
    !half.present &&
    half.fwMajor === 0 &&
    half.fwMinor === 0 &&
    half.fwPatch === 0 &&
    gitHashText(half.gitHashHex) === ""
  );
}

function VersionReadout({ version }: { version: VersionInfo }) {
  const { central, peripheral, halvesMismatch } = version;
  if (halvesMismatch) {
    return (
      <div className="version-readout mismatch" role="alert">
        <strong>Halves mismatch</strong>
        <small>
          left {describeHalf(central)} · right {describeHalf(peripheral)} — one half runs stale
          firmware; reflash it
        </small>
      </div>
    );
  }
  if (peripheralNeverSeen(peripheral)) {
    return (
      <div className="version-readout partial">
        <strong>{describeHalf(central)}</strong>
        <small>right half: no version reported since boot</small>
      </div>
    );
  }
  if (!peripheral.present) {
    return (
      <div className="version-readout partial">
        <strong>{describeHalf(central)}</strong>
        <small>right half offline · last known {describeHalf(peripheral)}</small>
      </div>
    );
  }
  return (
    <div className="version-readout">
      <strong>{describeHalf(central)}</strong>
      <small>both halves match</small>
    </div>
  );
}

function describeCapabilities(caps: Capabilities): string {
  const features = FEATURE_NAMES.filter(([bit]) => (caps.featureBits & bit) !== 0).map(([, name]) => name);
  return [
    `protocol v${caps.protocolMajor}.${caps.protocolMinor}`,
    `${caps.ledCountLeft + caps.ledCountRight} LEDs`,
    features.join(", "),
  ].join(" · ");
}

function connectionError(error: unknown): string {
  if (error instanceof DOMException && (error.name === "NotFoundError" || error.name === "NotAllowedError")) {
    return "Connection cancelled";
  }
  return error instanceof Error ? error.message : String(error);
}

export function App() {
  const [panel, setPanel] = useState<PanelName>("overlay");
  const [brush, setBrush] = useState<Brush>(DEFAULT_BRUSH);
  const [client, setClient] = useState<ProtocolClient | null>(null);
  const [capabilities, setCapabilities] = useState<Capabilities | null>(null);
  const [version, setVersion] = useState<VersionInfo | null>(null);
  const [connecting, setConnecting] = useState<TransportKind | null>(null);
  const [status, setStatus] = useState<StatusUpdate>({
    tone: "idle",
    message: "Connect a keyboard — or explore in demo mode",
  });

  useEffect(() => {
    return () => {
      client?.close().catch(() => undefined);
    };
  }, [client]);

  const connect = async (kind: TransportKind) => {
    setConnecting(kind);
    setStatus({
      tone: "busy",
      message: kind === "demo" ? "Starting the demo keyboard…" : `Waiting for a ${kind.toUpperCase()} device…`,
    });
    let transport: Transport | null = null;
    try {
      transport =
        kind === "usb"
          ? await connectWebHid()
          : kind === "ble"
            ? await connectWebBluetooth()
            : new MockTransport(createDemoKeyboard());
      const nextClient = new ProtocolClient(transport);
      // GET_CAPABILITIES is mandatory before anything else; the UI trusts
      // only what the keyboard advertises here.
      const caps = await nextClient.connect();
      // GET_VERSION right after the handshake, when the firmware offers it.
      // Failure is non-fatal: the connection is useful without it.
      let versionInfo: VersionInfo | null = null;
      if ((caps.featureBits & FEATURE_VERSION) !== 0) {
        versionInfo = await nextClient.getVersion().catch(() => null);
      }
      nextClient.onDisconnect(() => {
        setClient(null);
        setCapabilities(null);
        setVersion(null);
        setStatus({ tone: "error", message: "Connection lost — the keyboard went away" });
      });
      setClient(nextClient);
      setCapabilities(caps);
      setVersion(versionInfo);
      setStatus({
        tone: "ok",
        message:
          kind === "demo"
            ? `Demo keyboard · ${describeCapabilities(caps)}`
            : `Connected · ${describeCapabilities(caps)}`,
      });
    } catch (error) {
      if (transport) await transport.close().catch(() => undefined);
      setStatus({ tone: "error", message: connectionError(error) });
    } finally {
      setConnecting(null);
    }
  };

  const disconnect = async () => {
    if (!client) return;
    // Deliberately no clear here: host-overlay cells without TTL survive
    // until an explicit clear or reboot, and a disconnect must never change
    // how the keyboard looks (docs/lighting-design.md).
    await client.close().catch(() => undefined);
    setClient(null);
    setCapabilities(null);
    setVersion(null);
    setStatus({ tone: "idle", message: "Disconnected — the keyboard keeps whatever it was showing" });
  };

  const demo = client?.transport.kind === "demo";

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
              <strong>{client ? client.transport.label : "No keyboard"}</strong>
              <small>
                {client && capabilities
                  ? `${client.transport.kind.toUpperCase()} · ${describeCapabilities(capabilities)}`
                  : "Offline editor"}
              </small>
            </span>
          </div>
          {client && version && <VersionReadout version={version} />}
          {client ? (
            <button className="button subtle" onClick={() => void disconnect()}>
              Disconnect
            </button>
          ) : (
            <div className="connect-actions">
              <button
                className="button primary"
                disabled={!webHidSupported() || connecting !== null}
                onClick={() => void connect("usb")}
                title={webHidSupported() ? "Connect over WebHID" : "WebHID is unavailable in this browser"}
              >
                {connecting === "usb" ? "Connecting…" : "Connect USB"}
              </button>
              <button
                className="button subtle"
                disabled={!webBluetoothSupported() || connecting !== null}
                onClick={() => void connect("ble")}
                title={
                  webBluetoothSupported()
                    ? "Connect with Web Bluetooth (the keyboard must already be paired)"
                    : "Web Bluetooth is unavailable in this browser"
                }
              >
                {connecting === "ble" ? "Connecting…" : "Connect BLE"}
              </button>
              <button
                className="button subtle"
                disabled={connecting !== null}
                onClick={() => void connect("demo")}
                title="An in-memory keyboard implementing the full protocol"
              >
                {connecting === "demo" ? "Starting…" : "Demo mode"}
              </button>
            </div>
          )}
        </div>
      </header>

      {demo && (
        <div className="demo-banner" role="note">
          Demo mode — an in-memory keyboard is answering the protocol. Nothing you do here touches hardware.
        </div>
      )}

      <div className="panel-bar">
        <div className="panel-tabs" role="tablist" aria-label="Workbench panels">
          <button
            role="tab"
            aria-selected={panel === "overlay"}
            className={panel === "overlay" ? "selected" : ""}
            onClick={() => setPanel("overlay")}
          >
            Live overlay
          </button>
          <button
            role="tab"
            aria-selected={panel === "config"}
            className={panel === "config" ? "selected" : ""}
            onClick={() => setPanel("config")}
            title="The lighting the keyboard boots with, applied transactionally"
          >
            Persistent config
          </button>
          <button
            role="tab"
            aria-selected={panel === "keymap"}
            className={panel === "keymap" ? "selected" : ""}
            onClick={() => setPanel("keymap")}
            title={
              !capabilities || (capabilities.featureBits & FEATURE_KEYMAP) !== 0
                ? "Edit the live keymap (same store as Vial)"
                : "This keyboard does not advertise keymap editing"
            }
          >
            Keymap
          </button>
        </div>
        <div className={`operation-status ${status.tone}`} role="status" aria-live="polite">
          <span className="status-dot" aria-hidden="true" />
          {status.message}
        </div>
      </div>

      {panel === "overlay" ? (
        <OverlayPanel
          client={client}
          capabilities={capabilities}
          brush={brush}
          onBrushChange={setBrush}
          onStatus={setStatus}
        />
      ) : panel === "config" ? (
        <ConfigPanel
          client={client}
          capabilities={capabilities}
          brush={brush}
          onBrushChange={setBrush}
          onStatus={setStatus}
        />
      ) : (
        <KeymapPanel client={client} capabilities={capabilities} onStatus={setStatus} />
      )}
    </main>
  );
}
