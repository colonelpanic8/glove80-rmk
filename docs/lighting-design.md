# Lighting design

The contract for the Glove80 lighting system. General system goals live in
[`design-goals.md`](./design-goals.md).

## Core model

- One lighting engine, no "modes" — nothing like ZMK's solid/spectrum/swirl
  that can leave lighting stuck in the wrong mode.
- Every lighting definition is a sparse map: **key → cell**.
- A cell is either **transparent** or **{ color, effect, params }**.
- Effects at launch:
  - **static**
  - **blink** — period, phase, duty cycle
  - **breathe** — period, phase
- A blinking cell's dark phase renders black — it does not become
  see-through.
- New effects must be addable without changing the meaning of existing
  records.

## Layering (composited bottom → top)

1. **Base** — always on.
2. **Layer lighting** — active while its keymap layer is active.
3. **Toggle overlays** — switched on/off by name, independent of layers.
   Toggle state is non-persistent unless the toggle opts in.
4. **Host overlay** — live, RAM-only; what Lightbench / CLI / a background
   service write.
5. **Status & safety** — always on top; low battery beats everything.

- A defined cell replaces what's below; transparent reveals it.
- All five are the same record type — they differ only by activation
  predicate (always / layer / toggle / host session / firmware state).
- Within a class, priority plus stable order decide composition.

## Conditions and gates

- Condition kinds: layer-active(n), toggle(id), plus firmware-state
  conditions: usb-connected (central only — the right half's port is
  charge-only), charging (per half), split-link-up.
- Every record may carry one optional **gate**: a second condition that
  must also hold for the record to activate. Carried in the record
  header's reserved bytes, so old configs decode unchanged (no gate).
- This one primitive covers status displays: layer-indicator records
  (one per layer, painting the live layer bitmap) shown permanently when
  ungated, or press-and-hold when gated on the Magic layer — the stock
  Glove80 "Magic shows status" behavior, rebuilt from general parts.
- Deliberately NOT in firmware: scripted/arbitrary-logic lighting. The
  host overlay is the escape hatch for unbounded logic — a host process
  computes anything and paints the result. Firmware conditions stay
  simple and verifiable.

## Host overlay

- Sparse and RAM-only; never persisted.
- Operations: set cells (any effect), unset cells, clear all, query
  capabilities/key count, **read back the full overlay**, **atomically
  replace the full overlay** (the force-sync primitive — one idempotent
  operation puts the keyboard in a provably known state).
- Optional **TTL per write**, enforced by the firmware: on expiry the cell
  reverts to transparent. Default is no TTL. Firmware-side expiry means an
  indicator written by a crashed client can't outlive its writer when the
  writer opted in.
- Cells without TTL survive until explicit clear or reboot. No implicit
  timeouts — a daemon crash never changes how the keyboard looks.
- Partial application (peripheral offline) is reported, not hidden.

## Behavior guarantees

- Changing lighting never requires recompiling or reflashing.
- Lighting work never delays keystrokes.
- Static-only state = no animation timer at all; only animated cells
  recompute on a tick.
- Brightness is a runtime setting scaling everything below the ceiling.
- The safety ceiling is a compile-time constant (default 80%, per MoErgo's
  current/warranty limit). Hosts may lower the effective ceiling at runtime
  but can never raise it above the compiled value.
- Future refinement: replace the per-channel clamp with a frame-level
  current budget (estimate total draw, scale the frame proportionally) so
  sparse bright cells are allowed while whole-board floods stay limited.

## Split

- Each half renders its own 40 LEDs; the central owns configuration and the
  host protocol.
- Persistent lighting renders on the peripheral from compact synced
  config/state; live host updates use bounded batches.
- Central↔peripheral messages are bounded, versioned, and tolerant of
  retransmission and reconnect. No lighting transfer may block key events.
- The power-button LED is an independent firmware-status output on both
  halves.

## Open questions

- **Cross-half animation phase**: must two blinking keys on opposite halves
  blink in unison (needs a shared clock epoch over the split link), or is
  per-half phase acceptable for v1?
- **Layer lighting scope**: are layer definitions typically whole-board
  scenes or sparse accents on the keys the layer binds? Both fit the model;
  this shapes default configs and what Lightbench's editor emphasizes
  first.
