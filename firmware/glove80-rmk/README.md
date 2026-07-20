# Glove80 RMK spike

RMK firmware for the MoErgo Glove80, built as the bounded hardware spike
described in [`docs/rmk-evaluation.md`](../../docs/rmk-evaluation.md). The
known-good ZMK firmware in `zmk/` remains the recovery baseline; nothing here
modifies it.

## Layout and hardware sources

All hardware facts are transcribed from the ZMK board definition in
`zmk/app/boards/arm/glove80/`:

- 6x14 logical grid identical to the ZMK matrix transform (columns 0-6 left
  half, 7-13 right half; columns 6/7 are the thumb clusters; positions
  (0,5), (0,8), (5,5), (5,8) are unpopulated).
- Left half is the split central (USB + BLE), right half the BLE peripheral.
- Flash layout matches the ZMK partition table: app `0x26000`-`0xdc000`,
  reserved runtime-config partition `0xdc000`-`0xec000` (untouched), RMK
  storage `0xec000`-`0xf4000` (the ZMK settings partition), bootloader at
  `0xf4000`. The SoftDevice region `0x0`-`0x26000` is left in place unused.
- Battery uses the internal VDDH/5 ADC channel on both halves.
- UF2 family IDs: left `0x9807B007`, right `0x9808B007`.

Stage 5 ports the LEDs: WS2812 key chains (left data `P0.27`, enable `P0.31`;
right data `P0.13`, enable `P0.19`; GRB, 40 LEDs per half, SPIM3 at 4 MHz)
and the power-button PWM LED (left `P1.15`, right `P0.16`, 20 us period at
~5% duty). Still not ported: the Glove80-ext connector.

## Lighting (Stage 5 driver + Phase 1 compositor)

Lighting is split between `src/lighting.rs` (hardware driver + RMK task
glue) and the host-testable compositor crate in
[`../glove80-compositor/`](../glove80-compositor/); both binaries register
the lighting task as a custom RMK processor from inside the
`#[rmk_central]` / `#[rmk_peripheral]` module.

Integration decision: **stay on the macro flow**. Reading rmk-macro at our
pinned revision showed the annotated mod supports exactly the extension
points we need, so converting to manual-main (as in
`examples/use_rust/nrf52840_ble_split`) would have meant re-implementing the
whole generated central/peripheral bring-up only to add one task. What the
macro flow gives us:

- `#[register_processor(event)] fn name() { ... }` inside the mod: the body
  is inlined into the generated `main` (with the embassy-nrf peripherals `p`
  in scope) as an initializer, and the returned value's
  `Processor::process_loop()` is joined with the other firmware tasks.
- Plain `use` items in the mod pass through; other items can live in a
  normal sibling module (`mod lighting;` at the crate root), including our
  own `bind_interrupts!` for SPIM3, which composes with the macro's `Irqs`
  struct because RMK never binds SPIM3 itself.
- The future compositor needs the same shape (an event-driven task owning
  the frame), so nothing here pushes toward manual-main later. If we ever
  need to restructure `main` itself, `#[Overwritten(entry)]` and
  `add_interrupt!` exist as escape hatches before a full conversion.

Design: `lighting.rs` splits into a frame sink (`Ws2812Chain`: SPIM3 +
EasyDMA buffer, encodes a `Frame = [Rgb; 40]` in chain order) and a frame
source — the sparse lighting compositor (Phase 1 of
`docs/implementation-plan.md`, contract in `docs/lighting-design.md`). The
right half renders locally but still tracks the active layer because RMK's
split peripheral republishes the synced layer state as a local
`LayerChangeEvent` (no split-protocol changes).

### Compositor (Phase 1)

- `../glove80-compositor/`: pure-logic `no_std` crate, zero dependencies,
  own workspace; `cargo test` runs the whole contract on the host. Time is
  an abstract `now_ms: u64` input, the LED count a const generic.
- Cell = `Transparent | Solid | Blink{period, phase, duty} |
  Breathe{period, phase}`. A blinking cell's dark phase renders black — it
  occludes, it does not become see-through.
- Record = sparse `key -> Cell` map plus an activation predicate: `Always /
  LayerActive(n) / Toggle(id) / HostOverlay / Status`. Fixed capacities:
  16 records, 40 cells per record, 40 live host cells.
