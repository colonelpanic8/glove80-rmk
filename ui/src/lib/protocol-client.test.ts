import { describe, expect, it } from "vitest";

import {
  encodeLightingConfig,
  type Effect,
  type LightingConfig,
} from "./host-protocol";
import { MockKeyboard, MockTransport } from "./mock-device";
import { ProtocolClient, StatusError, type ConfigApplyStage } from "./protocol-client";

const GREEN: Effect = { kind: "solid", r: 0, g: 255, b: 0, periodMs: 0, phaseMs: 0, dutyPercent: 0 };

function makeClient(keyboard = new MockKeyboard()) {
  const transport = new MockTransport(keyboard);
  return { keyboard, transport, client: new ProtocolClient(transport) };
}

const CONFIG: LightingConfig = {
  togglePersistMask: 0,
  toggleInitialState: 0,
  records: [
    {
      activation: { kind: "always" },
      cells: Array.from({ length: 40 }, (_, key) => ({ key: key * 2, effect: GREEN })),
    },
    { activation: { kind: "layerActive", layer: 2 }, cells: [{ key: 1, effect: GREEN }] },
  ],
};

describe("ProtocolClient over MockTransport", () => {
  it("performs the capability handshake through 32-byte padded frames", async () => {
    const { client } = makeClient();
    const caps = await client.connect();
    expect(caps.ledCountLeft).toBe(40);
    expect(caps.ledCountRight).toBe(40);
    expect(client.capabilities).toEqual(caps);
  });

  it("correlates queued requests by request id", async () => {
    const { client } = makeClient();
    await client.connect();
    // Fire several requests without awaiting; the client must serialize them
    // (one in flight) and route every response to its caller.
    const results = await Promise.all([
      client.setBrightness(10),
      client.setBrightness(20),
      client.getBrightness(),
    ]);
    expect(results).toEqual([10, 20, 20]);
  });

  it("round-trips a multi-frame overlay write and read-back", async () => {
    const { client, keyboard } = makeClient();
    await client.connect();
    const cells = Array.from({ length: 40 }, (_, key) => ({ key, effect: GREEN }));
    const result = await client.setCells(1500, cells);
    expect(result).toEqual({ partial: false, pendingKeys: [] });
    expect(keyboard.overlaySize()).toBe(40);
    const state = await client.readOverlay();
    expect(state).toHaveLength(40);
    expect(state[0].effect).toEqual(GREEN);
    expect(state[0].remainingTtlMs).toBeGreaterThan(0);
  });

  it("surfaces PARTIAL_APPLY as a result, not an error", async () => {
    const { client } = makeClient(new MockKeyboard({ peripheralOffline: true }));
    await client.connect();
    const result = await client.setCells(0, [
      { key: 0, effect: GREEN },
      { key: 79, effect: GREEN },
    ]);
    expect(result).toEqual({ partial: true, pendingKeys: [79] });
  });

  it("throws a StatusError with the device status for failures", async () => {
    const { client } = makeClient();
    await client.connect();
    await expect(client.getToggle(31)).rejects.toMatchObject({ status: "unknownToggle" });
  });

  it("runs the full config apply session with staged progress", async () => {
    const { client, keyboard } = makeClient();
    await client.connect();
    const blob = encodeLightingConfig(CONFIG);
    const stages: ConfigApplyStage[] = [];
    await client.applyConfigBlob(blob, (stage) => stages.push(stage));
    expect(keyboard.activeConfigBlob()).toEqual(blob);
    expect(stages[0]).toEqual({ stage: "begin" });
    expect(stages.at(-2)).toEqual({ stage: "commit" });
    expect(stages.at(-1)).toEqual({ stage: "done" });
    const transfers = stages.filter((s) => s.stage === "transfer");
    expect(transfers.at(-1)).toMatchObject({ sent: blob.length, total: blob.length });
  });

  it("reads the active config back byte-for-byte", async () => {
    const { client } = makeClient();
    await client.connect();
    const blob = encodeLightingConfig(CONFIG);
    await client.applyConfigBlob(blob);
    expect(await client.readConfigBlob()).toEqual(blob);
  });

  it("returns null when the device has no stored config", async () => {
    const { client } = makeClient();
    await client.connect();
    expect(await client.readConfigBlob()).toBeNull();
  });

  it("maps a corrupted transfer to CRC_MISMATCH and leaves the old config", async () => {
    const { client, keyboard } = makeClient();
    await client.connect();
    const good = encodeLightingConfig(CONFIG);
    await client.applyConfigBlob(good);
    // Flip a body byte after encoding: the announced blob CRC then matches
    // what was sent, but the header's body CRC no longer does — the device
    // must answer CRC_MISMATCH at commit and keep the old config.
    const tampered = encodeLightingConfig({ ...CONFIG, toggleInitialState: 1 });
    tampered[20] ^= 0x01; // flip a body byte after encoding
    await expect(client.applyConfigBlob(tampered)).rejects.toMatchObject({ status: "crcMismatch" });
    expect(keyboard.activeConfigBlob()).toEqual(good);
  });

  it("rejects a blob larger than the advertised capacity before sending", async () => {
    const { client } = makeClient(new MockKeyboard({ capabilities: { maxConfigBlobLen: 32 } }));
    await client.connect();
    const blob = encodeLightingConfig(CONFIG);
    await expect(client.applyConfigBlob(blob)).rejects.toThrow(/at most 32/);
  });

  it("reads a whole keymap layer chunked to the advertised per-op limit", async () => {
    // A small per-op limit forces multiple KEYMAP_READ round trips.
    const { client } = makeClient(new MockKeyboard({ capabilities: { maxKeymapEntriesPerOp: 10 } }));
    await client.connect();
    const layer = await client.readKeymapLayer(0);
    expect(layer).toHaveLength(84);
    expect(layer[0]).toBe(0x003a); // KC_F1
    for (const hole of [5, 8, 75, 78]) expect(layer[hole]).toBe(0x0000);
    const partial = await client.readKeymapLayer(1, 5);
    expect(partial).toEqual([1, 1, 1, 1, 1]);
  });

  it("writes keymap entries and returns the canonical read-back", async () => {
    const { client, keyboard } = makeClient();
    await client.connect();
    const readback = await client.writeKeymap([
      { layer: 0, key: 14, keycode: 0x0004 }, // KC_A
      { layer: 0, key: 15, keycode: 0x52c3 }, // TT(3): stored as KC_NO (lossy)
    ]);
    expect(readback).toEqual([0x0004, 0x0000]);
    expect(keyboard.keycodeAt(0, 14)).toBe(0x0004);
    // The live keymap reflects the write immediately.
    expect((await client.readKeymapLayer(0, 16)).slice(14)).toEqual([0x0004, 0x0000]);
  });

  it("maps out-of-range keymap writes to a StatusError", async () => {
    const { client } = makeClient();
    await client.connect();
    await expect(
      client.writeKeymap([{ layer: 9, key: 0, keycode: 4 }]),
    ).rejects.toMatchObject({ status: "outOfRange" });
  });

  it("fetches both halves' build identity with getVersion", async () => {
    const { client } = makeClient(new MockKeyboard({ versionMode: "mismatch" }));
    await client.connect();
    const version = await client.getVersion();
    expect(version.halvesMismatch).toBe(true);
    expect(version.central.present).toBe(true);
    expect(version.central.gitHashHex).not.toBe(version.peripheral.gitHashHex);
  });

  it("fails pending requests when the transport drops", async () => {
    const { client, transport } = makeClient();
    await client.connect();
    let disconnected = false;
    client.onDisconnect(() => (disconnected = true));
    transport.simulateDisconnect();
    expect(disconnected).toBe(true);
    await expect(client.getBrightness()).rejects.toThrow(/closed/);
  });
});

describe("StatusError", () => {
  it("names the command and describes the status", () => {
    const error = new StatusError("crcMismatch", "configCommit");
    expect(error.message).toContain("configCommit");
    expect(error.message).toContain("CRC_MISMATCH");
  });
});
