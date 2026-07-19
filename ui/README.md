# Glove80 Lightbench

Lightbench is the browser workbench for the Glove80 host protocol
(`protocol/glove80-host-protocol/PROTOCOL.md`). It connects straight to the
keyboard — no daemon, no Studio, no install — and drives two things:

- the **live host overlay**: RAM-only lighting painted directly onto the
  board, with TTL and brightness control;
- the **persistent config**: the ordered lighting records the keyboard boots
  with, edited offline and applied through the transactional v1.1 config
  session;
- the **keymap**: the live key bindings, read and written over the v1.2
  KEYMAP_READ/KEYMAP_WRITE commands.

The ZMK-era Studio path (`@zmkfirmware/zmk-studio-ts-client`, the protobuf
lighting protocol) has been retired from the app; Lightbench now speaks only
our own protocol over WebHID and Web Bluetooth.

## Run locally

```sh
cd ui
npm ci
npm run dev
```

Open the printed URL in Chrome or Edge (WebHID and Web Bluetooth need a
Chromium browser; `localhost` counts as a secure context).

## Connecting

- **Connect USB** — WebHID. Lightbench matches the keyboard's dedicated
  host-protocol interface (VID `0x16C0` / PID `0x27DB`, usage page `0xFF88`,
  usage `0x01`) and never touches the Vial raw-HID interface. Pick the
  Glove80 in the browser prompt; no other program needs to be closed, the
  interface is exclusive to this protocol.
- **Connect BLE** — Web Bluetooth. The keyboard must already be paired
  (bonded) with the OS; the `fc550001-…` GATT service requires an encrypted
  link and is claimed via `optionalServices`. Requests go out as
  write-without-response chunks; responses arrive as notifications.
- **Demo mode** — an in-memory keyboard (`src/lib/mock-device.ts`)
  implementing the full protocol, including the config transfer session,
  partial-apply semantics, a live keymap (seeded with a QWERTY base layer,
  all-or-nothing writes, canonical read-back) and a GET_VERSION answer.
  Everything in the UI can be exercised with no hardware; a banner makes the
  mode unmistakable.

The first exchange on any connection is `GET_CAPABILITIES`; the connection
readout shows the protocol version and advertised features, and every panel
gates itself on what the keyboard actually advertises.

When the firmware advertises build-identity reporting (feature bit 8),
Lightbench also issues `GET_VERSION` and shows both halves' firmware version
and short git hash next to the connection readout. A **halves mismatch**
(different hash or semver on the two halves — flashed one, forgot the other)
is flagged prominently. A peripheral that has not reported a version since
the central booted shows as "no version reported since boot" — a real state
while the split link has not synced, not an error.

## Live overlay panel

- Click or drag to paint; the brush chooses color and effect (solid, blink
  with period/duty, breathe with period), or **Erase** to make keys
  transparent again.
- **TTL** applies to subsequent strokes: the firmware reverts those cells by
  itself when it expires (the board shows a countdown). No TTL means cells
  survive until an explicit clear or reboot — disconnecting never changes
  what the keyboard shows.
- **Brightness** is the device's global scalar (0–255) under the compiled
  safety ceiling.
- **Sync from keyboard** (`READ_OVERLAY`) adopts whatever the keyboard is
  currently showing; **Push my state** (`REPLACE_OVERLAY`) makes the keyboard
  match the canvas exactly. **Clear overlay** removes everything.
- If the right half is offline, writes answer `PARTIAL_APPLY`; the affected
  keys are marked pending on the board instead of pretending they lit. They
  apply automatically when the half reconnects.

## Persistent config editor

- A config is an **ordered list of records** (composition order), each with
  an activation — always on, active on a keymap layer, or bound to a toggle
  id — and a sparse map of key → effect cells. Unpainted keys stay
  transparent and reveal the records below.
