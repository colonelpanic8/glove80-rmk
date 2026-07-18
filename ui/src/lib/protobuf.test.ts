import { describe, expect, it } from "vitest";

import {
  ApplyResult,
  decodeStudioResponse,
  encodeClear,
  encodeGetCapabilities,
  encodeSetPixels,
} from "./protobuf";

describe("host-lighting protobuf codec", () => {
  it("encodes the Studio envelope and custom subsystem field", () => {
    expect([...encodeGetCapabilities(7)]).toEqual([0x08, 0x07, 0x32, 0x02, 0x08, 0x01]);
    expect([...encodeClear(9)]).toEqual([0x08, 0x09, 0x32, 0x02, 0x18, 0x01]);
  });

  it("encodes bounded pixel updates", () => {
    expect([...encodeSetPixels(3, [{ index: 40, rgb: 0x12ab34 }], true, 5000)]).toEqual([
      0x08, 0x03, 0x32, 0x0f, 0x12, 0x0d, 0x0a, 0x06, 0x08, 0x28, 0x10, 0xb4,
      0xd6, 0x4a, 0x10, 0x01, 0x18, 0x88, 0x27,
    ]);
  });

  it("decodes capability and apply responses", () => {
    const capabilities = Uint8Array.from([
      0x0a, 0x13,
      0x08, 0x01, 0x10, 0x50, 0x18, 0x28, 0x20, 0x08, 0x28, 0x14,
      0x30, 0x88, 0x27, 0x38, 0xb0, 0xea, 0x01, 0x40, 0x60,
    ]);
    const envelope = Uint8Array.from([
      0x0a, capabilities.length + 4, 0x08, 0x2a, 0x32, capabilities.length, ...capabilities,
    ]);
    const decoded = decodeStudioResponse(envelope);
    expect(decoded.kind).toBe("capabilities");
    if (decoded.kind === "capabilities") {
      expect(decoded.requestId).toBe(42);
      expect(decoded.capabilities).toMatchObject({
        protocolVersion: 1,
        pixelCount: 80,
        pixelsPerHalf: 40,
        maxUpdatesPerRequest: 8,
        maxUpdateHz: 20,
        maxChannelValue: 96,
      });
    }

    expect(decodeStudioResponse(Uint8Array.from([0x0a, 0x06, 0x08, 0x05, 0x32, 0x02, 0x10, 0x02]))).toEqual({
      requestId: 5,
      kind: "setPixels",
      result: ApplyResult.Partial,
    });
  });
});
