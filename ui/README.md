# Glove80 Lightbench

Lightbench connects straight to the keyboard — no daemon or Studio — and uses
one Rynk WebHID session for live lighting and keymaps. It drives:

- the **live host overlay**: RAM-only lighting painted directly onto the
  board, with TTL and brightness control;
- the **legacy persistent config editor**: retained for offline compatibility
  and demo-mode testing, but not exposed by current RMK lighting firmware;
- the **keymap**: live bindings read and written through Rynk.

The ZMK-era Studio path has been retired. Wired and already-paired Bluetooth
connections both use Rynk's WebHID collection (usage page `0xFF60`, usage
`0x61`).

## Run locally

```sh
cd ui
npm ci
npm run dev
```

Open the printed URL in Chrome or Edge (WebHID and Web Bluetooth need a
Chromium browser; `localhost` counts as a secure context).

## Connecting

- **Connect USB** — choose the Glove80's Rynk WebHID collection.
- **Connect BLE** — first pair the keyboard with the OS, then choose that
  paired keyboard's Rynk HID collection. Browser code does not access a custom
  GATT service.
- **Demo mode** — an in-memory keyboard (`src/lib/mock-device.ts`)
  implementing the full protocol, including the config transfer session,
  partial-apply semantics, a live keymap (seeded with a QWERTY base layer,
  all-or-nothing writes, canonical read-back) and a GET_VERSION answer.
  Everything in the UI can be exercised with no hardware; a banner makes the
  mode unmistakable.

The Rynk handshake advertises keymap and lighting support; Lightbench then
queries lighting limits/topology and authoritative mutable state.

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
- Rynk exposes authoritative state and overlay size, but intentionally does
  not duplicate every overlay cell for readback. **Push my state** uses the
  atomic replace transaction; **Clear overlay** removes everything.
- Every mutation is revision-checked and returns the new authoritative state.
  The central owns the board-wide frame and forwards complete staged frames to
  the right half.

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

Production keymap editing shares the Rynk connection used by live lighting.
Demo mode retains the frozen v1.2 backend solely so the UI can be exercised
without hardware.

- Pick a **layer** (0–7); the board shows that layer's bindings as key
  legends, read live through Rynk. **Reload from keyboard** re-reads the layer
  from RMK's runtime state.
- **Click a key** to edit its binding. Enter a keycode by name (`KC_A`,
  `MO(2)`, `LT(1, KC_A)`, `LSFT_T(KC_ESC)`, …), as raw hex (`0x0004`), or
  via the built-in search — the same name table and spellings as
  `glove80-control keymap` (`src/lib/keycodes.ts` mirrors the CLI's
  `keycodes.rs`).
- Edits are **staged** (dashed outline on the board), then written through
  Rynk and read back. A rejected write leaves staged edits available to retry.
  Successful writes change the live keymap immediately and persist in RMK.
- The firmware echoes the keycode it actually stored (canonical read-back).
  A stored value that differs from the request is flagged **LOSSY** on the
  board and listed with both values — some nameable keycodes (e.g. `TT(n)`)
  have no RMK representation and store as `KC_NO`.
- The editor still presents QMK/VIA-style 16-bit keycodes for continuity with
  existing config files. `src/lib/rynk-keycode.ts` converts them to typed Rynk
  actions and flags conversions that cannot round-trip exactly.
- The four grid holes (positions 5, 8, 75, 78 of the 6×14 matrix) have no
  physical key and are not shown; they always read `KC_NO`.

## Architecture

- `src/lib/host-protocol.ts` — the TypeScript codec (messages, frame layer,
  config blob), locked to the Rust codec by shared golden vectors under
  `crates/glove80-host-protocol/vectors/` (v1.0 through v1.3).
- `src/lib/keycodes.ts` — the VIA keycode name table (format, parse,
  search), mirroring `tools/glove80-control/src/keycodes.rs` so the web UI
  and the CLI speak the same names.
- `src/lib/rynk-keycode.ts` / `rynk-web-client.ts` — the typed action converter
  and WebHID client for keymaps plus revision-checked lighting, backed by the
  generated `src/vendor/rynk-wasm` package.
- `src/lib/glove80-layout.ts` — the LED chain order, plus the 6×14 keymap
  grid ↔ physical key mapping (from `firmware/glove80-rmk/vial.json`).
- `src/lib/transport.ts` + `webhid-transport.ts` / `webbluetooth-transport.ts`
  — one chunk-level `Transport` interface and the two browser transports.
- `src/lib/protocol-client.ts` — frame split/reassembly, request-id
  correlation, one-in-flight serialization, timeouts, and the config
  transfer/read flows.
- `src/lib/mock-device.ts` — the in-memory keyboard used by both the test
  suite and demo mode.
- `src/components/` — `Board` (the shared keyboard rendering, protocol key
  space = LED chain order), `BrushControls`, `OverlayPanel`, `ConfigPanel`,
  `KeymapPanel`, `TogglePanel`, `CellEditor`.

## Advanced compositor UI

- **Toggles tab**: probes all 32 toggle ids on connect (GET_TOGGLE) so
  device-configured toggles appear automatically; live on/off switches
  (SET_TOGGLE), boot-state and persist flags (config blob bits), and chips
  listing the records each toggle activates (click to jump to the record).
- **Toggle names** are host-side only — the blob cannot carry them. They
  persist per keyboard identity in localStorage and travel in a
  `.names.json` sidecar next to exported blobs; importing a blob looks for
  its sidecar.
- **Record depth** (config tab): per-cell effect parameter editing with
  codec-range validation, drag or button reordering ("later records win
  within a class"), duplicate/delete, and solo-preview of one record on
  the board.
- **Composed preview**: a client-side simulation
  (`src/lib/compositor-preview.ts`, fixture-tested against the Rust
  compositor's semantics) renders the full stack for any chosen layer,
  toggle states, and sample host cells — animations included. It is a
  simulation and labeled as such; the keyboard itself stays untouched.
- **Effective ceiling**: shown read-only beside brightness. The protocol
  currently has no ceiling command; runtime ceiling control awaits a
  protocol addition.

`npm test` covers the codec against the golden vectors, the frame layer, the
mock device's protocol semantics (including the config session state
machine), and the client against the mock end to end. `npm run build` type
checks and produces the production bundle.
