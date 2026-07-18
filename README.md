# Glove80 ZMK Studio Config

Custom Glove80 firmware for Ivan's layout, generated from the MoErgo layout
`34957465-9943-4236-852c-d88044706dcb`.

This uses the current MoErgo Glove80 ZMK distribution with ZMK Studio enabled.
The keyboard remains fully functional with the generated keymap when Studio is
not connected, while Studio can edit and persist bindings at runtime over USB
or Bluetooth.

The repository carries small compatibility patches for the MoErgo build: one
adds Studio's `nanopb` and protocol-message dependencies inside Nix, and one
adds the firmware hook used by the experimental host-lighting extension. The
host protocol is protobuf over ZMK Studio RPC; it does not depend on the
JavaScript keymap generator.

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
python3 scripts/glove80-lighting.py capabilities
python3 scripts/glove80-lighting.py all ff0066
python3 scripts/glove80-lighting.py set 0=ff0000 1=00ff00 40=0000ff
python3 scripts/glove80-lighting.py clear
```

The login running the command must have read/write access to `/dev/ttyACM0`
(normally through the `dialout` group).

The left Magic/MoErgo key is reserved as a firmware status pixel: cyan means a
host lighting frame is active, green means USB HID is ready, blue means the
active Bluetooth profile is connected, amber means the selected transport is
not ready, and dim white means the firmware is running without a more specific
connection state.

## Build

```sh
nix run .#generate-keymap
nix build .#firmware
```

The combined firmware is written to:

```sh
result/glove80.uf2
```

Flash that same `.uf2` to both halves.

## Updating From MoErgo

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
