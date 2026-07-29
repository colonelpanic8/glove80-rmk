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

- `config validate|diff|apply|pull|show`
- `keymap read|set|default|monitor|find`
- `lighting ping|caps|set|unset|clear|read|replace|brightness`
- `lighting scene-read|scene-set|scene-unset|scene-policy|params`
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

Overlay cells are transient. Per-layer scene cells are stored by the keyboard
and survive a reboot. For example, this makes LED 29 blue whenever layer 1 is
active and composes it with the other active layers:

```bash
cargo run -p glove80-control -- lighting scene-set 1 29 blue
cargo run -p glove80-control -- lighting scene-policy active-stack
cargo run -p glove80-control -- lighting scene-read
```

Both bootloader commands may require the keyboard's configured physical
presence chord. The CLI displays the requested keys and waits for the unlock.

The `config` commands provide a bidirectional TOML snapshot of managed runtime
state. `config diff FILE` compares the file with a live keyboard;
`config apply FILE` writes only differences and verifies readback; `config
pull FILE` writes the keyboard state to disk; and `config show` prints it.
Lighting extensions remain generic: effect and palette names come from Rynk's
extension descriptor, regardless of the firmware-side effect provider. Effects
may also advertise tunable parameters, which the file addresses by name:

```toml
[lighting.effects.params.Rain]
Density = 6
"Trail Length" = 128
```

A file owns only the parameters it lists; the rest keep whatever value the
keyboard holds. `config pull` records only parameters that differ from their
firmware default. Firmware that predates the parameter commands simply
advertises none, so reads degrade quietly and only a file that names a
parameter fails.

`lighting params` lists every effect that advertises parameters, `lighting
params EFFECT` lists one effect's, and `lighting params EFFECT NAME VALUE`
writes one.
