#!/usr/bin/env bash
# Compatibility wrapper for the repository-level release build.
#
# bindgen (nrf-mpsl-sys / nrf-sdc-sys) needs the libclang and freestanding
# headers supplied by the repository flake. Keeping both in the dev shell
# prevents ABI mismatches between independently selected nixpkgs revisions.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
exec "$repo_root/scripts/build-release.sh"