- Toggles used by records get boot-state and persistence checkboxes (the
  blob's `toggle_initial_state` / `toggle_persist_mask`), plus live on/off
  control when connected.
- **Apply to keyboard** runs the transactional session
  (`CONFIG_BEGIN → CONFIG_DATA… → CONFIG_COMMIT`) with staged progress. The
  commit is all-or-nothing: on `CRC_MISMATCH`, `INVALID_CONFIG` or any other
  failure the previous config stays active and the error says so precisely.
- **Load from keyboard** (`CONFIG_READ`) pulls the active blob back
  byte-stable into the editor.
- **Export/Import .bin** moves the raw config blob to and from disk; imports
  are fully validated before they touch the editor. The same blob works with
  the CLI.
- The **sync indicator** compares the editor's encoded bytes against the
  last blob seen from the keyboard, so drift is always visible. Drafts are
  kept in browser local storage; client-side validation (the same rules the
  firmware enforces) runs on every edit.

## Keymap editor

Available when the firmware advertises keymap editing (protocol v1.2,
feature bit 7).

- Pick a **layer** (0–7); the board shows that layer's bindings as key
  legends, read live from the keyboard (`KEYMAP_READ`, chunked to the
  advertised per-op limit). **Reload from keyboard** re-reads the layer —
  Vial edits, host-protocol writes and compiled defaults all read back
  through the same runtime state.
- **Click a key** to edit its binding. Enter a keycode by name (`KC_A`,
  `MO(2)`, `LT(1, KC_A)`, `LSFT_T(KC_ESC)`, …), as raw hex (`0x0004`), or
  via the built-in search — the same name table and spellings as
  `glove80-control keymap` (`src/lib/keycodes.ts` mirrors the CLI's
  `keycodes.rs`).
- Edits are **staged** (dashed outline on the board) and sent in one batched
  `KEYMAP_WRITE`. Each batch is all-or-nothing on the device; a rejected
  batch leaves the staged edits staged. Writes change the live keymap
  **immediately** — no reboot — and are persisted per key in RMK storage.
- The firmware echoes the keycode it actually stored (canonical read-back).
  A stored value that differs from the request is flagged **LOSSY** on the
  board and listed with both values — some nameable keycodes (e.g. `TT(n)`)
  have no RMK representation and store as `KC_NO`.
- **Vial interop**: bindings travel as VIA/Vial 16-bit keycodes and hit the
  same store Vial edits over its own protocol. Lightbench, the CLI and Vial
  always agree — each sees the others' writes verbatim.
- The four grid holes (positions 5, 8, 75, 78 of the 6×14 matrix) have no
  physical key and are not shown; they always read `KC_NO`.

## Architecture

- `src/lib/host-protocol.ts` — the TypeScript codec (messages, frame layer,
  config blob), locked to the Rust codec by shared golden vectors under
  `protocol/vectors/` (v1.0 through v1.3).
- `src/lib/keycodes.ts` — the VIA keycode name table (format, parse,
  search), mirroring `tools/glove80-control/src/keycodes.rs` so the web UI
  and the CLI speak the same names.
- `src/lib/glove80-layout.ts` — the LED chain order, plus the 6×14 keymap
  grid ↔ physical key mapping (from `rmk/glove80/vial.json`).
- `src/lib/transport.ts` + `webhid-transport.ts` / `webbluetooth-transport.ts`
  — one chunk-level `Transport` interface and the two browser transports.
- `src/lib/protocol-client.ts` — frame split/reassembly, request-id
  correlation, one-in-flight serialization, timeouts, and the config
  transfer/read flows.
- `src/lib/mock-device.ts` — the in-memory keyboard used by both the test
  suite and demo mode.
- `src/components/` — `Board` (the shared keyboard rendering, protocol key
  space = LED chain order), `BrushControls`, `OverlayPanel`, `ConfigPanel`,
  `KeymapPanel`.

`npm test` covers the codec against the golden vectors, the frame layer, the
mock device's protocol semantics (including the config session state
machine), and the client against the mock end to end. `npm run build` type
checks and produces the production bundle.
