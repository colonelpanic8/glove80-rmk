// In-memory Glove80 for tests and Lightbench's demo mode.
//
// MockKeyboard implements the device side of the host protocol against the
// same TS codec the app uses: overlay semantics (TTL, replace, partial
// apply), brightness, toggles, the full v1.1 config transfer session, the
// v1.2 keymap read/write semantics and the v1.3 build-identity report, per
// PROTOCOL.md. MockTransport frames it like the USB HID transport
// (32-byte zero-padded chunks) so the whole client stack is exercised.

import { KEYMAP_COLS, KEYMAP_KEY_COUNT, KEYMAP_ROWS } from "./glove80-layout";
import {
  CONFIG_HEADER_LEN,
  crc32,
  decodeLightingConfig,
  decodeRequest,
  encodeLightingConfig,
  encodeResponse,
  FEATURE_ATOMIC_REPLACE,
  FEATURE_BOOTLOADER_ENTRY,
  FEATURE_KEYMAP,
  FEATURE_OVERLAY_READBACK,
  FEATURE_PARTIAL_APPLY,
  FEATURE_PERSISTENT_CONFIG,
  FEATURE_TOGGLES,
  FEATURE_TTL,
  FEATURE_VERSION,
  MAX_CONFIG_DATA_PER_MESSAGE,
  BOOTLOADER_MAGIC,
  ProtocolError,
  Reassembler,
  splitFrames,
  type Capabilities,
  type CellState,
  type CellWrite,
  type HalfVersion,
  type LightingConfig,
  type Request,
  type Response,
  type ResponsePayload,
  type StatusName,
} from "./host-protocol";
import { parseKeycode } from "./keycodes";
import type { Transport } from "./transport";

const LED_COUNT = 80;
const LEFT_LED_COUNT = 40;

export const MOCK_CAPABILITIES: Capabilities = {
  protocolMajor: 1,
  protocolMinor: 3,
  ledCountLeft: LEFT_LED_COUNT,
  ledCountRight: LED_COUNT - LEFT_LED_COUNT,
  layerCapacity: 8,
  maxCellsPerOp: 40,
  effectMask: 0b111, // solid | blink | breathe
  overlayCellCapacity: 80,
  maxMessageLen: 1536,
  featureBits:
    FEATURE_TTL |
    FEATURE_TOGGLES |
    FEATURE_BOOTLOADER_ENTRY |
    FEATURE_ATOMIC_REPLACE |
    FEATURE_OVERLAY_READBACK |
    FEATURE_PARTIAL_APPLY |
    FEATURE_PERSISTENT_CONFIG |
    FEATURE_KEYMAP |
    FEATURE_VERSION,
  maxConfigBlobLen: 7148,
  keymapRows: KEYMAP_ROWS,
  keymapCols: KEYMAP_COLS,
  maxKeymapEntriesPerOp: 84,
};

/** ASCII → the 8-byte zero-padded gitHashHex the wire carries. */
function hashHex(text: string): string {
  let hex = "";
  for (let i = 0; i < 8; i++) {
    hex += (i < text.length ? text.charCodeAt(i) : 0).toString(16).padStart(2, "0");
  }
  return hex;
}

export const MOCK_CENTRAL_VERSION: HalfVersion = {
  present: true,
  fwMajor: 0,
  fwMinor: 1,
  fwPatch: 0,
  gitHashHex: hashHex("1a2b3c4d"),
  dirty: false,
};

/** The all-zero "never seen since the central booted" peripheral entry. */
export const NEVER_SEEN_VERSION: HalfVersion = {
  present: false,
  fwMajor: 0,
  fwMinor: 0,
  fwPatch: 0,
  gitHashHex: hashHex(""),
  dirty: false,
};

/** How the mock answers GET_VERSION — the states the real firmware can be
 * in, for UI testing. */
export type VersionMode = "match" | "mismatch" | "peripheralNeverSeen";

function versionFor(mode: VersionMode): { central: HalfVersion; peripheral: HalfVersion; halvesMismatch: boolean } {
  switch (mode) {
    case "match":
      return {
        central: MOCK_CENTRAL_VERSION,
        peripheral: { ...MOCK_CENTRAL_VERSION },
        halvesMismatch: false,
      };
    case "mismatch":
      return {
        central: { ...MOCK_CENTRAL_VERSION, dirty: true },
        peripheral: { ...MOCK_CENTRAL_VERSION, gitHashHex: hashHex("9f8e7d6c") },
        halvesMismatch: true,
      };
    case "peripheralNeverSeen":
      return {
        central: MOCK_CENTRAL_VERSION,
        peripheral: NEVER_SEEN_VERSION,
        halvesMismatch: false,
      };
  }
}

