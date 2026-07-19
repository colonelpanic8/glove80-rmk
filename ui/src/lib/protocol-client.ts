// Request/response client for the Glove80 host protocol.
//
// Owns frame split/reassembly, request-id correlation, the one-in-flight
// rule, timeouts, and the v1.1 config transfer session. Transport-agnostic:
// give it anything implementing Transport (transport.ts).

import {
  BOOTLOADER_MAGIC,
  decodeResponse,
  encodeRequest,
  FEATURE_KEYMAP,
  FEATURE_PERSISTENT_CONFIG,
  MAX_CONFIG_DATA_PER_MESSAGE,
  MAX_KEYMAP_ENTRIES_PER_MESSAGE,
  PROTOCOL_VERSION_MAJOR,
  PROTOCOL_VERSION_MINOR,
  ProtocolError,
  Reassembler,
  REQUEST_HEADER_LEN,
  RESPONSE_HEADER_LEN,
  splitFrames,
  crc32,
  type BootTarget,
  type Capabilities,
  type CellState,
  type CellWrite,
  type KeymapEntry,
  type Request,
  type Response,
  type StatusName,
  type VersionInfo,
} from "./host-protocol";
import type { Transport } from "./transport";

export const DEFAULT_TIMEOUT_MS = 5_000;

/** A response arrived but its status was not OK (or an allowed alternative). */
export class StatusError extends Error {
  constructor(
    readonly status: StatusName,
    readonly command: string,
  ) {
    super(`${command}: keyboard answered ${describeStatus(status)}`);
    this.name = "StatusError";
  }
}

export function describeStatus(status: StatusName): string {
  switch (status) {
    case "ok":
      return "OK";
    case "unknownCommand":
      return "UNKNOWN_COMMAND — the firmware does not understand this request";
    case "malformed":
      return "MALFORMED — the payload failed to parse";
    case "outOfRange":
      return "OUT_OF_RANGE — a key or value is outside the advertised capacity";
    case "capacityExceeded":
      return "CAPACITY_EXCEEDED — batch or overlay larger than the device allows";
    case "partialApply":
      return "PARTIAL_APPLY — applied on the central; the other half is pending";
    case "busy":
      return "BUSY — try again";
    case "unknownToggle":
      return "UNKNOWN_TOGGLE — that toggle id is not configured";
    case "badMagic":
      return "BAD_MAGIC — bootloader magic rejected";
    case "unsupportedVersion":
      return "UNSUPPORTED_VERSION — protocol major version not supported";
    case "noSession":
      return "NO_SESSION — no config transfer session is open";
    case "badOffset":
      return "BAD_OFFSET — config data arrived out of order; the session was aborted";
    case "configIncomplete":
      return "CONFIG_INCOMPLETE — commit before all announced bytes arrived";
    case "crcMismatch":
      return "CRC_MISMATCH — the transferred blob failed its checksum; the old config is untouched";
    case "invalidConfig":
      return "INVALID_CONFIG — the blob failed validation; the old config is untouched";
  }
}

export interface OverlayWriteResult {
  /** True when the write answered PARTIAL_APPLY (peripheral offline). */
  partial: boolean;
  /** Right-half keys accepted on the central but not yet on the peripheral. */
  pendingKeys: number[];
}

export type ConfigApplyStage =
  | { stage: "begin" }
  | { stage: "transfer"; sent: number; total: number }
  | { stage: "commit" }
  | { stage: "done" };

export type ConfigReadProgress = { read: number; total: number };

interface Pending {
  requestId: number;
  command: Request["command"];
  resolve: (response: Response) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/**
 * One client per connection. `connect()` performs the mandatory
 * GET_CAPABILITIES handshake and caches the result on `capabilities`.
 */
export class ProtocolClient {
  private nextRequestId = 0;
  private pending: Pending | null = null;
  private queue: Array<() => void> = [];
  private reassembler = new Reassembler();
  private closed = false;
  private disconnectHandler: (() => void) | null = null;
  capabilities: Capabilities | null = null;

  constructor(readonly transport: Transport) {
    transport.onChunk((chunk) => this.handleChunk(chunk));
    transport.onDisconnect(() => {
      this.failPending(new ProtocolError("connection lost"));
      this.closed = true;
      this.disconnectHandler?.();
    });
  }

