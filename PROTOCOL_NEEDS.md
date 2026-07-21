# Protocol needs

This is an append-only coordination log for work that may require changes to
the shared RMK/Rynk protocol. Add a dated section; do not rewrite or remove
another agent's requirements.

## 2026-07-21 — Rynkbench transient-overlay readback

Owner/requester: Rynkbench overlay editor task.

Status: protocol proposal only. No RMK, Glove80, or Rynkbench commit has been
created. Coordinate the command allocation and shared protocol edits here
before landing anything.

### Problem

The current lighting protocol exposes `LightingState.overlay_len` and supports
set, unset, clear, and atomic replacement of transient overlay cells, but it
cannot return the current overlay contents. Consequently the WebHID backend's
`readOverlay()` unconditionally throws and Rynkbench displays
`overlay readback unsupported — started empty`. This is a wire-protocol gap,
not merely a Rynkbench gap.

### Required wire behavior

- Add a paged overlay-read endpoint, analogous to `GetLightingScenes` but
  available for every standard lighting controller.
- Request fields: expected `LightingState.revision` and cell `offset`.
- Response fields: sampled `revision`, `total_count`, and a bounded page of
  existing `LightingOverlayCell` values.
- Returned `ttl_ms` values must be remaining relative lifetimes at the atomic
  sample time. `None` continues to mean no expiry. Never expose firmware
  absolute deadlines.
- Page ordering must remain stable while the revision remains unchanged.
- Reject a stale pin with `LightingError::StateRevisionConflict`; a host can
  restart from `GetLightingState` and obtain a coherent whole-overlay snapshot.
- Expired cells must not be returned. If pruning an expired cell changes the
  overlay while serving a read, advance the lighting revision so the caller's
  stale pin conflicts instead of silently observing a different snapshot.
- Older firmware must remain compatible by answering `UnknownCmd`. Rynkbench
  will keep its current empty-overlay fallback for that case.

Suggested type names are `LightingOverlayPageRequest`, `LightingOverlayPage`,
and `LightingOverlayPageResult`; suggested client method is
`get_lighting_overlay`. `0x091B` is a **provisional** command ID only because it
follows `SetLightingLayerPolicy = 0x091A`; reserve it only after checking the
other agents' protocol work for collisions. Whether to add an
`OVERLAY_READBACK` feature bit or rely on command probing is also a shared
protocol decision.

### Required implementation surfaces

1. `rmk-types`: payloads, endpoint table, maximum postcard-size assertions,
   golden vectors/snapshots.
2. Standard lighting engine: atomic paged read of active overlay entries,
   conversion of absolute expiry to remaining TTL, and revision advancement
   when read-time pruning occurs.
3. RMK Rynk firmware service: command routing and stable-LED conversion.
4. Native `rynk::Client` and `rynk-wasm`: typed endpoint exposure.
5. Rynkbench WebHID session: bounded retry loop over revision-pinned pages;
   retain graceful fallback for firmware without the endpoint.

### Acceptance coverage

- Empty and multi-page overlays round-trip with stable LED IDs and effects.
- Persistent and finite-TTL cells return the correct TTL shape; expired cells
  are absent.
- A stale revision is rejected, including when expiry happens between the
  state read and the first/next page.
- The response remains below `LIGHTING_PAYLOAD_SIZE` at maximum page capacity.
- Existing firmware without the command still connects in Rynkbench and shows
  the unsupported/read-empty fallback.

### Isolated WIP (do not treat as landed)

Exploratory, uncommitted edits exist in:

`/home/imalison/Projects/glove80-config/dependencies/glove80-rmk/.worktrees/overlay-readback/dependencies/rmk`

That worktree is based on RMK `e4976e38` and currently sketches the payload,
endpoint, engine page, firmware bridge, native client, and WASM method. It has
not passed formatting or tests and should be reconciled with other protocol
changes rather than copied blindly.

A mistaken earlier worktree also exists under the standalone
`/home/imalison/Projects/glove80-rmk/.worktrees/overlay-readback`; it is not the
authoritative checkout for this task and has not been committed.

## 2026-07-21 — Rynkbench complete per-layer status

Owner/requester: Rynkbench Live View task.

Status: protocol proposal with partial exploratory edits in an isolated
worktree. Coordinate the command allocation and shared protocol files before
landing.

### Problem