// A QWERTY-ish Glove80 base layer for the demo (grid order, 84 positions;
// holes hold KC_NO). Parsed through the shared keycode table at startup.
const DEFAULT_LAYER_NAMES: readonly string[] = [
  // r0
  "KC_F1", "KC_F2", "KC_F3", "KC_F4", "KC_F5", "KC_NO", "KC_ESC", "KC_BSPC", "KC_NO", "KC_F6", "KC_F7", "KC_F8", "KC_F9", "KC_F10",
  // r1
  "KC_EQL", "KC_1", "KC_2", "KC_3", "KC_4", "KC_5", "KC_DEL", "MO(4)", "KC_6", "KC_7", "KC_8", "KC_9", "KC_0", "KC_MINS",
  // r2
  "KC_TAB", "KC_Q", "KC_W", "KC_E", "KC_R", "KC_T", "USER(0)", "KC_LGUI", "KC_Y", "KC_U", "KC_I", "KC_O", "KC_P", "KC_BSLS",
  // r3
  "KC_LCTL", "KC_A", "KC_S", "KC_D", "KC_F", "KC_G", "KC_BSPC", "KC_SPC", "KC_H", "KC_J", "KC_K", "KC_L", "KC_SCLN", "KC_QUOT",
  // r4
  "KC_LSFT", "KC_Z", "KC_X", "KC_C", "KC_V", "KC_B", "KC_LGUI", "KC_ENT", "KC_N", "KC_M", "KC_COMM", "KC_DOT", "KC_SLSH", "KC_RSFT",
  // r5
  "KC_GRV", "KC_HOME", "KC_END", "KC_LEFT", "KC_RGHT", "KC_NO", "KC_LALT", "KC_RGUI", "KC_NO", "KC_UP", "KC_DOWN", "KC_LBRC", "KC_RBRC", "MO(3)",
];

interface OverlayCell {
  effect: CellWrite["effect"];
  /** Clock timestamp at which the cell expires; null = no TTL. */
  expiresAt: number | null;
}

interface TransferSession {
  totalLen: number;
  blobCrc32: number;
  received: number;
  buffer: Uint8Array;
}

export interface MockKeyboardOptions {
  /** Injectable clock (ms) so tests can advance TTL time deterministically. */
  now?: () => number;
  capabilities?: Partial<Capabilities>;
  /** Simulate the right half being offline: right-half writes answer
   * PARTIAL_APPLY listing the pending keys. */
  peripheralOffline?: boolean;
  /** Preloaded persisted config (as if committed earlier). */
  initialConfig?: LightingConfig;
  /** GET_VERSION answer shape (default "match"). */
  versionMode?: VersionMode;
}

/** The firmware's canonical re-encoding of a stored keycode. Mirrors the
 * lossy cases the Vial conversion documents: TT(n) is nameable but has no
 * RMK representation and stores as KC_NO. */
function canonicalKeycode(keycode: number): number {
  if (keycode >= 0x52c0 && keycode <= 0x52df) return 0x0000; // TT(n)
  return keycode;
}

export class MockKeyboard {
  readonly capabilities: Capabilities;
  private readonly now: () => number;
  peripheralOffline: boolean;
  versionMode: VersionMode;

  private overlay = new Map<number, OverlayCell>();
  private brightness = 255;
  private toggles = new Map<number, boolean>();
  private configBlob: Uint8Array | null = null;
  private session: TransferSession | null = null;
  /** layer → keycodes, flat grid order (keymapRows * keymapCols entries). */
  private keymap: Uint16Array[];

