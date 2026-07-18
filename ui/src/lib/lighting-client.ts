import { get_decoder, get_encoder } from "@zmkfirmware/zmk-studio-ts-client/framing";
import type { RpcTransport } from "@zmkfirmware/zmk-studio-ts-client/transport/index";

import {
  ApplyResult,
  decodeStudioResponse,
  encodeClear,
  encodeGetCapabilities,
  encodeSetPixels,
  type LightingCapabilities,
  type LightingResponse,
  type PixelUpdate,
} from "./protobuf";

export interface LightingClient {
  readonly label: string;
  readonly capabilities: LightingCapabilities;
  setPixels(pixels: PixelUpdate[], replace?: boolean, timeoutMs?: number): Promise<void>;
  applyFrame(colorsByPixel: readonly number[], timeoutMs?: number): Promise<void>;
  clear(): Promise<void>;
  close(): Promise<void>;
}

export class LightingApplyError extends Error {
  constructor(readonly result: ApplyResult) {
    super(`Keyboard rejected the lighting update (${ApplyResult[result] ?? result})`);
  }
}

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function scaleToChannelLimit(rgb: number, maximum: number): number {
  const red = (rgb >>> 16) & 0xff;
  const green = (rgb >>> 8) & 0xff;
  const blue = rgb & 0xff;
  const peak = Math.max(red, green, blue);
  if (peak <= maximum || peak === 0) return rgb;
  const scale = maximum / peak;
  return (
    (Math.round(red * scale) << 16) |
    (Math.round(green * scale) << 8) |
    Math.round(blue * scale)
  );
}

export class ZmkLightingClient implements LightingClient {
  private requestId = 1;
  private operation = Promise.resolve();
  private readonly requestWriter: WritableStreamDefaultWriter<Uint8Array>;
  private readonly responseReader: ReadableStreamDefaultReader<Uint8Array>;

  private constructor(
    private readonly transport: RpcTransport,
    readonly capabilities: LightingCapabilities,
  ) {
    const requestStream = new TransformStream<Uint8Array, Uint8Array>(get_encoder());
    requestStream.readable.pipeTo(transport.writable).catch(() => undefined);
    this.requestWriter = requestStream.writable.getWriter();
    this.responseReader = transport.readable
      .pipeThrough(new TransformStream<Uint8Array, Uint8Array>(get_decoder()))
      .getReader();
  }

  static async connect(transport: RpcTransport): Promise<ZmkLightingClient> {
    const provisional = new ZmkLightingClient(transport, {
      protocolVersion: 0,
      pixelCount: 0,
      pixelsPerHalf: 0,
      maxUpdatesPerRequest: 0,
      maxUpdateHz: 0,
      defaultTimeoutMs: 0,
      maxTimeoutMs: 0,
      maxChannelValue: 0,
      supportsReplace: false,
      supportsSplit: false,
    });
    const response = await provisional.call(encodeGetCapabilities(provisional.nextRequestId()));
    if (response.kind !== "capabilities") {
      await provisional.close();
      throw new Error("The selected device does not expose the host-lighting protocol");
    }
    if (response.capabilities.protocolVersion !== 1) {
      await provisional.close();
      throw new Error(
        `Unsupported host-lighting protocol version ${response.capabilities.protocolVersion}`,
      );
    }
    Object.assign(provisional.capabilities, response.capabilities);
    return provisional;
  }

  get label(): string {
    return this.transport.label;
  }

  private nextRequestId(): number {
    const current = this.requestId;
    this.requestId = (this.requestId + 1) >>> 0;
    return current;
  }

  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const queued = this.operation.then(operation, operation);
    this.operation = queued.then(
      () => undefined,
      () => undefined,
    );
    return queued;
  }

  private async call(payload: Uint8Array): Promise<LightingResponse> {
    const expectedId = this.requestId - 1;
    await this.requestWriter.write(payload);
    while (true) {
      const { done, value } = await this.responseReader.read();
      if (done || !value) throw new Error("The keyboard disconnected before responding");
      const response = decodeStudioResponse(value);
      if (response.kind === "notification") continue;
      if (response.requestId !== expectedId) {
        throw new Error(
          `Mismatched Studio response: expected ${expectedId}, received ${response.requestId}`,
        );
      }
      return response;
    }
  }

  async setPixels(
    pixels: PixelUpdate[],
    replace = false,
    timeoutMs = this.capabilities.defaultTimeoutMs,
  ): Promise<void> {
    return this.enqueue(async () => {
      const limit = this.capabilities.maxUpdatesPerRequest;
      if (pixels.length > limit) {
        throw new Error(`At most ${limit} pixels may be sent in one update`);
      }
      const safePixels = pixels.map((pixel) => ({
        ...pixel,
        rgb: scaleToChannelLimit(pixel.rgb, this.capabilities.maxChannelValue),
      }));
      const response = await this.call(
        encodeSetPixels(this.nextRequestId(), safePixels, replace, timeoutMs),
      );
      if (response.kind !== "setPixels") throw new Error("Unexpected lighting response");
      if (response.result !== ApplyResult.Ok) throw new LightingApplyError(response.result);
    });
  }

  async applyFrame(
    colorsByPixel: readonly number[],
    timeoutMs = this.capabilities.defaultTimeoutMs,
  ): Promise<void> {
    const updates = colorsByPixel.map((rgb, index) => ({ index, rgb }));
    const chunkSize = this.capabilities.maxUpdatesPerRequest;
    const interval = Math.ceil(1000 / this.capabilities.maxUpdateHz);
    for (let offset = 0; offset < updates.length || offset === 0; offset += chunkSize) {
      await this.setPixels(updates.slice(offset, offset + chunkSize), offset === 0, timeoutMs);
      if (offset + chunkSize < updates.length) await wait(interval);
    }
  }

  async clear(): Promise<void> {
    return this.enqueue(async () => {
      const response = await this.call(encodeClear(this.nextRequestId()));
      if (response.kind !== "clear") throw new Error("Unexpected lighting response");
      if (response.result !== ApplyResult.Ok) throw new LightingApplyError(response.result);
    });
  }

  async close(): Promise<void> {
    this.requestWriter.releaseLock();
    this.responseReader.releaseLock();
    this.transport.abortController.abort();
  }
}
