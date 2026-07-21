# Glove80 RMK firmware

This crate builds the two nRF52840 images for the Glove80 against the exact RMK
revision pinned at `../../dependencies/rmk`.

## Lighting architecture

The central owns one board-wide RMK `StandardLightingEngine` with 80 stable LED
IDs. `keyboard.toml` declares:

- every LED's logical matrix key, split node, output, and electrical index;
- the two 40-pixel addressable outputs;
- the board-wide dim-white background and per-layer accent scenes; and
- the topology revision exposed to hosts.

`src/central_lighting.rs` binds that engine to Rynk's standard lighting
controller. Live mutations use optimistic state revisions, and state queries
read the same authoritative engine state. The central writes its local chain
and sends the right half a sequence-numbered frame through RMK's split
application channel. The peripheral stages every chunk and touches hardware
only after a complete 40-pixel frame arrives.

`src/lighting.rs` contains the shared WS2812 hardware driver and peripheral
receiver. Both halves use SPIM3 at 4 MHz with GRB wire order. Every channel is
hard-clamped to 204/255 (80%) in the final driver, below all host-controlled
state. Chain power settles for 120 ms before the first frame.

The current firmware no longer compiles the former downstream compositor,
custom host-protocol transport, or shared-flash lighting configuration. Those
crates remain in the repository as legacy compatibility/reference material.

## Build

Run from the repository root:

```bash
nix develop --command bash -lc '
  cd crates/glove80-rmk
  cargo build --release --bin glove80_lh
  cargo build --release --bin glove80_rh
'
```

The supported release command packages both ELFs and UF2s, validates their
family IDs and flash ranges, and writes provenance under `dist/`:

```bash
make firmware
```

- left/central UF2 family: `0x9807B007`
- right/peripheral UF2 family: `0x9808B007`
- validated application range: `0x00026000..0x000dc000`

## Hardware qualification

A successful cross-build is not hardware validation. Before release, qualify
both halves together: typing, layer scenes, whole-board background, live Rynk
set/unset/clear/replace, state and brightness readback, TTL expiry, split
disconnect/reconnect, USB and BLE transports, sleep/resume, and bootloader
recovery. See `../../docs/qualification.md`.
