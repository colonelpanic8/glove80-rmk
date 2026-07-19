import { describe, expect, it } from "vitest";

import {
  crc32,
  encodeLightingConfig,
  type Effect,
  type LightingConfig,
  type Request,
} from "./host-protocol";
import { MockKeyboard, MOCK_CAPABILITIES, MOCK_CENTRAL_VERSION, NEVER_SEEN_VERSION } from "./mock-device";

const SOLID_RED: Effect = { kind: "solid", r: 255, g: 0, b: 0, periodMs: 0, phaseMs: 0, dutyPercent: 0 };
const BLINK_BLUE: Effect = { kind: "blink", r: 0, g: 0, b: 255, periodMs: 1000, phaseMs: 0, dutyPercent: 50 };

function makeClock(start = 1_000) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

function send(keyboard: MockKeyboard, request: Request) {
  return keyboard.handle(7, request);
}

const SAMPLE_CONFIG: LightingConfig = {
  togglePersistMask: 0,
  toggleInitialState: 1 << 3,
  records: [
    { activation: { kind: "always" }, cells: [{ key: 0, effect: SOLID_RED }] },
    { activation: { kind: "toggle", id: 3 }, cells: [{ key: 79, effect: BLINK_BLUE }] },
  ],
};

function applyBlob(keyboard: MockKeyboard, blob: Uint8Array, blobCrc = crc32(blob)) {
  expect(send(keyboard, { command: "configBegin", totalLen: blob.length, blobCrc32: blobCrc }).status).toBe("ok");
  const chunk = 100;
  for (let offset = 0; offset < blob.length; offset += chunk) {
    const status = send(keyboard, {
      command: "configData",
      offset,
      data: blob.subarray(offset, Math.min(offset + chunk, blob.length)),
    }).status;
    expect(status).toBe("ok");
  }
  return send(keyboard, { command: "configCommit" });
}

describe("MockKeyboard capabilities", () => {
  it("answers GET_CAPABILITIES with the advertised capabilities", () => {
    const response = send(new MockKeyboard(), { command: "getCapabilities", clientMajor: 1, clientMinor: 1 });
    expect(response.status).toBe("ok");
    expect(response.payload).toMatchObject({ type: "capabilities", ...MOCK_CAPABILITIES });
  });

  it("rejects an unsupported client major version", () => {
    const response = send(new MockKeyboard(), { command: "getCapabilities", clientMajor: 2, clientMinor: 0 });
    expect(response.status).toBe("unsupportedVersion");
  });
});

