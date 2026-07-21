# glove80-control

CLI for the Glove80. Live keymap editing, topology-aware lighting, state
queries, and bootloader entry for either half use RMK's native Rynk protocol.
Persistent lighting config and build-identity commands are legacy compatibility
paths for older firmware (`PROTOCOL.md` in `crates/glove80-host-protocol/`).
(The legacy ZMK Studio serial commands were retired after the RMK
cutover.)

## Transports

- `--usb` — Linux hidraw for Rynk HID. `--device /dev/ttyACM…` remains
  available for older serial-based Rynk firmware.
- `--ble` — BlueZ over D-Bus using Rynk's native GATT service.
- Default is auto: USB when present, otherwise BLE.
- `--device` disambiguates: a `/dev/hidraw*` path or a BLE address
  (`AA:BB:CC:DD:EE:FF`).
- Legacy product-protocol device-identification constants remain in
  `src/transport/ids.rs` for the compatibility commands.

## Lighting commands (Rynk)

Capabilities are queried first on every connection; all parameters are
validated against what the device advertises.

- `lighting ping` — round-trip a Rynk version query and report latency.
- `lighting caps` — topology identity, capacities, effects, and feature bits.
- `lighting set <KEYS> <COLOR> [--effect blink|breathe] [--period MS]
  [--phase MS] [--duty PCT] [--ttl MS]` — set overlay cells. `KEYS` is a
  comma/range list (`0-5,12,40`); `COLOR` is `#RRGGBB` or a named color
  (`red`, `green`, `blue`, `white`, `black`/`off`, `yellow`, `cyan`,
  `magenta`, `orange`, `purple`, `pink`). Batches larger than the device's
  `max_cells_per_op` are split automatically.
- `lighting unset <KEYS>...` — revert cells to transparent.
- `lighting clear` — clear the whole host overlay.
- `lighting read` — authoritative active layer, right-half connection,
  revision, output/background state, brightness, and overlay cell count.
- `lighting replace [FILE] [--ttl MS]` — atomically replace the whole
  overlay from cell-spec lines (stdin when `FILE` is omitted or `-`).
  One cell per line, `#`-comments and blank lines ignored:

  ```
  # KEY COLOR [EFFECT] [period=MS] [phase=MS] [duty=PCT]
  12 #ff0000
  40 00ff00 blink period=750 duty=30
  41 blue breathe period=3000 phase=1500
  ```

  An empty spec is equivalent to `lighting clear`.
- `lighting brightness [VALUE]` — get, or set (0-255), the global
  brightness scalar.
- `lighting toggle` is retained as a legacy parser but rejected because named
  toggle overlays are not part of RMK's standard lighting model.
- `bootloader [--peripheral] [--yes] [--legacy-host-protocol]` — enter the
  selected half's UF2 bootloader through Rynk. When bootloader entry is locked,
  the CLI displays the configured physical-presence keys, polls while they are
  held, and reports success only after the selected half actually disconnects.
  `--legacy-host-protocol` is a recovery path for older firmware that predates
  Rynk bootloader entry.

## Canonical configuration file (Rynk keymap + legacy lighting)

Current RMK lighting firmware does not expose the old transactional product
protocol. This section documents the retained compatibility tooling and file
format for older builds.

One TOML file configures the whole keyboard — keymap layers through Rynk and
persistent lighting through the product protocol — with one apply flow.
`examples/glove80.toml` is the full-keyboard starting point (the
stock Base/Lower/Magic/Games/Mac-Hyper keymap plus the default lighting);
`examples/lighting-default.toml` remains a lighting-only example, and such
files keep working unchanged.

- Workflow: edit the TOML → `config validate` (offline) → `config apply`
  → the keymap is live immediately, the lighting config is active and
  persisted. `config export` makes a backup of whatever is active.
- `config validate FILE` — offline parse of both sections + the exact
  lighting validation the firmware runs before commit (`.json` files are
  checked against the legacy keymap schema instead). No device needed.
- `config apply FILE [--dry-run]` — validate client-side, then apply each
  section that is present, reporting every stage. `--dry-run` stops before
  touching the device. `FILE` may also be a raw lighting blob (detected by
  the `G80L` magic or a `.bin` extension).
- `config export FILE [--raw]` — read every keymap layer and the active
  lighting blob back into one canonical TOML. `--raw` writes the
  byte-stable lighting blob only (the keymap has no blob form). Degrades
  gracefully with a note: keymap-only when the device is running
  compiled-in lighting defaults, lighting-only when the firmware does not
  advertise keymap editing.
- `config show` — read the populated keymap layers and current lighting state
  through Rynk. It does not require the retired product-protocol endpoint.

### Keymap section

```toml
[[layer]]
id = "base"            # stable host-side ID (must not be purely numeric)
name = "Base"          # display name, host-side only
keys = """
KC_F1   KC_F2  ...     # 6 rows x 14 columns, whitespace-separated
...
"""
```

- A `[[layer]]`'s **position in the file is its firmware slot** (0-7).
  Layer IDs and names never reach the firmware; lighting records reference
  layers as `{ layer = "base" }` and the CLI resolves the ID to the slot
  number at encode time (bare integers still mean literal slots).
- `keys` is the full 6x14 grid, row-major: exactly 84 whitespace-separated
  tokens, one row per line by convention. Tokens are the same QMK-style
  names `keymap read`/`keymap set` use (`KC_A`, `MO(2)`, `LT(1,KC_ESC)`,
  `LSFT(KC_9)`, ...); whitespace inside parentheses is fine. `--` means
  unbound (`KC_NO`) and marks the four physical holes (r0c5, r0c8, r5c5,
  r5c8). `#` starts a comment running to the end of the line. Export
  produces this exact shape deterministically — aligned columns, `--` for
  every unbound key — so exports diff cleanly in git.
