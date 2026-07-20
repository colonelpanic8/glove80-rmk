# Glove80 Host Integration Roadmap (historical)

This file records the path from the former ZMK Studio firmware to the RMK
stack. The RMK firmware, host protocol, CLI, Lightbench, transactional lighting
configuration, keymap editing, and split-aware control described here are now
implemented. Current qualification and remaining hardware checks live in
`docs/qualification.md`; this checklist is retained as design provenance.

The detailed architecture and phased implementation plan now lives in
[`docs/runtime-configuration-plan.md`](./docs/runtime-configuration-plan.md).
The firmware-independent requirements are captured in
[`docs/design-goals.md`](./docs/design-goals.md) and [`docs/lighting-design.md`](./docs/lighting-design.md), with a side-by-side RMK
assessment and hardware spike plan in
[`docs/rmk-evaluation.md`](./docs/rmk-evaluation.md).

The active replacement firmware lives in
[`firmware/glove80-rmk/`](./firmware/glove80-rmk/README.md) and is consumed against the pinned RMK
fork submodule.

The keyboard must always remain a complete standalone keyboard. Host software
may enhance lighting and configuration, but typing, the stock keymap, and saved
Studio configuration must never depend on a daemon being present.

## Foundation: ZMK Studio migration

- [x] Build against the maintained MoErgo Glove80 ZMK distribution.
- [x] Support ZMK Studio over USB serial and Bluetooth GATT.
- [x] Preserve the generated keymap as the recoverable stock configuration.
- [x] Replace user-visible reserved-layer semantics with one total runtime layer capacity.
- [x] Disable Studio locking and remove the physical unlock binding.

## Next: host-controlled lighting

Add a small ZMK Studio RPC subsystem for ephemeral lighting commands. The
initial protocol should support setting a bounded collection of key colors,
clearing host colors, and reporting protocol capabilities.

Firmware requirements:

- [x] Add a versioned, capability-negotiated Studio RPC extension.
- [x] Bound each request to eight pixel updates.
- [x] Keep rendering on ZMK's low-priority work queue.
- [x] Do not persist live lighting frames to flash.
- [x] Restore firmware-managed lighting after clear or timeout.
- [x] Clamp RGB channels and advertise a conservative maximum update rate.
- [x] Propagate four-pixel batches through a dedicated BLE/wired split command.
- [x] Add a daemon-independent manual editor with USB and BLE transports.
- [x] Map all logical keys to their hardware LED-chain indices.
- [x] Render per-key static, blink, and breathe effects locally in firmware.
- [x] Add effect controls and previews to the daemon-independent editor.
- [ ] Coalesce incoming frames and enforce the update-rate limit in firmware.
- [ ] Verify USB, Bluetooth, timeout, split, and low-battery behavior on hardware.
- [ ] Add a native/background transport service for shared device ownership.

## Next: Codex bridge daemon

Build a user service that connects Codex app-server events to the keyboard RPC
transport. It should discover USB and BLE, prefer USB when both are available,
and keep the same logical session across transport changes.

Initial state mapping:

- idle
- unread completion
- working/thinking
- waiting for approval or user input
- completed
- error

The daemon must be optional. Disconnecting it should only remove live host
lighting and must not alter typing or saved key bindings.

## Next: keyboard-driven Codex actions

Reserve bindings that emit otherwise-unused keycodes and let the daemon map
them to explicit Codex operations such as selecting a thread, starting a new
thread, interrupting work, approving or rejecting a request, and changing
reasoning effort.

Persistent or consequential actions should remain protected by explicit user
intent; ordinary typing must never be interpreted as a Codex command.

## Runtime configuration and composited lighting

- [x] Define and validate the versioned symbolic runtime schema in Rust.
- [ ] Represent factory keymap defaults as a versioned data snapshot.
- [ ] Load factory and edited keymaps into one runtime layer representation.
- [ ] Add transactional import, export, validation, and recovery.
- [ ] Replace legacy underglow modes with one sparse lighting compositor.
- [ ] Associate lighting with active keymap layers.
- [ ] Add independently toggleable and stackable lighting layers.
- [ ] Treat live host lighting as a sparse highest-priority overlay.
- [ ] Extend the Rust CLI and Lightbench around one canonical schema.

## Configuration tooling

Use standard ZMK Studio RPC for supported keymap changes. Extend configuration
only where Studio cannot express the desired behavior, and keep custom RPCs
versioned and capability-negotiated.

Potential additions:

- Import and export the runtime keymap in a source-controlled format.
- Reconcile saved Studio settings with the generated stock keymap.
- Expose precompiled macro and behavior parameters safely.
- Add transactional configuration updates with validation and rollback.
- Provide a physical recovery gesture that restores stock settings.
