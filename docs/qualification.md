# Phase 7 qualification run-sheet

> RMK commands and artifacts in this checklist use this repository. ZMK
> recovery commands refer to the separate legacy `glove80-config` repository
> and must be run there; its sources are intentionally not vendored here.

Hardware acceptance run-sheet for replacing the Glove80 ZMK baseline with
RMK. Run from the repository root on branch `codex/rmk-evaluation`.

## Execution labels

- **AUTO** — shell commands can complete and verify the checklist item once
  both halves are powered and connected.
- **HANDS** — needs typing, a cable pull, a power switch, Magic-hold, or a
  visual LED check.
- **PENDING** — the named firmware/tooling feature does not yet exist; do not
  record a pass from a partial substitute.
- Item counts use one mutually exclusive label per top-level checklist item.
- Commands which only read state are marked **read-only**.
- Commands which change keymap, lighting config, firmware, bonds, or flash are
  marked **destructive** and have a safety note next to them.

## Safety and recovery preflight

- [ ] Connect both halves and verify that neither bootloader volume is
      mounted:

  ```sh
  test ! -e /run/media/imalison/GLV80LHBOOT
  test ! -e /run/media/imalison/GLV80RHBOOT
  ```

- [ ] Create a temporary evidence directory and build the CLI:

  ```sh
  export G80_QUAL_DIR="$(mktemp -d /tmp/glove80-qualification.XXXXXX)"
  export G80CTL=./target/debug/glove80-control
  cargo build -p glove80-control
  git branch --show-current | tee "$G80_QUAL_DIR/branch.txt"
  git rev-parse --short=8 HEAD | tee "$G80_QUAL_DIR/git-hash.txt"
  ```

  - Expected branch: `codex/rmk-evaluation`.
  - Keep `G80_QUAL_DIR` until the results and recovery drill are complete.

- [ ] **AUTO, build only:** build both RMK images:

  ```sh
  make dist
  test -f dist/glove80-rmk-0.1.0-lh.uf2
  test -f dist/glove80-rmk-0.1.0-rh.uf2
  ```

- [ ] **AUTO, build only:** build and retain the four ZMK recovery images:

  ```sh
  nix build .#firmware --out-link "$G80_QUAL_DIR/zmk-firmware"
  test -f "$G80_QUAL_DIR/zmk-firmware/glove80-left.uf2"
  test -f "$G80_QUAL_DIR/zmk-firmware/glove80-right.uf2"
  test -f "$G80_QUAL_DIR/zmk-firmware/glove80-left-settings-reset.uf2"
  test -f "$G80_QUAL_DIR/zmk-firmware/glove80-right-settings-reset.uf2"
  ls -lh "$G80_QUAL_DIR/zmk-firmware"/*.uf2
  ```

- [ ] Record the baseline before changing device state:

  ```sh
  "$G80CTL" --usb version | tee "$G80_QUAL_DIR/version-before.txt"
  "$G80CTL" --usb lighting caps | tee "$G80_QUAL_DIR/caps-before.txt"
  "$G80CTL" --usb config export "$G80_QUAL_DIR/config-before.toml"
  "$G80CTL" --usb config export "$G80_QUAL_DIR/lighting-before.bin" --raw
  "$G80CTL" --usb keymap read --all --raw > "$G80_QUAL_DIR/keymap-before.txt"
  ```

  - `version` must print this shape, with build-specific values in braces:

    ```text
    glove80-control v{semver} ({git-hash}{-dirty})
    Rynk protocol: v0.3
    firmware: glove80-rmk v{semver} ({git-hash}{-dirty}) / RMK v{semver}
    RMK: v{semver}
    device: {manufacturer} {product} (USB {vid}:{pid})
    serial: {serial}
    ```

  - The firmware label and structured RMK line must name the intended versions.
  - A dirty suffix is acceptable only for an intentionally retained local build.
  - This query currently identifies the central/application image; verify the
    peripheral artifact separately until Rynk exposes routed split build info.
  - `lighting caps` must print:

    ```text
    protocol: v1.3
    keys: 40 left + 40 right
    layer capacity: 8
    max cells per operation: 80
    overlay cell capacity: 80
    max message length: 1536
    effects: solid, blink, breathe
    features: per-write TTL, toggles, bootloader entry, atomic replace, overlay read-back, partial-apply reporting
    ```

  - `config export --raw` requires an already stored lighting config. If it
    reports compiled-in defaults instead, exact restoration to the original
    no-record state is not implemented. Do not run destructive config tests
    unless that loss is acceptable; record the factory-restore gap under item
    6.