- Composition is bottom-to-top by class (base, layer, toggle, host,
  status), insertion order within a class; defined cells replace,
  transparent reveals.
- Host overlay slot: `host_set` / `host_unset` / `host_clear` /
  `host_replace` (atomic, the force-sync primitive) / `host_cells`
  (read-back), each cell with an optional TTL; on expiry the cell reverts
  to transparent, enforced compositor-side against the passed-in clock.
- Global brightness scalar plus a runtime *effective ceiling* applied at
  composition output. `CHANNEL_CEILING` (204 = 80%, the MoErgo
  current/warranty limit — full warning at its definition in the compositor
  crate) is the compile-time value; `set_ceiling` can lower the effective
  ceiling at runtime but can never raise it above the compiled constant,
  and the driver clamp below remains the hard backstop either way.
- `render(now)` returns the frame, a `changed` flag (unchanged frames skip
  the SPI write), and `next_wake` — the next instant the frame can change
  by itself (blink edge, 32 ms breathe tick, TTL expiry of a *visible*
  cell). `None` means fully static: `LightingProcessor` arms no timer at
  all, preserving the no-ticker-when-static guarantee. The processor loop
  is `select(deadline, LayerChangeEvent)`; nothing runs in the key-scan
  path.

Current clamp: the driver (`Ws2812Chain::write`) hard-clamps every color
channel to `CHANNEL_CEILING` at encode time. Calling code cannot bypass it;
the compositor's effective ceiling only ever lowers the limit further.

SPIM3/BLE coexistence: transfers are short (~1 KiB encoded, ~2 ms at 4 MHz)
and EasyDMA-timed, so radio interrupts cannot stretch WS2812 bit timings;
the latch gap is a >= 80 us all-zero tail inside the same transfer, and
MODE_0 keeps MOSI idle-low between frames. Known risk: nRF52840 anomaly 198
(SPIM3 TX corruption under concurrent RAM traffic) is not worked around by
embassy-nrf; worst case is an occasional glitched frame, which the ZMK
firmware (same SPIM3 arrangement) has not shown in practice.

What to observe when flashing (per half, in order):

1. Immediately at boot: the rear power-button LED comes on dim (~5%) --
   this restores the ZMK "rear LED" behavior on both halves.
2. ~120 ms after boot: all six thumb keys light dim white (base record),
   then the top thumb row (3 keys) plus the four keys of the inner
   main-grid column show the layer color — blue at layer 0. The bottom
   thumb row (3 keys) stays dim white: the base showing through where the
   layer record is transparent.
3. Layer changes recolor exactly those seven accent keys on both halves
   (right half after the layer sync): 1 Lower green, 2 Magic (held)
   magenta, 3 Games (toggle) red, 4 Mac Hyper (toggle) cyan; returning to
   base restores blue and the bottom thumb row never changes.
4. The host overlay starts empty (the Phase 1 hardcoded amber-blink /
   purple-breathe placeholders are gone): any blink/breathe/TTL behavior on
   top of the base + layer accents now comes from a live host writing the
   overlay through the Phase 2 protocol (see "Host protocol transports").
5. All other key LEDs stay dark (the frame drives them off explicitly, so
   no stale bootloader colors persist).
6. Typing latency/split behavior should be unchanged; lighting renders only
   on layer events and self-scheduled animation deadlines, and unchanged
   frames are not even written to the chain.

## Host protocol transports (Phase 2)

The host protocol (`crates/glove80-host-protocol/PROTOCOL.md`, including
its "Transports" addendum) is exposed on the **central (left) half** over
both USB and BLE. Protocol work is split three ways:

- **RMK extension APIs** (`../../dependencies/rmk`, pinned submodule): a vendor
  raw-HID interface on the USB composite (`VendorHidReport`, usage page
  `0xFF88` / usage `0x01`, 32-byte IN/OUT
  reports — deliberately not multiplexed onto Vial's `0xFF60` interface,
  whose opcodes collide) and a custom GATT service
  (`fc550001-f8e0-459f-b421-c254fc42b138`; request characteristic
  `fc550002-…` write-without-response, response characteristic `fc550003-…`
  notify — not a HID service, so Web Bluetooth can reach it via
  `optionalServices`). The patches contain zero protocol knowledge: they
  only shuttle opaque chunks through `rmk::vendor_transport` channels and
  publish the negotiated ATT payload size.
