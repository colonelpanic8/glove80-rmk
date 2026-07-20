#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

submodule_line="$(git submodule status -- dependencies/rmk)"
if [[ "${submodule_line:0:1}" != " " ]]; then
  echo "dependencies/rmk is uninitialized, modified, or on the wrong commit: $submodule_line" >&2
  exit 1
fi

rmk_commit="$(git rev-parse HEAD:dependencies/rmk)"
if [[ "$(git -C dependencies/rmk rev-parse HEAD)" != "$rmk_commit" ]] ||
   [[ -n "$(git -C dependencies/rmk status --porcelain)" ]]; then
  echo "dependencies/rmk must be clean and checked out at $rmk_commit" >&2
  exit 1
fi

dirty=false
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  dirty=true
  if [[ "${GLOVE80_ALLOW_DIRTY:-0}" != "1" ]]; then
    echo "release bundles require a clean repository (set GLOVE80_ALLOW_DIRTY=1 only for local validation)" >&2
    exit 1
  fi
fi

node scripts/check-rynk-wasm-provenance.mjs "$rmk_commit"

version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' firmware/glove80-rmk/Cargo.toml | head -1)"
protocol_version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' crates/glove80-host-protocol/Cargo.toml | head -1)"
rust_toolchain="$(sed -nE 's/^channel[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' rust-toolchain.toml | head -1)"
source_commit="$(git rev-parse HEAD)"
rmk_version="$(git -C dependencies/rmk describe --tags --always)"
config_commit="${GLOVE80_CONFIG_GIT_COMMIT:-standalone}"
config_dirty="${GLOVE80_CONFIG_GIT_DIRTY:-false}"

(
  cd firmware/glove80-rmk
  cargo build --release --bin glove80_lh
  cargo build --release --bin glove80_rh
)

target_dir="firmware/glove80-rmk/target/thumbv7em-none-eabihf/release"
mkdir -p dist
install -m 0644 "$target_dir/glove80_lh" "dist/glove80-rmk-${version}-lh.elf"
install -m 0644 "$target_dir/glove80_rh" "dist/glove80-rmk-${version}-rh.elf"

node scripts/elf-to-uf2.mjs \
  --elf "$target_dir/glove80_lh" \
  --family 0x9807B007 \
  --out "dist/glove80-rmk-${version}-lh.uf2"
node scripts/elf-to-uf2.mjs \
  --elf "$target_dir/glove80_rh" \
  --family 0x9808B007 \
  --out "dist/glove80-rmk-${version}-rh.uf2"

node scripts/package-release.mjs \
  --dist dist \
  --version "$version" \
  --source-commit "$source_commit" \
  --dirty "$dirty" \
  --config-commit "$config_commit" \
  --config-dirty "$config_dirty" \
  --rmk-commit "$rmk_commit" \
  --rmk-version "$rmk_version" \
  --rust-toolchain "$rust_toolchain" \
  --protocol-version "$protocol_version"