- Flashing rules for every procedure below:

  - Never put both halves in their bootloaders simultaneously.
  - Always update the peripheral first and wait for it to boot and reconnect;
    only then update the central.
  - Copy only `*_rh_*`/`glove80-right*` to `GLV80RHBOOT` and only
    `*_lh_*`/`glove80-left*` to `GLV80LHBOOT`.
  - A successful `cp` normally causes the UF2 volume to disappear and the
    half to reboot. Run `sync` before continuing.
  - Keep normal ZMK, ZMK settings-reset, and both RMK UF2s available throughout
    the run.

## Optional fresh RMK flash

- **AUTO, destructive:** use only when both halves already run a compatible
  RMK host protocol. This is also the exact reflash sequence used by the
  bootloader qualification.
- Safety: the same-build RMK UF2s and all four ZMK recovery UF2s must have
  passed the preflight checks above.

1. Flash and rejoin the peripheral:

   ```sh
   "$G80CTL" --usb bootloader --peripheral --yes
   timeout 30 sh -c 'until test -d /run/media/imalison/GLV80RHBOOT; do sleep 1; done'
   cp dist/glove80-rmk-0.1.0-rh.uf2 /run/media/imalison/GLV80RHBOOT/
   sync
   timeout 30 sh -c 'while test -d /run/media/imalison/GLV80RHBOOT; do sleep 1; done'
   timeout 45 sh -c 'until ./target/debug/glove80-control --usb version >/dev/null 2>&1; do sleep 2; done'
   ```

   - Expected bootloader output: `peripheral half acknowledged the bootloader
     request` or `no response — the peripheral half most likely reset into its
     bootloader`.
   - The right UF2 volume appears and disappears after the copy; later split
     typing tests confirm that the peripheral rejoined.

2. Flash the central only after the peripheral has rejoined:

   ```sh
   "$G80CTL" --usb bootloader --yes
   timeout 30 sh -c 'until test -d /run/media/imalison/GLV80LHBOOT; do sleep 1; done'
   cp dist/glove80-rmk-0.1.0-lh.uf2 /run/media/imalison/GLV80LHBOOT/
   sync
   timeout 30 sh -c 'while test -d /run/media/imalison/GLV80LHBOOT; do sleep 1; done'
   timeout 45 sh -c 'until ./target/debug/glove80-control --usb version >/dev/null 2>&1; do sleep 2; done'
   "$G80CTL" --usb version
   ```

   - Expected bootloader output: `central half acknowledged the bootloader
     request` or `no response — the central half most likely reset into its
     bootloader`.
   - Final `version` must show the expected application and RMK build identity.

## 1. USB typing (left-local) — HANDS

- [ ] Connect the USB cable to the left/central half.
- [ ] **Read-only:** confirm that USB is the selected host transport:

  ```sh
  "$G80CTL" --usb lighting ping --data usb-left
  ```

  - Expected: `PING 8 bytes over USB (...): {latency} ms`.
  - Any BLE transport description is a failure.

- [ ] Capture only left-half typing in a terminal:

  ```sh
  od -An -tx1 -v
  ```

  - **HANDS:** type `test` using left-half keys, then press left Ctrl-D twice.
  - Expected bytes: `74 65 73 74` in that order, with no duplicates.
  - Also verify left Shift, Ctrl, Tab, Backspace, and the left thumb-layer key
    in a scratch editor.

## 2. BLE typing — HANDS

- [ ] Pair/bond the Glove80 through the desktop Bluetooth UI if it is not
      already bonded.
- [ ] **HANDS:** unplug the left-half USB data cable. Leave both power
      switches on.
