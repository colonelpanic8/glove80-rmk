# RMK downstream changes and upstreaming proposal

Status: historical planning record. The active 2026-07-20 topic branches,
upstream PRs, and integration recipe are documented in
[`PATCHES.md`](./PATCHES.md) and [`BRANCH-STACK.md`](./BRANCH-STACK.md).
The campaign below explains earlier decisions and must not be used as the
current branch or submission checklist.

## Execution board

Verified against GitHub and the local object database on 2026-07-19:

- upstream `main` is `a0ebb564`;
- Rynk PR #962 is open, ready for review, mergeable, and points to
  `aa3398ea`;
- upstream `feat/forward_split_message` exists at `4181965f` but is not yet a
  replacement for the Glove80 split contract;
- the generic shared-flash commits `ddad3826` and `ed6bd38d` are present and
  already based directly on `a0ebb564`, but no standalone fork branch has been
  published;
- the previously documented DFU race-fix commit/ref is absent from both the
  current local object database and the fork, so it must be reconstructed from
  `ISSUE-dfu-hash.md` and current upstream code; and
- no existing upstream issue or PR was found for shared application flash,
  the BLE split-DFU announcement race, or the Glove80 split requirements.

The concrete submission order is:

| Order | Work item | Starting point | Exit gate | Upstream action |
| ---: | --- | --- | --- | --- |
| 1 | BLE split-DFU hash announcement race | New `fix/dfu-split-hash-announce` branch from `a0ebb564`; reconstruct the minimal BLE-only readiness barrier and fake-driver tests | Focused tests plus the complete RMK test/format scripts pass; patch changes no serial behavior | File the prepared issue, then open one linked ready-for-review bug-fix PR |
| 2 | Shared application/RMK flash | New `feat/shared-flash` branch at `ed6bd38d` (commits `ddad3826`, `ed6bd38d`) | Re-run fake-flash, macro/config, docs, formatting, and full RMK matrix on the normal Nix host | Push the branch and open one ready-for-review PR, preserving the two-commit implementation/safety split |
| 3 | Split application forwarding | Compare `f84ac245` with upstream `4181965f`; do not publish `f84ac245` as a competing PR | Maintainer agrees on readiness, routing, queue-admission, and link-state semantics | Open a design issue or comment on the maintainer's eventual PR; contribute narrow deltas to their branch |
| 4 | Rynk application extension seam | Design against PR #962, not `main` | A vendor namespace and application handler can carry lighting/config/version/bootloader messages without a second transport stack | Propose the API on #962, then submit a small coordinated PR against `feat/rynk` if invited |
| 5 | nRF physical VBUS state | Fresh branch from the then-current upstream `main`; extract only the behavior now embedded in `8089822e` | RMK-native API/name, documented initial/edge semantics, tests or a minimal nRF example | Design issue first, isolated PR second |
| 6 | Downstream retirement | Glove80 repository and fork integration branch | Local CRC replaces the public-RMK CRC patch; Rynk/split upstream facilities replace compatibility transports only after hardware qualification | No upstream PR for CRC, retired keymap bridge, or the Glove80 lighting protocol itself |

Only one code PR should be opened at a time until maintainers respond. The DFU
fix is deliberately first: it is a small correctness change to an existing
feature, has a concrete hardware reproducer, introduces no public API, and
lets us establish review expectations before asking maintainers to evaluate
the larger shared-flash API.

### Additional Rynk follow-ups discovered during integration

These are separate from the fork-extraction campaign. They are gaps found by
actually integrating the open Rynk branch into a native CLI and browser UI.
No matching upstream issue or PR was found during the 2026-07-19 audit.

