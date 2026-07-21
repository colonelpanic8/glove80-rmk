# Rynk migration

## Current decision

Rynk owns live keymap and lighting control. The downstream RMK dependency is
`colonelpanic8/rmk:master` at `6bcf2d94`, pinned as the
`dependencies/rmk` submodule. It composes upstream Rynk with the six
reviewable topics listed in [upstream/PATCHES.md](./upstream/PATCHES.md).

The older `glove80-rynk` branch and the pre-Rynk `glove80` branch remain
rollback/provenance refs only. The current firmware no longer depends on their
vendor transport, shared-flash, keymap-operation, CRC, or VBUS patches.

## Runtime ownership

| Capability | Owner | Transport |
| --- | --- | --- |
| Keymap read/write and persistence | Rynk | USB HID; native BLE GATT; browser WebHID |
| Lighting topology, state, overlays, per-layer scenes, and readback | RMK lighting through Rynk | USB HID; native BLE GATT; browser WebHID |
| Vial RGB Matrix compatibility | RMK lighting service | Vial host protocol |
| Cross-half lighting state and remote boot request | Firmware over `rmk::split_app` | RMK split link |
| Animation sampling and physical LED output | Each Glove80 half locally | local RMK renderer, WS2812, and power-button PWM drivers |

The RMK lighting service is authoritative. A successful Rynk or Vial mutation
changes that service state; subsequent Rynk/Vial queries and rendered frames
observe the same value rather than a host-side shadow copy.

## Browser packaging

Lightbench commits a release `wasm-pack --target web` package under
`ui/src/vendor/rynk-wasm`, tied to the exact RMK gitlink by
`provenance.json`. Regenerate it from the repository root with:

```sh
RUSTUP_TOOLCHAIN=1.97.0 wasm-pack build --release --target web \
  --out-dir "$PWD/ui/src/vendor/rynk-wasm" dependencies/rmk/rynk/rynk-wasm
```

`wasm-pack` replaces the directory `.gitignore` and the generated README
header. Restore the repository comment and provenance header before committing,
then update the recorded WASM SHA-256. `make check` verifies both the RMK commit
and checksum.

## Qualification

- Both halves have previously been flashed with the Rynk/HID firmware and the
  USB Rynk path, all-layer reads, lighting mutation, and state readback were
  exercised on physical Glove80 hardware.
- The complete composed RMK feature matrix, Rynk native tests/doctests, WASM
  package/typecheck, and clippy gates pass at the current integration tree.
- Split renderer replication and both embedded halves compile at the current
  development pin; physical phase/latency qualification remains pending.
- The repository check and both release cross-builds are required after every
  pin update.
- Interactive browser chooser, BLE-only hardware sessions, reconnect/outage,
  persistence, and fresh-device recovery remain useful manual regression
  checks; they are not replaced by host-side tests.

## Upstream posture

The required generic changes are now proposed independently as upstream PRs
[#984](https://github.com/HaoboGu/rmk/pull/984),
[#985](https://github.com/HaoboGu/rmk/pull/985),
[#986](https://github.com/HaoboGu/rmk/pull/986), and
[#987](https://github.com/HaoboGu/rmk/pull/987). Refresh and composition rules
live in [upstream/BRANCH-STACK.md](./upstream/BRANCH-STACK.md).