- [ ] **Read-only:** force the custom BLE transport:

  ```sh
  "$G80CTL" --ble version
  "$G80CTL" --ble lighting ping --data ble
  ```

  - `version` must show the expected application and RMK build identity.
  - Expected ping: `PING 3 bytes over BLE (...): {latency} ms`.

- [ ] Capture typing over BLE:

  ```sh
  od -An -tx1 -v
  ```

  - **HANDS:** type `typewriter` and a space using keys from both halves, then
    press left Ctrl-D twice.
  - Expected bytes: `74 79 70 65 77 72 69 74 65 72 20` exactly once and in
    order.
- [ ] **HANDS:** reconnect USB after this item unless the next command is
      explicitly `--ble`.

## 3. Full split typing; right-half disconnect/reconnect — HANDS

- [ ] Start with both halves connected and verify the split:

  ```sh
  "$G80CTL" --usb version
  ```

  - Expected peripheral state: `connected`.

- [ ] **HANDS:** turn the right-half power switch off.
- [ ] Wait longer than the peripheral overlay grace period and inspect state:

  ```sh
  sleep 7
  "$G80CTL" --usb version
  "$G80CTL" --usb lighting set 40 blue
  ```

  - `version` must show `peripheral ... disconnected (last known)`.
  - The overlay write must say:

    ```text
    set 1 cell(s): PARTIAL APPLY — peripheral half offline
      applied on the central half now
      pending on the peripheral: keys 40 (will sync when it reconnects)
    ```

- [ ] While the right half is off, repeat the item-1 `test` capture on the
      left half. Left-local typing must remain correct.
- [ ] **HANDS:** turn the right-half power switch on.
- [ ] Verify reconnect and queued overlay resync:

  ```sh
  timeout 45 sh -c 'until ./target/debug/glove80-control --usb version | grep -q "peripheral.*connected"; do sleep 2; done'
  "$G80CTL" --usb version
  "$G80CTL" --usb lighting read
  ```

  - `version` must show `peripheral ... connected`.
  - `lighting read` must include key `40`, effect `solid`, color `#0000ff`,
    TTL `none`; the matching right key must visibly become blue.
- [ ] **HANDS:** type `hijklnm,./` on the right half into a scratch editor.
      Every character must arrive once and in order.
- [ ] Clean up:

  ```sh
  "$G80CTL" --usb lighting clear
  ```

  - Expected: `clear overlay: applied to both halves`.

## 4. Config editing + persistence over USB and over BLE — HANDS

- This item is command-driven except for two power cycles and a USB cable
  pull.
- **Destructive:** `config apply` writes the live keymap and persistent
  lighting store.
- Safety: require the preflight exports. The lighting section commits
  atomically; keymap writes are only atomic per batch and are not rolled back
  after a later failure. Restore `config-before.toml` at the end.

### USB apply and persistence

- [ ] Validate and apply the repository baseline:

  ```sh
  "$G80CTL" config validate tools/glove80-control/examples/glove80.toml
  "$G80CTL" --usb config apply tools/glove80-control/examples/glove80.toml
  "$G80CTL" --usb config show | tee "$G80_QUAL_DIR/config-usb-show.txt"
  "$G80CTL" --usb config export "$G80_QUAL_DIR/config-usb.toml"
  ```

  - Validation must report `5 layer grid(s) to write` with bound counts
    `80, 80, 11, 80, 80` and this lighting summary:

    ```text
    9 record(s), 1437-byte blob
    REC  ACTIVATION  CELLS  KEYS                 EFFECTS
    0    always      12     0-5,40-45            solid
    1    layer 0     14     0-2,6-9,40-42,46-49  solid
    2    layer 1     14     0-2,6-9,40-42,46-49  solid
    3    layer 2     14     0-2,6-9,40-42,46-49  solid
    4    layer 3     14     0-2,6-9,40-42,46-49  solid
    5    layer 4     14     0-2,6-9,40-42,46-49  solid
    6    layer 5     14     0-2,6-9,40-42,46-49  solid
    7    layer 6     14     0-2,6-9,40-42,46-49  solid
    8    layer 7     14     0-2,6-9,40-42,46-49  solid
    toggles persisted across reboots: none
    toggles initially on: none
    ```

  - Apply must print `keymap applied: 420 positions written across 5
    layer(s); changes are live and persisted`, then `session opened`,
    `transferred 1437/1437 bytes`, and `commit OK: the new lighting config is
    active and persisted`.
  - `config show` must start with `keymap (5 populated layer(s)):` and end
    with the same nine-record lighting table.