| Candidate | Proposed review unit | Dependency / disposition |
| --- | --- | --- |
| USB transport choice on nRF52840 | The PR's CDC-ACM OUT path received frames on Glove80, but CDC IN never completed a host URB. DTR, a host read pending before the request, direct libusb, interface reordering, and endpoint-number changes did not alter the failure. Reusing Rynk's existing 32-byte HID framing passed on the same hardware. | **Report the evidence on #962 first.** Propose an additive/configurable USB HID transport rather than globally replacing serial until CDC is reproduced on another nRF board. The downstream fork uses HID now. |
| Shared VIA action conversion | First move RMK's private `u16` VIA ↔ typed `KeyAction` converter into a shared, tested Rust module without behavior changes. In a second Rynk PR, expose that converter through `rynk-wasm`. | Avoids the downstream 323-line TypeScript mirror and gives existing VIA-based configurators a canonical migration adapter. Confirm module/crate placement with the maintainer before coding. |
| Raw vendor command client surface | Reserve an application/vendor request range and topic range; add raw request/response/topic methods to native and WASM clients while retaining one-request-in-flight and payload-size checks. | **Design issue after #962 settles.** This is the protocol half of extensibility and must not silently weaken command typing or lock policy. |
| Instance-owned firmware extension handler | Let a constructed `RynkService` delegate only reserved-range messages to an application-owned handler; make unlock policy explicit and keep handler state/session ownership unambiguous. | Separate PR after the namespace/client surface. Do not use global request/reply channels, which recreate the concurrency problem fixed in shared flash. |
| Split-targeted reboot/bootloader | Add a new additive endpoint that selects central or peripheral instead of changing the frozen `BootloaderJump = ()` request. Require physical unlock and return delivery failure. | Depends on target-aware upstream split routing and admission reporting; do not build it on the current broadcast-only forwarding branch. |
| Application build identity | Add an additive endpoint for the keyboard application's semver/build id, distinct from `DeviceInfo.rmk_version`; later extend it to query split targets. | Useful for detecting half-flashed keyboards, but lower priority than split routing and the extension seam. Do not append fields to existing response types within protocol major 0. |
| Standard RMK lighting endpoints | Implement the existing `lighting_enabled` capability TODO around RMK's eventual standard lighting state and actions. | Coordinate with the Rynk author. Keep Glove80 overlays, gates, transactional records, and compositor semantics behind the vendor extension rather than declaring them RMK standard behavior. |
| WASM package distribution | Publish/version the generated `rynk-wasm` package so consumers do not vendor generated JS, declarations, and WASM binaries. | Useful after the protocol and PR layout stabilize; not a merge blocker for #962. |

The local five-second connection timeout remains application policy, not an
upstream requirement: Rynk deliberately relies on the caller to supervise and
cancel a session. DTR was investigated and disproved as the Glove80 failure;
do not submit the abandoned DTR change.

### Glove80 hardware qualification result

Both halves were flashed and verified on 2026-07-19. The central and
peripheral reported the same application build, the split link remained
connected, product-protocol lighting capabilities and ping passed, and Rynk
read all eight 6×14 keymap layers over the new USB HID link. The full read also
found and fixed missing VIA compatibility mappings for QMK output-selection
and debug keycodes. The native CLI's 89 tests, Lightbench's 184 tests and
production build, four focused HID loopbacks, and the 659-test RMK Rynk feature
set all pass.

Browser code now uses the same WebHID link for wired and BLE Rynk. Its build and
unit tests pass; an interactive browser chooser and BLE-only hardware session
remain manual follow-ups, not blockers for the wired firmware qualification.

## Recommendation

Do **not** submit the `glove80` integration branch as one PR. It combines
independent facilities, a bug fix, two convenience changes, and work that now
overlaps active upstream development. Keep that branch as the reproducible
firmware pin and upstream only the smallest generally useful pieces.

| Change | Why Glove80 needed it | Recommendation | Priority |
| --- | --- | --- | --- |
| BLE split-DFU hash announcement fix | The peripheral could silently lose its proactive hash notification before the BLE subscription was ready. | **Upstream now:** issue plus one focused bug-fix PR. | P0 |
| Shared application/RMK flash access | Runtime configuration must share nRF flash safely with RMK's storage task and SoftDevice/MPSL. | **Upstream now:** the existing two-commit feature branch. | P0 |
| Host protocol transports | Glove80 lighting/config needs a product-specific USB vendor-HID and BLE path. | **Keep downstream for now:** Rynk now owns keymaps, but has no application-command extension seam; propose one only with a concrete migration design. | P1 |
| Keymap operations | The configurator needs ownership, persistence, typed actions, and readback. | **Retired:** production firmware, CLI, and browser now use Rynk; do not PR the old bridge. | Done |
| Split application messages and link state | Lighting sync, firmware identity, resync, and remote bootloader control must cross the split link. | **Engage, then upstream deltas:** work with `feat/forward_split_message`; retain ours until it meets the requirements below. | P1 |
| nRF physical-VBUS state | Conditional lighting needs charging/power presence, which is different from USB enumeration. | **Possible separate PR:** first extract, rename, document, and test it. | P2 |
| Public CRC-32 module | The downstream config store reused RMK's existing implementation. | **Keep out of upstream:** use a downstream CRC implementation/dependency instead. | P3 |
| Nix development shell | Reproducible RMK formatting and tests. | **No action:** this capability is already on upstream `main`. | Done |

