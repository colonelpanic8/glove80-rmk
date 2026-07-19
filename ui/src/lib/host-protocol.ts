// Glove80 host protocol v1.3 — TypeScript codec.
//
// Byte-level spec: protocol/glove80-host-protocol/PROTOCOL.md.
// This mirrors the Rust codec (protocol/glove80-host-protocol); both are
// pinned to the shared golden vectors in protocol/vectors/
// (host-protocol-v1.json, frozen, plus host-protocol-v1.1.json,
// host-protocol-v1.2.json and host-protocol-v1.3.json).
// All integers little-endian.

export const PROTOCOL_VERSION_MAJOR = 1;
export const PROTOCOL_VERSION_MINOR = 3;
export const RESPONSE_FLAG = 0x80;
export const REQUEST_HEADER_LEN = 4;
export const RESPONSE_HEADER_LEN = 5;
export const MAX_MESSAGE_LEN = 1536;
export const MAX_CELLS_PER_MESSAGE = 80;
export const MAX_PING_LEN = 64;
export const BOOTLOADER_MAGIC = 0xb00710ad;
/** Max config bytes in one CONFIG_DATA request / CONFIG_READ response. */
export const MAX_CONFIG_DATA_PER_MESSAGE = 1024;
/** Codec-side cap on entries in one KEYMAP_READ/WRITE (devices advertise
 * their own, smaller `maxKeymapEntriesPerOp`). Mirrors the Rust codec. */
export const MAX_KEYMAP_ENTRIES_PER_MESSAGE = 128;
/** GET_VERSION git hash: ASCII short commit hash, zero-padded to 8 bytes. */
export const GIT_HASH_LEN = 8;

export type CommandName =
  | "getCapabilities"
  | "ping"
  | "getVersion"
  | "setCells"
  | "unsetCells"
  | "clearOverlay"
  | "readOverlay"
  | "replaceOverlay"
  | "getBrightness"
  | "setBrightness"
  | "getToggle"
  | "setToggle"
  | "configBegin"
  | "configData"
  | "configCommit"
  | "configAbort"
  | "configRead"
  | "keymapRead"
  | "keymapWrite"
  | "enterBootloader";

export const OPCODES: Record<CommandName, number> = {
  getCapabilities: 0x01,
  ping: 0x02,
  getVersion: 0x03,
  setCells: 0x10,
  unsetCells: 0x11,
  clearOverlay: 0x12,
  readOverlay: 0x13,
  replaceOverlay: 0x14,
  getBrightness: 0x20,
  setBrightness: 0x21,
  getToggle: 0x30,
  setToggle: 0x31,
  configBegin: 0x40,
  configData: 0x41,
  configCommit: 0x42,
  configAbort: 0x43,
  configRead: 0x44,
  keymapRead: 0x50,
  keymapWrite: 0x51,
  enterBootloader: 0x7f,
};

const COMMAND_BY_OPCODE = new Map<number, CommandName>(
  (Object.entries(OPCODES) as [CommandName, number][]).map(([name, op]) => [op, name]),
);

const OVERLAY_WRITE_COMMANDS: ReadonlySet<CommandName> = new Set([
  "setCells",
  "unsetCells",
  "clearOverlay",
  "replaceOverlay",
]);

export type StatusName =
  | "ok"
  | "unknownCommand"
  | "malformed"
  | "outOfRange"
  | "capacityExceeded"
  | "partialApply"
  | "busy"
  | "unknownToggle"
  | "badMagic"
  | "unsupportedVersion"
  | "noSession"
  | "badOffset"
  | "configIncomplete"
  | "crcMismatch"
  | "invalidConfig";

const STATUS_VALUES: Record<StatusName, number> = {
  ok: 0x00,
  unknownCommand: 0x01,
  malformed: 0x02,
  outOfRange: 0x03,
  capacityExceeded: 0x04,
  partialApply: 0x05,
  busy: 0x06,
  unknownToggle: 0x07,
  badMagic: 0x08,
  unsupportedVersion: 0x09,
  noSession: 0x0a,
  badOffset: 0x0b,
  configIncomplete: 0x0c,
  crcMismatch: 0x0d,
  invalidConfig: 0x0e,
};

const STATUS_BY_VALUE = new Map<number, StatusName>(
  (Object.entries(STATUS_VALUES) as [StatusName, number][]).map(([name, v]) => [v, name]),
);

export type EffectKind = "solid" | "blink" | "breathe";

const EFFECT_KIND_VALUES: Record<EffectKind, number> = { solid: 0, blink: 1, breathe: 2 };
const EFFECT_KIND_BY_VALUE = new Map<number, EffectKind>(
  (Object.entries(EFFECT_KIND_VALUES) as [EffectKind, number][]).map(([name, v]) => [v, name]),
);

/** Fixed 10-byte effect record. Fields not applicable to `kind` should be 0
 * but round-trip verbatim either way. */
export interface Effect {
  kind: EffectKind;
  r: number;
  g: number;
  b: number;
  periodMs: number;
  phaseMs: number;
  dutyPercent: number;
}

export const EFFECT_ENCODED_LEN = 10;

export interface CellWrite {
  key: number;
  effect: Effect;
}

export interface CellState {
  key: number;
  effect: Effect;
  /** 0 = no TTL on this cell. */
  remainingTtlMs: number;
}