  onDisconnect(handler: () => void): void {
    this.disconnectHandler = handler;
  }

  async close(): Promise<void> {
    this.closed = true;
    this.failPending(new ProtocolError("connection closed"));
    await this.transport.close();
  }

  private handleChunk(chunk: Uint8Array): void {
    let message: Uint8Array | null;
    try {
      message = this.reassembler.push(chunk);
    } catch (error) {
      this.failPending(error instanceof Error ? error : new ProtocolError(String(error)));
      return;
    }
    if (message === null) return;
    let response: Response;
    try {
      response = decodeResponse(message);
    } catch (error) {
      this.failPending(error instanceof Error ? error : new ProtocolError(String(error)));
      return;
    }
    const pending = this.pending;
    if (!pending) return; // unsolicited response; nothing is waiting
    if (response.requestId !== pending.requestId || response.command !== pending.command) {
      this.failPending(
        new ProtocolError(
          `response mismatch: expected ${pending.command}#${pending.requestId}, ` +
            `got ${response.command}#${response.requestId}`,
        ),
      );
      return;
    }
    clearTimeout(pending.timer);
    this.pending = null;
    pending.resolve(response);
    this.drainQueue();
  }

  private failPending(error: Error): void {
    const pending = this.pending;
    this.pending = null;
    this.reassembler.reset();
    if (pending) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.drainQueue();
  }

  private drainQueue(): void {
    const next = this.queue.shift();
    next?.();
  }

