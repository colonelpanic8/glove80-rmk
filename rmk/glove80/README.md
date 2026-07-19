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

The host protocol (`protocol/glove80-host-protocol/PROTOCOL.md`, including
its "Transports" addendum) is exposed on the **central (left) half** over
both USB and BLE. Protocol work is split three ways:

- **Vendored RMK patches** (`../vendor/rmk`, every site marked
  `GLOVE80 PATCH`): a vendor raw-HID interface on the USB composite
  (`HostProtocolReport`, usage page `0xFF88` / usage `0x01`, 32-byte IN/OUT
  reports — deliberately not multiplexed onto Vial's `0xFF60` interface,
  whose opcodes collide) and a custom GATT service
  (`fc550001-f8e0-459f-b421-c254fc42b138`; request characteristic
  `fc550002-…` write-without-response, response characteristic `fc550003-…`
  notify — not a HID service, so Web Bluetooth can reach it via
  `optionalServices`). The patches contain zero protocol knowledge: they
  only shuttle opaque chunks through `rmk::host_proto_pipe` channels and
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

Phase 2 split scope (documented in the PROTOCOL.md addendum): keys 0-39
apply locally; keys 40-79 are accepted and reported pending via
`PARTIAL_APPLY` but then dropped until Phase 3 forwards them (they never
render and are absent from `READ_OVERLAY`; `CLEAR_OVERLAY` answers `OK`).
Toggle ids 0-31 are accepted (the compositor's toggle bitmask; no default
records reference them yet, so they have no visual effect until lighting
config gains toggle records). `ENTER_BOOTLOADER` on target central answers
OK, waits ~300 ms for the response to flush, then reboots via the Adafruit
bootloader GPREGRET magic; target peripheral answers `OUT_OF_RANGE` until
Phase 3.

RMK is now consumed as a **vendored git subtree** at the previously pinned
revision `1156f82` (`rmk/vendor/rmk`; implementation-plan.md expected this at
Phase 3) because both the USB composite and the trouble-host GATT server are
assembled inside RMK with no extension hook. `git log --grep=git-subtree-dir`
shows the subtree provenance; keep patches minimal and marked.

## Building

```sh
./build.sh
```

produces `glove80_lh_rmk.uf2` and `glove80_rh_rmk.uf2`. The toolchain is
pinned in `rust-toolchain.toml`; RMK is consumed from the vendored subtree at
`../vendor/rmk` (frozen at the previously pinned upstream revision, plus the
marked Glove80 patches) and nrf-sdc stays pinned to an exact revision in
`Cargo.toml`.

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
      placeholders are removed. RMK vendored as a subtree under
      `rmk/vendor/rmk` with marked patches. UF2 ranges after the change:
      left `0x26000`-`0x98da4`, right `0x26000`-`0x6f89c`.

## Safety rules for flashing

1. Never flash the right half before Stage 2 has proven left-half recovery.
2. Keep the known-good ZMK UF2s for both halves at hand before any flash.
3. Both halves keep their physical bootloader entry (reset-button double-tap /
   Magic+bootloader binding on the ZMK side); verify it before and after.