- **Transport pumps** (`src/host_pump.rs`, central only, registered as one
  extra RMK processor): reassemble chunks (`Reassembler<1536>` per
  transport), decode with the shared codec crate, hand the request to the
  lighting task, then encode + frame the response back out (32-byte padded
  reports on USB, ATT-payload-sized notifications on BLE). One message in
  flight per transport by construction; malformed/unknown requests still get
  their one response (status `MALFORMED` / `UNKNOWN_COMMAND` /
  `CAPACITY_EXCEEDED`).
- **Semantics** (`src/host_proto.rs`, shared): `apply()` runs inside the
  `LightingProcessor` select loop, so the compositor keeps exactly one owner.
  Capabilities advertise 40+40 keys, 8 layers, effects solid/blink/breathe,
  `max_cells_per_op` 80, overlay capacity 80, and feature bits
  TTL/toggles/bootloader/atomic-replace/read-back/partial-apply — all backed
  by working code paths.

Split scope (upgraded by Phase 3, see "Split lighting transfer" below):
keys 0-39 apply locally; keys 40-79 are stored authoritatively on the
central and forwarded to the peripheral over the split link. Toggle ids
0-31 are accepted (the compositor's toggle bitmask, now mirrored to the
peripheral; no default records reference them yet, so they have no visual
effect until lighting config gains toggle records). `ENTER_BOOTLOADER` on
target central answers OK, waits ~300 ms for the response to flush, then
reboots via the Adafruit bootloader GPREGRET magic; target peripheral is
forwarded over the Phase 3 split application channel as a magic-guarded
`EnterBootloader` message and the peripheral reboots the same way (`OK` =
dispatched to a connected peripheral, `BUSY` = peripheral offline, nothing
happened).

RMK is consumed as a **git submodule** at `dependencies/rmk`, pinned to the
consolidated `glove80` branch of `colonelpanic8/rmk`. That branch carries the
generic extension hooks needed by the firmware while keeping their upstream
feature histories independently reviewable.

## Split lighting transfer (Phase 3)

Host writes to keys 40-79 now light the right half. The central's
compositor state stays authoritative for all 80 keys; the peripheral is a
mirror that renders its 40. Three layers:

- **RMK split hook**: a bounded application-message hook on the split
  protocol, written to be upstreamable. `rmk/src/split_app.rs` (opaque
  `SplitAppData` payload
  ≤ 26 bytes, bounded `SPLIT_APP_TX`/`SPLIT_APP_RX` channels, and a
  state-based `SPLIT_APP_LINK` watch for link edges); a final
  `SplitMessage::Application` variant (`rmk/src/split/mod.rs`, appended last
  so existing discriminants are stable); the central's `PeripheralManager`
  drains `SPLIT_APP_TX` as its lowest-priority outgoing arm and reports link
  up/down (`rmk/src/split/driver.rs`); the peripheral forwards received
  payloads into `SPLIT_APP_RX` with `try_send` and reports its link state
  (`rmk/src/split/peripheral.rs`). The payload cap keeps
  `SPLIT_MESSAGE_MAX_SIZE` at 32 (trouble's GATT arrays require ≤ 32, and
  every split transfer — key events included — is that size on the wire).
- **Sync codec + remote store** (`../glove80-compositor/src/sync.rs`,
  host-tested): versioned messages `[version, tag, body]` — `SetCells`
  (≤ 2 cells/message, LOCAL right-half keys 0-39, no TTL on the wire),
  `UnsetKeys` (≤ 16), `Clear`, and `State` (brightness, effective ceiling,
  toggle bitmap). Unknown versions/tags are ignored by receivers (new kinds
  = new tags; breaking layouts bump the version). `RemoteOverlay` is the
  central's authoritative right-half store with all TTL bookkeeping.
- **Firmware glue** (`src/split_lighting.rs`): `CentralSplit` /
  `PeripheralSplit`, owned by the lighting task on each half (the
  compositor keeps exactly one owner). All queueing is `try_send` — split
  lighting can never block key or event traffic, and the split driver
  always polls its read arm first.