export interface Capabilities {
  protocolMajor: number;
  protocolMinor: number;
  ledCountLeft: number;
  ledCountRight: number;
  layerCapacity: number;
  maxCellsPerOp: number;
  /** Bit n set ⇔ effect kind n supported. */
  effectMask: number;
  overlayCellCapacity: number;
  maxMessageLen: number;
  featureBits: number;
  /** Largest config blob the device accepts (v1.1). On the wire this u32 is
   * present iff FEATURE_PERSISTENT_CONFIG is set; otherwise it decodes as 0. */
  maxConfigBlobLen: number;
  /** Keymap grid rows (v1.2). With the next two fields, on the wire exactly
   * when FEATURE_KEYMAP is set; an absent extension decodes as 0. */
  keymapRows: number;
  /** Keymap grid columns (v1.2); key = row * keymapCols + col. */
  keymapCols: number;
  /** Max entries in one KEYMAP_READ/KEYMAP_WRITE (v1.2). */
  maxKeymapEntriesPerOp: number;
}

export const FEATURE_TTL = 1 << 0;
export const FEATURE_TOGGLES = 1 << 1;
export const FEATURE_BOOTLOADER_ENTRY = 1 << 2;
export const FEATURE_ATOMIC_REPLACE = 1 << 3;
export const FEATURE_OVERLAY_READBACK = 1 << 4;
export const FEATURE_PARTIAL_APPLY = 1 << 5;
export const FEATURE_PERSISTENT_CONFIG = 1 << 6;
export const FEATURE_KEYMAP = 1 << 7;
export const FEATURE_VERSION = 1 << 8;

export type BootTarget = "central" | "peripheral";

/** One KEYMAP_WRITE entry: a VIA 16-bit keycode at (layer, grid key). */
export interface KeymapEntry {
  layer: number;
  key: number;
  /** VIA/Vial 16-bit keycode (0x0000 = KC_NO, 0x0001 = KC_TRNS). */
  keycode: number;
}

/** One half's build identity (GET_VERSION, v1.3). */
export interface HalfVersion {
  /** Central: always true in its own response. Peripheral: false while the
   * split link is down (all-zero fields ⇔ never seen since boot). */
  present: boolean;
  fwMajor: number;
  fwMinor: number;
  fwPatch: number;
  /** The 8 git-hash bytes as 16 hex digits (ASCII short hash, zero-padded
   * on the right); "unknown0" when the build had no git. */
  gitHashHex: string;
  /** Built from a tree with uncommitted changes. */
  dirty: boolean;
}

/** GET_VERSION's answer: both halves plus the firmware-computed mismatch. */
export interface VersionInfo {
  central: HalfVersion;
  peripheral: HalfVersion;
  /** True exactly when both halves are present and their git hash or
   * firmware semver differ — a flash-one-half-and-forgot-the-other state. */
  halvesMismatch: boolean;
}

/** Decode a HalfVersion git hash to display text (trailing NULs dropped). */
export function gitHashText(gitHashHex: string): string {
  let out = "";
  for (let i = 0; i + 1 < gitHashHex.length; i += 2) {
    const byte = parseInt(gitHashHex.slice(i, i + 2), 16);
    if (Number.isNaN(byte) || byte === 0) break;
    out += String.fromCharCode(byte);
  }
  return out;
}

export type Request =
  | { command: "getCapabilities"; clientMajor: number; clientMinor: number }
  | { command: "ping"; data: Uint8Array }
  | { command: "getVersion" }
  | { command: "setCells"; ttlMs: number; cells: CellWrite[] }
  | { command: "unsetCells"; keys: number[] }
  | { command: "clearOverlay" }
  | { command: "readOverlay" }
  | { command: "replaceOverlay"; ttlMs: number; cells: CellWrite[] }
  | { command: "getBrightness" }
  | { command: "setBrightness"; level: number }
  | { command: "getToggle"; id: number }
  | { command: "setToggle"; id: number; state: boolean }
  | { command: "configBegin"; totalLen: number; blobCrc32: number }
  | { command: "configData"; offset: number; data: Uint8Array }
  | { command: "configCommit" }
  | { command: "configAbort" }
  | { command: "configRead"; offset: number; maxLen: number }
  | { command: "keymapRead"; layer: number; startKey: number; maxCount: number }
  | { command: "keymapWrite"; entries: KeymapEntry[] }
  | { command: "enterBootloader"; magic: number; target: BootTarget };

export type ResponsePayload =
  | { type: "empty" }
  | ({ type: "capabilities" } & Capabilities)
  | { type: "echo"; data: Uint8Array }
  | { type: "overlayAck"; pendingKeys: number[] }
  | { type: "overlayState"; cells: CellState[] }
  | { type: "brightness"; level: number }
  | { type: "toggle"; id: number; state: boolean }
  | { type: "configData"; totalLen: number; data: Uint8Array }
  | ({ type: "version" } & VersionInfo)
  | { type: "keymapActions"; layer: number; startKey: number; keycodes: number[] }
  | { type: "keymapWritten"; keycodes: number[] };

export interface Response {
  requestId: number;
  command: CommandName;
  status: StatusName;
  payload: ResponsePayload;
}

export class ProtocolError extends Error {}

// --- little-endian cursor helpers ----------------------------------------

class Writer {
  private buf: Uint8Array;
  pos = 0;

  constructor(capacity: number = MAX_MESSAGE_LEN) {
    this.buf = new Uint8Array(capacity);
  }

  private ensure(n: number): void {
    if (this.pos + n > this.buf.length) {
      throw new ProtocolError(`message exceeds the encoder capacity (${this.buf.length})`);
    }
  }

  u8(v: number): void {
    this.ensure(1);
    this.buf[this.pos++] = v & 0xff;
  }

  u16(v: number): void {
    this.u8(v);
    this.u8(v >>> 8);
  }

  u32(v: number): void {
    this.u16(v);
    this.u16(v >>> 16);
  }

  bytes(src: Uint8Array | number[]): void {
    this.ensure(src.length);
    this.buf.set(src instanceof Uint8Array ? src : Uint8Array.from(src), this.pos);
    this.pos += src.length;
  }

