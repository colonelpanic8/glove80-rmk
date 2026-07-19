# RMK upstream-alignment memo

- Scope: `origin/feat/forward_split_message` at `4181965f`,
  `glove80-import/split-app-messages` at `8f80acb5`,
  `origin/feat/host_service` at `3083dbd6`, and
  `glove80-import/host-transport-hooks` at `ffe05dee`.
- Recommendation in one line:
  - Split messages: **adopt theirs plus deltas, while keeping ours temporarily**.
  - Host transports: **engage on the design now, but keep ours until upstream has code and a browser-capable transport seam**.

## Area 1 — forwardable split messages

### What upstream built

- Wire shape:
  - `SplitMessage` gains `User(SplitUserPacket)` alongside the built-in split messages
    (`origin/feat/forward_split_message:rmk/src/split/mod.rs:41-67`).
  - `SplitUserPacket` is `kind: u16`, `len: u8`, and a fixed
    `[u8; SPLIT_USER_PAYLOAD_MAX_SIZE]`; inbound packets are additionally stamped
    with `peripheral_id` for local dispatch
    (`origin/feat/forward_split_message:rmk/src/split/mod.rs:21-35`).
  - Events are postcard-encoded into the packet's fixed data buffer; kind mismatch,
    overlong `len`, and decode failure are rejected
    (`origin/feat/forward_split_message:rmk/src/split/forward.rs:43-75`).
  - The payload default is 16 bytes and is build-configurable
    (`origin/feat/forward_split_message:rmk-config/src/lib.rs:228-230,292-297`).

- Registration and user hook:
  - A user marks an ordinary RMK event with `#[event(split = N)]`; `N = 0`
    selects a 16-bit FNV-1a hash of the Rust type name, while an explicit nonzero
    value pins the wire kind
    (`origin/feat/forward_split_message:rmk-macro/src/event.rs:14-53,220-250`).
  - The macro emits a linker symbol per kind to catch duplicate kinds at link time
    and requires `Serialize`, `Deserialize`, `MaxSize`, and `Clone`
    (`origin/feat/forward_split_message:rmk-macro/src/event.rs:241-257,382-407`).
  - Generated publishers forward the event and publish it locally; generated
    subscribers select between the local event channel and the global remote
    dispatch channel
    (`origin/feat/forward_split_message:rmk/src/split/forward.rs:77-121,124-169`).
  - User code otherwise uses normal `publish_event(...)` and `#[processor]`
    subscriptions. The example is concise and representative
    (`origin/feat/forward_split_message:examples/use_rust/nrf52840_ble_split/src/split_event.rs:1-18`;
    `origin/feat/forward_split_message:examples/use_rust/nrf52840_ble_split/src/peripheral.rs:173-188`).

- Routing and direction:
  - The global forward channel is pub/sub. Every running central
    `PeripheralManager` and every split peripheral subscribes to it
    (`origin/feat/forward_split_message:rmk/src/split/forward.rs:22-41`;
    `origin/feat/forward_split_message:rmk/src/split/driver.rs:80-103`;
    `origin/feat/forward_split_message:rmk/src/split/peripheral.rs:79-104`).
  - Central → peripheral works: a central manager wraps a forwarded packet as
    `SplitMessage::User` and writes it to its peripheral
    (`origin/feat/forward_split_message:rmk/src/split/driver.rs:120-151`).
  - Peripheral → central works: the peripheral sends the same `User` variant and
    the central stamps its peripheral id before dispatch
    (`origin/feat/forward_split_message:rmk/src/split/peripheral.rs:90-104`;
    `origin/feat/forward_split_message:rmk/src/split/driver.rs:201-219`).
  - There is no destination field. On a multi-peripheral central, each manager's
    subscription receives the same central-originated event, so forwarding is
    effectively broadcast. The typed subscriber also discards the stamped source
    id; source-aware code must consume the raw dispatch channel instead
    (`origin/feat/forward_split_message:rmk/src/split/forward.rs:124-169`).

- Buffering and failure behavior:
  - Both forwarding stages use bounded `PubSubChannel`s. Defaults are eight queued
    packets, four forward publishers/subscribers, and two dispatch publishers/eight
    subscribers
    (`origin/feat/forward_split_message:rmk-config/src/default_config/event_default.toml:96-105`).
  - Forwarding uses `publish_immediate`, including from the nominal async publisher;
    callers receive no admission/failure result and do not back-pressure the split
    writer (`origin/feat/forward_split_message:rmk/src/split/forward.rs:88-121`).
  - The two global queues are shared by every split event kind. A late subscriber
    has no retained state, and a lagging subscriber can lose packets.