Behavior:

- **Forwarding**: right-half protocol writes update `RemoteOverlay` and go
  out as deltas. `OK` means the peripheral was connected and the deltas
  were dispatched; `PARTIAL_APPLY` (with the right-half keys) means the
  peripheral is unavailable and the cells are stored pending. TTL authority
  stays central-side: on expiry the central sends the unset (the peripheral
  never sees TTLs).
- **Reconnect resync**: on every link-up edge — and as the fallback
  whenever the delta queue overflows — the central pushes the complete
  right-half picture: `Clear`, every live cell, then `State`. Idempotent,
  so replays and races with stale queued deltas are harmless (the leading
  `Clear` wipes them).
- **Link-loss policy**: the peripheral clears its host overlay 5 s after
  losing the central (the TTL/authority for those cells is gone, so they
  must not outlive it; the grace avoids flicker across routine reconnects,
  which end in a resync anyway). Brightness/ceiling/toggles are kept across
  link loss, like the synced layer state.
- **Protocol semantics** (PROTOCOL.md addendum updated): writes to 40-79
  answer `OK` when forwarded, `PARTIAL_APPLY` only when the peripheral is
  genuinely unavailable; `READ_OVERLAY` reports all 80 keys with TTLs;
  offline `CLEAR_OVERLAY`/`REPLACE_OVERLAY` answer `PARTIAL_APPLY` (empty
  pending list for a bare clear). Peripheral bootloader entry forwards over
  this channel too (see "Host protocol transports" above).

## Persistent lighting (Phase 4)

Base / layer / toggle lighting records now persist across reboots. The unit
of persistence and transfer is the protocol v1.1 **config blob**
(`crates/glove80-host-protocol/PROTOCOL.md`, "Persistent configuration"):
storage treats it as opaque validated bytes; only
`src/lighting_config.rs` interprets it, via the shared
`glove80_host_protocol::config` codec (no firmware-side reimplementation).

### Storage: the reserved runtime-config partition

`src/config_store.rs` owns `0xdc000`-`0xec000` (64 KiB — the partition the
ZMK-era flash map reserved for runtime config, untouched until now) as two
32 KiB generation slots (A `0xdc000`, B `0xe4000`). Slot layout: a 32-byte
header (commit magic `"G80C"`, generation counter, blob length, blob CRC-32,
header CRC-32) followed by the blob bytes.

A save is transactional by construction: it only ever touches the inactive
slot — erase, write blob, read back + CRC verify, write the header fields,
then write the 4-byte magic **last** (a single NVMC word program, the commit
point). Boot picks the valid slot with the highest generation. Power loss or
a malformed write at ANY byte leaves the previous slot untouched and still
winning; there is no state in which a torn save validates.

Flash access shares the radio-safe `nrf_mpsl::Flash` singleton with RMK's
storage task through the opt-in `shared_flash` feature. The driver lives in an
async mutex; RMK storage uses the internal locking adapter, while this
firmware uniquely acquires an `rmk::shared_flash::SharedFlash` client scoped
to the reserved partition. A service task executes bounded requests
(256-byte chunks, one erase page per lock), so every operation stays inside
that immutable window and no flash work blocks key scanning.

### Boot

The central's lighting task loads the newest valid stored config before its
first frame and applies it over the compiled defaults (recovery order per
design-goals.md: newest valid stored config → compiled defaults; the
defaults in `default_compositor()` are unchanged and remain the no-config
behavior). The peripheral persists nothing — central is authoritative.
Toggle state: non-persisted toggles boot to their `toggle_initial_state`
bit; opted-in (`toggle_persist_mask`) toggles keep runtime state across a
config commit, but flash write-back of runtime flips is not implemented yet,
so they boot off until it is (needs its own small record so a toggle
keypress does not rewrite the whole blob).

### Split record sync

