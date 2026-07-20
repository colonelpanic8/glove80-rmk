# Implementation plan

> Historical note: this plan spans work completed before repository extraction.
> References to `host-lighting`, `protocol/proto`, `config`, or the ZMK recovery
> build describe the separate legacy `glove80-config` repository and are not
> paths or build inputs in this repository.

How we get from the working RMK spike to the full system in
[`design-goals.md`](./design-goals.md) and
[`lighting-design.md`](./lighting-design.md). Phases are ordered by
dependency; each has a crisp exit. Keep the ZMK images flashable until the
final phase.

## Already done (spike, 2026-07-18)

- RMK port of both halves: boots via stock bootloader, USB + BLE typing,
  wireless split, Vial editing (USB), battery. `firmware/glove80-rmk/`
- Minimum lighting engine: WS2812 driver with hard 80% clamp, power-button
  PWM, event-driven frame task, layer color on one key.
- Vial-over-BLE ruled out (BlueZ bug, documented) — wireless config goes
  through our own protocol.

## Working decisions

- **Cross-half animation phase**: per-half for v1. Revisit shared-epoch sync
  only if visible drift annoys in practice.
- **Layer lighting scope**: the sparse model supports both accents and
  full scenes; no schema decision needed. Lightbench starts with per-key
  editing either way.
- **RMK dependency**: the original git subtree has been replaced by the
  `dependencies/rmk` submodule, pinned to the fork's
  `glove80-rmk/integration` branch. Generic extensions remain split into
  independently reviewable fork branches; `docs/upstream/PATCHES.md` records
  their provenance and current disposition.
- **Patch style rule**: any new fork-side RMK patch is written as a generic
  RMK extension point (a hook, channel, or registration mechanism any
  keyboard could use), never as Glove80-specific logic inside RMK. Glove80
  specifics live in our crates on top. This keeps every patch a candidate
  upstream PR from the day it is written (see Phase 8).
- **Old ZMK-era host code**: replaced by the new protocol; retired
  incrementally. The CLI's ZMK Studio serial path (legacy
  `capabilities`/`all`/`set`/`effect`/`clear` verbs and the
  `bootloader left|right` serial routing) has been removed. `protocol/proto`
  and `host-lighting/` cannot be deleted yet even though the live system no
  longer uses them: the ZMK recovery build (`config/default.nix`) consumes
  both (`studioMessagesOverlay` and `extraModules`). They go when the ZMK
  recovery baseline is retired after Phase 7 qualification. The
  protobuf/Studio parts of `ui/` are retired separately in the app.

## Phase 1 — Compositor core (firmware)

The heart of the lighting contract, built as pure logic first.

- `firmware/glove80-rmk/src/compositor/`: cell type (transparent | color+effect),
  record + activation predicate (always / layer / toggle / host / status),
  priority-ordered composition into a `Frame`.
- Effects: static, blink (period/phase/duty), breathe (period/phase).
  Ticker exists only while animated cells are visible; recompute animated
  cells only.
- Global runtime brightness scalar (driver clamp stays the ceiling).
- Host-overlay slot with per-cell optional TTL (firmware timer).
- Replaces the stage-5 frame source; layer-accent default config so
  behavior is visibly richer than one key.
- Pure-logic parts unit-tested on the host (std test crate).
- Exit: both halves render base + layer + a hardcoded host-overlay test
  cell with blink/breathe on real hardware; typing unaffected.

## Phase 2 — Host protocol + codec

One protocol, three transports, one codec.

- Versioned command set (capability query first): set/unset cells, clear,
  read-back, atomic replace, TTL, brightness, toggles, bootloader entry.
- Framing: 32-byte-report friendly (USB raw HID) and GATT characteristic
  friendly (custom service; Web Bluetooth reachable). Same payload codec.
- Rust codec crate shared by firmware and CLI (`no_std` + `std`);
  TypeScript codec for Lightbench with golden test vectors shared across
  both.
- Firmware: custom GATT service (new UUID, not HID) + USB vendor interface,
  feeding the compositor's host overlay.
- Exit: CLI sets/clears/replaces overlay cells over USB and BLE;
  Lightbench does the same from the browser (WebHID + Web Bluetooth).

## Phase 3 — Split lighting transfer