- [ ] **HANDS:** power-cycle the right half, wait for it to reconnect, then
      power-cycle the left half. Never switch both off at once during this
      step.
- [ ] Verify that USB sees the same state after reboot:

  ```sh
  "$G80CTL" --usb config show
  "$G80CTL" --usb config export "$G80_QUAL_DIR/config-usb-after-reboot.toml"
  diff -u "$G80_QUAL_DIR/config-usb.toml" "$G80_QUAL_DIR/config-usb-after-reboot.toml"
  ```

  - Expected `diff`: no output and exit status 0.

### BLE apply and persistence

- [ ] **HANDS:** unplug USB data so this cannot silently exercise USB.
- [ ] Apply the exported config through BLE, then read it back through BLE:

  ```sh
  "$G80CTL" --ble config apply "$G80_QUAL_DIR/config-usb.toml"
  "$G80CTL" --ble config show
  "$G80CTL" --ble config export "$G80_QUAL_DIR/config-ble.toml"
  diff -u "$G80_QUAL_DIR/config-usb.toml" "$G80_QUAL_DIR/config-ble.toml"
  ```

  - Apply must show the same keymap progress and a successful atomic lighting
    commit.
  - Expected `diff`: no output and exit status 0.

- [ ] **HANDS:** power-cycle right, wait for reconnect, then power-cycle
      left; leave USB unplugged.
- [ ] Verify persistence through BLE and restore the preflight backup:

  ```sh
  "$G80CTL" --ble config export "$G80_QUAL_DIR/config-ble-after-reboot.toml"
  diff -u "$G80_QUAL_DIR/config-ble.toml" "$G80_QUAL_DIR/config-ble-after-reboot.toml"
  "$G80CTL" --ble config apply "$G80_QUAL_DIR/config-before.toml"
  ```

  - Expected `diff`: no output and exit status 0.
  - The restore must complete without `LOSSY`, `PARTIAL`, `BUSY`, or any
    protocol error.
- [ ] **HANDS:** reconnect USB.

## 5. Eight uniform editable layers — AUTO

- **Destructive:** the test writes one harmless matrix hole on every layer,
  reads it back, then restores all eight holes to `KC_NO`.
- Safety: this item first applies `examples/glove80.toml`, where position
  `r0,c5` is `KC_NO` on all eight layers; retain `config-before.toml` as the
  broader recovery path.

- [ ] Establish the known keymap fixture:

  ```sh
  "$G80CTL" --usb config apply tools/glove80-control/examples/glove80.toml
  ```

  - Expected: 420 matching keymap read-backs, no `LOSSY`, and a successful
    1437-byte lighting commit.

- [ ] Read all advertised layer slots:

  ```sh
  "$G80CTL" --usb keymap read --all --raw | tee "$G80_QUAL_DIR/keymap-8-before.txt"
  rg '^layer [0-7] ' "$G80_QUAL_DIR/keymap-8-before.txt"
  ```

  - Expected: exactly eight headers, `layer 0` through `layer 7`, each saying
    `(6x14 grid, key = row*14 + col)`.
  - The four holes at flat positions 5, 8, 75, and 78 print `--` when they
    contain `KC_NO`.

- [ ] Write and read back the same value in all eight slots:

  ```sh
  "$G80CTL" --usb keymap set \
    0 0,5 KC_A 1 0,5 KC_A 2 0,5 KC_A 3 0,5 KC_A \
    4 0,5 KC_A 5 0,5 KC_A 6 0,5 KC_A 7 0,5 KC_A
  "$G80CTL" --usb keymap read --all --raw | tee "$G80_QUAL_DIR/keymap-8-written.txt"
  ```

  - Expected write output: eight lines of the form `layer N key 5 (r0,c5):
    KC_A (0x0004)`, followed by `wrote 8 entries (read-back matches; changes
    are live and persisted)`.
  - Each layer's `r0` row must contain `0x0004` at column 5.