Applying a config splits each record by half: keys 0-39 go into the
central's compositor (atomic `replace_records` swap); keys 40-79 are
remapped to local 0-39 and streamed to the peripheral over the Phase 3
split application channel as new sync-codec tags (`ConfigReset` /
`ConfigRecord` / `ConfigCells` / `ConfigCommit`, additive in
`glove80-compositor/src/sync.rs`). The peripheral stages the incoming set
(`ConfigStage`) and swaps its compositor records only on a complete commit —
a link drop mid-transfer discards the stage and keeps the previous records.
The central re-streams the whole set on every link-up edge (paced at 4
messages / 20 ms so the peripheral's bounded inbox can never overflow; a
full 16×40-cell set transfers in under two seconds). While no stored config
exists nothing is pushed and both halves render their identical compiled
defaults.

### Host protocol session (v1.1, wired)

`CONFIG_BEGIN` / `CONFIG_DATA` accumulate into a central-side 8 KiB RAM
buffer (one session per keyboard, shared across USB and BLE; a new BEGIN
replaces it). `CONFIG_COMMIT` re-checks the announced CRC, runs the shared
validator, persists via the transactional store, then applies live (central
swap + toggle persist state + peripheral push) — old config or new config,
never a hybrid; a storage failure answers `BUSY` with nothing changed.
`CONFIG_ABORT` discards the session. `CONFIG_READ` streams the active blob
straight from flash (byte-stable export, independent of any open session).
Capabilities now advertise feature bit 6 with
`max_config_blob_len = 7148`. All of this runs inside the lighting task —
the compositor, split state, store, and session keep exactly one owner.

## Keymap over Rynk

Rynk is the production keymap owner. The qualified firmware enables USB HID,
native BLE GATT/WebHID, bulk keymap, and persistence paths over the
6×14 matrix and eight layers. The Glove80 host protocol deliberately leaves
its historical v1.2 keymap feature bit clear and no longer dispatches
`KEYMAP_READ`/`KEYMAP_WRITE`; lighting and configuration remain on that
protocol. The CLI and Lightbench convert their existing QMK/VIA-style u16 text
format to Rynk's typed actions at the host boundary.

## Build identity over the host protocol (v1.3)

`GET_VERSION` (0x03, feature bit 8) reports both halves' firmware build
identity in one exchange: crate semver, git short hash, and a dirty flag,
embedded at build time by `build.rs` (`version_embedding()` — `git
rev-parse --short=8 HEAD` plus `git status --porcelain`, falling back to
the literal `unknown0`/clean when git is unavailable; the flag reflects
the whole repo's working tree, so any uncommitted change — not just to
firmware sources — marks the build dirty). `src/version.rs` exposes the
embedded constants to both halves.

The peripheral announces its identity to the central once per split
link-up edge, as a `PeripheralVersion` sync message over RMK's split app
channel (peripheral → central direction, `SPLIT_APP_PERIPH_TX`; retried briefly if the
bounded queue is momentarily full). The central caches the announcement
in `CentralSplit`: while the link is down the last-known version is
retained and reported with `present = 0`; all-zero fields mean the
peripheral has not been seen since the central booted. The firmware also
computes `halves_mismatch` (both present, hash or semver differ) so hosts
can warn about a half-flashed keyboard.

## Building

```sh
nix develop --command ./build.sh
```

produces `glove80_lh_rmk.uf2` and `glove80_rh_rmk.uf2`. The toolchain is
pinned in the repository's `rust-toolchain.toml` and supplied with libclang
by its flake; RMK is pinned by `../../dependencies/rmk`
submodule to the fork's Rynk integration branch `glove80-rynk` at `67f444b2`
(rollback: `glove80` at `8089822e`), and nrf-sdc stays
pinned to an exact revision in `Cargo.toml`.

The compositor's contract tests run on the host:

```sh
cd ../glove80-compositor && cargo test
```

Note: the RMK chip defaults for nrf52840 inject a `[dfu]` section, which makes
config resolution print warnings that our `[storage]` addresses are ignored.
They are not: the `dfu_nrf` cargo feature is disabled, so the generated code
uses the explicit `start_addr = 0xec000`. The warnings are cosmetic.

## Spike status

- [x] Stage 1: compile-only board skeleton. Both halves build reproducibly;
      UF2 address ranges verified against the bootloader layout
      (left `0x26000`-`0x9478c`, right `0x26000`-`0x6d134`; initial SP
      `0x2003fc08`, reset vector `0x26101`).