The practical upstream campaign is therefore two definite code PRs, targeted
collaboration on two active upstream designs, one optional later PR, and one
downstream dependency cleanup. That is substantially easier to review and
less likely to create parallel APIs RMK then has to support.

## What the fork added

### 1. Split application-message channel

Branch: `split-app-messages`, commit `f84ac245`; active module:
`rmk::split_app`.

The fork adds opaque, bounded, bidirectional application payloads to RMK's
existing split transport. The Glove80 firmware uses them for:

- lighting snapshots and deltas;
- reconnect/resynchronization requests;
- peripheral firmware/build identity;
- shared runtime state; and
- a magic-guarded request to enter the peripheral bootloader.

It also adds a stateful split-link watch. For BLE, "up" means that application
traffic is actually deliverable, not merely that a low-level connection object
exists. Drop guards ensure cancellation publishes the corresponding down edge.
Inbound queues are deliberately loss-tolerant; state owners recover by sending
snapshots after reconnect or an explicit resync request.

This is generic RMK functionality, but it overlaps the upstream
`feat/forward_split_message` branch. That upstream design offers typed events
and useful macro integration, while the Glove80 implementation currently has
the stronger operational contract. Before Glove80 can migrate, upstream's
design needs:

1. a stateful, deliverability-qualified split-session signal;
2. no gating on whether the keyboard itself has a USB/BLE host connection;
3. explicit source/destination and direction semantics for multi-peripheral
   keyboards;
4. a way to suppress local echo for commands such as remote bootloader entry;
5. observable queue-admission failure or clearly documented loss semantics;
6. configurable capacity sufficient for the current 26-byte payload and resync
   burst; and
7. tests for BLE readiness, cancellation, reconnect, and queue saturation.

**Disposition:** do not open our whole branch as a competing PR yet. Contact
the author/maintainer of `feat/forward_split_message`, attach the requirement
matrix above, and offer small PRs against that design. Good review units would
be (a) session/readiness tracking, (b) removing host-state coupling, and (c)
routing/admission semantics. If the upstream branch becomes inactive, the
single-commit Glove80 branch is a reasonable fallback PR, with its current
single-peripheral assumptions documented explicitly.

### 2. Application host transports

Branch: `host-transport-hooks`, commit `722ddcdf`; active module:
`rmk::vendor_transport`.

The fork provides application-owned request and response queues over:

- a dedicated 32-byte USB vendor raw-HID interface, separate from Vial; and
- a custom non-HID BLE GATT service with encryption and negotiated-MTU-aware
  payload handling.

RMK owns the physical transports, but the application owns framing, commands,
responses, events, and compatibility. Glove80 uses this for its transactional
configuration and lighting protocol.