- [ ] Restore and verify all eight slots:

  ```sh
  "$G80CTL" --usb keymap set \
    0 0,5 KC_NO 1 0,5 KC_NO 2 0,5 KC_NO 3 0,5 KC_NO \
    4 0,5 KC_NO 5 0,5 KC_NO 6 0,5 KC_NO 7 0,5 KC_NO
  "$G80CTL" --usb keymap read --all --raw > "$G80_QUAL_DIR/keymap-8-restored.txt"
  ```

  - Expected: eight `KC_NO (0x0000)` write lines, a matching eight-entry
    summary, and `--` again at `r0,c5` in every layer.

## 6. Reboot, factory restore, corrupt-record fallback, interrupted update — PENDING

- **Pending features:** factory snapshot/restore command, deterministic
  config-slot corruption hook, and a CONFIG transfer fault-injection/pause
  harness.
- Do not pass this item from the reboot subtest alone.
- Current recovery is newest valid lighting config → compiled lighting
  defaults. The complete factory keymap snapshot → minimal recovery keymap
  chain from `design-goals.md` is not implemented.

- [ ] **HANDS, available subtest:** hold Magic and tap `QK_RBT` once to
      exercise the central keycode-driven reboot, then power-cycle the right
      half separately.
  - Magic is the left inner thumb `MO(2)` key.
  - `QK_RBT` is an outer key on row 4 in the Magic layer. Split key actions
    are interpreted by the central; do not treat the right-side binding as a
    physical right-half reboot route.
  - Wait for the central to re-enumerate and the peripheral to reconnect
    before using the right power switch.
  - Expected after both rejoin: `version` shows matching, connected halves;
    `config show` matches its pre-reboot output.

- [ ] **PENDING:** factory restore.
  - There is no `config restore` CLI verb and no firmware factory snapshot
    restore command.
  - ZMK settings-reset UF2s are recovery artifacts, not a qualifying RMK
    factory-restore implementation.

- [ ] **PENDING:** corrupt-record fallback.
  - The store has CRC-checked A/B generations, but there is no bounded host
    command or test image that corrupts one selected RMK config slot for a
    repeatable hardware test.
  - Do not use an arbitrary SWD/raw-flash write during routine qualification.

- [ ] **PENDING:** interrupted config update.
  - `config apply` sends BEGIN/DATA/COMMIT without a pause point; a small blob
    completes too quickly for a controlled cable pull.
  - Required fixture: pause after at least one CONFIG_DATA chunk, request a
    cable/power interruption, reboot, then prove the previous raw export is
    byte-identical.

## 7. Programmatic bootloader entry, both halves — AUTO

- **Destructive:** entering the bootloader stops typing; copying a UF2
  rewrites application flash.
- Safety: use the same-build RMK images created in preflight, peripheral
  first, and never leave both bootloader volumes mounted.

- [ ] Run the two-step procedure under **Optional fresh RMK flash**.
- [ ] Verify after each `bootloader` command:
  - Correct half-specific volume appears.
  - Other half's bootloader volume does not exist.
  - Copying the matching RMK UF2 makes the volume disappear.
  - Peripheral rejoins before central bootloader entry begins.
- [ ] Final verification:

  ```sh
  test ! -e /run/media/imalison/GLV80LHBOOT
  test ! -e /run/media/imalison/GLV80RHBOOT
  "$G80CTL" --usb version
  ```

  - Both halves must be connected, match semver/hash, and show no mismatch
    warning.

## 8. Static / blink / breathe on both halves — HANDS

