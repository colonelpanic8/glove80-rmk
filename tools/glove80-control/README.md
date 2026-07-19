# glove80-control

CLI for the Glove80. It speaks the RMK host protocol (`PROTOCOL.md` in
`protocol/glove80-host-protocol/`) over USB raw HID or BLE: lighting,
keymap editing, persistent config, build identity, and bootloader entry.
(The legacy ZMK Studio serial commands were retired after the RMK
cutover.)

## Transports

- `--usb` — Linux hidraw. Enumerates `/dev/hidraw*`, matches VID `16c0`
  PID `27db`, and picks the interface whose report descriptor carries the
  protocol's vendor usage page.
- `--ble` — BlueZ over D-Bus. Discovers by the custom GATT service UUID;
  requests go via write-without-response, responses via notifications.
- Default is auto: USB when present, otherwise BLE.
- `--device` disambiguates: a `/dev/hidraw*` path or a BLE address
  (`AA:BB:CC:DD:EE:FF`).
- Device-identification constants (vendor usage page/usage, GATT UUIDs)
  live in `src/transport/ids.rs`, the single place to keep in sync with
  the firmware's transport definitions.

## Lighting commands (RMK host protocol)

Capabilities are queried first on every connection; all parameters are
validated against what the device advertises.

- `lighting ping [--data TEXT]` — round-trip latency check.
- `lighting caps` — advertised capacities, effects, and feature bits.
- `lighting set <KEYS> <COLOR> [--effect blink|breathe] [--period MS]
  [--phase MS] [--duty PCT] [--ttl MS]` — set overlay cells. `KEYS` is a
  comma/range list (`0-5,12,40`); `COLOR` is `#RRGGBB` or a named color
  (`red`, `green`, `blue`, `white`, `black`/`off`, `yellow`, `cyan`,
  `magenta`, `orange`, `purple`, `pink`). Batches larger than the device's
  `max_cells_per_op` are split automatically.
- `lighting unset <KEYS>...` — revert cells to transparent.
- `lighting clear` — clear the whole host overlay.
- `lighting read` — table of the current overlay, including remaining TTLs.
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
- `lighting toggle <ID> [on|off]` — get or set a toggle overlay's state.
- `bootloader [--peripheral] [--yes]` — send `ENTER_BOOTLOADER` over the
  host protocol (asks for confirmation unless `--yes`). Targets the
  central half unless `--peripheral`.

## Persistent lighting config (host protocol v1.1)

The canonical lighting configuration is a human-editable TOML file
(`examples/lighting-default.toml` approximates the compiled-in defaults and
is the recommended starting point). It encodes to the protocol's config
blob, which the firmware applies **transactionally**: after a successful
apply the new config is active and survives reboots; any failure leaves the
previous config untouched.

- Workflow: edit the TOML → `config validate` (offline) → `config apply`
  → reboot the keyboard and the config persists. `config export` makes a
  backup of whatever is active.
- `config validate FILE` — offline parse + the exact validation the
  firmware runs before commit (`.json` files are checked against the
  canonical keymap schema instead). No device needed.
- `config apply FILE [--dry-run]` — parse and validate client-side (also
  against the advertised feature bit and `max_config_blob_len`), then run
  the CONFIG_BEGIN → chunked CONFIG_DATA → CONFIG_COMMIT session, reporting
  each stage. `FILE` is canonical TOML or a raw blob (detected by the
  `G80L` magic or a `.bin` extension). `--dry-run` stops before touching
  the device.
- `config export FILE [--raw]` — read the active blob back
  (byte-stable) and write canonical TOML, or the raw blob with `--raw`.
  A device still on compiled defaults reports that clearly instead of
  exporting anything.
- `config show` — summary table of the active config: records,
  activations, cell counts/keys, effects, and toggle persistence.
- TOML format: optional `[[toggle]]` entries (`id`, optional `name`,
  `persist`, `initial_on`) plus ordered `[[record]]` entries with
  `activation = "always" | { layer = N } | { toggle = N }` and
  `cells = [{ keys = "0-5,12", color = "#RRGGBB"|named, effect =
  "solid|blink|breathe", period_ms, phase_ms, duty_pct }]` (`keys` uses the
  same list/range syntax as `lighting set`).
- Comments and toggle names live only in the file — they never enter the
  blob, so they are absent from a later export. Keep your edited TOML in
  version control; the device round-trips the semantics, not the prose.

Partial application (peripheral half offline) is reported, never hidden:
overlay writes print the keys still pending on the peripheral.

## Keymap editing (host protocol v1.2)

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
- Writes are validated all-or-nothing per batch, applied to the live
  keymap immediately (no reboot), and persisted per key. The firmware
  echoes what it actually stored; the CLI prints that canonical read-back
  and flags any entry stored differently than requested (`LOSSY`) — some
  actions have no exact VIA encoding.
- `keymap find FRAGMENT` searches the keycode name table (names and
  aliases, case-insensitive), e.g. `keymap find vol`.
- Unknown/unnameable codes always print as hex (`0x1234`) and can be
  entered the same way; nothing round-trips through the CLI lossily.
- Vial interop: these commands and Vial edit the same runtime keymap and
  the same storage — an edit made in either is immediately visible to the
  other, byte-for-byte (the wire format is the VIA 16-bit keycode
  encoding Vial itself uses).
- Gated on capability feature bit 7; the CLI refuses cleanly when the
  firmware does not advertise keymap editing.

## Build identity (host protocol v1.3)

- `version` prints this CLI's own build identity (crate semver plus the git
  short hash embedded at build time, `-dirty` when built from a tree with
  uncommitted changes) and then queries GET_VERSION for both keyboard
  halves: semver, git hash, dirty flag, and connection state per half.
- The peripheral's identity is announced to the central over the split link
  at link-up; while the link is down the CLI shows the last-known version
  as `disconnected (last known)`, or `never seen since the central booted`
  when the central has no announcement cached.
- When both halves are present but built from different commits or crate
  versions the firmware sets a mismatch flag and the CLI prints a prominent
  `WARNING: HALVES MISMATCH` — the usual cause is flashing one half and
  forgetting the other.
- A note is printed when the keyboard speaks a different protocol version
  than the CLI. Gated on capability feature bit 8; the CLI refuses cleanly
  when the firmware does not advertise build-identity reporting.

## Development

- Build/test from the repo root: `cargo build -p glove80-control`,
  `cargo test -p glove80-control`. Tests run a mock transport; no hardware
  needed.
- The wire codec (messages, framing, reassembly) comes from
  `protocol/glove80-host-protocol`; this crate adds transports,
  request/response correlation, validation, and rendering.
