# moergo-control

Native Glove80 and Go60 control CLI using RMK's Rynk protocol over USB HID or BLE. It
controls current firmware only; the retired Glove80 product protocol
is intentionally not supported.

Run from the repository's Nix development shell:

```bash
cargo run -p moergo-control -- --help
cargo run -p moergo-control -- --usb version
```

The top-level commands are:

- `battery [--json]` (standard BLE GATT Battery Services for every half)
- `config validate|diff|apply|pull|show`
- `connection status|switch|clear|name`
- `device-data` (namespaced static metadata and live state as JSON)
- `keymap read|set|default|name|monitor|find`
- `lighting ping|caps|set|unset|clear|read|frame|replica-status|replace|brightness`
- `lighting scene-read|scene-set|scene-unset|scene-policy|params`
- `version`
- `bootloader [--peripheral] [--yes]`
- `maintenance`

Device selection prefers USB, falling back to BLE only when no accessible
Rynk USB device is found. Ambiguous USB selection is an error. Use `--usb` or
`--ble` to require one transport, and `--device` to select a `/dev/hidraw*` path
or full BLE address when multiple keyboards are available. An explicit selector
must match a device; a missing path never selects another keyboard.

`battery` reads the central and split-peripheral levels over Rynk when USB is
available. With `--ble`, it reads each standard Battery Service instance and
uses its GATT descriptors to distinguish the halves; the keyboard must already
be connected over BLE. `--device` selects one when several keyboards are
connected, and `battery --json` emits a machine-readable report.

`keymap set` accepts `LAYER KEY KEYCODE` triples. A key may be a flat index or
`row,col`; keycodes use familiar names such as `KC_A`, `MO(2)`, and
`LT(1,KC_ESC)`. Matrix dimensions come from the connected keyboard. All
entries are checked for faithful Rynk conversion before any are written;
read-back mismatches return an error.

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
cargo run -p moergo-control -- lighting scene-set 1 29 blue
cargo run -p moergo-control -- lighting scene-policy active-stack
cargo run -p moergo-control -- lighting scene-read
```

The maintenance lock covers all remote mutations, matrix monitoring, storage
reset, and bootloader entry. Hold Magic and tap R to toggle it: R glows green
while the lock is off and unattended automation is allowed, and red while the
lock is engaged. Use `moergo-control maintenance` to read both the live and
compiled-default state.

The `config` commands provide a bidirectional TOML snapshot of managed runtime
state. `config diff FILE` compares the file with a live keyboard;
`config apply FILE` writes only differences and verifies readback; `config
pull FILE` writes the keyboard state to disk; and `config show` prints it.
Set `bluetooth_name = "Glove80 {slot}"` at the top level to give each active
BLE profile a distinct one-based advertising name. The same persistent value
can be inspected or changed directly with `connection name get` and
`connection name set TEMPLATE`; names are limited to 16 UTF-8 bytes.
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

Conditional lighting rules — the runtime counterpart of the ones a board
compiles in — are managed the same way. A rule applies when every condition it
names holds; naming none makes it unconditional:

```toml
[[lighting.conditional_scene]]
led = 75
color = "#0040a0"
layer = { layer = 2, active = true }
battery = { node = 1, min_level = 81, charge = "charging" }
```

Use `key = N` instead of `led = N` in either scene table to target the logical
key at index `N` in the keyboard's canonical layout. `config diff` and `config
apply` resolve it through the topology advertised by the connected keyboard,
including every emitter associated with that key. Raw LED targets remain
available for underglow, indicators, and other emitters that are not keys.

The same selectors address keys themselves. A `keys` grid is the fastest way to
read a whole layer and the worst way to say "every key in this zone", and a
standalone scene table puts a key's color hundreds of lines from its action.
`[[layer.key]]` entries answer both: one selector, lowered to matrix keys for
the action and to emitters for the lighting.

```toml
[[layer]]
id = "magic"
name = "Magic"
keys = """…"""

# A black floor over every per-key emitter.
[[layer.key]]
zone = 1
color = "#000000"

# What this key does and how it looks, in one place. The inline color is the
# key's unconditional look.
[[layer.key]]
key = [3, 0]
action = "QK_BOOT"
color = "#ff0000"
effect = "blink"

# A key with several looks: rule arms read top down, first match wins, and the
# inline color is the final arm. The enclosing layer supplies the "this layer
# is active" condition on every arm.
[[layer.key]]
key = [0, 6]
action = "QK_OUTPUT_USB"
color = "#ff0000"

[[layer.key.rule]]
when = { connection = { transport = "usb" } }
color = "#00ff00"

[[layer.key.rule]]
when = { connection = { usb_connected = true } }
color = "#0000ff"
```

Within one arm every named condition must hold together; across arms the first
match shows. An arm naming no conditions is the fallback and must come last —
anything after it could never show and is rejected. The fallback lowers to a
durable scene cell and conditional arms to conditional rules, which is exactly
the compositor's own priority order.

Two conditions look past the key's own layer. `layers` gates on a set of
layers at once — every layer in `active` held and none in `inactive` — which
is how a status layer shows which *other* layers are on while it is held.
`indicators` gates on the host's caps, num, and scroll lock:

```toml
[[layer.key]]
key = [1, 3]
color = "#000000"

[[layer.key.rule]]
when = { layers = { active = [3] } }
color = "#ff00ff"

[[layer.key]]
key = [0, 2]
color = "#000000"

[[layer.key.rule]]
when = { indicators = { caps_lock = true } }
color = "#ff0000"
```

Standalone `[[lighting.conditional_scene]]` entries accept the same two
fields.

An entry with only an `action` is a binding; one with only lighting is a scene
written next to the layer it belongs to, which is also what saves repeating
`layer = N` on every cell. Emitters that belong to no key — underglow,
indicators — take the same shape under `[[layer.light]]`, which accepts every
selector but no `action`:

```toml
[[layer.light]]
led = 87
color = "#202020"

[[layer.light.rule]]
when = { battery = { node = 0, max_level = 20 } }
color = "#ff0000"
```

Entries apply in file order and the last one covering a key wins it, for the
action and for the lighting alike. That ordering is what lets a broad selector
be written first and corrected afterwards — `[[lighting.scene]]` rejects two
cells for one slot, and a layer-attached cell overrides instead, including
over a standalone cell for the same slot. `led = N` in `[[layer.key]]` names
the key that owns that emitter, and `all = true` names every key the topology
advertises.

Only `key = [row, col]` is a question about the matrix, and only for a binding;
everything else is a question about the board. `config validate` checks that
those are well formed and names them as resolving against a connected keyboard,
while `config diff` and `config apply` resolve them for real. A file that
lights a key needs a `[lighting]` section, since synthesizing one would claim
the brightness, background and policy values in it.

The keyboard stores a grid and resolved cells rather than the selectors a file
used to describe them, so `config pull` writes layers back as `keys` and
lighting back as `led` cells.

Unlike `[[lighting.scene]]`, this table is ordered: matching rules compose in
table order and later ones win the slots they share, so `config diff` reports
by position and reordering two rules is a real difference. The table is written
as one atomic replacement, and a runtime rule outranks a compiled one on the
same LED, which is what lets a host replace a board's built-in status lighting
rather than only add to it. Firmware without the runtime conditional commands
reports no table at all, so a file that names no rules still applies cleanly
and only one that names them fails.
