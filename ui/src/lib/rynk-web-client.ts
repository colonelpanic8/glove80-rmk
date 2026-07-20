// Browser Rynk keymap client. Rynk owns keymap operations; Lightbench's
// product protocol remains responsible for lighting and persistent lighting
// records during the migration.

import initRynk, {
  connect,
  type DeviceCapabilities,
  type KeyAction,
  type RynkClient,
} from "../vendor/rynk-wasm/rynk_wasm";

import type { KeymapEntry } from "./host-protocol";
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

export interface BrowserKeymapClient {
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

  constructor(
    readonly label: string,
    private readonly link: ByteLink,
    private readonly client: RynkClient,
    private readonly capabilities: DeviceCapabilities,
  ) {
    this.rows = capabilities.num_rows;
    this.cols = capabilities.num_cols;
    this.layers = capabilities.num_layers;
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
    return new ConnectedRynkClient(link.label, link, client, capabilities);
  } catch (error) {
    await link.close().catch(() => undefined);
    throw error;
  }
}