- A layer without `keys` only defines an ID for lighting references; apply
  leaves its bindings untouched. Omit all layer keys for a lighting-only
  file, or all lighting tables for a keymap-only file (the other side of
  the keyboard's state is then left exactly as it was).
- When any layer has `keys`, the file is authoritative for the keymap's
  length: every firmware slot after the final declared `[[layer]]` is cleared
  and read back. This prevents an exported or hand-written five-layer config
  from silently retaining stale bindings in slots 5–7.
- On export the device has no IDs/names to offer, so they are synthesized
  as `layer0..layerN` (position = slot) and trailing all-unbound layers
  are dropped. Export → apply → export is stable.

### Lighting section

- Optional `[[toggle]]` entries (`id`, optional `name`, `persist`,
  `initial_on`) plus ordered `[[record]]` entries with `activation =
  "always" | { layer = N } | { layer = "id" } | { toggle = N }` and
  `cells = [{ keys = "0-5,12", color = "#RRGGBB"|named, effect =
  "solid|blink|breathe", period_ms, phase_ms, duty_pct }]` (`keys` uses the
  same list/range syntax as `lighting set`, LED chain positions 0-79).
- Comments, toggle names, and layer IDs/names live only in the file — they
  never enter the blob, so they are absent from a later export. Keep your
  edited TOML in version control; the device round-trips the semantics,
  not the prose.

### Apply semantics — what is atomic and what is not

- **Lighting is atomic.** The blob goes through one CONFIG_BEGIN → chunked
  CONFIG_DATA → CONFIG_COMMIT session; the keyboard activates and persists
  either the complete new lighting config or keeps the old one, never a
  hybrid.
- **Keymap apply is best-effort per Rynk page.** Bulk-capable firmware writes
  one Rynk page at a time; other builds write individual keys. Every key is
  read back, trailing undeclared slots are cleared, and lossy conversions are
  reported, but an interrupted multi-page apply leaves earlier pages written
  — there is no whole-keymap transaction.
- The keymap section is applied **first**, so a keymap failure stops the
  run before the lighting config is touched.

Partial application (peripheral half offline) is reported, never hidden:
overlay writes print the keys still pending on the peripheral.

## Keymap editing (Rynk)

- `keymap read` dumps layer 0 as a 6x14 grid of QMK-style keycode names;
  `--layer N` picks another layer, `--all` dumps every layer, `--raw`
  prints hex u16 VIA keycodes instead of names. The four grid positions
  with no physical key (5, 8, 75, 78) render as `--`.
- `keymap set LAYER KEY KEYCODE [...]` writes one or more keys; triples
  repeat. `KEY` is a flat grid index (`key = row*14 + col`) or `row,col`.
  `KEYCODE` is hex (`0x0004`), a QMK name (`KC_A`, `KC_MPLY`), or a
  composite (`MO(2)`, `TG(3)`, `LT(1, KC_A)`, `LSFT_T(KC_ESC)`,
  `OSM(MOD_LSFT)`, `HYPR(KC_Z)`, `TD(4)`, `MACRO(0)`, `USER(7)`).
  Examples:
  - `glove80-control keymap set 0 28 KC_A`
  - `glove80-control keymap set 0 2,0 LCTL_T(KC_ESC) 1 2,0 KC_TRNS`
- Writes are applied to the live keymap immediately (no reboot), persisted by
  RMK, and read back through Rynk. The firmware
  echoes what it actually stored; the CLI prints that canonical read-back
  and flags any entry stored differently than requested (`LOSSY`) — some
  actions have no exact VIA encoding.
- `keymap find FRAGMENT` searches the keycode name table (names and
  aliases, case-insensitive), e.g. `keymap find vol`.
- Unknown/unnameable codes always print as hex (`0x1234`) and can be
  entered the same way; nothing round-trips through the CLI lossily.
- The CLI's QMK/VIA-style u16 names are a compatibility editing format. At the
  transport boundary they are converted to Rynk's typed `KeyAction`; actions
  with no exact u16 representation are reported as lossy instead of hidden.
- Capabilities, matrix size, layer count, and bulk support come from the Rynk
  handshake rather than product-protocol feature bit 7.

## Build identity (Rynk)

- `version` keeps three concepts separate: the structured Rynk protocol
  version, the application-defined firmware build label, and the structured
  RMK crate version.
- Glove80's default firmware label is
  `config <git-hash>[-dirty] / glove80-rmk v<semver> (<git-hash>[-dirty]) / RMK <git-describe>`.
  Direct product-repository builds use `config standalone`; downstream builds
  supply `GLOVE80_CONFIG_GIT_COMMIT` and `GLOVE80_CONFIG_GIT_DIRTY`. The RMK
  identity names the exact custom integration commit, not only its inherited
  Cargo semver. Firmware can replace the whole bounded label with
  `HostService::with_build_label` without changing protocol compatibility.
- The current Rynk endpoint describes the central/application build. Split
  target identity and half-mismatch detection need a future routed or
  per-peripheral build-info query; `version` does not claim to validate the
  peripheral image yet.

## Development

- Build/test from the repo root: `cargo build -p glove80-control`,
  `cargo test -p glove80-control`. Tests run a mock transport; no hardware
  needed.
- The wire codec (messages, framing, reassembly) comes from
  `crates/glove80-host-protocol`; this crate adds transports,
  request/response correlation, validation, and rendering.
