# glove80-rmk

This repository contains the active Glove80 firmware and control stack built
on [RMK](https://github.com/HaoboGu/rmk): firmware for both keyboard halves,
Glove80 lighting topology and hardware integration, legacy product-protocol
compatibility code, the `glove80-control` CLI, and the Lightbench browser UI.

The embedded firmware, compositor, and protocol crate intentionally remain
separate Cargo workspaces because they use different targets. Root `make`
commands provide the supported repository-level interface.

## Initialize

Clone with submodules, or initialize the exact RMK/Rynk revision after cloning:

```bash
git submodule update --init --recursive
```

The repository pins Rust and Nix inputs. Firmware builds must run through the
Nix development shell because Nordic bindgen requires its matching libclang.

## Root commands

```bash
make fmt              # format every Rust workspace
make check            # path/provenance checks, Rust checks/tests, UI test/build
make host-test        # glove80-control and product-protocol tests
make compositor-test  # native no_std compositor tests
make ui-install       # reproducible npm install
make ui-test          # Lightbench tests
make ui-build         # Lightbench production build
make firmware         # release-build and package both keyboard halves
make dist             # same complete release bundle, staged under dist/
```

`make dist` requires a clean repository and an initialized, unmodified RMK
submodule. It produces ignored release artifacts:

- `dist/glove80-rmk-<version>-lh.uf2` for the left/central half, UF2 family
  `0x9807B007`;
- `dist/glove80-rmk-<version>-rh.uf2` for the right/peripheral half, UF2 family
  `0x9808B007`;
- retained `.elf` files for both halves;
- `dist/SHA256SUMS`; and
- `dist/manifest.json`, including source/RMK/toolchain/protocol provenance,
  checksums, targets, families, and validated flash ranges.

The packager rejects wrong UF2 family IDs and images outside the application
range `0x00026000..0x000dc000`.

## Control CLI

Build or run the CLI from the repository root:

```bash
nix develop --command cargo run -p glove80-control -- --help
nix develop --command cargo run -p glove80-control -- devices
```

See [`tools/glove80-control/README.md`](tools/glove80-control/README.md) for USB,
BLE, keymap, lighting, configuration, version, and bootloader operations.

## Lightbench

Lightbench is the React browser UI in `ui/`:

```bash
npm ci --prefix ui
npm run dev --prefix ui
```

It supports mock-device development and browser transports. See
[`ui/README.md`](ui/README.md) for browser requirements and workflows. The
vendored Rynk WASM package records the exact RMK commit and checksum in
`ui/src/vendor/rynk-wasm/provenance.json`; `make check` prevents it from
drifting from the submodule used by firmware and native clients.

## Protocol surfaces

- **Rynk** is RMK's native keymap/configuration protocol and now owns live
  keymap plus topology-aware lighting operations. Firmware, the CLI, and the
  browser package all derive it from the pinned RMK revision.
- **Vial** remains a compatibility surface represented by the firmware's
  `vial.json`; this product currently selects Rynk and does not use Vial as the
  owner of lighting behavior.
- **glove80-host-protocol** is retained for legacy persistent configuration,
  firmware-version tooling, compatibility tests, and the browser demo. Current
  firmware no longer exposes its separate transport for live lighting.

## Lighting ownership

This repository owns Glove80-specific hardware facts and product policy: LED
topology and chain routing, the 80% hardware safety ceiling, default scenes,
and split-frame presentation. Generic composition, state revisioning,
scheduling, topology readback, and Rynk lighting commands come from pinned RMK.
The former downstream compositor remains only as compatibility/reference code.

Generic event, composition, scheduling, driver, split, storage, and Rynk
mechanisms belong upstream in RMK. Moving generic pieces upstream must preserve
the downstream Glove80 policy and is deliberately separate from this repository
extraction.

## Legacy recovery project

The legacy ZMK/Zephyr sources and known-good recovery workflow remain in the
separate [glove80-config repository](https://github.com/colonelpanic8/glove80-config).
They are not build inputs or release outputs of this RMK repository. Keep
recovery images clearly named and separate from the two RMK half images.

## Validation status

Host protocol golden vectors, compositor tests, CLI tests, Lightbench tests and
production build, both release cross-builds, UF2 family IDs, and flash ranges
are automated. See [`docs/migration.md`](docs/migration.md) for the extraction
record and [`docs/qualification.md`](docs/qualification.md) for qualification
procedures.

No hardware qualification was performed as part of repository extraction.
Typing, split reconnect, USB/BLE configuration, lighting output, persistence,
and bootloader recovery on both physical halves remain to be qualified. A
successful cross-build is not hardware validation.