This now overlaps Rynk substantially. The open
[Rynk PR #962](https://github.com/HaoboGu/rmk/pull/962) includes a 32-byte vendor
HID transport, native BLE GATT transport, BLE HID/WebHID support, session
lifecycle, full-duplex communication, bulk transfer, host libraries, and a
WASM bridge. A second standalone transport abstraction would be expensive for
RMK to maintain.

The integration is now implemented on `glove80-rynk`: firmware keymaps use
Rynk; the qualified native CLI uses USB HID/native BLE GATT; Lightbench uses
WebHID. The Glove80 product protocol remains in parallel for
lighting, transactional lighting configuration, version, and bootloader.

The remaining question is extension, not transport. Rynk's command table and
dispatch are currently closed around RMK-owned commands, while Glove80 has
application-specific lighting and configuration commands. Its browser BLE
path also needs real Linux/BlueZ hardware qualification: Rynk deliberately
uses BLE WebHID for browsers, whereas our implementation uses Web Bluetooth
against a custom GATT service.

**Disposition:** do not submit `host-transport-hooks`. Instead:

1. qualify the implemented integration over USB, native BLE, and browser BLE
   on real hardware;
2. propose a reserved vendor/application command namespace plus handler/topic
   registration if Rynk still has no supported extension seam;
3. propose an optional Web Bluetooth GATT adapter only if BLE WebHID fails the
   Glove80/Linux test matrix; and
4. only retire the local transport branch after Glove80's product commands
   have a supported Rynk extension path.

Any Rynk contribution should be a narrowly scoped PR against the Rynk branch
while #962 is open, coordinated with its author, rather than a PR against
`main` that duplicates the feature.

### 3. Shared flash

Branch: `shared-flash`, commits `ddad3826` and `ed6bd38d`; active module:
`rmk::shared_flash`.

RMK storage and the Glove80 runtime configuration store both need access to
the same nRF internal flash. Ordinary direct flash access is unsafe while the
radio stack is active. The branch adds an opt-in `shared_flash` feature that:

- constructs one radio-safe `nrf_mpsl::Flash` instance;
- serializes RMK and application access with an async mutex;
- exposes one uniquely acquired `SharedFlash` client;
- permanently confines that client to a validated application window;
- uses checked address arithmetic and chunked operations;
- erases page by page;
- reports explicit errors; and
- includes fake-flash and macro/configuration tests plus feature documentation.

This is generic, self-contained, and has no known upstream replacement.

**Disposition:** submit it essentially as prepared. Preserve the two commits:
the feature implementation followed by the API-safety hardening. Rebase it on
the then-current upstream `main`, remove any Glove80 framing from commit/PR
text, run RMK's complete formatter/test scripts, push `shared-flash` to the
fork, and open a ready-for-review PR. Do not fold the downstream false-positive
`[dfu]` configuration warning into this PR; that can be a separate issue if it
persists on a minimal upstream example.

### 4. Keymap operation bridge

Branch: `keymap-ops`, commit `b0b89891`; historical module:
`rmk::keymap_ops` (absent from the production integration tree).

The bridge sends one external Get/Set operation at a time through the Vial
task. That preserves a single keymap owner and reuses RMK's VIA/Vial
conversion, validation, persistence, and canonical readback rather than
modifying the keymap independently.

Rynk now implements individual and bulk keymap Get/Set commands with the same
ownership and persistence goals. Upstreaming a second global request/result
channel would duplicate that API.

**Disposition:** migration complete; do not PR the current branch. If
application firmware later needs keymap
access without going through Rynk, first document that concrete caller and
propose a shared `KeymapService` or context API that both Vial and Rynk can
use. An issue is justified only if that non-Rynk use case survives the
migration.

### 5. BLE split-DFU hash announcement race

Branch: `fix/dfu-split-hash-announce`, commit `9375920e`.

The split peripheral proactively announces its firmware hash. On BLE, the
write could occur before the central had subscribed to the characteristic, so
the notification was silently lost. The patch:

- waits until the first successfully decoded central message before the BLE
  announcement;
- retains immediate announcement for serial transports;
- marks the announcement complete only after a successful write; and
- tests pre-read behavior, read failure, single announcement, retry after
  write failure, and serial behavior.

**Disposition:** upstream this as the first submission because it is a small,
isolated correctness fix to an existing upstream feature. File the prepared
reproducer as an issue, then immediately open the one-commit PR and link them.
Rebase before submission and run both the focused tests and RMK's full test
script.

### 6. Public CRC-32 availability

Integration-only commit: `e26faf69`.

This removes the `dfu_split` feature gate from RMK's existing CRC-32 module so
the Glove80 configuration format can reuse it. It adds no RMK behavior and
makes an internal utility public solely for downstream convenience.

**Disposition:** do not upstream it. Add a small downstream CRC implementation
or an appropriate `no_std` dependency, then remove this fork commit. Revisit
only if RMK itself develops multiple non-DFU consumers and wants to declare
CRC-32 part of its supported public API.

### 7. nRF physical-VBUS state

Integration-only commit: `8089822e`; active API:
`rmk::usb::USB_VBUS_DETECTED`.

The change wraps Embassy's nRF `VbusDetect`, publishes physical VBUS edges via
an Embassy watch, and makes generated central and split-peripheral USB drivers
use the wrapper. Glove80 consumes this as charging/power state for conditional
lighting. This is intentionally different from RMK's USB host/configuration
state: a charge-only cable can supply VBUS without enumerating HID.

The capability is plausibly useful to other battery-powered nRF keyboards,
but the current integration commit still contains patch markers, has an
application-specific commit subject, exposes a global directly, and has no
dedicated tests or user documentation.

**Disposition:** possible later PR, never bundled with another feature. First
create a branch from current upstream `main`, choose an RMK-native name/API,
document initial value and edge semantics, add wrapper tests if Embassy's trait
can be faked, and provide an nRF example. A short design issue should confirm
whether maintainers prefer a public watch, a connection/power event, or a
generated task handle. This is useful but not blocking once the fork remains
pinned, so it should follow the P0 and Rynk work.

### 8. Nix development environment

The upstream repository now contains the Nix flake development environment
that was needed to run the same toolchain and checks reproducibly.

**Disposition:** complete upstream; do not carry or submit another change.

## Things that belong only in Glove80

The following are enabled by RMK extension points but are product policy or
wire formats, not RMK features. They should remain in this repository:

- the split synchronization codec and its message IDs;
- lighting layer composition, snapshots, deltas, and reconnect policy;
- the remote-bootloader magic value and physical `User12` routing;
- the transactional runtime-configuration record format and flash layout;
- Glove80 host command meanings, events, and compatibility rules;
- the current Glove80 build-identity wire layout and half-mismatch policy (a
  future additive Rynk application-build endpoint may reuse the requirement,
  not this payload); and
- keymap batching and configurator UI behavior.

Keeping this boundary explicit will prevent otherwise generic PRs from being
reviewed as Glove80 product changes.

## Proposed campaign

### Stage 0: preserve both integration points

Use the published `glove80-rynk` branch as the production candidate and retain
`glove80`/`8089822e` as the pre-Rynk rollback while #962 is open. Never use
either merge branch as a PR head. Before each
submission, recreate or rebase the corresponding feature branch from current
upstream `main`; currently only the integration branch is published on the
fork, so each PR branch still needs to be pushed.

### Stage 1: submit the two independent changes

1. File the split-DFU issue and open `fix/dfu-split-hash-announce` as a
   one-commit PR.
2. After the first PR is in review, open `feat/shared-flash` as its existing
   two-commit series.

These PRs should not depend on each other. Submit them sequentially to keep
maintainer attention focused, but continue to rebase each directly on
upstream `main`.

### Stage 2: qualify and contribute to Rynk

1. Report the nRF52840 CDC-IN failure and successful USB-HID qualification on
   #962 with the compact reproduction matrix above.
2. Ask whether upstream wants USB HID as a selectable transport, an nRF
   default, or a CDC bug investigation before code is submitted.
3. Ask where the canonical VIA/action converter should live; then submit the
   behavior-preserving shared-Rust refactor and WASM exposure as separate PRs.
4. Report the remaining results on #962 and propose the reserved vendor
   namespace/client surface before implementing a firmware handler.
5. Submit only the smallest extension-hook or transport-adapter PRs the Rynk
   maintainers agree to accept.
6. Keep keymap access on Rynk. Delete the local host transport only if Rynk
   gains an accepted application-command seam covering the product protocol.

### Stage 3: converge on upstream split events

Coordinate around `feat/forward_split_message`, starting with the
deliverability/session semantics. Migrate only after the requirements in
section 1 are covered and tested. If that work does not progress, ask whether
maintainers prefer the existing Glove80 implementation as a smaller fallback
before opening a competing PR. Add targeted peripheral reboot/bootloader
control only after destination routing and queue-admission failure are part of
that contract.

### Stage 4: reduce the remaining fork

Replace the CRC dependency downstream. Decide whether the demonstrated VBUS
use case merits a polished PR. Once accepted upstream features reach `main`,
rebuild the `glove80` integration branch from upstream plus only the still-open
deltas and update the submodule pin.

## Gate for every RMK PR

Before opening or refreshing a PR:

- rebase the feature branch on the current upstream `main` without merge
  commits;
- ensure each commit is independently coherent and conventionally named;
- remove all `GLOVE80 PATCH` markers and product-specific terminology;
- add public API documentation, configuration documentation, and an example
  where appropriate;
- test default and feature-enabled configurations, including macro expansion
  where code generation is touched;
- run `nix develop --command sh scripts/format_all.sh` and
  `nix develop --command sh scripts/test_all.sh`;
- build both Glove80 halves against the candidate branch as downstream
  integration proof; and
- open the PR ready for review with motivation, behavior/contract, limitations,
  test evidence, and any related issue/branch linked.

## Desired end state

The long-term target is not a zero-diff fork at any cost. It is a small,
auditable integration branch containing only Glove80-specific policy and any
temporarily unmerged generic work. If the plan succeeds, shared flash and the
DFU fix live on upstream `main`, host/keymap behavior uses Rynk, split messaging
uses the upstream event path with the required reliability semantics, CRC is
downstream-owned, and VBUS is the only optional generic delta still under
discussion.