- Connection coupling:
  - Peripheral-originated traffic is sent only while the peripheral's copy of
    `CONNECTION_STATE` says the central is connected to a host
    (`origin/feat/forward_split_message:rmk/src/split/peripheral.rs:108-163`).
  - On the central, inbound `User` packets are dispatched only inside the same
    host-connected guard
    (`origin/feat/forward_split_message:rmk/src/split/driver.rs:201-225`).
  - This couples the application side channel to host attachment rather than to
    split-link availability. Lighting/state synchronization and version discovery
    should continue across a live split link even when no USB/BLE host is attached.

- Documentation status:
  - The branch has module docs, macro diagnostics, configuration defaults, tests,
    and one Rust example.
  - It adds no user-facing page under the RMK docs tree. Direction, broadcast
    behavior, local echo, overflow semantics, reconnection, and event-kind stability
    are not documented as public contracts.

### Mapping Glove80 requirements

- Lighting sync batches:
  - Natural mapping: define one explicitly numbered `Glove80SplitMessage` event
    whose variants are the existing sync codec messages, or define one opaque
    `Glove80SplitFrame` event and keep the codec application-owned.
  - Blocker: our codec allows 26-byte payloads; upstream defaults to 16. Raising
    `split_user_payload_max_size` must also be tested against the resulting fixed
    `SPLIT_MESSAGE_MAX_SIZE` and BLE characteristic array, not treated as a config-only
    change.
  - Blocker: upstream's forward queue defaults to 8, while our central TX queue is
    sized to 26 so one complete resync burst fits
    (`glove80-import/split-app-messages:rmk/src/split_app.rs:104-118`).
  - Local echo must be avoidable. Upstream deliberately publishes every outgoing
    split event locally too. A bootloader command or remote overlay mutation must
    not be mistaken for a received command on its sender.

- State sync — brightness, toggles, USB-connected:
  - Fits as a central → peripheral state variant and should be sent as an idempotent
    snapshot on every usable link-up edge.
  - The `usb-connected` value is application data. It must not gate delivery of the
    very message that communicates it; remove the `CONNECTION_STATE` guards from
    user-message forwarding.

- Peripheral version announcement:
  - Fits as a peripheral → central event.
  - It is not reliable today: the peripheral's generic `CentralConnectedEvent(true)`
    is emitted when the BLE connection is accepted, before the split forward
    subscriber is installed
    (`origin/feat/forward_split_message:rmk/src/split/ble/peripheral.rs:143-171,187-189`).
  - Even if it reaches the split writer, a peripheral notification issued before
    the central's CCCD subscription can be silently lost. The central subscribes
    only later during GATT setup
    (`origin/feat/forward_split_message:rmk/src/split/ble/central.rs:381-394`).
  - The announcement also currently disappears whenever host `CONNECTION_STATE`
    is false.

- Peripheral bootloader-entry command:
  - Fits as a central → peripheral event with an application magic/authorization
    check.
  - It needs remote-only delivery or explicit source/destination metadata. With
    current local echo, a shared handler can act on the central's own publication.
  - Delivery should be acknowledged at the application layer before the peripheral
    resets; the generic immediate pub/sub API gives the sender no enqueue result.

- Resync-on-reconnect:
  - This is the main missing primitive. Upstream has connection *events*, not a
    stateful, deliverability-qualified link watch
    (`origin/feat/forward_split_message:rmk/src/event/split.rs:7-22`).
  - The central publishes `PeripheralConnectedEvent(true)` before GATT discovery
    and notification subscription
    (`origin/feat/forward_split_message:rmk/src/split/ble/central.rs:231-261,381-394`).
  - The peripheral likewise publishes connected before the central has subscribed.
    A late application subscriber cannot recover the current state from pub/sub.
  - Our watch deliberately defines peripheral link-up as the first inbound message,
    when bidirectional delivery is known to work, and retains the latest state for
    late receivers
    (`glove80-import/split-app-messages:rmk/src/split_app.rs:28-35,120-124`;
    `glove80-import/split-app-messages:rmk/src/split/peripheral.rs:81-113,152-162`).