  patchU16(at: number, v: number): void {
    this.buf[at] = v & 0xff;
    this.buf[at + 1] = (v >>> 8) & 0xff;
  }

  patchU32(at: number, v: number): void {
    this.patchU16(at, v & 0xffff);
    this.patchU16(at + 2, v >>> 16);
  }

  /** The bytes written so far (a view, not a copy). */
  written(): Uint8Array {
    return this.buf.subarray(0, this.pos);
  }

  finish(): Uint8Array {
    return this.buf.slice(0, this.pos);
  }
}

class ReaderCursor {
  pos = 0;
  constructor(private buf: Uint8Array) {}

  get remaining(): number {
    return this.buf.length - this.pos;
  }

  u8(): number {
    if (this.remaining < 1) throw new ProtocolError("message truncated");
    return this.buf[this.pos++];
  }

  u16(): number {
    return this.u8() | (this.u8() << 8);
  }

  u32(): number {
    return (this.u16() | (this.u16() << 16)) >>> 0;
  }

  bytes(n: number): Uint8Array {
    if (this.remaining < n) throw new ProtocolError("message truncated");
    const out = this.buf.slice(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  finish(): void {
    if (this.remaining !== 0) {
      throw new ProtocolError("length field disagrees with buffer");
    }
  }
}

// --- effect / cell records ------------------------------------------------

function writeEffect(w: Writer, e: Effect): void {
  w.u8(EFFECT_KIND_VALUES[e.kind]);
  w.u8(e.r);
  w.u8(e.g);
  w.u8(e.b);
  w.u16(e.periodMs);
  w.u16(e.phaseMs);
  w.u8(e.dutyPercent);
  w.u8(0); // reserved
}

function readEffect(r: ReaderCursor): Effect {
  const kindByte = r.u8();
  const kind = EFFECT_KIND_BY_VALUE.get(kindByte);
  if (kind === undefined) throw new ProtocolError(`unknown effect kind ${kindByte}`);
  const effect: Effect = {
    kind,
    r: r.u8(),
    g: r.u8(),
    b: r.u8(),
    periodMs: r.u16(),
    phaseMs: r.u16(),
    dutyPercent: r.u8(),
  };
  r.u8(); // reserved, ignored
  return effect;
}

function writeCells(w: Writer, ttlMs: number, cells: CellWrite[]): void {
  if (cells.length > MAX_CELLS_PER_MESSAGE) {
    throw new ProtocolError(`too many cells (max ${MAX_CELLS_PER_MESSAGE})`);
  }
  w.u32(ttlMs);
  w.u8(cells.length);
  for (const cell of cells) {
    w.u8(cell.key);
    writeEffect(w, cell.effect);
  }
}

function readCells(r: ReaderCursor): { ttlMs: number; cells: CellWrite[] } {
  const ttlMs = r.u32();
  const count = r.u8();
  if (count > MAX_CELLS_PER_MESSAGE) throw new ProtocolError("count exceeds codec capacity");
  const cells: CellWrite[] = [];
  for (let i = 0; i < count; i++) {
    cells.push({ key: r.u8(), effect: readEffect(r) });
  }
  return { ttlMs, cells };
}

// --- version records (GET_VERSION, v1.3) ----------------------------------

function readBit(r: ReaderCursor, what: string): boolean {
  const byte = r.u8();
  if (byte > 1) throw new ProtocolError(`${what} must be 0 or 1, got ${byte}`);
  return byte === 1;
}

function writeHalfVersion(w: Writer, half: HalfVersion): void {
  if (half.gitHashHex.length !== GIT_HASH_LEN * 2 || /[^0-9a-fA-F]/.test(half.gitHashHex)) {
    throw new ProtocolError(`gitHashHex must be ${GIT_HASH_LEN * 2} hex digits`);
  }
  w.u8(half.present ? 1 : 0);
  w.u8(half.fwMajor);
  w.u8(half.fwMinor);
  w.u8(half.fwPatch);
  for (let i = 0; i < GIT_HASH_LEN; i++) {
    w.u8(parseInt(half.gitHashHex.slice(i * 2, i * 2 + 2), 16));
  }
  w.u8(half.dirty ? 1 : 0);
  w.bytes([0, 0, 0]); // reserved
}

function readHalfVersion(r: ReaderCursor): HalfVersion {
  const present = readBit(r, "present");
  const fwMajor = r.u8();
  const fwMinor = r.u8();
  const fwPatch = r.u8();
  const gitHashHex = [...r.bytes(GIT_HASH_LEN)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  const dirty = readBit(r, "dirty");
  r.bytes(3); // reserved, ignored
  return { present, fwMajor, fwMinor, fwPatch, gitHashHex, dirty };
}

// --- request codec --------------------------------------------------------

export function encodeRequest(requestId: number, request: Request): Uint8Array {
  const w = new Writer();
  w.u8(OPCODES[request.command]);
  w.u8(requestId);
  w.u16(0); // payload_len, patched below
  switch (request.command) {
    case "getCapabilities":
      w.u8(request.clientMajor);
      w.u8(request.clientMinor);
      break;
    case "ping":
      if (request.data.length > MAX_PING_LEN) {
        throw new ProtocolError(`ping payload exceeds ${MAX_PING_LEN} bytes`);
      }
      w.bytes(request.data);
      break;
    case "getVersion":
      break;
    case "setCells":
    case "replaceOverlay":
      writeCells(w, request.ttlMs, request.cells);
      break;
    case "unsetCells":
      if (request.keys.length > MAX_CELLS_PER_MESSAGE) {
        throw new ProtocolError(`too many keys (max ${MAX_CELLS_PER_MESSAGE})`);
      }
      w.u8(request.keys.length);
      w.bytes(request.keys);
      break;
    case "clearOverlay":
    case "readOverlay":
    case "getBrightness":
      break;
    case "setBrightness":
      w.u8(request.level);
      break;
    case "getToggle":
      w.u8(request.id);
      break;
    case "setToggle":
      w.u8(request.id);
      w.u8(request.state ? 1 : 0);
      break;
    case "configBegin":
      w.u32(request.totalLen);
      w.u32(request.blobCrc32);
      break;
    case "configData":
      if (request.data.length > MAX_CONFIG_DATA_PER_MESSAGE) {
        throw new ProtocolError(`config chunk exceeds ${MAX_CONFIG_DATA_PER_MESSAGE} bytes`);
      }
      w.u32(request.offset);
      w.bytes(request.data);
      break;
    case "configCommit":
    case "configAbort":
      break;
    case "configRead":
      w.u32(request.offset);
      w.u16(request.maxLen);
      break;
    case "keymapRead":
      w.u8(request.layer);
      w.u8(request.startKey);
      w.u8(request.maxCount);
      break;
    case "keymapWrite":
      if (request.entries.length > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
        throw new ProtocolError(`too many keymap entries (max ${MAX_KEYMAP_ENTRIES_PER_MESSAGE})`);
      }
      w.u8(request.entries.length);
      for (const entry of request.entries) {
        w.u8(entry.layer);
        w.u8(entry.key);
        w.u16(entry.keycode);
      }
      break;
    case "enterBootloader":
      w.u32(request.magic);
      w.u8(request.target === "peripheral" ? 1 : 0);
      break;
  }
  w.patchU16(2, w.pos - REQUEST_HEADER_LEN);
  return w.finish();
}

export interface DecodedRequest {
  requestId: number;
  request: Request;
}

export function decodeRequest(bytes: Uint8Array): DecodedRequest {
  const r = new ReaderCursor(bytes);
  const opcode = r.u8();
  const command = COMMAND_BY_OPCODE.get(opcode);
  if (command === undefined) {
    throw new ProtocolError(`unknown opcode 0x${opcode.toString(16)}`);
  }
  const requestId = r.u8();
  const payloadLen = r.u16();
  if (r.remaining !== payloadLen) {
    throw new ProtocolError("length field disagrees with buffer");
  }
  let request: Request;
  switch (command) {
    case "getCapabilities":
      request = { command, clientMajor: r.u8(), clientMinor: r.u8() };
      break;
    case "ping":
      if (payloadLen > MAX_PING_LEN) throw new ProtocolError("ping payload too long");
      request = { command, data: r.bytes(payloadLen) };
      break;
    case "getVersion":
      request = { command };
      break;
    case "setCells":
    case "replaceOverlay":
      request = { command, ...readCells(r) };
      break;
    case "unsetCells": {
      const count = r.u8();
      if (count > MAX_CELLS_PER_MESSAGE) throw new ProtocolError("count exceeds codec capacity");
      request = { command, keys: [...r.bytes(count)] };
      break;
    }
    case "clearOverlay":
    case "readOverlay":
    case "getBrightness":
      request = { command };
      break;
    case "setBrightness":
      request = { command, level: r.u8() };
      break;
    case "getToggle":
      request = { command, id: r.u8() };
      break;
    case "setToggle": {
      const id = r.u8();
      const stateByte = r.u8();
      if (stateByte > 1) throw new ProtocolError(`toggle state must be 0 or 1, got ${stateByte}`);
      request = { command, id, state: stateByte === 1 };
      break;
    }
    case "configBegin":
      request = { command, totalLen: r.u32(), blobCrc32: r.u32() };
      break;
    case "configData": {
      const offset = r.u32();
      if (r.remaining > MAX_CONFIG_DATA_PER_MESSAGE) {
        throw new ProtocolError("config chunk exceeds codec capacity");
      }
      request = { command, offset, data: r.bytes(r.remaining) };
      break;
    }
    case "configCommit":
    case "configAbort":
      request = { command };
      break;
    case "configRead":
      request = { command, offset: r.u32(), maxLen: r.u16() };
      break;
    case "keymapRead":
      request = { command, layer: r.u8(), startKey: r.u8(), maxCount: r.u8() };
      break;
    case "keymapWrite": {
      const count = r.u8();
      if (count > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
        throw new ProtocolError("count exceeds codec capacity");
      }
      const entries: KeymapEntry[] = [];
      for (let i = 0; i < count; i++) {
        entries.push({ layer: r.u8(), key: r.u8(), keycode: r.u16() });
      }
      request = { command, entries };
      break;
    }
    case "enterBootloader": {
      const magic = r.u32();
      const targetByte = r.u8();
      if (targetByte > 1) throw new ProtocolError(`unknown boot target ${targetByte}`);
      request = { command, magic, target: targetByte === 1 ? "peripheral" : "central" };
      break;
    }
  }
  r.finish();
  return { requestId, request };
}

// --- response codec -------------------------------------------------------

function payloadMatches(command: CommandName, status: StatusName, payload: ResponsePayload): boolean {
  if (status === "ok") {
    switch (payload.type) {
      case "capabilities":
        return command === "getCapabilities";
      case "echo":
        return command === "ping";
      case "overlayAck":
        return OVERLAY_WRITE_COMMANDS.has(command);
      case "overlayState":
        return command === "readOverlay";
      case "brightness":
        return command === "getBrightness" || command === "setBrightness";
      case "toggle":
        return command === "getToggle" || command === "setToggle";
      case "configData":
        return command === "configRead";
      case "version":
        return command === "getVersion";
      case "keymapActions":
        return command === "keymapRead";
      case "keymapWritten":
        return command === "keymapWrite";
      case "empty":
        return (
          command === "enterBootloader" ||
          command === "configBegin" ||
          command === "configData" ||
          command === "configCommit" ||
          command === "configAbort"
        );
    }
  }
  if (status === "partialApply") {
    return OVERLAY_WRITE_COMMANDS.has(command) && payload.type === "overlayAck";
  }
  return payload.type === "empty";
}

export function encodeResponse(response: Response): Uint8Array {
  const { command, status, payload } = response;
  if (!payloadMatches(command, status, payload)) {
    throw new ProtocolError("response payload does not match command/status");
  }
  const w = new Writer();
  w.u8(OPCODES[command] | RESPONSE_FLAG);
  w.u8(response.requestId);
  w.u8(STATUS_VALUES[status]);
  w.u16(0); // payload_len, patched below
  switch (payload.type) {
    case "empty":
      break;
    case "capabilities":
      w.u8(payload.protocolMajor);
      w.u8(payload.protocolMinor);
      w.u8(payload.ledCountLeft);
      w.u8(payload.ledCountRight);
      w.u8(payload.layerCapacity);
      w.u8(payload.maxCellsPerOp);
      w.u16(payload.effectMask);
      w.u16(payload.overlayCellCapacity);
      w.u16(payload.maxMessageLen);
      w.u32(payload.featureBits);
      // Extensions append in feature-bit order (PROTOCOL.md "Versioning").
      if ((payload.featureBits & FEATURE_PERSISTENT_CONFIG) !== 0) {
        w.u32(payload.maxConfigBlobLen);
      }
      if ((payload.featureBits & FEATURE_KEYMAP) !== 0) {
        w.u8(payload.keymapRows);
        w.u8(payload.keymapCols);
        w.u8(payload.maxKeymapEntriesPerOp);
        w.u8(0); // reserved
      }
      break;
    case "echo":
      if (payload.data.length > MAX_PING_LEN) {
        throw new ProtocolError(`echo payload exceeds ${MAX_PING_LEN} bytes`);
      }
      w.bytes(payload.data);
      break;
    case "overlayAck":
      if (payload.pendingKeys.length > MAX_CELLS_PER_MESSAGE) {
        throw new ProtocolError(`too many pending keys (max ${MAX_CELLS_PER_MESSAGE})`);
      }
      w.u8(payload.pendingKeys.length);
      w.bytes(payload.pendingKeys);
      break;
    case "overlayState":
      if (payload.cells.length > MAX_CELLS_PER_MESSAGE) {
        throw new ProtocolError(`too many cells (max ${MAX_CELLS_PER_MESSAGE})`);
      }
      w.u8(payload.cells.length);
      for (const cell of payload.cells) {
        w.u8(cell.key);
        writeEffect(w, cell.effect);
        w.u32(cell.remainingTtlMs);
      }
      break;
    case "brightness":
      w.u8(payload.level);
      break;
    case "toggle":
      w.u8(payload.id);
      w.u8(payload.state ? 1 : 0);
      break;
    case "configData":
      if (payload.data.length > MAX_CONFIG_DATA_PER_MESSAGE) {
        throw new ProtocolError(`config chunk exceeds ${MAX_CONFIG_DATA_PER_MESSAGE} bytes`);
      }
      w.u32(payload.totalLen);
      w.bytes(payload.data);
      break;
    case "version":
      writeHalfVersion(w, payload.central);
      writeHalfVersion(w, payload.peripheral);
      w.u8(payload.halvesMismatch ? 1 : 0);
      break;
    case "keymapActions":
      if (payload.keycodes.length > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
        throw new ProtocolError(`too many keycodes (max ${MAX_KEYMAP_ENTRIES_PER_MESSAGE})`);
      }
      w.u8(payload.layer);
      w.u8(payload.startKey);
      w.u8(payload.keycodes.length);
      for (const keycode of payload.keycodes) w.u16(keycode);
      break;
    case "keymapWritten":
      if (payload.keycodes.length > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
        throw new ProtocolError(`too many keycodes (max ${MAX_KEYMAP_ENTRIES_PER_MESSAGE})`);
      }
      w.u8(payload.keycodes.length);
      for (const keycode of payload.keycodes) w.u16(keycode);
      break;
  }
  w.patchU16(3, w.pos - RESPONSE_HEADER_LEN);
  return w.finish();
}

export function decodeResponse(bytes: Uint8Array): Response {
  const r = new ReaderCursor(bytes);
  const opcode = r.u8();
  if ((opcode & RESPONSE_FLAG) === 0) {
    throw new ProtocolError(`not a response opcode 0x${opcode.toString(16)}`);
  }
  const command = COMMAND_BY_OPCODE.get(opcode & ~RESPONSE_FLAG);
  if (command === undefined) {
    throw new ProtocolError(`unknown opcode 0x${opcode.toString(16)}`);
  }
  const requestId = r.u8();
  const statusByte = r.u8();
  const status = STATUS_BY_VALUE.get(statusByte);
  if (status === undefined) {
    throw new ProtocolError(`unknown status 0x${statusByte.toString(16)}`);
  }
  const payloadLen = r.u16();
  if (r.remaining !== payloadLen) {
    throw new ProtocolError("length field disagrees with buffer");
  }
  let payload: ResponsePayload;
  if (status === "ok") {
    switch (command) {
      case "getCapabilities": {
        const caps = {
          type: "capabilities" as const,
          protocolMajor: r.u8(),
          protocolMinor: r.u8(),
          ledCountLeft: r.u8(),
          ledCountRight: r.u8(),
          layerCapacity: r.u8(),
          maxCellsPerOp: r.u8(),
          effectMask: r.u16(),
          overlayCellCapacity: r.u16(),
          maxMessageLen: r.u16(),
          featureBits: r.u32(),
          maxConfigBlobLen: 0,
          keymapRows: 0,
          keymapCols: 0,
          maxKeymapEntriesPerOp: 0,
        };
        if ((caps.featureBits & FEATURE_PERSISTENT_CONFIG) !== 0) {
          caps.maxConfigBlobLen = r.u32();
        }
        if ((caps.featureBits & FEATURE_KEYMAP) !== 0) {
          caps.keymapRows = r.u8();
          caps.keymapCols = r.u8();
          caps.maxKeymapEntriesPerOp = r.u8();
          r.u8(); // reserved
        }
        // Newer protocol minors append further extensions in feature-bit
        // order (PROTOCOL.md "Versioning"). A v1.3 client skips what it
        // does not understand rather than rejecting the handshake.
        r.bytes(r.remaining);
        payload = caps;
        break;
      }
      case "ping":
        if (payloadLen > MAX_PING_LEN) throw new ProtocolError("echo payload too long");
        payload = { type: "echo", data: r.bytes(payloadLen) };
        break;
      case "getVersion": {
        const central = readHalfVersion(r);
        const peripheral = readHalfVersion(r);
        const halvesMismatch = readBit(r, "halves_mismatch");
        payload = { type: "version", central, peripheral, halvesMismatch };
        break;
      }
      case "setCells":
      case "unsetCells":
      case "clearOverlay":
      case "replaceOverlay":
        payload = readOverlayAck(r);
        break;
      case "readOverlay": {
        const count = r.u8();
        if (count > MAX_CELLS_PER_MESSAGE) throw new ProtocolError("count exceeds codec capacity");
        const cells: CellState[] = [];
        for (let i = 0; i < count; i++) {
          cells.push({ key: r.u8(), effect: readEffect(r), remainingTtlMs: r.u32() });
        }
        payload = { type: "overlayState", cells };
        break;
      }
      case "getBrightness":
      case "setBrightness":
        payload = { type: "brightness", level: r.u8() };
        break;
      case "getToggle":
      case "setToggle": {
        const id = r.u8();
        const stateByte = r.u8();
        if (stateByte > 1) throw new ProtocolError(`toggle state must be 0 or 1, got ${stateByte}`);
        payload = { type: "toggle", id, state: stateByte === 1 };
        break;
      }
      case "configBegin":
      case "configData":
      case "configCommit":
      case "configAbort":
      case "enterBootloader":
        payload = { type: "empty" };
        break;
      case "configRead": {
        const totalLen = r.u32();
        if (r.remaining > MAX_CONFIG_DATA_PER_MESSAGE) {
          throw new ProtocolError("config chunk exceeds codec capacity");
        }
        payload = { type: "configData", totalLen, data: r.bytes(r.remaining) };
        break;
      }
      case "keymapRead": {
        const layer = r.u8();
        const startKey = r.u8();
        const count = r.u8();
        if (count > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
          throw new ProtocolError("count exceeds codec capacity");
        }
        const keycodes: number[] = [];
        for (let i = 0; i < count; i++) keycodes.push(r.u16());
        payload = { type: "keymapActions", layer, startKey, keycodes };
        break;
      }
      case "keymapWrite": {
        const count = r.u8();
        if (count > MAX_KEYMAP_ENTRIES_PER_MESSAGE) {
          throw new ProtocolError("count exceeds codec capacity");
        }
        const keycodes: number[] = [];
        for (let i = 0; i < count; i++) keycodes.push(r.u16());
        payload = { type: "keymapWritten", keycodes };
        break;
      }
    }
  } else if (status === "partialApply") {
    if (!OVERLAY_WRITE_COMMANDS.has(command)) {
      throw new ProtocolError("partialApply is only valid on overlay writes");
    }
    payload = readOverlayAck(r);
  } else {
    payload = { type: "empty" };
  }
  r.finish();
  return { requestId, command, status, payload };
}

function readOverlayAck(r: ReaderCursor): ResponsePayload {
  const count = r.u8();
  if (count > MAX_CELLS_PER_MESSAGE) throw new ProtocolError("count exceeds codec capacity");
  return { type: "overlayAck", pendingKeys: [...r.bytes(count)] };
}

// --- frame layer (per-transport segmentation) -----------------------------

export const FRAME_HEADER_LEN = 2;
export const FRAME_FINAL_FLAG = 0x80;
export const FRAME_SEQ_MASK = 0x7f;
export const MAX_FRAMES_PER_MESSAGE = 128;
export const MIN_CHUNK_LEN = FRAME_HEADER_LEN + 1;

function payloadPerFrame(chunkLen: number): number {
  if (chunkLen < MIN_CHUNK_LEN) throw new ProtocolError("chunk size below minimum (3)");
  return Math.min(chunkLen - FRAME_HEADER_LEN, 255);
}

/**
 * Split an encoded message into transport chunks. With `pad`, every frame is
 * zero-padded to `chunkLen` (USB HID fixed-size reports); without, frames
 * are exactly header + payload (BLE GATT writes).
 */
export function splitFrames(message: Uint8Array, chunkLen: number, pad = false): Uint8Array[] {
  if (message.length === 0) throw new ProtocolError("cannot frame an empty message");
  const per = payloadPerFrame(chunkLen);
  const count = Math.ceil(message.length / per);
  if (count > MAX_FRAMES_PER_MESSAGE) {
    throw new ProtocolError(`message exceeds ${MAX_FRAMES_PER_MESSAGE} frames`);
  }
  const frames: Uint8Array[] = [];
  for (let i = 0; i < count; i++) {
    const payload = message.subarray(i * per, Math.min((i + 1) * per, message.length));
    const frame = new Uint8Array(pad ? chunkLen : FRAME_HEADER_LEN + payload.length);
    frame[0] = i === count - 1 ? i | FRAME_FINAL_FLAG : i;
    frame[1] = payload.length;
    frame.set(payload, FRAME_HEADER_LEN);
    frames.push(frame);
  }
  return frames;
}

/**
 * Reassembles one message at a time. A frame with sequence 0 always starts a
 * new message (dropping an incomplete one); errors reset the reassembler.
 */
export class Reassembler {
  private chunks: Uint8Array[] = [];
  private length = 0;
  private nextSeq = 0;

  reset(): void {
    this.chunks = [];
    this.length = 0;
    this.nextSeq = 0;
  }

  /** Feed one received chunk (padding beyond the declared payload length is
   * ignored). Returns the complete message on the FINAL frame, else null. */
  push(frame: Uint8Array): Uint8Array | null {
    if (frame.length < FRAME_HEADER_LEN) {
      this.reset();
      throw new ProtocolError("frame shorter than its header");
    }
    const control = frame[0];
    const seq = control & FRAME_SEQ_MASK;
    const isFinal = (control & FRAME_FINAL_FLAG) !== 0;
    const payloadLen = frame[1];
    if (payloadLen === 0) {
      this.reset();
      throw new ProtocolError("frame has zero-length payload");
    }
    if (frame.length < FRAME_HEADER_LEN + payloadLen) {
      this.reset();
      throw new ProtocolError("frame shorter than declared payload");
    }
    if (seq === 0) {
      this.chunks = [];
      this.length = 0;
    } else if (seq !== this.nextSeq) {
      const expected = this.nextSeq;
      this.reset();
      throw new ProtocolError(`expected frame sequence ${expected}, got ${seq}`);
    }
    if (this.length + payloadLen > MAX_MESSAGE_LEN) {
      this.reset();
      throw new ProtocolError("reassembled message exceeds MAX_MESSAGE_LEN");
    }
    this.chunks.push(frame.slice(FRAME_HEADER_LEN, FRAME_HEADER_LEN + payloadLen));
    this.length += payloadLen;
    if (isFinal) {
      const message = new Uint8Array(this.length);
      let at = 0;
      for (const chunk of this.chunks) {
        message.set(chunk, at);
        at += chunk.length;
      }
      this.reset();
      return message;
    }
    if (seq === MAX_FRAMES_PER_MESSAGE - 1) {
      this.reset();
      throw new ProtocolError(`message exceeds ${MAX_FRAMES_PER_MESSAGE} frames`);
    }
    this.nextSeq = seq + 1;
    return null;
  }
}

// --- persistent lighting config blob (v1.1) -------------------------------
//
// The unit of persistence and transfer for CONFIG_BEGIN/DATA/COMMIT/READ.
// Mirrors protocol/glove80-host-protocol/src/config.rs; byte layout in
// PROTOCOL.md ("Persistent configuration (v1.1)").

/** Blob magic ("G80L" read as a little-endian u32). */
export const CONFIG_MAGIC = 0x4c303847;
export const CONFIG_VERSION = 1;
export const CONFIG_HEADER_LEN = 16;
export const CONFIG_BODY_HEADER_LEN = 12;
export const CONFIG_RECORD_HEADER_LEN = 5;
export const MAX_CONFIG_RECORDS = 16;
export const MAX_CELLS_PER_RECORD = 40;
export const CONFIG_KEY_COUNT = 80;
export const CONFIG_LAYER_COUNT = 8;
export const CONFIG_TOGGLE_COUNT = 32;
/** Largest possible blob: header + body prefix + 16 full records. */
export const MAX_CONFIG_BLOB_LEN =
  CONFIG_HEADER_LEN +
  CONFIG_BODY_HEADER_LEN +
  MAX_CONFIG_RECORDS *
    (CONFIG_RECORD_HEADER_LEN + MAX_CELLS_PER_RECORD * (1 + EFFECT_ENCODED_LEN));

/** Activation predicate of a persistable record. Host-overlay and status
 * records are runtime state and deliberately not representable. */
export type ConfigActivation =
  | { kind: "always" }
  | { kind: "layerActive"; layer: number }
  | { kind: "toggle"; id: number };

export interface ConfigRecord {
  activation: ConfigActivation;
  /** Sparse key → effect map; a key absent here is transparent and a key
   * may appear at most once. */
  cells: CellWrite[];
}

export interface LightingConfig {
  /** Bit n set ⇔ toggle n's runtime state is persisted across reboots. */
  togglePersistMask: number;
  /** Bit n = toggle n's state on boot (for non-persisted toggles). */
  toggleInitialState: number;
  /** Blob order = composition order within each activation class. */
  records: ConfigRecord[];
}

// Half-byte lookup table for CRC-32/ISO-HDLC (reflected poly 0xEDB88320).
const CRC32_TABLE = [
  0x00000000, 0x1db71064, 0x3b6e20c8, 0x26d930ac, 0x76dc4190, 0x6b6b51f4, 0x4db26158, 0x5005713c,
  0xedb88320, 0xf00f9344, 0xd6d6a3e8, 0xcb61b38c, 0x9b64c2b0, 0x86d3d2d4, 0xa00ae278, 0xbdbdf21c,
];

/** CRC-32/ISO-HDLC (the zlib/PNG CRC). crc32("123456789") = 0xCBF43926. */
export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const b of bytes) {
    crc = (crc >>> 4) ^ CRC32_TABLE[(crc ^ b) & 0xf];
    crc = (crc >>> 4) ^ CRC32_TABLE[(crc ^ (b >>> 4)) & 0xf];
  }
  return ~crc >>> 0;
}

function activationToWire(a: ConfigActivation): [number, number] {
  switch (a.kind) {
    case "always":
      return [0, 0];
    case "layerActive":
      if (a.layer >= CONFIG_LAYER_COUNT || a.layer < 0) {
        throw new ProtocolError(`layer ${a.layer} out of range (< ${CONFIG_LAYER_COUNT})`);
      }
      return [1, a.layer];
    case "toggle":
      if (a.id >= CONFIG_TOGGLE_COUNT || a.id < 0) {
        throw new ProtocolError(`toggle ${a.id} out of range (< ${CONFIG_TOGGLE_COUNT})`);
      }
      return [2, a.id];
  }
}

function activationFromWire(kind: number, arg: number): ConfigActivation {
  switch (kind) {
    case 0:
      return { kind: "always" };
    case 1:
      if (arg >= CONFIG_LAYER_COUNT) {
        throw new ProtocolError(`layer ${arg} out of range (< ${CONFIG_LAYER_COUNT})`);
      }
      return { kind: "layerActive", layer: arg };
    case 2:
      if (arg >= CONFIG_TOGGLE_COUNT) {
        throw new ProtocolError(`toggle ${arg} out of range (< ${CONFIG_TOGGLE_COUNT})`);
      }
      return { kind: "toggle", id: arg };
    default:
      throw new ProtocolError(`unknown activation ${kind}`);
  }
}

/** Encode a config as a complete blob (header + body, CRC filled in). The
 * output is canonical: encoding a decoded blob reproduces it byte-for-byte. */
export function encodeLightingConfig(config: LightingConfig): Uint8Array {
  if (config.records.length > MAX_CONFIG_RECORDS) {
    throw new ProtocolError(`too many records (max ${MAX_CONFIG_RECORDS})`);
  }
  const w = new Writer(MAX_CONFIG_BLOB_LEN);
  w.u32(CONFIG_MAGIC);
  w.u16(CONFIG_VERSION);
  w.u16(0); // reserved
  w.u32(0); // body_len, patched below
  w.u32(0); // body_crc32, patched below
  w.u8(config.records.length);
  w.u32(config.togglePersistMask);
  w.u32(config.toggleInitialState);
  w.bytes([0, 0, 0]); // reserved
  for (const record of config.records) {
    if (record.cells.length > MAX_CELLS_PER_RECORD) {
      throw new ProtocolError(`too many cells (max ${MAX_CELLS_PER_RECORD})`);
    }
    const [kind, arg] = activationToWire(record.activation);
    w.u8(kind);
    w.u8(arg);
    w.u16(0); // reserved
    w.u8(record.cells.length);
    const seen = new Set<number>();
    for (const cell of record.cells) {
      if (cell.key >= CONFIG_KEY_COUNT || cell.key < 0) {
        throw new ProtocolError(`key ${cell.key} out of range (< ${CONFIG_KEY_COUNT})`);
      }
      if (seen.has(cell.key)) {
        throw new ProtocolError(`key ${cell.key} appears twice in one record`);
      }
      seen.add(cell.key);
      w.u8(cell.key);
      writeEffect(w, cell.effect);
    }
  }
  w.patchU32(8, w.pos - CONFIG_HEADER_LEN);
  w.patchU32(12, crc32(w.written().subarray(CONFIG_HEADER_LEN)));
  return w.finish();
}

/** Decode and fully validate a config blob: magic, version, lengths, body
 * CRC, record/cell counts, activation and effect kinds, key/layer/toggle
 * ranges, and per-record key uniqueness. Any error means the blob must not
 * be applied. */
export function decodeLightingConfig(bytes: Uint8Array): LightingConfig {
  const r = new ReaderCursor(bytes);
  const magic = r.u32();
  if (magic !== CONFIG_MAGIC) {
    throw new ProtocolError(`bad config magic 0x${magic.toString(16)}`);
  }
  const version = r.u16();
  if (version !== CONFIG_VERSION) {
    throw new ProtocolError(`unsupported config version ${version}`);
  }
  r.u16(); // reserved
  const bodyLen = r.u32();
  const expectedCrc = r.u32();
  if (r.remaining !== bodyLen) {
    throw new ProtocolError("config length fields disagree with blob");
  }
  const actualCrc = crc32(bytes.subarray(CONFIG_HEADER_LEN));
  if (actualCrc !== expectedCrc) {
    throw new ProtocolError(
      `config body crc mismatch: header 0x${expectedCrc.toString(16)}, body 0x${actualCrc.toString(16)}`,
    );
  }
  const recordCount = r.u8();
  if (recordCount > MAX_CONFIG_RECORDS) {
    throw new ProtocolError(`record count ${recordCount} exceeds ${MAX_CONFIG_RECORDS}`);
  }
  const togglePersistMask = r.u32();
  const toggleInitialState = r.u32();
  r.bytes(3); // reserved
  const records: ConfigRecord[] = [];
  for (let i = 0; i < recordCount; i++) {
    const kind = r.u8();
    const arg = r.u8();
    const activation = activationFromWire(kind, arg);
    r.u16(); // reserved
    const cellCount = r.u8();
    if (cellCount > MAX_CELLS_PER_RECORD) {
      throw new ProtocolError(`cell count ${cellCount} exceeds ${MAX_CELLS_PER_RECORD}`);
    }
    const seen = new Set<number>();
    const cells: CellWrite[] = [];
    for (let c = 0; c < cellCount; c++) {
      const key = r.u8();
      if (key >= CONFIG_KEY_COUNT) {
        throw new ProtocolError(`key ${key} out of range (< ${CONFIG_KEY_COUNT})`);
      }
      if (seen.has(key)) {
        throw new ProtocolError(`key ${key} appears twice in one record`);
      }
      seen.add(key);
      cells.push({ key, effect: readEffect(r) });
    }
    records.push({ activation, cells });
  }
  r.finish();
  return { togglePersistMask, toggleInitialState, records };
}
