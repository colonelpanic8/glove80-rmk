# glove80-rmk

Glove80 firmware and native control tooling built on
[RMK](https://github.com/HaoboGu/rmk).

## Layout

```text
crates/
├── glove80-rmk/      # embedded firmware for both keyboard halves
├── glove80-control/  # native Rynk CLI
└── xtask/            # repository checks and release packaging
dependencies/
└── rmk/              # pinned upstream RMK/Rynk submodule
```

The firmware is a standalone Cargo workspace because it cross-compiles for the
nRF52840. The two native packages share the root workspace. Generated release
artifacts go in `dist/`.

## Setup

```bash
git submodule update --init --recursive
nix develop
```

The development shell provides the pinned Rust toolchain, `just`, Nordic
bindgen support, and native BLE build dependencies.

## Commands

Run `just` inside the development shell to list the supported tasks:

```bash
just fmt       # format both Cargo workspaces
just check     # validate repository paths and run native checks/tests
just host-test # test the CLI and repository task runner
just firmware  # build and package both keyboard halves
just dist      # alias of firmware
```

Run the CLI directly with `cargo run -p glove80-control -- --help`. See
[`crates/glove80-control/README.md`](crates/glove80-control/README.md) for its
commands and [`crates/glove80-rmk/README.md`](crates/glove80-rmk/README.md) for
firmware details.

## Release artifacts

`just dist` requires a clean repository and the exact clean RMK submodule
revision. It writes both ELF and UF2 images, `SHA256SUMS`, and a provenance
manifest under `dist/`. Packaging validates each half's UF2 family ID and the
application flash range `0x00026000..0x000dc000`.

Downstream configuration repositories may set `GLOVE80_CONFIG_GIT_COMMIT` and
`GLOVE80_CONFIG_GIT_DIRTY` to include their source identity in firmware build
labels and release manifests.

A successful build is not hardware qualification. Before release, test both
halves together: typing, layer lighting, state mutation/readback, split
reconnect, USB/BLE transports, sleep/resume, persistence, and bootloader
recovery.