  constructor(options: MockKeyboardOptions = {}) {
    this.capabilities = { ...MOCK_CAPABILITIES, ...options.capabilities };
    this.now = options.now ?? (() => Date.now());
    this.peripheralOffline = options.peripheralOffline ?? false;
    this.versionMode = options.versionMode ?? "match";
    const gridSize = this.capabilities.keymapRows * this.capabilities.keymapCols;
    this.keymap = Array.from({ length: this.capabilities.layerCapacity }, (_, layer) => {
      const codes = new Uint16Array(gridSize);
      if (layer === 0 && gridSize === KEYMAP_KEY_COUNT) {
        DEFAULT_LAYER_NAMES.forEach((name, key) => {
          codes[key] = parseKeycode(name);
        });
      } else if (layer > 0) {
        codes.fill(0x0001); // KC_TRNS
        if (gridSize === KEYMAP_KEY_COUNT) {
          for (const hole of [5, 8, 75, 78]) codes[hole] = 0x0000; // holes
        }
      }
      return codes;
    });
    if (options.initialConfig) {
      this.configBlob = encodeLightingConfig(options.initialConfig);
      this.adoptConfigToggles(options.initialConfig);
    }
  }

  /** Handle one decoded request; returns the response the device would send. */
  handle(requestId: number, request: Request): Response {
    const respond = (status: StatusName, payload: ResponsePayload = { type: "empty" }): Response => ({
      requestId,
      command: request.command,
      status,
      payload,
    });
    switch (request.command) {
      case "getCapabilities":
        if (request.clientMajor !== this.capabilities.protocolMajor) {
          return respond("unsupportedVersion");
        }
        return respond("ok", { type: "capabilities", ...this.capabilities });
      case "ping":
        return respond("ok", { type: "echo", data: request.data });
      case "getVersion": {
        if ((this.capabilities.featureBits & FEATURE_VERSION) === 0) {
          return respond("unknownCommand");
        }
        return respond("ok", { type: "version", ...versionFor(this.versionMode) });
      }
      case "keymapRead": {
        if ((this.capabilities.featureBits & FEATURE_KEYMAP) === 0) {
          return respond("unknownCommand");
        }
        const gridSize = this.capabilities.keymapRows * this.capabilities.keymapCols;
        if (request.layer >= this.capabilities.layerCapacity || request.startKey >= gridSize) {
          return respond("outOfRange");
        }
        const count = Math.min(
          request.maxCount,
          this.capabilities.maxKeymapEntriesPerOp,
          gridSize - request.startKey,
        );
        const keycodes = [...this.keymap[request.layer].slice(request.startKey, request.startKey + count)];
        return respond("ok", {
          type: "keymapActions",
          layer: request.layer,
          startKey: request.startKey,
          keycodes,
        });
      }
      case "keymapWrite": {
        if ((this.capabilities.featureBits & FEATURE_KEYMAP) === 0) {
          return respond("unknownCommand");
        }
        if (request.entries.length > this.capabilities.maxKeymapEntriesPerOp) {
          return respond("capacityExceeded");
        }
        const gridSize = this.capabilities.keymapRows * this.capabilities.keymapCols;
        // All-or-nothing: validate every entry before writing anything.
        for (const entry of request.entries) {
          if (entry.layer >= this.capabilities.layerCapacity || entry.key >= gridSize) {
            return respond("outOfRange");
          }
        }
        // Apply in order (later entries win), then read back what stuck.
        for (const entry of request.entries) {
          this.keymap[entry.layer][entry.key] = canonicalKeycode(entry.keycode);
        }
        const keycodes = request.entries.map((entry) => this.keymap[entry.layer][entry.key]);
        return respond("ok", { type: "keymapWritten", keycodes });
      }
      case "setCells":
      case "replaceOverlay": {
        if (request.cells.length > this.capabilities.maxCellsPerOp) {
          return respond("capacityExceeded");
        }
        for (const cell of request.cells) {
          if (cell.key >= LED_COUNT) return respond("outOfRange");
        }
        const next =
          request.command === "replaceOverlay"
            ? new Map<number, OverlayCell>()
            : new Map(this.overlay);
        const expiresAt = request.ttlMs > 0 ? this.now() + request.ttlMs : null;
        for (const cell of request.cells) next.set(cell.key, { effect: cell.effect, expiresAt });
        if (next.size > this.capabilities.overlayCellCapacity) return respond("capacityExceeded");
        this.overlay = next;
        return this.overlayAck(
          respond,
          request.cells.map((cell) => cell.key),
        );
      }
      case "unsetCells": {
        if (request.keys.length > this.capabilities.maxCellsPerOp) {
          return respond("capacityExceeded");
        }
        for (const key of request.keys) {
          if (key >= LED_COUNT) return respond("outOfRange");
        }
        for (const key of request.keys) this.overlay.delete(key);
        return this.overlayAck(respond, request.keys);
      }
      case "clearOverlay": {
        const hadRightCells = [...this.overlay.keys()].some((key) => key >= LEFT_LED_COUNT);
        this.overlay.clear();
        if (this.peripheralOffline && hadRightCells) {
          // A pending clear on the offline half: PARTIAL_APPLY, no key list.
          return respond("partialApply", { type: "overlayAck", pendingKeys: [] });
        }
        return respond("ok", { type: "overlayAck", pendingKeys: [] });
      }
      case "readOverlay": {
        this.pruneExpired();
        const cells: CellState[] = [...this.overlay.entries()]
          .sort(([a], [b]) => a - b)
          .map(([key, cell]) => ({
            key,
            effect: cell.effect,
            remainingTtlMs:
              cell.expiresAt === null ? 0 : Math.max(1, cell.expiresAt - this.now()),
          }));
        return respond("ok", { type: "overlayState", cells });
      }
      case "getBrightness":
        return respond("ok", { type: "brightness", level: this.brightness });
      case "setBrightness":
        this.brightness = request.level;
        return respond("ok", { type: "brightness", level: this.brightness });
      case "getToggle": {
        const state = this.toggles.get(request.id);
        if (state === undefined) return respond("unknownToggle");
        return respond("ok", { type: "toggle", id: request.id, state });
      }
      case "setToggle": {
        if (!this.toggles.has(request.id)) return respond("unknownToggle");
        this.toggles.set(request.id, request.state);
        return respond("ok", { type: "toggle", id: request.id, state: request.state });
      }
      case "configBegin": {
        if (request.totalLen > this.capabilities.maxConfigBlobLen) {
          this.session = null;
          return respond("capacityExceeded");
        }
        this.session = {
          totalLen: request.totalLen,
          blobCrc32: request.blobCrc32,
          received: 0,
          buffer: new Uint8Array(request.totalLen),
        };
        return respond("ok");
      }
      case "configData": {
        const session = this.session;
        if (!session) return respond("noSession");
        if (
          request.offset !== session.received ||
          request.offset + request.data.length > session.totalLen
        ) {
          this.session = null; // a bad offset aborts the session
          return respond("badOffset");
        }
        session.buffer.set(request.data, request.offset);
        session.received += request.data.length;
        return respond("ok");
      }
      case "configCommit": {
        const session = this.session;
        this.session = null; // every commit, success or failure, ends the session
        if (!session) return respond("noSession");
        if (session.received < session.totalLen) return respond("configIncomplete");
        const blob = session.buffer;
        if (crc32(blob) !== session.blobCrc32) return respond("crcMismatch");
        if (blob.length < CONFIG_HEADER_LEN) return respond("invalidConfig");
        const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength);
        const bodyCrc = view.getUint32(12, true);
        if (crc32(blob.subarray(CONFIG_HEADER_LEN)) !== bodyCrc) return respond("crcMismatch");
        let config: LightingConfig;
        try {
          config = decodeLightingConfig(blob);
        } catch {
          return respond("invalidConfig");
        }
        this.configBlob = blob.slice(); // atomically activate + persist
        this.adoptConfigToggles(config);
        return respond("ok");
      }
      case "configAbort":
        this.session = null; // idempotent
        return respond("ok");
      case "configRead": {
        const blob = this.configBlob;
        const totalLen = blob?.length ?? 0;
        if (request.offset > totalLen) return respond("outOfRange");
        const maxLen = Math.min(request.maxLen, MAX_CONFIG_DATA_PER_MESSAGE);
        const data = blob
          ? blob.slice(request.offset, Math.min(request.offset + maxLen, totalLen))
          : new Uint8Array(0);
        return respond("ok", { type: "configData", totalLen, data });
      }
      case "enterBootloader":
        if (request.magic !== BOOTLOADER_MAGIC) return respond("badMagic");
        if (request.target === "peripheral" && this.peripheralOffline) return respond("busy");
        return respond("ok");
    }
  }

  /** The active (committed) blob, byte-stable; null when none is stored. */
  activeConfigBlob(): Uint8Array | null {
    return this.configBlob ? this.configBlob.slice() : null;
  }

  /** The stored keycode at (layer, grid key) — for tests. */
  keycodeAt(layer: number, key: number): number {
    return this.keymap[layer][key];
  }

  overlaySize(): number {
    this.pruneExpired();
    return this.overlay.size;
  }

  private adoptConfigToggles(config: LightingConfig): void {
    const next = new Map<number, boolean>();
    for (const record of config.records) {
      if (record.activation.kind === "toggle") {
        const id = record.activation.id;
        const persisted = (config.togglePersistMask & (1 << id)) !== 0 && this.toggles.has(id);
        next.set(
          id,
          persisted
            ? (this.toggles.get(id) as boolean)
            : (config.toggleInitialState & (1 << id)) !== 0,
        );
      }
    }
    this.toggles = next;
  }

  private pruneExpired(): void {
    const now = this.now();
    for (const [key, cell] of this.overlay) {
      if (cell.expiresAt !== null && cell.expiresAt <= now) this.overlay.delete(key);
    }
  }

  private overlayAck(
    respond: (status: StatusName, payload?: ResponsePayload) => Response,
    writtenKeys: number[],
  ): Response {
    if (this.peripheralOffline) {
      const pendingKeys = [...new Set(writtenKeys.filter((key) => key >= LEFT_LED_COUNT))].sort(
        (a, b) => a - b,
      );
      if (pendingKeys.length > 0) {
        return respond("partialApply", { type: "overlayAck", pendingKeys });
      }
    }
    return respond("ok", { type: "overlayAck", pendingKeys: [] });
  }
}

