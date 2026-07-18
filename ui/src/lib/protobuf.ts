export type PixelUpdate = {
  index: number;
  rgb: number;
};

export type LightingCapabilities = {
  protocolVersion: number;
  pixelCount: number;
  pixelsPerHalf: number;
  maxUpdatesPerRequest: number;
  maxUpdateHz: number;
  defaultTimeoutMs: number;
  maxTimeoutMs: number;
  maxChannelValue: number;
  supportsReplace: boolean;
  supportsSplit: boolean;
};

export enum ApplyResult {
  Ok = 0,
  InvalidPixel = 1,
  Partial = 2,
  PeripheralUnavailable = 3,
  InternalError = 4,
}

export type LightingResponse =
  | { requestId: number; kind: "capabilities"; capabilities: LightingCapabilities }
  | { requestId: number; kind: "setPixels" | "clear"; result: ApplyResult }
  | { requestId: number; kind: "notification" }
  | { requestId: number; kind: "unknown" };

class Writer {
  readonly data: number[] = [];

  uint32(value: number): void {
    let remaining = value >>> 0;
    while (remaining >= 0x80) {
      this.data.push((remaining & 0x7f) | 0x80);
      remaining >>>= 7;
    }
    this.data.push(remaining);
  }

  tag(field: number, wireType: number): void {
    this.uint32((field << 3) | wireType);
  }

  bool(field: number, value: boolean): void {
    this.tag(field, 0);
    this.uint32(value ? 1 : 0);
  }

  fieldUint32(field: number, value: number): void {
    this.tag(field, 0);
    this.uint32(value);
  }

  message(field: number, write: (writer: Writer) => void): void {
    const child = new Writer();
    write(child);
    this.tag(field, 2);
    this.uint32(child.data.length);
    this.data.push(...child.data);
  }

  finish(): Uint8Array {
    return Uint8Array.from(this.data);
  }
}

class Reader {
  position = 0;

  constructor(readonly data: Uint8Array) {}

  get done(): boolean {
    return this.position >= this.data.length;
  }

  uint32(): number {
    let result = 0;
    let shift = 0;
    while (shift < 35 && this.position < this.data.length) {
      const byte = this.data[this.position++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) {
        return result >>> 0;
      }
      shift += 7;
    }
    throw new Error("Invalid protobuf varint");
  }

  bytes(): Uint8Array {
    const length = this.uint32();
    const end = this.position + length;
    if (end > this.data.length) {
      throw new Error("Truncated protobuf field");
    }
    const value = this.data.subarray(this.position, end);
    this.position = end;
    return value;
  }

  skip(wireType: number): void {
    if (wireType === 0) {
      this.uint32();
      return;
    }
    if (wireType === 2) {
      this.bytes();
      return;
    }
    if (wireType === 1) {
      this.position += 8;
      return;
    }
    if (wireType === 5) {
      this.position += 4;
      return;
    }
    throw new Error(`Unsupported protobuf wire type ${wireType}`);
  }
}

function readFields(
  bytes: Uint8Array,
  visit: (field: number, wireType: number, reader: Reader) => void,
): void {
  const reader = new Reader(bytes);
  while (!reader.done) {
    const tag = reader.uint32();
    visit(tag >>> 3, tag & 0x07, reader);
  }
}

function encodeEnvelope(requestId: number, writeHostRequest: (writer: Writer) => void): Uint8Array {
  const writer = new Writer();
  writer.fieldUint32(1, requestId);
  writer.message(6, writeHostRequest);
  return writer.finish();
}

export function encodeGetCapabilities(requestId: number): Uint8Array {
  return encodeEnvelope(requestId, (host) => host.bool(1, true));
}

export function encodeSetPixels(
  requestId: number,
  pixels: PixelUpdate[],
  replace: boolean,
  timeoutMs: number,
): Uint8Array {
  return encodeEnvelope(requestId, (host) => {
    host.message(2, (request) => {
      for (const pixel of pixels) {
        request.message(1, (item) => {
          item.fieldUint32(1, pixel.index);
          item.fieldUint32(2, pixel.rgb);
        });
      }
      if (replace) request.bool(2, true);
      if (timeoutMs > 0) request.fieldUint32(3, timeoutMs);
    });
  });
}

export function encodeClear(requestId: number): Uint8Array {
  return encodeEnvelope(requestId, (host) => host.bool(3, true));
}

function decodeCapabilities(bytes: Uint8Array): LightingCapabilities {
  const values = new Map<number, number>();
  readFields(bytes, (field, wireType, reader) => {
    if (wireType === 0) values.set(field, reader.uint32());
    else reader.skip(wireType);
  });
  return {
    protocolVersion: values.get(1) ?? 0,
    pixelCount: values.get(2) ?? 0,
    pixelsPerHalf: values.get(3) ?? 0,
    maxUpdatesPerRequest: values.get(4) ?? 0,
    maxUpdateHz: values.get(5) ?? 0,
    defaultTimeoutMs: values.get(6) ?? 0,
    maxTimeoutMs: values.get(7) ?? 0,
    maxChannelValue: values.get(8) ?? 0,
    supportsReplace: values.get(9) === 1,
    supportsSplit: values.get(10) === 1,
  };
}

function decodeHostResponse(requestId: number, bytes: Uint8Array): LightingResponse {
  let response: LightingResponse = { requestId, kind: "unknown" };
  readFields(bytes, (field, wireType, reader) => {
    if (field === 1 && wireType === 2) {
      response = {
        requestId,
        kind: "capabilities",
        capabilities: decodeCapabilities(reader.bytes()),
      };
    } else if ((field === 2 || field === 3) && wireType === 0) {
      response = {
        requestId,
        kind: field === 2 ? "setPixels" : "clear",
        result: reader.uint32() as ApplyResult,
      };
    } else {
      reader.skip(wireType);
    }
  });
  return response;
}

function decodeRequestResponse(bytes: Uint8Array): LightingResponse {
  let requestId = 0;
  let hostResponse: Uint8Array | undefined;
  readFields(bytes, (field, wireType, reader) => {
    if (field === 1 && wireType === 0) requestId = reader.uint32();
    else if (field === 6 && wireType === 2) hostResponse = reader.bytes();
    else reader.skip(wireType);
  });
  return hostResponse
    ? decodeHostResponse(requestId, hostResponse)
    : { requestId, kind: "unknown" };
}

export function decodeStudioResponse(bytes: Uint8Array): LightingResponse {
  let response: LightingResponse = { requestId: 0, kind: "unknown" };
  readFields(bytes, (field, wireType, reader) => {
    if (field === 1 && wireType === 2) response = decodeRequestResponse(reader.bytes());
    else if (field === 2 && wireType === 2) {
      reader.bytes();
      response = { requestId: 0, kind: "notification" };
    } else reader.skip(wireType);
  });
  return response;
}