RMK owns the complete active-layer mask: `KeyMap::is_layer_active` exposes it
internally, and `KeymapLightingState` already snapshots it for the lighting
compositor. Rynk exports only `GetCurrentLayer -> u8` plus the scalar
`LayerChange(u8)` topic. That tells a host the highest active layer but not
which other layers are active.

Rynkbench consequently labels layers between the default and highest active
layer as `not reported`. More importantly, it cannot exactly reproduce RMK's
transparent-key resolution or `ActiveStack` scene composition. The UI's
current `effective · top of stack` emphasis is an artifact of this incomplete
wire surface; the desired UI simply reports `active`/`inactive` for each layer
and marks the default layer.

### Required wire behavior

- Add an authoritative layer-state snapshot endpoint.
- Response must contain `default_layer: u8` and a complete active-layer bitmap
  covering RMK's current 64-layer bound.
- The default layer must be marked active in the returned bitmap even though
  RMK stores it separately from the mutable layer mask.
- Existing `LayerChange` can remain the notification/invalidation signal: it
  is published for layer-mask changes, after which a host fetches the complete
  snapshot. No second topic is required.
- Older firmware should remain compatible via `UnknownCmd`; Rynkbench can fall
  back to its existing current/default reads and mark that snapshot incomplete.

Suggested type/endpoint names are `LayerState` and `GetLayerState`. `0x0808` is
a **provisional** command ID only because it follows
`GetLedIndicator = 0x0807`; reserve it only after checking other agents' work.

### Required implementation surfaces

1. `rmk-types`: status payload, endpoint table, round-trip/golden snapshots.
2. RMK host context/status handler: snapshot the default layer and every
   `KeyMap::is_layer_active` bit.
3. Native `rynk::Client` and `rynk-wasm`: typed getter.
4. Rynkbench session/state: fetch at connect and refetch on `LayerChange`.
5. Rynkbench Live View: show each layer's actual status, remove the top-of-stack
   framing, and use the active set for key and lighting composition.

### Acceptance coverage

- Default-only, multiple-active, tri-layer, and active-below-default snapshots
  report the exact mask.
- A layer-mask change that leaves the highest active layer unchanged still
  causes the host to refresh and observe the changed mask.
- Key resolution considers exactly the active layers plus the default layer in
  RMK precedence order.
- `ActiveStack` preview includes every active layer rather than only the
  default and highest active layers.

### Isolated WIP (do not treat as landed)

Partial, uncommitted edits exist under:

`/home/imalison/Projects/glove80-config/.worktrees/overlay-layer-readback/dependencies/glove80-rmk/dependencies/rmk`

They tentatively touch the status payload/command, native and WASM clients, and
RMK host context/handler. That worktree also contains a partial overlay draft
created before coordination was requested. It has not been formatted or
tested and should be reconciled with whichever agent owns the shared protocol
edit.

## 2026-07-21 — Rynkbench compiled layer-scene readback

Owner/requester: Rynkbench lighting-state task.

Status: protocol proposal only. No RMK, Glove80, or Rynkbench implementation
has been retained. Coordinate endpoint and feature-bit allocation with the
overlay-readback and complete-layer-status work above before editing shared
protocol files.

### Problem

The standard lighting engine renders two distinct layer-scene sources in the
same compositor band:

1. board-compiled `LayerScenes` (`StandardLightingEngine::layers`), then
2. mutable/persisted runtime overrides (`StandardLightingEngine::scenes`).

Runtime cells override compiled cells at the same `(layer, LED)`. The existing
`GetLightingSceneStatus` and `GetLightingScenes` endpoints expose only the
runtime `SceneTable`, so they do not describe all declarative lighting state
that the firmware is currently rendering.

This is visible on the Glove80 base layer: firmware renders the compiled blue
inner-column/upper-thumb scene, but Rynkbench receives an empty runtime scene
table and cannot display that pattern in either Live or Lighting. The UI must
not hardcode board scenes; firmware must report the compiled source generically.

### Required wire behavior

- Expose all board-compiled layer-scene cells using stable `LightingLedId`,
  layer, and `LightingEffect` values.
- Keep compiled scenes and mutable runtime overrides distinguishable. A merged
  table is insufficient for correct editing and replacement semantics.
- Return all configured layers, not only the currently active/effective set.
- Keep `GetLightingScenes` defined as the mutable table used by existing
  set/unset/replace and persistence operations.
- Prefer additive commands/types rather than changing existing postcard
  layouts or endpoint meanings.