describe("MockKeyboard overlay", () => {
  it("merges SET_CELLS and reads them back", () => {
    const keyboard = new MockKeyboard();
    send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 4, effect: SOLID_RED }] });
    send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 44, effect: BLINK_BLUE }] });
    const read = send(keyboard, { command: "readOverlay" });
    expect(read.payload).toEqual({
      type: "overlayState",
      cells: [
        { key: 4, effect: SOLID_RED, remainingTtlMs: 0 },
        { key: 44, effect: BLINK_BLUE, remainingTtlMs: 0 },
      ],
    });
  });

  it("rejects out-of-range keys and oversized batches", () => {
    const keyboard = new MockKeyboard();
    expect(send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 80, effect: SOLID_RED }] }).status).toBe("outOfRange");
    const big = Array.from({ length: 41 }, (_, key) => ({ key, effect: SOLID_RED }));
    expect(send(keyboard, { command: "setCells", ttlMs: 0, cells: big }).status).toBe("capacityExceeded");
    expect(keyboard.overlaySize()).toBe(0);
  });

  it("expires TTL cells on the injected clock", () => {
    const clock = makeClock();
    const keyboard = new MockKeyboard({ now: clock.now });
    send(keyboard, { command: "setCells", ttlMs: 500, cells: [{ key: 1, effect: SOLID_RED }] });
    send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 2, effect: SOLID_RED }] });
    clock.advance(400);
    let read = send(keyboard, { command: "readOverlay" });
    expect(read.payload).toMatchObject({
      cells: [
        { key: 1, remainingTtlMs: 100 },
        { key: 2, remainingTtlMs: 0 },
      ],
    });
    clock.advance(200);
    read = send(keyboard, { command: "readOverlay" });
    expect(read.payload).toMatchObject({ cells: [{ key: 2 }] });
  });

  it("REPLACE_OVERLAY atomically replaces; UNSET and CLEAR remove", () => {
    const keyboard = new MockKeyboard();
    send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 1, effect: SOLID_RED }, { key: 2, effect: SOLID_RED }] });
    send(keyboard, { command: "replaceOverlay", ttlMs: 0, cells: [{ key: 9, effect: BLINK_BLUE }] });
    expect(send(keyboard, { command: "readOverlay" }).payload).toMatchObject({ cells: [{ key: 9 }] });
    send(keyboard, { command: "unsetCells", keys: [9] });
    expect(keyboard.overlaySize()).toBe(0);
    send(keyboard, { command: "setCells", ttlMs: 0, cells: [{ key: 3, effect: SOLID_RED }] });
    send(keyboard, { command: "clearOverlay" });
    expect(keyboard.overlaySize()).toBe(0);
  });

  it("reports PARTIAL_APPLY for right-half writes while the peripheral is offline", () => {
    const keyboard = new MockKeyboard({ peripheralOffline: true });
    const response = send(keyboard, {
      command: "setCells",
      ttlMs: 0,
      cells: [{ key: 5, effect: SOLID_RED }, { key: 45, effect: SOLID_RED }, { key: 41, effect: SOLID_RED }],
    });
    expect(response.status).toBe("partialApply");
    expect(response.payload).toEqual({ type: "overlayAck", pendingKeys: [41, 45] });
    // A bare right-half clear: PARTIAL_APPLY with an empty pending list.
    const clear = send(keyboard, { command: "clearOverlay" });
    expect(clear.status).toBe("partialApply");
    expect(clear.payload).toEqual({ type: "overlayAck", pendingKeys: [] });
  });
});

describe("MockKeyboard brightness and toggles", () => {
  it("stores brightness and echoes the applied level", () => {
    const keyboard = new MockKeyboard();
    expect(send(keyboard, { command: "setBrightness", level: 90 }).payload).toEqual({ type: "brightness", level: 90 });
    expect(send(keyboard, { command: "getBrightness" }).payload).toEqual({ type: "brightness", level: 90 });
  });

  it("only knows toggles the active config defines", () => {
    const keyboard = new MockKeyboard({ initialConfig: SAMPLE_CONFIG });
    expect(send(keyboard, { command: "getToggle", id: 3 }).payload).toEqual({ type: "toggle", id: 3, state: true });
    expect(send(keyboard, { command: "setToggle", id: 3, state: false }).payload).toEqual({ type: "toggle", id: 3, state: false });
    expect(send(keyboard, { command: "getToggle", id: 9 }).status).toBe("unknownToggle");
  });
});

