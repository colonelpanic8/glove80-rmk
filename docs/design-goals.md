# Glove80 system design goals

The keyboard is a complete, reliable input device on its own. Desktop
software (Lightbench, the Rust CLI, a background service) only *enhances*
it — typing, layers, saved configuration, lighting, recovery, and firmware
updates never depend on a host being present.

Lighting has its own contract: [`lighting-design.md`](./lighting-design.md).
The RMK implementation and its hardware verification live in
[`../firmware/glove80-rmk/`](../firmware/glove80-rmk/README.md).

## Non-negotiables

- Types correctly with no desktop software installed.
- USB and Bluetooth expose the same keymap and configuration semantics.
- Losing the split link disables the missing half only — the central keeps
  typing and stays configurable.
- Key scanning and HID reporting outrank everything else (rendering,
  storage, config transfer, logging).
- A malformed config or interrupted write can never strand the keyboard.
- Both halves always keep a physical *and* programmatic route into their
  bootloaders.
- The firmware enforces the LED current ceiling no matter what a host asks.
- Routine keymap and lighting changes never require recompiling or
  reflashing.
- No physical unlock chord for configuration.
- A known-good recovery image and factory configuration exist at every point
  during development and migration.

## Keymap model

- One compile-time layer capacity (currently 8). Every slot is the same
  mutable runtime record — no factory/static/dynamic layer castes.
- A layer: stable ID, display name, 80 bindings.
- Stable IDs live in the canonical config; firmware sees plain slot numbers.
  Renaming/reordering never breaks references.
- Factory defaults are versioned *data*, copied into the same runtime
  representation on first boot or restore — then editable like anything
  else.
- Editing supports: inspect capacity, add/remove/rename/reorder layers,
  read/replace bindings, export everything, restore factory.
- A complete-config apply is transactional: old config or new config, never
  a hybrid.
- Boot recovery order: newest valid config → factory snapshot → minimal
  recovery keymap (USB + config + reset + bootloader).

## Host interfaces

- One versioned canonical schema shared by every tool. No private formats,
  no persisted implementation quirks. Tools query capacity/capabilities
  instead of assuming them.
- **Lightbench** (browser): daemon-independent; connects directly to the
  keyboard over USB or BLE.
- **Rust CLI**: lighting, validation, export, transactional apply, restore,
  capability inspection, bootloader entry. Python is not in the control
  path.
- **Background service** (optional): translates app state (e.g. Codex
  status) into host-overlay lighting. Its absence affects only that
  overlay.
- Keyboard-driven host actions use dedicated, deliberately-bound keycodes.
  Ordinary typing is never interpreted as a command.

## Update and recovery

- Either half enters its UF2 bootloader physically; compatible firmware can
  also request it programmatically (peripheral first, then central).
- Reset/recovery images erase runtime config while leaving an obvious route
  back to normal firmware.
- Firmware images are half-specific.
- During a firmware-substrate migration, the known-good previous images
  remain flashable until the replacement passes the full checklist below.

## Performance and power

- Rendering and storage run on low-priority async tasks.
- Static state costs nothing per tick; only animated cells recompute.
- Flash writes and validation yield between bounded steps.
- Max animation + config writes must not measurably affect typing.
- Idle powers down what it can; battery reporting works on both halves;
  low-battery protection overrides host lighting.

## Qualification checklist

A firmware implementation is ready to replace the incumbent when all of
these pass on hardware:

- [ ] USB typing (left-local)
- [ ] BLE typing
- [ ] Full split typing; right-half disconnect/reconnect
- [ ] Config editing + persistence over USB and over BLE
- [ ] Eight uniform editable layers
- [ ] Reboot, factory restore, corrupt-record fallback, interrupted update
- [ ] Programmatic bootloader entry, both halves
- [ ] Static / blink / breathe on both halves
- [ ] Full lighting stack composed simultaneously (base + layer + toggle +
      host + status)
- [ ] Sparse host clear reveals the stack below
- [ ] Power-button LED on both halves
- [ ] Battery reporting and low-battery behavior
- [ ] Sustained fast typing during animation + flash writes
- [ ] Recovery to a known-good image after every destructive test