- TTL-driven unsets:
  - Fits as central → peripheral `UnsetKeys` batches; the central remains TTL
    authority and the peripheral does not need clock synchronization.
  - Individual unset loss is tolerable only if link-up reliably triggers a full
    idempotent snapshot. Without the link watch, immediate/drop-on-lag forwarding
    can leave stale right-half cells indefinitely.
  - Queue-full information is useful to the application so it can mark resync as
    owed and retry; upstream's publisher currently returns no such signal.

### Gaps to raise in review

- **No usable link/session state:** existing split connection events are transient,
  precede transport deliverability, and are not tied to the forwarding API.
- **Notify-before-subscribe hazard:** both the app's connection notification and the
  forward channel subscriber can race ahead of BLE CCCD subscription.
- **No cancellation-safe down edge:** the BLE central session races manager,
  connection-monitor, and sleep futures with `select3`
  (`origin/feat/forward_split_message:rmk/src/split/ble/central.rs:317-327`). A
  down notification placed only after an awaited loop can be skipped when that
  future is cancelled.
- **Bounded but not observable:** capacities are static, but immediate publication
  has no producer-visible success/failure and all event kinds compete in the same
  queues.
- **Local echo is mandatory:** useful for symmetric RMK events, unsafe for directed
  application commands unless every payload and handler adds role filtering.
- **Direction/source are implicit:** no central-only/peripheral-only declaration,
  no destination, central sends broadcast to all managers, and typed receive drops
  `peripheral_id`.
- **Host state is the wrong gate:** user packets should follow split-session readiness,
  not whether the central currently has a host connection.
- **Wire-kind stability needs a contract:** an auto kind hashes only the unqualified
  type name. Renaming the type changes the wire id; unrelated crates can reuse a
  type name. Glove80 should use an explicit kind regardless.
- **Docs are not yet sufficient:** overflow, ordering, local echo, broadcast, kind
  allocation, and reconnection semantics need explicit documentation.

### Recommended alignment strategy

- Decision: **adopt theirs plus deltas; keep ours temporarily**.
- Why adopt:
  - Upstream's typed event registration, collision check, ordinary RMK
    publish/subscribe integration, and bidirectional transport insertion are more
    generally useful than a Glove80-specific opaque queue.
  - Our entire sync codec can ride as one explicit event kind, so adopting the
    framework does not require upstream to understand lighting, versions, TTLs, or
    bootloader commands.
- Why not switch now:
  - Reliable reconnect resync and peripheral announcement are correctness
    requirements, not polish.
  - Mandatory local echo and host-connection gating make directed control messages
    unsafe or unavailable without application workarounds.
  - Current capacity defaults do not cover our payload and full-resync burst.

- Concrete review feedback / small PRs to offer:
  1. Add a public, stateful split-session watch, keyed by peripheral id on the
     central; define `up` as bidirectionally deliverable, not merely connected.
  2. Install the forwarding subscriber before publishing `up`; on BLE peripheral,
     publish `up` only after the first inbound central message. This is the same
     gating lesson captured in our implementation.
  3. Lower the watch from a `Drop` guard on both halves so cancellation always
     produces the `true → false` edge. Our central guard documents the concrete
     outer-`select` cancellation case
     (`glove80-import/split-app-messages:rmk/src/split/driver.rs:114-130`).
  4. Remove host `CONNECTION_STATE` gating for `SplitMessage::User`; gate only on
     live split transport and return on `Disconnected`.
  5. Add a remote-only subscriber/publisher mode, or return
     `{ source: Local | Central | Peripheral(id), event }` so directed commands do
     not require payload-level guesses.
  6. Expose nonblocking enqueue outcome (`try_publish`/`Result`) and document lag
     behavior; add tests for publish-before-subscriber, queue overflow, reconnect,
     and manager cancellation.
  7. Document multi-peripheral broadcast/targeting and provide either a target id
     or an explicit broadcast API before treating the interface as general.
  8. Add a docs page and a capacity example covering a payload larger than the
     default. Verify a 26-byte event end-to-end over BLE before Glove80 migrates.

## Area 2 — host service and transport hooks

### What the upstream branch contains today

- The remote branch contains two plan documents plus a substantial shared ICD under
  `rmk-types/src/protocol/rmk/`; it does **not** contain the planned firmware
  `rmk/src/host/rmk_protocol/` server or `rmk-host-tool` transport implementation.
