#!/usr/bin/env bash
# Phase 6 hardware qualification: unified canonical config (keymap + lighting)
# against a real Glove80. Run manually, with the keyboard connected; NOT part
# of CI (cargo tests cover the same flows over a mock transport).
#
#   ./scripts/phase6-hw-test.sh            # auto transport (USB if present)
#   ./scripts/phase6-hw-test.sh --ble      # force BLE
#
# The script backs the current state up first and restores it at the end
# (also on failure), but read the output before walking away: keymap writes
# are best-effort per batch and the restore is itself an apply.
set -euo pipefail
cd "$(dirname "$0")/.."

TRANSPORT_ARGS=("$@")
CLI=(cargo run -q -p glove80-control -- "${TRANSPORT_ARGS[@]}")
WORK="$(mktemp -d /tmp/glove80-phase6.XXXXXX)"
echo "work dir: $WORK (kept for inspection)"

step() { printf '\n=== %s ===\n' "$*"; }

step "device identity + capabilities"
"${CLI[@]}" version
"${CLI[@]}" lighting caps

step "backup: export the active config (keymap + lighting)"
"${CLI[@]}" config export "$WORK/backup.toml"
restore() {
    step "restore: re-apply the backup"
    "${CLI[@]}" config apply "$WORK/backup.toml"
}
trap restore EXIT

step "offline validation of the full-keyboard example"
"${CLI[@]}" config validate examples/glove80.toml

step "dry run (must not touch the device)"
"${CLI[@]}" config apply --dry-run examples/glove80.toml

step "unified apply: keymap (batched, read-back verified) then lighting (atomic)"
"${CLI[@]}" config apply examples/glove80.toml

step "spot-check: keymap read-back matches the example"
"${CLI[@]}" keymap read --all
"${CLI[@]}" config show

step "export round-trip: export -> apply -> export must be stable"
"${CLI[@]}" config export "$WORK/export1.toml"
"${CLI[@]}" config apply "$WORK/export1.toml"
"${CLI[@]}" config export "$WORK/export2.toml"
diff -u "$WORK/export1.toml" "$WORK/export2.toml"
echo "round trip stable"

step "Vial interop (manual): edit a key in Vial now, then re-run"
echo "  ${CLI[*]} keymap read --layer 0"
echo "and confirm the Vial edit reads back. Skipping in-script."

step "done — restoring the backup"
# trap performs the restore; disarm the failure message path.
