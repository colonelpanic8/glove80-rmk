#!/usr/bin/env bash
# Build both Glove80 RMK halves and produce flashable UF2 images.
#
# bindgen (nrf-mpsl-sys / nrf-sdc-sys) needs the libclang and freestanding
# headers supplied by the repository flake. Keeping both in the dev shell
# prevents ABI mismatches between independently selected nixpkgs revisions.
set -euo pipefail
cd "$(dirname "$0")"

: "${LIBCLANG_PATH:?run this build through: nix develop --command ./build.sh}"
: "${BINDGEN_EXTRA_CLANG_ARGS:?run this build through: nix develop --command ./build.sh}"

cargo build --release --bin glove80_lh
cargo build --release --bin glove80_rh

node ../../scripts/elf-to-uf2.mjs \
    --elf target/thumbv7em-none-eabihf/release/glove80_lh \
    --family 0x9807B007 --out glove80_lh_rmk.uf2
node ../../scripts/elf-to-uf2.mjs \
    --elf target/thumbv7em-none-eabihf/release/glove80_rh \
    --family 0x9808B007 --out glove80_rh_rmk.uf2