- [ ] Clear the overlay, install one pair per effect, and read it back:

  ```sh
  "$G80CTL" --usb lighting clear
  "$G80CTL" --usb lighting set 10,50 red
  "$G80CTL" --usb lighting set 11,51 green --effect blink --period 1000 --duty 25
  "$G80CTL" --usb lighting set 12,52 blue --effect breathe --period 2000 --phase 0
  "$G80CTL" --usb lighting read
  ```

  - Each write must say `set 2 cell(s): applied to both halves`.
  - `lighting read` must print six rows with these values:

    | KEY | EFFECT | COLOR | PERIOD | PHASE | DUTY | TTL |
    | ---: | --- | --- | --- | --- | --- | --- |
    | 10 | solid | `#ff0000` | `-` | `-` | `-` | `none` |
    | 11 | blink | `#00ff00` | `1000ms` | `0ms` | `25%` | `none` |
    | 12 | breathe | `#0000ff` | `2000ms` | `0ms` | `-` | `none` |
    | 50 | solid | `#ff0000` | `-` | `-` | `-` | `none` |
    | 51 | blink | `#00ff00` | `1000ms` | `0ms` | `25%` | `none` |
    | 52 | breathe | `#0000ff` | `2000ms` | `0ms` | `-` | `none` |

- [ ] **HANDS, visual:** on both halves confirm:
  - Local key 10 is steady red.
  - Local key 11 is green for about 250 ms and black for about 750 ms each
    second; the dark phase does not reveal lighting below it.
  - Local key 12 fades smoothly blue over a two-second cycle.
  - Opposite-half animation phase may drift; per-half phase is the v1 design.
  - No channel is visibly driven above the compiled 80% ceiling.
- [ ] Clean up:

  ```sh
  "$G80CTL" --usb lighting clear
  "$G80CTL" --usb lighting read
  ```

  - Expected: `clear overlay: applied to both halves`, then `host overlay is
    empty`.

## 9. Full lighting stack composed simultaneously — PENDING

- **Pending feature:** complete conditional-lighting/status-and-safety
  integration.
- The compositor can order base + layer + toggle + host + status in pure
  logic, and the current persistent format can express base/layer/toggle.
  The public CLI/config path cannot yet install a qualifying status/safety
  record, and the in-flight per-record gates/firmware-state conditions are
  not wired end-to-end through released firmware and CLI.
- `lighting toggle` alone is not sufficient: a toggle state has no visible
  effect unless the stored config contains a record activated by that ID.
- Acceptance fixture still needed:
  - One overlapping key with a distinguishable color at every class.
  - Magic-held or firmware-state gate exposing the status record.
  - Host overlay above toggle and below status.
  - `config show` rendering the gate/status information.
- Do not record a pass until one hardware run visibly proves priority
  `status > host > toggle > layer > base` on both halves.

## 10. Sparse host clear reveals the stack below — HANDS

- Precondition: `examples/glove80.toml` is active and layer 0 is selected.
- [ ] Put a sparse host overlay over both a layer-painted key and a base-only
      key on each half:

  ```sh
  "$G80CTL" --usb lighting set 0,3,40,43 yellow
  "$G80CTL" --usb lighting read
  ```

  - Expected four solid `#ffff00` rows with TTL `none`.
  - **HANDS, visual:** keys 0, 3, 40, and 43 are yellow; unlisted keys retain
    their underlying lighting.
- [ ] Clear only the host overlay:

  ```sh
  "$G80CTL" --usb lighting clear
  "$G80CTL" --usb lighting read
  ```

  - Expected: `clear overlay: applied to both halves`, then `host overlay is
    empty`.
  - **HANDS, visual:** keys 0 and 40 immediately reveal layer-0 blue; keys 3
    and 43 immediately reveal base dim white. The rest of the base/layer
    frame must not blink, clear, or retain yellow.

## 11. Power-button LED on both halves — HANDS

- [ ] **HANDS:** inspect both rear power-button LEDs while both halves run.
  - Expected: both are continuously on at a dim, approximately 5% duty-cycle
    level.
- [ ] **HANDS:** power-cycle the right half, then the left half separately.
  - Expected on each boot: rear LED comes on immediately; around 120 ms later
    the key LED frame appears.
  - No rear LED should remain dark, flash at full power, or depend on the
    split link.

## 12. Battery reporting and low-battery behavior — PENDING

- **Pending features:** right-half battery observability in a qualification
  tool and low-battery status/safety override integrated with the lighting
  stack.
- The RMK config enables VDDH/5 ADC measurement on both halves and exposes a
  BLE Battery Service, but the control CLI has no battery verb. Current host
  output does not establish both half-specific readings.

