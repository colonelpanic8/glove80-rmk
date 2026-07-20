#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

submodule_line="$(git submodule status -- dependencies/rmk)"
if [[ "${submodule_line:0:1}" != " " ]]; then
  echo "dependencies/rmk is uninitialized, modified, or on the wrong commit: $submodule_line" >&2
  exit 1
fi

expected_rmk="$(git rev-parse HEAD:dependencies/rmk)"
actual_rmk="$(git -C dependencies/rmk rev-parse HEAD)"
if [[ "$actual_rmk" != "$expected_rmk" ]]; then
  echo "RMK checkout $actual_rmk does not match gitlink $expected_rmk" >&2
  exit 1
fi
if [[ -n "$(git -C dependencies/rmk status --porcelain)" ]]; then
  echo "dependencies/rmk has local changes" >&2
  exit 1
fi

while IFS= read -r manifest; do
  manifest_dir="$(dirname "$manifest")"
  while IFS= read -r relative_path; do
    resolved="$(realpath -m "$manifest_dir/$relative_path")"
    case "$resolved" in
      "$repo_root"|"$repo_root"/*) ;;
      *)
        echo "$manifest has a path dependency outside the repository: $relative_path" >&2
        exit 1
        ;;
    esac
  done < <(sed -nE 's/.*path[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$manifest")
done < <(find . -path ./dependencies/rmk -prune -o -name Cargo.toml -print)

node scripts/check-rynk-wasm-provenance.mjs "$actual_rmk"

cargo check --workspace --all-targets
cargo test --workspace
cargo test --manifest-path crates/glove80-host-protocol/Cargo.toml
cargo test --manifest-path crates/glove80-compositor/Cargo.toml

npm ci --prefix ui
npm test --prefix ui
npm run build --prefix ui
