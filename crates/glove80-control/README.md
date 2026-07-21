# glove80-control

Native Glove80 control CLI using RMK's Rynk protocol over USB HID, USB serial,
or BLE. It controls current firmware only; the retired Glove80 product protocol
is intentionally not supported.

Run from the repository's Nix development shell:

```bash
cargo run -p glove80-control -- --help
cargo run -p glove80-control -- --usb version
```

The top-level commands are:

- `keymap read|set|default|find`
- `lighting ping|caps|set|unset|clear|read|replace|brightness`
- `version`
- `bootloader [--peripheral] [--yes]`

Device selection defaults to USB with BLE fallback. Use `--usb` or `--ble` to
require one transport, and `--device` to select a `/dev/hidraw*`,
`/dev/ttyACM*`, or BLE address when multiple keyboards are available.

`keymap set` accepts `LAYER KEY KEYCODE` triples. A key may be a flat index or
`row,col`; keycodes use familiar names such as `KC_A`, `MO(2)`, and
`LT(1,KC_ESC)`.

Lighting commands operate on RMK's topology-aware overlay and revisioned
state. `lighting replace` accepts one cell per line:

```text
12 ff0000
40 00ff00 blink period=750 duty=30
```

Both bootloader commands may require the keyboard's configured physical
presence chord. The CLI displays the requested keys and waits for the unlock.
