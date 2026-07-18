# Glove80 Host Integration Roadmap

The keyboard must always remain a complete standalone keyboard. Host software
may enhance lighting and configuration, but typing, the stock keymap, and saved
Studio configuration must never depend on a daemon being present.

## Foundation: ZMK Studio migration

- [x] Build against the maintained MoErgo Glove80 ZMK distribution.
- [x] Support ZMK Studio over USB serial and Bluetooth GATT.
- [x] Preserve the generated keymap as the recoverable stock configuration.
- [x] Reserve extra layers for runtime editing.
- [x] Require the physical Studio unlock binding before persistent changes.

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

## Later: configuration tooling

Use standard ZMK Studio RPC for supported keymap changes. Extend configuration
only where Studio cannot express the desired behavior, and keep custom RPCs
versioned and capability-negotiated.

Potential additions:

- Import and export the runtime keymap in a source-controlled format.
- Reconcile saved Studio settings with the generated stock keymap.
- Expose precompiled macro and behavior parameters safely.
- Add transactional configuration updates with validation and rollback.
- Provide a physical recovery gesture that restores stock settings.
