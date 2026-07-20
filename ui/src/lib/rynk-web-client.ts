// Browser Rynk client for live keymap and topology-aware lighting operations.
// The older product protocol remains only for legacy persisted config records.

import initRynk, {
  connect,
  type DeviceCapabilities,
  type KeyAction,
  type LightingCapabilities,
  type LightingEffect,
  type LightingOverlayCell,
  type LightingState,
  type RynkClient,
} from "../vendor/rynk-wasm/rynk_wasm";

import {
  FEATURE_ATOMIC_REPLACE,
  FEATURE_TTL,
  type Capabilities,
  type CellWrite,
  type Effect,
  type KeymapEntry,
} from "./host-protocol";
import type { OverlayWriteResult } from "./protocol-client";
import { fromViaKeycode, toViaKeycode } from "./rynk-keycode";

const RYNK_HEADER_LEN = 5;
const RYNK_HID_REPORT_SIZE = 32;
const RYNK_USAGE_PAGE = 0xff60;
const RYNK_USAGE = 0x61;

export type RynkBrowserTransport = "usb" | "ble";

interface ByteLink {
  readonly label: string;
  send(bytes: Uint8Array): Promise<void>;
  recv(): Promise<Uint8Array>;
  close(): Promise<void>;
}

function concat(
  a: Uint8Array<ArrayBufferLike>,
  b: Uint8Array<ArrayBufferLike>,
): Uint8Array<ArrayBuffer> {
  const result = new Uint8Array(new ArrayBuffer(a.length + b.length));
  result.set(a);
  result.set(b, a.length);
  return result;
}

function byteLink(
  label: string,
  start: (push: (bytes: Uint8Array) => void, end: () => void) => void,
  send: (bytes: Uint8Array) => Promise<void>,
  closeTransport: () => Promise<void>,
): ByteLink {
  let rx = new Uint8Array();
  let closed = false;
  let wake: (() => void) | null = null;
  const signal = () => {
    const pending = wake;
    wake = null;
    pending?.();
  };
  start(
    (bytes) => {
      rx = concat(rx, bytes);
      signal();
    },
    () => {
      closed = true;
      signal();
    },
  );
  return {
    label,
    send,
    async recv() {
      while (rx.length === 0 && !closed) await new Promise<void>((resolve) => (wake = resolve));
      if (rx.length === 0) return new Uint8Array();
      const bytes = rx;
      rx = new Uint8Array();
      return bytes;
    },
    async close() {
      closed = true;
      signal();
      await closeTransport();
    },
  };
}

async function openHidLink(): Promise<ByteLink> {
  if (!("hid" in navigator)) throw new Error("WebHID is unavailable; use Chrome or Edge");
  const devices = await navigator.hid.requestDevice({
    filters: [{ usagePage: RYNK_USAGE_PAGE, usage: RYNK_USAGE }],
  });
  const device = devices[0];
  if (!device) throw new Error("No Rynk HID device chosen");
  if (!device.opened) await device.open();
  let remaining = 0;
  let reportHandler: EventListener | null = null;
  let endLink: (() => void) | null = null;
  return byteLink(
    device.productName || "Rynk WebHID",
    (push, end) => {
      endLink = end;
      reportHandler = (rawEvent) => {
        const event = rawEvent as HIDInputReportEvent;
        const data = new Uint8Array(event.data.buffer, event.data.byteOffset, event.data.byteLength);
        if (remaining === 0 && data.length >= RYNK_HEADER_LEN) {
          remaining = RYNK_HEADER_LEN + data[3] + (data[4] << 8);
        }
        const take = Math.min(remaining, data.length);
        remaining -= take;
        push(data.slice(0, take));
      };
      device.addEventListener("inputreport", reportHandler);
    },
    async (bytes) => {
      for (let offset = 0; offset < bytes.length; offset += RYNK_HID_REPORT_SIZE) {
        const report = new Uint8Array(RYNK_HID_REPORT_SIZE);
        report.set(bytes.subarray(offset, offset + RYNK_HID_REPORT_SIZE));
        await device.sendReport(0, report);
      }
    },
    async () => {
      endLink?.();
      if (reportHandler) device.removeEventListener("inputreport", reportHandler);
      await device.close().catch(() => undefined);
    },
  );
}