- Vendor RMK as a subtree; add a bounded application-message hook to the
  split protocol (aim upstreamable).
- Forward host-overlay batches and toggle/brightness state to the
  peripheral; peripheral runs the same compositor locally.
- Exit: a host write lights the correct key on the right half over the
  split link; key latency unchanged under lighting load.

## Phase 4 — Persistent lighting + canonical config

- Persist base/layer/toggle lighting records in RMK storage; boot loads
  them into the compositor.
- Extend the canonical schema (`tools/glove80-control`, building on
  `runtime_manifest.rs`) to cover lighting records, activation predicates,
  toggles, and stable layer references.
- Transactional apply: complete-config import lands atomically or not at
  all; export round-trips.
- Exit: reboot restores configured lighting; interrupted apply leaves the
  previous config; export → import → export is byte-stable.

## Phase 5 — Tooling completion

- Lightbench: persistent lighting-layer editing on the new protocol (USB +
  BLE), TTL/brightness controls, capability-driven UI.
- Nice-to-have: additive FRAME_READ command exposing the final composed
  frame, enabling a true live-preview panel in Lightbench.
- CLI: full verb set (validate/apply/export/restore/watch), shared codec.
- Optional background service for app-state lighting (Codex states) as a
  thin overlay client.
- Exit: every lighting-design.md host operation is exercisable from both
  Lightbench and the CLI.

## Phase 6 — Unified keymap + lighting configuration (historical implementation)

The original Phase 6 used product-protocol v1.2 as a keymap bridge. The current
implementation keeps one editor/config file but uses Rynk for keymaps and the
product protocol for lighting. The v1.2 codec remains compatibility material,
not a production firmware capability.

- Rynk: typed keymap read/write by (layer, row, column), capability discovery,
  bulk pages, persistence, native USB/BLE clients, and browser WASM.
- Layer names and stable layer IDs live in the canonical config, not the
  firmware slots.
- Lightbench: keymap panel beside the lighting panels, same board
  rendering, both transports.
- CLI + canonical schema: the config file grows to cover bindings + layers
  + lighting; transactional apply covers it all; export round-trips.
- Exit: a fresh keyboard is fully configured (keymap + lighting) from one
  file, with explicit Rynk and product-protocol sessions.

## Phase 7 — Qualification and cutover

- Run the full checklist in design-goals.md on both halves.
- Fix stragglers (battery-idle behavior, bootloader entry from host, etc.).
- Cut daily use over to RMK; keep ZMK recovery images archived; retire the
  ZMK-era host code.
- Exit: checklist all green; ZMK tree no longer needed for daily use.

## Post-cutover candidate: Lightbench in Dioxus

Considered, deliberately deferred (working-first rule). The real argument
is codec unification: a Rust UI consumes glove80-host-protocol directly,
ending the dual Rust/TS codec maintained via golden vectors; a desktop
build reuses the CLI's native transports while a WASM build keeps the
no-install browser path. Costs: full UI rewrite, web-sys WebHID/Web
Bluetooth glue, and the browser build must remain first-class. The
current React app keeps all substance in framework-agnostic lib modules,
so a later port stays cheap.

## Phase 8 — Fork cleanup and upstreaming (final; cutover complete)

Deliberately last: only after the system works the way we want it, so the
hooks we upstream are the ones reality validated.

- [x] Restructure the vendored subtree into a proper fork + submodule: our own
  RMK fork repo, one logical branch per extension (split app messages,
  transport hooks, shared flash, bugfixes) off the pinned base, an
  integration branch merging them, and the monorepo consuming it as a
  submodule. `git subtree split` can extract the existing patch history so
  nothing needs rewriting from scratch.
- [x] Write and refresh PATCHES.md: historical marker sites, what/why,
  upstreamability, fork branches, and the post-cutover pin.
- Upstream PRs straight from the per-feature branches; as they merge,
  rebase the integration branch and drop local patches.
- [x] Integrate open Rynk PR #962, migrate firmware/CLI/Lightbench keymap
  ownership, and retire the downstream keymap bridge. Hardware qualification
  remains before release; rollback stays pinned at `8089822e`.
- [x] Exit: the vendored tree carries zero (or near-zero, documented) local
  patches, or is replaced outright by a plain pinned dependency; the repo
  reads like a standard RMK consumer with its own crates on top.
