# Glove80 ZMK Studio Config

Custom Glove80 firmware for Ivan's layout, generated from the MoErgo layout
`34957465-9943-4236-852c-d88044706dcb`.

This is a monorepo: the MoErgo Glove80 ZMK source is vendored as a Git subtree
under [`zmk/`](./zmk/), and the custom firmware code is maintained there as
ordinary source changes. There is no source-patching layer or separate firmware
fork to coordinate.

The keyboard remains fully functional with the generated keymap when Studio is
not connected, while Studio can edit and persist bindings at runtime over USB
or Bluetooth. The host-lighting protocol is protobuf over ZMK Studio RPC; it
does not depend on the JavaScript keymap generator.

## ZMK Studio

- Hold the `Magic` key and press the far-left key in the bottom row to unlock
  Studio configuration.
- USB Studio communication uses the CDC/ACM serial transport on the left half.
- Bluetooth Studio communication uses ZMK's GATT transport.
- Four empty layers are reserved so Studio can add layers without reflashing.

Open [ZMK Studio](https://zmk.studio/) after connecting and unlocking the
keyboard. If both USB and Bluetooth are connected, select the same keyboard
output transport that Studio is using.

Studio changes are stored on the keyboard. Later changes to the generated
`glove80.keymap` become the new stock configuration, but do not replace saved
Studio settings until **Restore Stock Settings** is used in Studio.

## Experimental host lighting

The firmware now contains the first roadmap implementation: a versioned,
ephemeral RPC for setting individual key LEDs. It works through Studio's USB or
Bluetooth transport, never writes live frames to flash, and restores ordinary
firmware lighting when the host clears the override or its timeout expires.

See [`docs/host-lighting-protocol.md`](./docs/host-lighting-protocol.md) for the
wire contract and current limitations. The firmware builds successfully, but
this extension still needs testing on a physical keyboard.

## Manual lighting editor

[`ui/`](./ui/) contains **Glove80 Lightbench**, a standalone per-key lighting
editor. It connects directly through the standard ZMK Studio USB or BLE
transport and does not depend on a daemon or any Codex integration.

```sh
cd ui
npm ci
npm run dev
```

Open the printed localhost URL in Chrome or Edge, connect the keyboard, select a
color, and click or drag across keys. See [`ui/README.md`](./ui/README.md) for
browser support, architecture, and connection details.

For terminal control over USB, use the standalone Python CLI (no daemon or
third-party packages required):

```sh
python3 scripts/glove80-control.py capabilities
python3 scripts/glove80-control.py all ff0066
python3 scripts/glove80-control.py set 0=ff0000 1=00ff00 40=0000ff
python3 scripts/glove80-control.py clear
```

The login running the command must have read/write access to `/dev/ttyACM0`
(normally through the `dialout` group).

After this custom firmware has been installed once, either half can be put into
its UF2 bootloader without using a key chord:

```sh
python3 scripts/glove80-control.py bootloader right
python3 scripts/glove80-control.py bootloader left
```

The command is USB-only and requires local permission to open the Studio serial
device, but deliberately does not require a physical Studio unlock. Request the
right bootloader before the left, since the left provides the split and USB RPC
transports used to reach the right.

The left Magic/MoErgo key is reserved as a firmware status pixel: cyan means a
host lighting frame is active, green means USB HID is ready, blue means the
active Bluetooth profile is connected, amber means the selected transport is
not ready, and dim white means the firmware is running without a more specific
connection state.

Right-half host lighting uses a dedicated split packet with four pixels per BLE
write and exposes LED indices 40 through 79. A partial-result response indicates
that the peripheral half was unavailable for at least one batch.

## Build

```sh
nix run .#generate-keymap
nix build .#firmware
```

The build produces half-specific images plus a combined archival artifact:

```sh
result/glove80-left.uf2
result/glove80-right.uf2
result/glove80-left-settings-reset.uf2
result/glove80-right-settings-reset.uf2
result/glove80.uf2
```

Flash `glove80-left.uf2` to the left bootloader and `glove80-right.uf2` to the
right bootloader. Do not use the combined artifact for routine flashing.

The settings-reset images are recovery tools. Flash the matching reset image,
allow it to boot once and erase persistent state, then return that half to its
bootloader and flash the matching normal image. Reset both halves together when
repairing their split bond.

## Updating From MoErgo

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