export interface BrowserLightingClient {
  readonly lightingCapabilities: Capabilities;
  readonly supportsOverlayReadback: false;
  getLightingState(): Promise<LightingState>;
  setCells(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult>;
  unsetCells(keys: number[]): Promise<OverlayWriteResult>;
  clearOverlay(): Promise<OverlayWriteResult>;
  replaceOverlay(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult>;
  getBrightness(): Promise<number>;
  setBrightness(level: number): Promise<number>;
}

export interface BrowserKeymapClient extends BrowserLightingClient {
  readonly label: string;
  readonly rows: number;
  readonly cols: number;
  readonly layers: number;
  readKeymapLayer(layer: number): Promise<number[]>;
  writeKeymap(entries: KeymapEntry[]): Promise<number[]>;
  close(): Promise<void>;
}

class ConnectedRynkClient implements BrowserKeymapClient {
  readonly rows: number;
  readonly cols: number;
  readonly layers: number;
  readonly lightingCapabilities: Capabilities;
  readonly supportsOverlayReadback = false as const;

  constructor(
    readonly label: string,
    private readonly link: ByteLink,
    private readonly client: RynkClient,
    private readonly capabilities: DeviceCapabilities,
    private readonly rynkLightingCapabilities: LightingCapabilities,
  ) {
    this.rows = capabilities.num_rows;
    this.cols = capabilities.num_cols;
    this.layers = capabilities.num_layers;
    this.lightingCapabilities = {
      protocolMajor: 1,
      protocolMinor: 0,
      ledCountLeft: Math.min(40, rynkLightingCapabilities.led_count),
      ledCountRight: Math.max(0, rynkLightingCapabilities.led_count - 40),
      layerCapacity: capabilities.num_layers,
      maxCellsPerOp: rynkLightingCapabilities.overlay_capacity,
      effectMask: rynkLightingCapabilities.effects,
      overlayCellCapacity: rynkLightingCapabilities.overlay_capacity,
      maxMessageLen: 256,
      featureBits: FEATURE_TTL | FEATURE_ATOMIC_REPLACE,
      maxConfigBlobLen: 0,
      keymapRows: capabilities.num_rows,
      keymapCols: capabilities.num_cols,
      maxKeymapEntriesPerOp: 0,
    };
  }

  async getLightingState(): Promise<LightingState> {
    return this.client.get_lighting_state();
  }

  async setCells(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult> {
    let state = await this.client.get_lighting_state();
    for (const cell of cells) {
      state = await this.client.set_lighting_overlay({
        expected_revision: state.revision,
        cell: wireCell(cell, ttlMs),
      });
    }
    return { partial: false, pendingKeys: [] };
  }

  async unsetCells(keys: number[]): Promise<OverlayWriteResult> {
    let state = await this.client.get_lighting_state();
    for (const key of keys) {
      state = await this.client.unset_lighting_overlay({
        expected_revision: state.revision,
        led_id: key,
      });
    }
    return { partial: false, pendingKeys: [] };
  }

  async clearOverlay(): Promise<OverlayWriteResult> {
    const state = await this.client.get_lighting_state();
    await this.client.clear_lighting_overlay({ expected_revision: state.revision });
    return { partial: false, pendingKeys: [] };
  }

  async replaceOverlay(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult> {
    const state = await this.client.get_lighting_state();
    const transaction = await this.client.begin_lighting_overlay_replace({
      expected_revision: state.revision,
      cell_count: cells.length,
    });
    try {
      const chunkSize = this.rynkLightingCapabilities.overlay_chunk_capacity;
      for (let offset = 0; offset < cells.length; offset += chunkSize) {
        await this.client.put_lighting_overlay_chunk({
          transaction_id: transaction.id,
          offset,
          cells: cells.slice(offset, offset + chunkSize).map((cell) => wireCell(cell, ttlMs)),
        });
      }
      await this.client.commit_lighting_overlay_replace({ transaction_id: transaction.id });
    } catch (error) {
      await this.client
        .abort_lighting_overlay_replace({ transaction_id: transaction.id })
        .catch(() => undefined);
      throw error;
    }
    return { partial: false, pendingKeys: [] };
  }

  async getBrightness(): Promise<number> {
    return (await this.client.get_lighting_state()).output_brightness;
  }

  async setBrightness(level: number): Promise<number> {
    const state = await this.client.get_lighting_state();
    const next = await this.client.set_lighting_state({
      expected_revision: state.revision,
      state: {
        output_enabled: state.output_enabled,
        output_brightness: level,
        background: state.background,
      },
    });
    return next.output_brightness;
  }

  async readKeymapLayer(layer: number): Promise<number[]> {
    if (layer < 0 || layer >= this.layers) throw new Error(`Rynk layer ${layer} is out of range`);
    const total = this.rows * this.cols;
    const actions: KeyAction[] = [];
    if (this.capabilities.bulk_transfer_supported) {
      while (actions.length < total) {
        const row = Math.floor(actions.length / this.cols);
        const col = actions.length % this.cols;
        const page = await this.client.get_keymap_bulk(layer, row, col);
        if (page.actions.length === 0) throw new Error(`Rynk bulk read stalled at key ${actions.length}`);
        actions.push(...page.actions.slice(0, total - actions.length));
      }
    } else {
      for (let key = 0; key < total; key++) {
        actions.push(await this.client.get_key(layer, Math.floor(key / this.cols), key % this.cols));
      }
    }
    return actions.map(toViaKeycode);
  }

  async writeKeymap(entries: KeymapEntry[]): Promise<number[]> {
    const readback: number[] = [];
    for (const entry of entries) {
      if (entry.layer >= this.layers || entry.key >= this.rows * this.cols) {
        throw new Error(`Rynk keymap position L${entry.layer}:${entry.key} is out of range`);
      }
      const row = Math.floor(entry.key / this.cols);
      const col = entry.key % this.cols;
      await this.client.set_key(entry.layer, row, col, fromViaKeycode(entry.keycode));
      readback.push(toViaKeycode(await this.client.get_key(entry.layer, row, col)));
    }
    return readback;
  }

  async close(): Promise<void> {
    this.client.free();
    await this.link.close();
  }
}

export async function connectRynkKeymap(
  _transport: RynkBrowserTransport,
): Promise<BrowserKeymapClient> {
  const link = await openHidLink();
  try {
    await initRynk();
    const client = await connect(link);
    const capabilities = await client.get_capabilities();
    if (capabilities.num_rows !== 6 || capabilities.num_cols !== 14) {
      client.free();
      throw new Error(
        `Rynk device is ${capabilities.num_rows}x${capabilities.num_cols}, expected the Glove80 6x14 grid`,
      );
    }
    if (!capabilities.lighting_enabled) {
      client.free();
      throw new Error("This Rynk firmware does not advertise topology-aware lighting");
    }
    const lightingCapabilities = await client.get_lighting_capabilities();
    return new ConnectedRynkClient(link.label, link, client, capabilities, lightingCapabilities);
  } catch (error) {
    await link.close().catch(() => undefined);
    throw error;
  }
}

function wireCell(cell: CellWrite, ttlMs: number): LightingOverlayCell {
  return {
    led_id: cell.key,
    effect: wireEffect(cell.effect),
    ttl_ms: ttlMs > 0 ? ttlMs : undefined,
  };
}

function wireEffect(effect: Effect): LightingEffect {
  const color = { r: effect.r, g: effect.g, b: effect.b };
  switch (effect.kind) {
    case "solid":
      return { Solid: { color } };
    case "blink":
      return {
        Blink: {
          color,
          period_ms: effect.periodMs,
          phase_ms: effect.phaseMs,
          duty: effect.dutyPercent,
        },
      };
    case "breathe":
      return {
        Breathe: {
          color,
          period_ms: effect.periodMs,
          phase_ms: effect.phaseMs,
          step_ms: 16,
        },
      };
  }
}
