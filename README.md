# Glove80 RMK Firmware and Control Stack

Custom Glove80 firmware for Ivan's layout, now running on RMK. The repository
contains the two-half firmware, the Rust control CLI and protocol codec, and
the browser-based Lightbench editor.

RMK is pinned as a submodule under `dependencies/rmk` to `glove80-rynk` at
`67f444b2` in `colonelpanic8/rmk`. This integrates the still-open upstream
Rynk PR with the Glove80 split-app, vendor-transport, shared-flash, and
VBUS-state extensions. The pre-Rynk rollback branch remains published as
`glove80` at `8089822e`.

The old MoErgo ZMK tree and its build inputs remain under `zmk/`, `config/`,
`host-lighting/`, and `maintenance/` as a recovery baseline. They are not the
active firmware or control path.

## Runtime configuration

- Keymap operations use RMK's native Rynk protocol: USB HID or native BLE GATT
  for the CLI, and Rynk's WebHID collection in Lightbench over USB or BLE.
- Lighting, persistent lighting config, version, and bootloader operations
  continue to use the Glove80 host protocol over USB vendor raw HID or its
  custom encrypted BLE GATT service.
- Lightbench and `glove80-control` can edit the live keymap, lighting records,
  toggles, and host overlay without reflashing.
- Rynk owns live keymap mutation and persistence. The CLI and Lightbench retain
  the existing QMK/VIA-style text editor through an explicit compatibility
  conversion at their boundary.
- Persistent lighting configuration uses transactional A/B records in the
  reserved runtime-config partition.
- Configuration is intentionally unlocked; no physical unlock chord is
  required.

## Host lighting

The versioned host protocol can set individual key LEDs over USB or Bluetooth.
Live overlays remain in RAM, compose over firmware-managed lighting, and clear
explicitly, on TTL expiry, or at reboot.

See [`docs/host-lighting-protocol.md`](./docs/host-lighting-protocol.md) for the
wire contract and current limitations. Static lighting has been exercised on
both halves over USB, including simultaneous blink and breathe effects.

## Manual lighting editor

[`ui/`](./ui/) contains **Glove80 Lightbench**, a standalone per-key lighting
and keymap editor. It connects directly to the firmware (Glove80 protocol for
lighting, Rynk for keymaps) and does not depend on a daemon.

```sh
cd ui
npm ci
npm run dev
```

Open the printed localhost URL in Chrome or Edge, connect the keyboard, select a
color, and click or drag across keys. See [`ui/README.md`](./ui/README.md) for
browser support, architecture, and connection details.

For terminal control, use the Rust CLI (no daemon required). It uses Rynk for
keymaps and the Glove80 host protocol for the remaining commands — see
[`tools/glove80-control/README.md`](./tools/glove80-control/README.md):

```sh
cargo run --quiet -- lighting caps
cargo run --quiet -- lighting set 0-5,12 ff0066
cargo run --quiet -- lighting clear
cargo run --quiet -- keymap read --layer 0
cargo run --quiet -- config validate path/to/config.json --layer-capacity 8
cargo run --quiet -- version
```

Run `cargo install --path tools/glove80-control` if you prefer a normal
`glove80-control` executable on your `PATH`.

With RMK firmware installed, either half can be put into its UF2 bootloader
without using a key chord:

```sh
cargo run --quiet -- bootloader --peripheral
cargo run --quiet -- bootloader
```

Request the peripheral bootloader before the central, since the central
provides the split and host-protocol transports used to reach the peripheral.
(The legacy ZMK Studio serial commands were retired after the RMK cutover;
the CLI no longer talks to the ZMK recovery firmware.)

The left Magic/MoErgo key is reserved as a firmware status pixel: cyan means a
host lighting frame is active, green means USB HID is ready, blue means the
active Bluetooth profile is connected, amber means the selected transport is
not ready, and dim white means the firmware is running without a more specific
connection state.

Right-half host lighting exposes LED indices 40 through 79. Static colors use
four-pixel split batches; animated effects use two-effect batches with 50 ms
timing resolution. Both fit a default BLE ATT payload and also work over the
wired split transport. A partial-result response indicates that the peripheral
half was unavailable for at least one batch.

## Build the RMK firmware

```sh
git submodule update --init
cd firmware/glove80-rmk
nix develop --command ./build.sh
```

The build produces the two half-specific images:

```sh
firmware/glove80-rmk/glove80_lh_rmk.uf2
firmware/glove80-rmk/glove80_rh_rmk.uf2
```

Flash the RH image first, then the LH image. The physical Magic-layer
bootloader keys now route to their respective halves; the CLI routes the same
requests programmatically.

## Legacy ZMK recovery build

The prior ZMK recovery baseline can still be built with:

```sh
nix run .#generate-keymap
nix build .#firmware
```

It produces the historical normal, settings-reset, and combined archival UF2
artifacts under `result/`. These images are recovery tools, not the active RMK
release path.

## Updating the legacy MoErgo baseline

To merge a newer MoErgo ZMK revision into the vendored subtree:

```sh
git subtree pull --prefix=zmk https://github.com/moergo-sc/zmk.git main --squash
```

Resolve any conflicts in the locally customized firmware source, then run the
full firmware build before committing the merge. The initial subtree import is
MoErgo ZMK revision `2f73a230e2fc7b2bd64a9736181e87bf54338131`.

To update the keyboard layout itself:

1. Export or fetch the MoErgo layout JSON.
2. Replace `config/moergo-layout.json`.
3. Run `nix run .#generate-keymap`.
4. Commit the regenerated `config/glove80.keymap`.

## Direction

See [`ROADMAP.md`](./ROADMAP.md) for the planned optional host integration,
including live Codex status lighting and keyboard-driven Codex actions.

`scripts/generate-keymap.mjs` is only a build-time converter for the existing
MoErgo JSON export. The manual editor uses TypeScript because it runs in a web
browser, but neither ZMK Studio nor the live host protocol requires JavaScript;
the generator and any future daemon can be replaced independently.