- Advertise support with a new feature bit or another explicit capability so
  older firmware degrades cleanly.
- Pin compiled-scene paging to `topology_revision` (compiled scenes are
  immutable for the firmware build); unrelated runtime lighting revisions
  should not invalidate the read.
- Document the behavior for boards with zero compiled cells: either supported
  empty readback or feature absent, consistently.

A likely shape is a read-only compiled-scene status endpoint plus a paged
compiled-scene endpoint, reusing `LightingSceneCell` for items. Do **not** use
the previously obvious `0x091B`/`0x091C` without coordination: the overlay
readback section above already identifies `0x091B` as provisional, so command
allocation is now a known collision point.

### Why source separation matters

- Deleting a runtime override must reveal the compiled default beneath it.
- Replacing the runtime table must not persist every compiled cell as an
  override.
- The editor must label inherited firmware defaults separately from stored
  mutable overrides.
- Masking a compiled cell off requires an explicit black runtime override;
  deleting that override restores the compiled cell.

### Required implementation surfaces

1. `rmk-types`: feature/capability, payloads, endpoint table, maximum-size
   assertions, and golden vectors.
2. `rmk::lighting::LayerScenes`: flattened count/page access that preserves
   each enclosing `LayerScene.layer`.
3. Standard lighting engine command/reply: atomic read-only compiled-scene
   paging alongside (not merged into) runtime `ReadScenes`.
4. RMK Rynk adapter/handlers: convert compositor slots to stable LED IDs.
5. Native `rynk::Client` and `rynk-wasm`: typed endpoints plus a bounded
   read-all helper.
6. Glove80 controller setup if compiled-scene capability advertisement needs
   explicit board opt-in.
7. Rynkbench session/model: read both sources, compose background < compiled
   scenes < runtime scenes < overlay, and edit/replace runtime cells only.

This work overlaps the same protocol tables, handler modules, client API, and
WASM bindings as overlay readback, so it should be implemented in the same
coordinated protocol branch or after the earlier owner allocates commands.

### Acceptance coverage

- A generic host reads the Glove80 compiled layer-0 cells and their stable LED
  IDs/effects without product-specific knowledge.
- Runtime overrides remain a separate read/write table and win over compiled
  cells for the same layer and LED.
- Clearing an override reveals the compiled default; whole-table replacement
  never copies compiled cells into persistence implicitly.
- Empty and multi-page compiled scene sets round-trip within payload bounds.
- Topology-revision conflicts are detected coherently across pages.
- Existing firmware without the new capability still connects and leaves the
  compiled source unavailable rather than guessed.

### Non-goal / possible follow-up

A sampled fully composed RGB-frame endpoint would improve exact Live snapshots
for animation/status/extension sources, but it does not replace source-aware
declarative readback required by the editor.

## 2026-07-21 — Coordinated RMK allocation and implementation

Owner/coordinator: shared RMK protocol overlay.

Status: implemented, verified, and pushed to the fork's composed `master` as
`d74ee7f4`. Downstream Rynkbench consumption remains in its owning
repository/task; this simplified Glove80 firmware repository no longer
contains the Rynkbench UI.

The coordinated allocation is:

- `GetLayerState = 0x0808` with command-probing fallback.
- `GetLightingOverlay = 0x091B` and
  `LightingFeatureFlags::OVERLAY_READBACK = 1 << 7`.
- `GetLightingCompiledSceneStatus = 0x091C`,
  `GetLightingCompiledScenes = 0x091D`, and
  `LightingFeatureFlags::COMPILED_LAYER_SCENES = 1 << 8`.

Compiled-scene readback is supported even when the board has zero compiled
cells; the status and first page are empty. The existing `LAYER_SCENES` bit and
`GetLightingScenes` retain their mutable runtime-table meaning. No downstream
`ProtocolVersion` was minted: older firmware compatibility continues through
capability checks plus `UnknownCmd`.

The overlay is based on the downstream-version normalization at `228f9bcd` and
deliberately excludes the isolated lighting-docs WIP at `e4976e38`; those
documents remain local untracked notes. It adds wire types and golden frames,
read-time overlay pruning/revision behavior, stable-LED firmware conversion,
compiled-scene flattening, native/WASM typed methods, bounded native read-all
helpers, independent compiled/runtime layer policies, complete layer-state
snapshots (including default-layer invalidation), and full-stack acceptance
coverage.