describe("MockKeyboard config session", () => {
  it("applies a valid blob transactionally and serves it back byte-stable", () => {
    const keyboard = new MockKeyboard();
    const blob = encodeLightingConfig(SAMPLE_CONFIG);
    expect(applyBlob(keyboard, blob).status).toBe("ok");
    expect(keyboard.activeConfigBlob()).toEqual(blob);
    // Chunked CONFIG_READ reproduces the committed bytes.
    const parts: Uint8Array[] = [];
    let offset = 0;
    for (;;) {
      const response = send(keyboard, { command: "configRead", offset, maxLen: 33 });
      expect(response.status).toBe("ok");
      const payload = response.payload as { type: "configData"; totalLen: number; data: Uint8Array };
      expect(payload.totalLen).toBe(blob.length);
      if (payload.data.length === 0) break;
      parts.push(payload.data);
      offset += payload.data.length;
      if (offset === blob.length) break;
    }
    expect(Uint8Array.from(parts.flatMap((part) => [...part]))).toEqual(blob);
  });

  it("reports an empty config as total_len 0", () => {
    const response = send(new MockKeyboard(), { command: "configRead", offset: 0, maxLen: 100 });
    expect(response.payload).toEqual({ type: "configData", totalLen: 0, data: new Uint8Array(0) });
  });

  it("answers NO_SESSION without a BEGIN", () => {
    const keyboard = new MockKeyboard();
    expect(send(keyboard, { command: "configData", offset: 0, data: new Uint8Array(4) }).status).toBe("noSession");
    expect(send(keyboard, { command: "configCommit" }).status).toBe("noSession");
    expect(send(keyboard, { command: "configAbort" }).status).toBe("ok"); // idempotent
  });

  it("aborts the session on a non-contiguous offset", () => {
    const keyboard = new MockKeyboard();
    const blob = encodeLightingConfig(SAMPLE_CONFIG);
    send(keyboard, { command: "configBegin", totalLen: blob.length, blobCrc32: crc32(blob) });
    expect(send(keyboard, { command: "configData", offset: 4, data: blob.subarray(4, 8) }).status).toBe("badOffset");
    expect(send(keyboard, { command: "configData", offset: 0, data: blob.subarray(0, 4) }).status).toBe("noSession");
  });

  it("rejects an early commit with CONFIG_INCOMPLETE", () => {
    const keyboard = new MockKeyboard();
    const blob = encodeLightingConfig(SAMPLE_CONFIG);
    send(keyboard, { command: "configBegin", totalLen: blob.length, blobCrc32: crc32(blob) });
    send(keyboard, { command: "configData", offset: 0, data: blob.subarray(0, 8) });
    expect(send(keyboard, { command: "configCommit" }).status).toBe("configIncomplete");
  });

  it("rejects a wrong announced CRC with CRC_MISMATCH and keeps the old config", () => {
    const keyboard = new MockKeyboard({ initialConfig: SAMPLE_CONFIG });
    const before = keyboard.activeConfigBlob();
    const blob = encodeLightingConfig({ togglePersistMask: 0, toggleInitialState: 0, records: [] });
    expect(applyBlob(keyboard, blob, crc32(blob) ^ 1).status).toBe("crcMismatch");
    expect(keyboard.activeConfigBlob()).toEqual(before);
  });

  it("rejects a structurally invalid blob with INVALID_CONFIG and keeps the old config", () => {
    const keyboard = new MockKeyboard({ initialConfig: SAMPLE_CONFIG });
    const before = keyboard.activeConfigBlob();
    const blob = encodeLightingConfig(SAMPLE_CONFIG);
    // Corrupt a record's activation kind, then re-seal the CRCs so only
    // structural validation can catch it.
    const bad = blob.slice();
    bad[28] = 9; // record 0 activation kind → unknown
    const view = new DataView(bad.buffer);
    view.setUint32(12, crc32(bad.subarray(16)), true);
    expect(applyBlob(keyboard, bad).status).toBe("invalidConfig");
    expect(keyboard.activeConfigBlob()).toEqual(before);
  });

  it("rejects a blob beyond max_config_blob_len at BEGIN", () => {
    const keyboard = new MockKeyboard({ capabilities: { maxConfigBlobLen: 64 } });
    expect(send(keyboard, { command: "configBegin", totalLen: 65, blobCrc32: 0 }).status).toBe("capacityExceeded");
  });
});

