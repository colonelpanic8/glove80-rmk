# Replicated half-state sync: a plan for upstreaming

Status: proposal. Owner: glove80-rmk. Target: upstream RMK via the
`fold/split-reliability` topic, after local proof (phases below).

## The problem, as found in the field

RMK's split layer contains at least five independent, hand-rolled
central↔peripheral state synchronizations: `SplitMessage::Layer`,
`::BatteryStatus`, `::ConnectionStatus`, `::Pointing`, and the raw
`split_app` byte channel over which this repository built a sixth — the
semantic lighting replication protocol (generations, revisions, staged
atomic snapshots, acks). Each reinvents delivery semantics, and each can
independently rediscover the same bug classes. The 2026-08 battery-gauge
investigation found four in ours, all now fixed board-side:

1. **Partial-fill livelock.** A multi-packet transaction enqueued
   packet-by-packet into a bounded queue, aborting at the tail, left the
   queue exactly full of a headless prefix; the retry refilled it faster
   than the link drained it. The peripheral received endless prefixes and
   never a commit; its replica froze for the life of the boot.
2. **Timer-restart starvation.** A retransmission timeout re-armed on
   every wakeup instead of held as an absolute deadline; any sub-timeout
   event stream postponed recovery indefinitely.
3. **Blocking retry loops.** A send-failure path that slept inline made
   the replication task deaf to acks and link edges while backlogged —
   including a link-down that meant the queue would never drain again.
4. **Up-edge inbox flush.** The peripheral marks the link up on the first
   inbound message, so a "flush stale packets on link change" step
   discarded the head of the very snapshot the reconnect delivers.

None of these are lighting bugs. They are properties of syncing state
over a bounded, lossy, in-order channel between halves, and every
hand-rolled sync in RMK is exposed to them. That is the case for solving
it once.

## Proposal: a generic sync primitive in RMK core

A `split_sync` facility with **registered state cells**. A subsystem or
board declares WHAT is synchronized; RMK owns HOW.

```rust
// Declaring side (central is the single writer per cell):
static LIGHTING: SyncCell<LightingReplica> = SyncCell::new(CellId::Lighting);
LIGHTING.mark_dirty();            // engine changed; schedule a snapshot

// Peripheral side:
LIGHTING.on_apply(|staged: &LightingReplica| { /* atomic install */ });
```

Two cell kinds, because their delivery semantics genuinely differ:

- **Durable cells** (tables, configuration): chunked snapshot transfer,
  staged and applied all-or-nothing, revision-numbered, digest-attested.
- **Ephemeral cells** (layer state, battery, indicator context):
  last-value-wins deltas, sequence-numbered, never staged, excluded from
  digests. Latest state always fully supersedes older state.

### Reliability core (each element kills a found bug class)

| Design element | Kills |
| --- | --- |
| Whole-transaction reservation before the first packet is enqueued (counting and sending share one walk) | partial-fill livelock (1) |
| Absolute deadlines for ack and backoff, folded into the task's select — never an inline sleep | starvation (2), deaf retry loops (3) |
| Peripheral drains its inbox only on the down edge; staleness handled by generation/revision rejection, not flushing | up-edge flush (4) |
| Digest attestation heartbeat: the peripheral periodically reports `(revision, digest, age)` unprompted; mismatch or silence triggers resnapshot with backoff | every wedge class not yet diagnosed |
| Attestation-first reconnect: peripheral leads with what it has; matching digests suppress the snapshot burst entirely | reconnect churn; most link blips resync in one packet |

### Digest rules (the part that rots if left informal)

- Hash the **canonical wire encoding**, never in-RAM structs. Halves
  intern and order internal storage independently; representation must
  not leak into the digest.
- A cell declares its shape at registration: **map** (keyed cells,
  duplicates impossible — order-independent fold permitted) or
  **sequence** (order is semantics, e.g. later-wins rule lists — hash in
  order). Getting this wrong silently false-matches reordered rule
  tables.
- The central digests the **projection it actually replicates** (e.g.
  the peripheral-half subset), walked by the same code that sends it.
- No incremental digest maintenance without an audit: recompute from
  scratch at mutation/attestation boundaries (sub-millisecond at these
  sizes). If ever profiled into incrementality, the heartbeat recomputes
  and `debug_assert`s against the maintained value.
- Digests are error detection, not security: FNV/CRC32 class is
  sufficient.

### Transport constraints inherited from today's split layer

- Bounded messages (26-byte payloads over BLE GATT today); the primitive
  chunks, callers never see packet boundaries.
- The channel is in-order and link-layer-reliable but host-lossy (queue
  overflow, notification eviction, session teardown). The primitive owns
  end-to-end recovery; transport-level acks stay out of scope.
- Application traffic remains lowest-priority behind key events —
  correctness must not depend on throughput, only on eventual delivery
  plus the heartbeat.
- **Multi-peripheral addressing from day one.** `split_app` currently
  assumes a single peripheral; the primitive should be per-peripheral
  (cell instance × peripheral id), since RMK itself supports more.

## Phased path

- **Phase 0 — prove it here (in flight).** Wedge fixes are live on both
  boards. Next: digest attestation + heartbeat as additive split-app
  tags in this repository's lighting protocol, surfaced to the host via
  the in-progress `GetLightingReplicaStatus` / `GetLightingFrame`
  Rynk endpoints (worktree branch `lighting-observability`, carried via
  the PR-1031 manifest entry).
- **Phase 1 — extract.** Lift the replication machinery into an
  in-workspace crate, const-generic over capacities; Glove80 and Go60
  consume it as a dependency instead of `#[path]` includes (which
  already silently broke once). The crate boundary is the API rehearsal
  for upstream.
- **Phase 2 — upstream RFC.** Propose `split_sync` on
  `fold/split-reliability` with the attestation data from phase 0 as
  evidence. Keep the lighting-specific schema downstream; upstream only
  the primitive.
- **Phase 3 — migrate RMK's own syncs.** `Layer`, `BatteryStatus`,
  `ConnectionStatus` become ephemeral cells; their bespoke split-message
  variants deprecate behind the primitive.

## Open questions

- Cell registry: static declaration vs macro-generated (mirroring the
  `#[event]` pattern) — macro likely, for channel sizing per cell.
- Heartbeat cadence and backoff constants: board-tunable or fixed?
- Whether ephemeral cells subsume RMK's existing eventual-consistency
  paths exactly, or some (pointing) need rate semantics the primitive
  should not own.
- How much of the Rynk observability surface (frame/replica readback)
  generalizes: per-cell replica status is generic; frame readback is
  lighting's own.