/**
 * Transport adapter over a MockKeyboard: frames like USB HID (32-byte
 * zero-padded chunks), delivers responses asynchronously.
 */
export class MockTransport implements Transport {
  readonly kind = "demo" as const;
  readonly label = "Demo keyboard";
  readonly chunkSize = 32;
  readonly pad = true;

  private reassembler = new Reassembler();
  private chunkHandler: ((chunk: Uint8Array) => void) | null = null;
  private disconnectHandler: (() => void) | null = null;
  private closed = false;

  constructor(readonly keyboard: MockKeyboard) {}

  async sendChunk(chunk: Uint8Array): Promise<void> {
    if (this.closed) throw new ProtocolError("demo transport closed");
    const message = this.reassembler.push(chunk);
    if (message === null) return;
    const { requestId, request } = decodeRequest(message);
    const response = encodeResponse(this.keyboard.handle(requestId, request));
    const frames = splitFrames(response, this.chunkSize, this.pad);
    queueMicrotask(() => {
      if (this.closed) return;
      for (const frame of frames) this.chunkHandler?.(frame);
    });
  }

  onChunk(handler: (chunk: Uint8Array) => void): void {
    this.chunkHandler = handler;
  }

  onDisconnect(handler: () => void): void {
    this.disconnectHandler = handler;
  }