- [ ] **HANDS/read-only, available central-only subtest:** unplug USB, remain
      connected over BLE, and inspect BlueZ:

  ```sh
  export G80_BLE_ADDR="$(bluetoothctl devices | awk 'tolower($0) ~ /glove80/ {print $2; exit}')"
  test -n "$G80_BLE_ADDR"
  bluetoothctl info "$G80_BLE_ADDR" | rg 'Connected|Battery Percentage'
  ```

  - Expected: `Connected: yes` and `Battery Percentage: 0xNN (NN)` with a
    plausible 0–100 value.
  - This is evidence for the central only and cannot pass the top-level item.

- [ ] **PENDING:** observe labeled left and right battery values through a
      supported diagnostic surface.
- [ ] **PENDING:** drive or safely simulate a low-battery event and prove that
      the status/safety record visibly overrides a conflicting host overlay
      on the affected half.
- [ ] **PENDING:** verify recovery from low battery/charging returns to the
      underlying composed stack without clearing it.
- Do not deliberately deep-discharge Li-ion cells for qualification.

## 13. Sustained fast typing during animation + flash writes — HANDS

- **Destructive:** repeated `config apply` writes RMK keymap/config flash.
- Safety: require the preflight backup, external power, both recovery image
  sets, and matching halves. Stop immediately on a keymap write error; earlier
  batches are not rolled back.

- [ ] Terminal A: fill all 80 overlay cells with the fastest practical
      breathe animation, then repeatedly apply the full config:

  ```sh
  "$G80CTL" --usb lighting set 0-79 white --effect breathe --period 256
  for n in $(seq 1 20); do
    "$G80CTL" --usb config apply tools/glove80-control/examples/glove80.toml \
      >"$G80_QUAL_DIR/stress-config-$n.log" 2>&1 || exit 1
  done
  ```

  - Lighting output must say `set 80 cell(s): applied to both halves`.
  - Every stress log must contain `commit OK` and no `LOSSY`, `BUSY`,
    `PARTIAL`, or `error:`.

- [ ] Terminal B, started while Terminal A is running:

  ```sh
  expected='the quick brown fox jumps over the lazy dog 0123456789'
  failures=0
  for n in $(seq 1 20); do
    IFS= read -r got
    if [ "$got" != "$expected" ]; then
      printf 'MISMATCH %s: <%s>\n' "$n" "$got"
      failures=$((failures + 1))
    fi
  done
  test "$failures" -eq 0
  ```

  - **HANDS:** type the exact expected line and press Enter 20 times at a
    sustained fast pace, deliberately using both halves.
  - Expected: no `MISMATCH` lines and final exit status 0.
  - **HANDS, observation:** animation remains smooth enough to show no long
    freezes; no disconnect, stuck modifier, missing/reordered/duplicated key,
    or visible corrupted persistent state occurs.

- [ ] Clean up and restore:

  ```sh
  "$G80CTL" --usb lighting clear
  "$G80CTL" --usb config apply "$G80_QUAL_DIR/config-before.toml"
  "$G80CTL" --usb config show
  ```

## 14. Recovery to a known-good image after every destructive test — HANDS

- This item proves the retained ZMK escape path and the return path to RMK.
- **Destructive:** firmware and settings-reset flashes replace application
  firmware and/or erase settings/bonds.
- Safety: never mount both bootloader volumes; ZMK right first, then ZMK
  left. Returning to RMK also uses right first, then left.

### Recover RMK to known-good ZMK

- [ ] While RMK still runs, enter and flash the peripheral:

  ```sh
  "$G80CTL" --usb bootloader --peripheral --yes
  timeout 30 sh -c 'until test -d /run/media/imalison/GLV80RHBOOT; do sleep 1; done'
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-right.uf2" /run/media/imalison/GLV80RHBOOT/
  sync
  timeout 30 sh -c 'while test -d /run/media/imalison/GLV80RHBOOT; do sleep 1; done'
  ```

- [ ] With the central still on RMK, enter and flash it:

  ```sh
  "$G80CTL" --usb bootloader --yes
  timeout 30 sh -c 'until test -d /run/media/imalison/GLV80LHBOOT; do sleep 1; done'
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-left.uf2" /run/media/imalison/GLV80LHBOOT/
  sync
  timeout 30 sh -c 'while test -d /run/media/imalison/GLV80LHBOOT; do sleep 1; done'
  ```