- The checked-in ICD still describes postcard-rpc over COBS-framed USB bulk and BLE
  streams
  (`origin/feat/host_service:rmk-types/src/protocol/rmk/mod.rs:1-34`), with typed
  endpoint tables and server-to-client topics
  (`origin/feat/host_service:rmk-types/src/protocol/rmk/endpoints.rs:31-65`;
  `origin/feat/host_service:rmk-types/src/protocol/rmk/topics.rs:1-33`).
- Therefore, the code is useful evidence of command/type scope and wire-compatibility
  discipline, but the transport/server API is still plan-level.

### Plan evolution

- Earlier transport plan (`plan.md`):
  - Per-transport servers share a keymap context but keep independent RX/dispatch
    state and failure boundaries
    (`origin/feat/host_service:plan.md:88-110`).
  - USB is a vendor-class bulk IN/OUT pair with WinUSB descriptors, explicitly
    chosen over raw HID for throughput despite driver cost
    (`origin/feat/host_service:plan.md:146-155`).
  - BLE is a dedicated non-HID primary service with write/write-without-response
    and notify characteristics sized to MTU−3
    (`origin/feat/host_service:plan.md:157-163`).
  - BLE readiness is to be signaled on the first response-CCCD subscription, an
    important deliverability boundary
    (`origin/feat/host_service:plan.md:122-144`).

- Newer custom-protocol plan (`plan_custom_protocol.md`) supersedes the wire layer:
  - Drop postcard-rpc and COBS; keep postcard only for payloads. Use a fixed five-byte
    `CMD u16 + SEQ u8 + LEN u16` header and frames up to 4096 bytes
    (`origin/feat/host_service:plan_custom_protocol.md:1-8,11-40`).
  - USB remains vendor bulk; BLE continuation packets carry raw payload and are
    reassembled by header length. Complete notify frames are serialized so topics
    cannot interleave inside responses
    (`origin/feat/host_service:plan_custom_protocol.md:42-58`).
  - A flat `Cmd` namespace covers requests and unsolicited topics; version and
    capability discovery replace postcard-rpc schema hashes
    (`origin/feat/host_service:plan_custom_protocol.md:62-146`).
  - The proposed `WireTx`/`WireRx` interface operates on complete frames; USB and
    BLE adapters own packetization/reassembly
    (`origin/feat/host_service:plan_custom_protocol.md:185-227`).
  - The host tool is planned around native `nusb` and `btleplug` transports, with
    response/topic demultiplexing
    (`origin/feat/host_service:plan_custom_protocol.md:252-275`).
  - The migration sequence explicitly has transport and host-tool implementation
    still ahead
    (`origin/feat/host_service:plan_custom_protocol.md:278-295`).

### Mapping Glove80 requirements

- 32-byte raw-HID vendor interface:
  - **Not met by the plan.** Upstream intentionally chooses USB bulk, while our hook
    exposes fixed 32-byte vendor HID IN/OUT reports on a distinct vendor usage page
    (`glove80-import/host-transport-hooks:rmk/src/hid.rs:61-84`;
    `glove80-import/host-transport-hooks:rmk/src/usb/mod.rs:245-249`).
  - Raw HID is important for the current browser path (WebHID) and avoids the native
    WinUSB/udev installation story. Bulk is attractive for the canonical RMK protocol,
    but is not a drop-in replacement for our shipped transport.
  - Alignment option: make raw HID an additional packet transport implementing a
    lower-level transport trait; do not ask upstream to abandon bulk.

- Non-HID GATT service reachable by Web Bluetooth:
  - **Architecturally aligned.** Both designs use a dedicated custom primary service,
    host writes, and keyboard notifications rather than HID-over-GATT.
  - Our branch makes the browser reason explicit and provides opaque variable-length
    chunks
    (`glove80-import/host-transport-hooks:rmk/src/ble/ble_server.rs:28-49`).
  - Upstream's first-CCCD readiness signal is the correct starting point. It should
    become session state with a cancellation-safe down edge and stale-TX cleanup,
    not only a one-shot wake signal.
  - UUID allocation, encryption policy, maximum characteristic value, and browser
    discovery filters must be made stable public contracts before the UI switches.

