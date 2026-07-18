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

## Lighting (Stage 5)

All lighting lives in `src/lighting.rs`; both binaries register it as a
custom RMK processor from inside the `#[rmk_central]` / `#[rmk_peripheral]`
module.

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

Design (compositor seed): `lighting.rs` splits into a frame sink
(`Ws2812Chain`: SPIM3 + EasyDMA buffer, encodes a `Frame = [Rgb; 40]` in
chain order) and a trivial frame source (`LightingProcessor` subscribed to
RMK `LayerChangeEvent`s: the active layer picks the color of chain index 0,
the thumb-cluster top-inner key). The compositor replaces the frame source
and renders into the same `Frame`; driver and task wiring stay. Rendering is
strictly event-driven -- one initial frame after the ~120 ms chain-power
settle, then a frame per layer change; no periodic tick, nothing in the
key-scan path. The right half renders locally but still tracks the active
layer because RMK's split peripheral republishes the synced layer state as a
local `LayerChangeEvent` (no split-protocol changes).

Current clamp: the driver (`Ws2812Chain::write`) hard-clamps every color
channel to 204/255 (80%) at encode time, per MoErgo's current/warranty
limit. Calling code cannot bypass it; the stock colors are far dimmer than
the clamp anyway.

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
2. ~120 ms after boot: the top-inner thumb key LED (chain index 0) lights
   dim blue (layer 0) on the left half; same LED on the right half once it
   has a synced layer state (also blue at boot).
3. Holding the layer-1 key (left thumb Escape/LT or right-hand semicolon
   LT) turns that LED green on the left half, and on the right half too
   once the layer sync arrives; releasing returns it to blue. Layer colors:
   0 blue, 1 green, 2 (Magic held) magenta, 3 (Games toggle) red, 4 (Mac
   Hyper toggle) cyan.
4. All other 39 key LEDs stay dark (the frame drives them to off
   explicitly, so no stale bootloader colors should persist).
5. Typing latency/split behavior should be unchanged; lighting only runs on
   layer events.

## Building

```sh
./build.sh
```

produces `glove80_lh_rmk.uf2` and `glove80_rh_rmk.uf2`. The toolchain is
pinned in `rust-toolchain.toml`; RMK and nrf-sdc are pinned to exact revisions
in `Cargo.toml`.

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
      Known limitation: browser Vial cannot reach the keyboard over BLE
      (WebHID has no GATT transport) — the planned custom host protocol
      must cover that path.
- [ ] Stage 5: minimum viable lighting (built, awaiting hardware test).
      Both halves: rear power-button LED dim at boot; WS2812 chain driven
      over SPIM3 with the layer color on chain index 0 (see "Lighting"
      above). UF2 ranges after the change: left `0x26000`-`0x94f94`,
      right `0x26000`-`0x6dde4`.

## Safety rules for flashing

1. Never flash the right half before Stage 2 has proven left-half recovery.
2. Keep the known-good ZMK UF2s for both halves at hand before any flash.
3. Both halves keep their physical bootloader entry (reset-button double-tap /
   Magic+bootloader binding on the ZMK side); verify it before and after.