- [x] Stage 2: left-half safety test (2026-07-18). Boots through the stock
      MoErgo bootloader from `0x26000` with no SoftDevice, enumerates as
      `16c0:27db` "MoErgo Glove80" (RMK identifiable by its `vial:` USB
      serial), types correctly.
- [x] Stage 3: wireless split (2026-07-18). Halves pair on the configured
      static addresses; right-side keys type through the central.
- [x] Stage 4: USB/BLE Vial editing and storage (2026-07-18). Live keymap
      editing via vial.rocks over USB WebHID; BLE pairs without a passkey,
      registers HID keyboard + mouse, and reports battery over GATT.
      Known limitation: Vial cannot reach the keyboard over BLE on Linux.
      Root cause (gdb + btmon verified 2026-07-18): a BlueZ bug in
      profiles/input/hog-lib.c `find_report()`, which decides output-report
      numbering from the HID Information flags byte instead of the kernel
      uhid dev_flags; RMK's HID Info flags (0x03) make BlueZ misread the
      unnumbered Vial report as numbered and silently drop every write
      (present in 5.86 and master; worth reporting upstream — the fix is
      using uhid_flags). Firmware-side Vial-over-BLE was proven working by
      driving the GATT path directly. Decision: Vial stays USB-only; the
      planned custom GATT host protocol owns the wireless path (Web
      Bluetooth could not reach HID-over-GATT anyway). Full analysis in
      docs/vial-ble-investigation.md.
- [ ] Stage 5: minimum viable lighting (built, awaiting hardware test).
      Both halves: rear power-button LED dim at boot; WS2812 chain driven
      over SPIM3 with the layer color on chain index 0 (see "Lighting"
      above). UF2 ranges after the change: left `0x26000`-`0x94f94`,
      right `0x26000`-`0x6dde4`.
- [ ] Phase 1: sparse lighting compositor (built, awaiting hardware test).
      Pure-logic compositor crate (28 host tests passing) replaces the
      stage-5 frame source on both halves: base + layer accents (the
      hardcoded host-overlay placeholders it shipped with were replaced by
      the real Phase 2 host protocol; see "Lighting" above).
      UF2 ranges after the change: left `0x26000`-`0x959f4`, right
      `0x26000`-`0x6ea7c`.
- [ ] Phase 2: host protocol transports (built, awaiting hardware test).
      Central exposes the host protocol over a USB vendor raw-HID interface
      and a custom GATT service, feeding the compositor's host overlay (see
      "Host protocol transports" above); the Phase 1 hardcoded overlay
      placeholders are removed. RMK was initially vendored as a subtree
      under `dependencies/rmk`; it is now a pinned submodule. UF2 ranges after
      the change:
      left `0x26000`-`0x98da4`, right `0x26000`-`0x6f89c`.
- [ ] Phase 3: split lighting transfer (built, awaiting hardware test).
      Host writes to keys 40-79 forward to the right half over a bounded
      split application channel; reconnects resync the full right-half
      overlay + brightness/ceiling/toggles; the peripheral clears its host
      overlay 5 s after losing the central (see "Split lighting transfer"
      above). UF2 ranges after the change: left `0x26000`-`0x9ae24`, right
      `0x26000`-`0x7109c`.
- [ ] Phase 4: persistent lighting (built, awaiting hardware test).
      Config blobs (protocol v1.1) persist transactionally in the reserved
      runtime-config partition, load at boot ahead of the compiled defaults,
      stream to the peripheral on link-up, and are written/read over the
      CONFIG_* session commands on both transports (see "Persistent lighting"
      above). Peripheral bootloader entry now works via a magic-guarded split
      message (`ENTER_BOOTLOADER` target 1 answers OK when dispatched, BUSY
      when the peripheral is offline). UF2 ranges after the change: left
      `0x26000`-`0x9ed5c`, right `0x26000`-`0x754ec`.

## Safety rules for flashing

1. Never flash the right half before Stage 2 has proven left-half recovery.
2. Keep the known-good ZMK UF2s for both halves at hand before any flash.
3. Both halves keep their physical bootloader entry (reset-button double-tap /
   Magic+bootloader binding on the ZMK side); verify it before and after.