- Chunked framing above opaque pipes:
  - **Not met as layered today.** Our RMK patch moves only opaque USB reports and BLE
    chunks; framing/encode/decode belongs to the firmware protocol pump
    (`glove80-import/host-transport-hooks:rmk/src/vendor_transport.rs:1-14,46-61`).
  - Upstream's proposed `WireRx` returns a complete frame, so its USB/BLE adapters
    necessarily know the fixed RMK header and reassembly rules.
  - This can align cleanly if upstream introduces a lower layer such as
    `PacketRx::recv(&mut [u8]) -> len` / `PacketTx::send(&[u8])`, then implements
    the canonical RMK frame codec above it. Our protocol can consume the same packet
    layer with its own 32-byte-HID/ATT chunk framing.

- Protocol overlap:
  - Upstream already plans version/capability discovery, bootloader entry, state
    topics, and configuration commands; Glove80 should reuse those concepts where
    practical rather than maintain two unrelated management protocols forever.
  - Lighting overlays, per-cell TTL, split-half targeting, and application-specific
    telemetry still need an extension mechanism. The proposed `Cmd` table reserves
    no documented vendor/application range.
  - Ask for a stable extension range plus a handler/topic registration hook. That
    creates a plausible later migration from our complete private protocol to
    canonical RMK framing plus Glove80 commands.

### Recommended engagement strategy

- Decision: **propose changes now; keep our host-transport-hooks patch temporarily**.
- Treat the current upstream branch as a design discussion, not an integration
  target. Its latest transport API and wire format do not exist in code yet.
- Open one focused design thread with these asks:
  1. Confirm that `plan_custom_protocol.md`, not the postcard-rpc/COBS text in
     `plan.md` and the current ICD module docs, is the intended direction.
  2. Factor packet/chunk transport below RMK framing, with explicit session-ready
     state and teardown. Keep the proposed complete-frame `WireTx`/`WireRx` as a
     codec/server-facing layer if desired.
  3. Keep USB bulk as the high-throughput default, but permit a feature-gated
     32-byte vendor raw-HID adapter for WebHID and constrained deployments.
  4. Make the custom GATT transport browser-compatible by contract: non-HID service,
     stable UUIDs, write-without-response plus notify, negotiated MTU handling,
     first-CCCD readiness, down edge, and stale-frame clearing.
  5. Reserve a vendor/application command range and support externally supplied
     handlers/topics so Glove80 lighting can live above the canonical RMK protocol.
  6. Add browser wire vectors or a transport-independent conformance suite; the
     planned Rust `nusb`/`btleplug` client alone does not validate WebHID/Web Bluetooth.
- Offer small PRs only after the layering direction is accepted:
  - first, session/readiness primitives and packet traits;
  - second, the raw-HID adapter with fixed 32-byte reports;
  - third, browser-oriented transport vectors and documentation.
- Do not port Glove80 onto an unpublished plan API. Continue carrying the isolated
  opaque-pipe patch and application framing until an upstream implementation is
  merged and passes USB WebHID plus BLE Web Bluetooth qualification.

## Sequencing

1. **Open the split-message review first.** It is implemented code and the current
   behavior can lose the exact first messages Glove80 needs on reconnect. Lead with
   two reproducible cases: peripheral version published before subscription, and
   resync skipped after cancellation omits the down edge.
2. **Offer the smallest split PRs in dependency order:** deliverability-qualified
   stateful link watch + drop guards; removal of host-state gating; then source/local
   echo and enqueue-result improvements. Keep payload/capacity/docs changes separate.
3. **Open the host-service design conversation immediately after the split review.**
   Upstream has not implemented the transport plan, so packet-layer factoring,
   WebHID, Web Bluetooth, and extension namespaces are cheapest to settle now.
4. **What the fork keeps meanwhile:**
   - bounded opaque split queues, the stateful link watch, first-inbound-message
     deliverability gate, and cancellation drop guards;
   - the 26-byte Glove80 sync codec, full reconnect resync, TTL unset propagation,
     version announcement, and peripheral bootloader command;
   - the 32-byte vendor raw-HID interface, non-HID vendor GATT service, opaque
     packet queues, and application-owned chunk framing.
5. **Retirement gate:** remove a fork patch only after the corresponding upstream
   code is merged, rebased onto the RMK version we consume, and passes Glove80's
   reconnect, queue-pressure, both-half bootloader, WebHID, and Web Bluetooth tests.