- [ ] **HANDS:** verify known-good ZMK USB typing, BLE typing, split typing,
      and physical Magic+bootloader access.
  - The RMK `version` command is expected to stop working after the central
    has been replaced by ZMK; that is not a recovery failure.

### Settings-reset recovery, only when settings/bonds are the failure

- [ ] **HANDS:** physically enter the right bootloader with reset-button
      double-tap or Magic-hold + its `QK_BOOT` key.
- [ ] Flash reset, then physically re-enter and flash normal ZMK:

  ```sh
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-right-settings-reset.uf2" /run/media/imalison/GLV80RHBOOT/
  sync
  # Physically re-enter GLV80RHBOOT after the reset image runs.
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-right.uf2" /run/media/imalison/GLV80RHBOOT/
  sync
  ```

- [ ] Repeat for the left half only after the right is out of bootloader:

  ```sh
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-left-settings-reset.uf2" /run/media/imalison/GLV80LHBOOT/
  sync
  # Physically re-enter GLV80LHBOOT after the reset image runs.
  cp "$G80_QUAL_DIR/zmk-firmware/glove80-left.uf2" /run/media/imalison/GLV80LHBOOT/
  sync
  ```

  - Settings reset erases bonds/settings. Expect to re-pair BLE.
  - Do not use settings-reset as an RMK factory-restore substitute.

### Return from ZMK to the RMK candidate

- [ ] **HANDS:** physically enter the right bootloader, then flash RMK:

  ```sh
  cp dist/glove80-rmk-0.1.0-rh.uf2 /run/media/imalison/GLV80RHBOOT/
  sync
  ```

- [ ] **HANDS:** after the right half leaves bootloader, physically enter the
      left bootloader, then flash RMK:

  ```sh
  cp dist/glove80-rmk-0.1.0-lh.uf2 /run/media/imalison/GLV80LHBOOT/
  sync
  ```

- [ ] Verify the candidate is restored:

  ```sh
  timeout 45 sh -c 'until ./target/debug/glove80-control --usb version >/dev/null 2>&1; do sleep 2; done'
  "$G80CTL" --usb version
  "$G80CTL" --usb config apply "$G80_QUAL_DIR/config-before.toml"
  ```

  - Expected: both halves connected, matching, and no mismatch warning.
  - **HANDS:** repeat a short USB and split typing smoke test.

## Results

Use `PASS`, `FAIL`, or `PENDING`; do not collapse a pending feature into a
pass from its available partial subtest.

| Item | Pass/fail | Notes | Date |
| --- | --- | --- | --- |
| 1. USB typing (left-local) |  |  |  |
| 2. BLE typing |  |  |  |
| 3. Full split typing; right-half disconnect/reconnect |  |  |  |
| 4. Config editing + persistence over USB and over BLE |  |  |  |
| 5. Eight uniform editable layers |  |  |  |
| 6. Reboot, factory restore, corrupt-record fallback, interrupted update |  |  |  |
| 7. Programmatic bootloader entry, both halves |  |  |  |
| 8. Static / blink / breathe on both halves |  |  |  |
| 9. Full lighting stack composed simultaneously |  |  |  |
| 10. Sparse host clear reveals the stack below |  |  |  |
| 11. Power-button LED on both halves |  |  |  |
| 12. Battery reporting and low-battery behavior |  |  |  |
| 13. Sustained fast typing during animation + flash writes |  |  |  |
| 14. Recovery to a known-good image after every destructive test |  |  |  |

## Coverage summary

- Qualification checklist items: **14**.
- Fully automatable items: **2** — items 5 and 7.
- Items requiring physical hands/visual observation: **9** — items 1, 2, 3,
  4, 8, 10, 11, 13, and 14.
- Pending-feature items: **3** — item 6 (factory restore and deterministic
  corruption/interruption fixtures), item 9 (conditional status/safety
  lighting end-to-end), and item 12 (both-half battery observability and
  low-battery safety override).