  /** Simulate the keyboard dropping the link (for tests). */
  simulateDisconnect(): void {
    this.closed = true;
    this.disconnectHandler?.();
  }

  async close(): Promise<void> {
    this.closed = true;
  }
}

/** A demo keyboard preloaded with a small, presentable persistent config. */
export function createDemoKeyboard(): MockKeyboard {
  const teal = { kind: "solid" as const, r: 0x2b, g: 0xd4, b: 0xc0, periodMs: 0, phaseMs: 0, dutyPercent: 0 };
  const amber = { kind: "breathe" as const, r: 0xf5, g: 0xa5, b: 0x24, periodMs: 2400, phaseMs: 0, dutyPercent: 0 };
  const red = { kind: "blink" as const, r: 0xf0, g: 0x5d, b: 0x3e, periodMs: 900, phaseMs: 0, dutyPercent: 40 };
  return new MockKeyboard({
    initialConfig: {
      togglePersistMask: 1 << 2,
      toggleInitialState: 1 << 2,
      records: [
        {
          activation: { kind: "always" },
          cells: [0, 1, 2, 3, 4, 5, 40, 41, 42, 43, 44, 45].map((key) => ({ key, effect: teal })),
        },
        {
          activation: { kind: "layerActive", layer: 1 },
          cells: [10, 16, 22, 50, 56, 62].map((key) => ({ key, effect: amber })),
        },
        {
          activation: { kind: "toggle", id: 2 },
          cells: [34, 74].map((key) => ({ key, effect: red })),
        },
      ],
    },
  });
}