  /** Send one request and await its response (any status). Requests are
   * serialized: one in flight per transport, as the protocol requires. */
  async request(request: Request, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<Response> {
    if (this.closed) throw new ProtocolError("connection closed");
    if (this.pending) {
      await new Promise<void>((resolve) => this.queue.push(resolve));
      if (this.closed) throw new ProtocolError("connection closed");
    }
    const requestId = this.nextRequestId;
    this.nextRequestId = (this.nextRequestId + 1) & 0xff;
    const message = encodeRequest(requestId, request);
    const frames = splitFrames(message, this.transport.chunkSize, this.transport.pad);
    return new Promise<Response>((resolve, reject) => {
      this.pending = {
        requestId,
        command: request.command,
        resolve,
        reject,
        timer: setTimeout(() => {
          this.failPending(new ProtocolError(`${request.command}: keyboard did not answer`));
        }, timeoutMs),
      };
      void (async () => {
        try {
          for (const frame of frames) {
            await this.transport.sendChunk(frame);
          }
        } catch (error) {
          this.failPending(error instanceof Error ? error : new ProtocolError(String(error)));
        }
      })();
    });
  }

  private async requestOk(request: Request, timeoutMs?: number): Promise<Response> {
    const response = await this.request(request, timeoutMs);
    if (response.status !== "ok") {
      throw new StatusError(response.status, request.command);
    }
    return response;
  }

  /** Mandatory first exchange. Caches and returns the capabilities. */
  async connect(): Promise<Capabilities> {
    const response = await this.requestOk({
      command: "getCapabilities",
      clientMajor: PROTOCOL_VERSION_MAJOR,
      clientMinor: PROTOCOL_VERSION_MINOR,
    });
    if (response.payload.type !== "capabilities") throw new ProtocolError("bad capabilities payload");
    const { type: _type, ...caps } = response.payload;
    this.capabilities = caps;
    return caps;
  }

  private overlayResult(response: Response, command: string): OverlayWriteResult {
    if (response.status === "ok" || response.status === "partialApply") {
      if (response.payload.type !== "overlayAck") throw new ProtocolError("bad overlay ack");
      return { partial: response.status === "partialApply", pendingKeys: response.payload.pendingKeys };
    }
    throw new StatusError(response.status, command);
  }

  async setCells(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult> {
    return this.overlayResult(await this.request({ command: "setCells", ttlMs, cells }), "setCells");
  }

  async replaceOverlay(ttlMs: number, cells: CellWrite[]): Promise<OverlayWriteResult> {
    return this.overlayResult(
      await this.request({ command: "replaceOverlay", ttlMs, cells }),
      "replaceOverlay",
    );
  }

  async unsetCells(keys: number[]): Promise<OverlayWriteResult> {
    return this.overlayResult(await this.request({ command: "unsetCells", keys }), "unsetCells");
  }

  async clearOverlay(): Promise<OverlayWriteResult> {
    return this.overlayResult(await this.request({ command: "clearOverlay" }), "clearOverlay");
  }

  async readOverlay(): Promise<CellState[]> {
    const response = await this.requestOk({ command: "readOverlay" });
    if (response.payload.type !== "overlayState") throw new ProtocolError("bad overlay state");
    return response.payload.cells;
  }

  async getBrightness(): Promise<number> {
    const response = await this.requestOk({ command: "getBrightness" });
    if (response.payload.type !== "brightness") throw new ProtocolError("bad brightness payload");
    return response.payload.level;
  }

  async setBrightness(level: number): Promise<number> {
    const response = await this.requestOk({ command: "setBrightness", level });
    if (response.payload.type !== "brightness") throw new ProtocolError("bad brightness payload");
    return response.payload.level;
  }

  async getToggle(id: number): Promise<boolean> {
    const response = await this.requestOk({ command: "getToggle", id });
    if (response.payload.type !== "toggle") throw new ProtocolError("bad toggle payload");
    return response.payload.state;
  }

  async setToggle(id: number, state: boolean): Promise<boolean> {
    const response = await this.requestOk({ command: "setToggle", id, state });
    if (response.payload.type !== "toggle") throw new ProtocolError("bad toggle payload");
    return response.payload.state;
  }

  /** GET_VERSION (v1.3): both halves' build identity in one exchange. */
  async getVersion(): Promise<VersionInfo> {
    const response = await this.requestOk({ command: "getVersion" });
    if (response.payload.type !== "version") throw new ProtocolError("bad version payload");
    const { type: _type, ...info } = response.payload;
    return info;
  }

  private keymapChunkLen(): number {
    const advertised = this.capabilities?.maxKeymapEntriesPerOp ?? 0;
    return advertised > 0
      ? Math.min(advertised, MAX_KEYMAP_ENTRIES_PER_MESSAGE)
      : MAX_KEYMAP_ENTRIES_PER_MESSAGE;
  }

  private requireKeymapSupport(): void {
    const caps = this.capabilities;
    if (caps && (caps.featureBits & FEATURE_KEYMAP) === 0) {
      throw new ProtocolError("this keyboard does not support keymap editing");
    }
  }

  /** Read one whole layer (or the leading `keyCount` positions) as VIA
   * keycodes, chunking KEYMAP_READ per the advertised per-op limit. */
  async readKeymapLayer(layer: number, keyCount?: number): Promise<number[]> {
    this.requireKeymapSupport();
    const caps = this.capabilities;
    const gridSize = caps ? caps.keymapRows * caps.keymapCols : 0;
    const wanted = keyCount ?? gridSize;
    if (wanted <= 0) throw new ProtocolError("keymap grid size unknown; connect first");
    const chunk = this.keymapChunkLen();
    const keycodes: number[] = [];
    while (keycodes.length < wanted) {
      const startKey = keycodes.length;
      const response = await this.requestOk({
        command: "keymapRead",
        layer,
        startKey,
        maxCount: Math.min(chunk, wanted - startKey),
      });
      if (response.payload.type !== "keymapActions") throw new ProtocolError("bad keymap payload");
      if (response.payload.startKey !== startKey || response.payload.keycodes.length === 0) {
        throw new ProtocolError("keyboard answered an unexpected keymap chunk");
      }
      keycodes.push(...response.payload.keycodes);
    }
    return keycodes.slice(0, wanted);
  }

  /** Write entries in advertised-size batches (each batch all-or-nothing on
   * the device). Returns the canonical read-back keycodes, request order —
   * compare with what you sent to surface lossy mappings. */
  async writeKeymap(entries: KeymapEntry[]): Promise<number[]> {
    this.requireKeymapSupport();
    const chunk = this.keymapChunkLen();
    const readback: number[] = [];
    for (let at = 0; at < entries.length; at += chunk) {
      const batch = entries.slice(at, at + chunk);
      const response = await this.requestOk({ command: "keymapWrite", entries: batch });
      if (response.payload.type !== "keymapWritten") throw new ProtocolError("bad keymap payload");
      if (response.payload.keycodes.length !== batch.length) {
        throw new ProtocolError("keymap read-back length mismatch");
      }
      readback.push(...response.payload.keycodes);
    }
    return readback;
  }

  async enterBootloader(target: BootTarget): Promise<void> {
    // The OK response may never arrive (the device resets); tolerate timeout.
    try {
      await this.requestOk({ command: "enterBootloader", magic: BOOTLOADER_MAGIC, target }, 2_000);
    } catch (error) {
      if (error instanceof StatusError) throw error;
    }
  }

  private configChunkLen(): number {
    const maxMessage = this.capabilities?.maxMessageLen ?? 0;
    const budget = Math.max(
      maxMessage - REQUEST_HEADER_LEN - 4, // request header + offset field
      maxMessage - RESPONSE_HEADER_LEN - 4, // response header + total_len — same 4
      0,
    );
    return maxMessage > 0
      ? Math.min(MAX_CONFIG_DATA_PER_MESSAGE, budget)
      : MAX_CONFIG_DATA_PER_MESSAGE;
  }

  /** Run the full v1.1 transactional apply: BEGIN → DATA… → COMMIT. On any
   * failure the previous config is untouched (StatusError says which). */
  async applyConfigBlob(
    blob: Uint8Array,
    onProgress?: (stage: ConfigApplyStage) => void,
  ): Promise<void> {
    const caps = this.capabilities;
    if (caps && (caps.featureBits & FEATURE_PERSISTENT_CONFIG) === 0) {
      throw new ProtocolError("this keyboard does not support persistent config");
    }
    if (caps && caps.maxConfigBlobLen > 0 && blob.length > caps.maxConfigBlobLen) {
      throw new ProtocolError(
        `config blob is ${blob.length} bytes; the keyboard accepts at most ${caps.maxConfigBlobLen}`,
      );
    }
    onProgress?.({ stage: "begin" });
    await this.requestOk({ command: "configBegin", totalLen: blob.length, blobCrc32: crc32(blob) });
    const chunkLen = Math.min(this.configChunkLen(), MAX_CONFIG_DATA_PER_MESSAGE);
    try {
      for (let offset = 0; offset < blob.length; offset += chunkLen) {
        const data = blob.subarray(offset, Math.min(offset + chunkLen, blob.length));
        await this.requestOk({ command: "configData", offset, data });
        onProgress?.({ stage: "transfer", sent: offset + data.length, total: blob.length });
      }
      onProgress?.({ stage: "commit" });
      await this.requestOk({ command: "configCommit" });
    } catch (error) {
      // Leave no half-open session behind on transfer errors. (COMMIT always
      // ends the session itself, ABORT is idempotent — safe either way.)
      await this.request({ command: "configAbort" }).catch(() => undefined);
      throw error;
    }
    onProgress?.({ stage: "done" });
  }

  /** Read back the active config blob byte-for-byte. Returns null when the
   * device has no stored config (total_len = 0). */
  async readConfigBlob(onProgress?: (progress: ConfigReadProgress) => void): Promise<Uint8Array | null> {
    const chunkLen = this.configChunkLen();
    const parts: Uint8Array[] = [];
    let read = 0;
    let total = 0;
    do {
      const response = await this.requestOk({ command: "configRead", offset: read, maxLen: chunkLen });
      if (response.payload.type !== "configData") throw new ProtocolError("bad config payload");
      total = response.payload.totalLen;
      if (total === 0) return null;
      if (response.payload.data.length === 0 && read < total) {
        throw new ProtocolError("keyboard answered an empty config chunk mid-read");
      }
      parts.push(response.payload.data);
      read += response.payload.data.length;
      onProgress?.({ read, total });
    } while (read < total);
    const blob = new Uint8Array(total);
    let at = 0;
    for (const part of parts) {
      blob.set(part, at);
      at += part.length;
    }
    return blob;
  }
}