describe("MockKeyboard keymap", () => {
  it("reads the seeded base layer in chunks with the spec count rule", () => {
    const keyboard = new MockKeyboard();
    const first = send(keyboard, { command: "keymapRead", layer: 0, startKey: 0, maxCount: 84 });
    expect(first.status).toBe("ok");
    const payload = first.payload as { type: "keymapActions"; keycodes: number[] };
    expect(payload.keycodes).toHaveLength(84);
    expect(payload.keycodes[0]).toBe(0x003a); // KC_F1 at r0,c0
    // Holes read back KC_NO.
    for (const hole of [5, 8, 75, 78]) expect(payload.keycodes[hole]).toBe(0x0000);
    // Count clamps to the end of the grid.
    const tail = send(keyboard, { command: "keymapRead", layer: 0, startKey: 80, maxCount: 84 });
    expect((tail.payload as { keycodes: number[] }).keycodes).toHaveLength(4);
    // maxCount 0 answers count 0.
    const empty = send(keyboard, { command: "keymapRead", layer: 3, startKey: 0, maxCount: 0 });
    expect((empty.payload as { keycodes: number[] }).keycodes).toHaveLength(0);
    // Upper layers default to transparent.
    const upper = send(keyboard, { command: "keymapRead", layer: 1, startKey: 0, maxCount: 4 });
    expect((upper.payload as { keycodes: number[] }).keycodes).toEqual([1, 1, 1, 1]);
  });

  it("rejects out-of-range reads", () => {
    const keyboard = new MockKeyboard();
    expect(send(keyboard, { command: "keymapRead", layer: 8, startKey: 0, maxCount: 1 }).status).toBe("outOfRange");
    expect(send(keyboard, { command: "keymapRead", layer: 0, startKey: 84, maxCount: 1 }).status).toBe("outOfRange");
  });

  it("writes are all-or-nothing and echo the canonical read-back", () => {
    const keyboard = new MockKeyboard();
    const before = keyboard.keycodeAt(0, 0);
    // One bad entry rejects the whole batch; nothing changes.
    const rejected = send(keyboard, {
      command: "keymapWrite",
      entries: [
        { layer: 0, key: 0, keycode: 0x0004 },
        { layer: 0, key: 84, keycode: 0x0004 },
      ],
    });
    expect(rejected.status).toBe("outOfRange");
    expect(keyboard.keycodeAt(0, 0)).toBe(before);

    // A good batch applies in order (later entries win) and reads back what
    // was actually stored: TT(3) has no RMK representation → KC_NO (LOSSY).
    const written = send(keyboard, {
      command: "keymapWrite",
      entries: [
        { layer: 0, key: 0, keycode: 0x0004 }, // KC_A
        { layer: 0, key: 0, keycode: 0x0005 }, // KC_B wins
        { layer: 2, key: 10, keycode: 0x52c3 }, // TT(3) → KC_NO
      ],
    });
    expect(written.status).toBe("ok");
    expect(written.payload).toEqual({ type: "keymapWritten", keycodes: [0x0005, 0x0005, 0x0000] });
    expect(keyboard.keycodeAt(0, 0)).toBe(0x0005);
    expect(keyboard.keycodeAt(2, 10)).toBe(0x0000);
    // Reads reflect the live keymap.
    const read = send(keyboard, { command: "keymapRead", layer: 0, startKey: 0, maxCount: 1 });
    expect((read.payload as { keycodes: number[] }).keycodes).toEqual([0x0005]);
  });

  it("rejects oversized batches with CAPACITY_EXCEEDED", () => {
    const keyboard = new MockKeyboard();
    const entries = Array.from({ length: 85 }, (_, i) => ({ layer: 0, key: i % 84, keycode: 4 }));
    expect(send(keyboard, { command: "keymapWrite", entries }).status).toBe("capacityExceeded");
  });

  it("answers UNKNOWN_COMMAND when the keymap feature bit is off", () => {
    const keyboard = new MockKeyboard({
      capabilities: { featureBits: MOCK_CAPABILITIES.featureBits & ~(1 << 7) },
    });
    expect(send(keyboard, { command: "keymapRead", layer: 0, startKey: 0, maxCount: 1 }).status).toBe("unknownCommand");
    expect(send(keyboard, { command: "keymapWrite", entries: [] }).status).toBe("unknownCommand");
  });
});

describe("MockKeyboard version", () => {
  it("answers GET_VERSION with a matching pair by default", () => {
    const response = send(new MockKeyboard(), { command: "getVersion" });
    expect(response.status).toBe("ok");
    expect(response.payload).toEqual({
      type: "version",
      central: MOCK_CENTRAL_VERSION,
      peripheral: MOCK_CENTRAL_VERSION,
      halvesMismatch: false,
    });
  });

  it("supports the mismatch and never-seen modes for UI testing", () => {
    const mismatched = new MockKeyboard({ versionMode: "mismatch" });
    const response = send(mismatched, { command: "getVersion" });
    expect(response.payload).toMatchObject({ halvesMismatch: true, central: { dirty: true } });

    const neverSeen = new MockKeyboard({ versionMode: "peripheralNeverSeen" });
    const payload = send(neverSeen, { command: "getVersion" }).payload;
    expect(payload).toMatchObject({ halvesMismatch: false, peripheral: NEVER_SEEN_VERSION });
  });

  it("answers UNKNOWN_COMMAND when the version feature bit is off", () => {
    const keyboard = new MockKeyboard({
      capabilities: { featureBits: MOCK_CAPABILITIES.featureBits & ~(1 << 8) },
    });
    expect(send(keyboard, { command: "getVersion" }).status).toBe("unknownCommand");
  });
});
