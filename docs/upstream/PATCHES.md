# RMK dependency inventory

Refreshed on 2026-07-20. The active firmware depends on six generic RMK
topics. They are published as stable branch names on `colonelpanic8/rmk`,
with the first four proposed upstream as ready PRs against
`HaoboGu/rmk:feat/rynk`, and composed into the release branch, `master`.

| Topic | Fork branch | Current tip | Upstream PR | Active API |
| --- | --- | --- | --- | --- |
| Split application messages | `glove80-rmk/split-app` | `6f436cf1` | [#984](https://github.com/HaoboGu/rmk/pull/984) | `rmk::split_app` |
| Topology-aware lighting | `glove80-rmk/lighting` | `c7c090ca` | [#987](https://github.com/HaoboGu/rmk/pull/987) | `rmk::lighting`, renderer replica snapshots, Rynk lighting, Vial RGB Matrix |
| Macro runtime hooks | `glove80-rmk/runtime-hooks` | `47922960` | [#985](https://github.com/HaoboGu/rmk/pull/985) | custom `HostService`, `Runnable` processors |
| Rynk USB HID | `glove80-rmk/rynk-usb-hid` | `902c9d63` | [#986](https://github.com/HaoboGu/rmk/pull/986) | fixed-report Rynk USB transport |
| Rynk build identity | `glove80-rmk/build-info` | `8b5dd4d0` | — | application build label discovery |
| Runtime lighting scenes | `glove80-rmk/scene-lighting` | `c7c090ca` | — | durable per-layer scenes, Rynk scene transactions, native/WASM host APIs |

The lighting branch is intentionally stacked on the split-message branch.
This lets the lighting PR demonstrate split behavior while keeping the split
primitive reviewable on its own. After #984 lands, rebase the lighting branch
to remove the already-merged commit.

The `master` tip is `6bcf2d94`. Its ancestry contains an octopus merge over
upstream Rynk tip `8bfc94f7`, with lighting (including split messages), runtime
hooks, and USB HID as its non-base parents, followed by the build-info merge
and the existing split-bootloader routing and unlock-policy fixes, then the
runtime scene-lighting merge. The superproject pins the full commit rather
than following the moving branch implicitly. The current pin is `228f9bcd`,
the `glove80-rmk/scene-master-merge` tip proposed as fork PR #6: `master` plus
one commit reverting the fork-minted `ProtocolVersion` minors to the upstream
base's v0.1 (see the versioning rule in
[BRANCH-STACK.md](./BRANCH-STACK.md)). Once PR #6 merges, the pin moves to the
resulting `master` commit.

`glove80-rmk/lighting` was fast-forwarded to include the scene commits, so
the lighting and scene rows currently share tip `c7c090ca`. Earlier revisions
of this table recorded the pre-scene lighting tip `aac695ad`.

## Deliberately absent downstream patches

The current firmware does not use the retired vendor transport, shared-flash,
keymap-operation bridge, public RMK CRC helper, or nRF VBUS hook from the older
`glove80` / `glove80-rynk` campaign. Those branches remain historical rollback
and provenance records; they are not inputs to the current firmware and must
not be merged into the new integration branch.

See [BRANCH-STACK.md](./BRANCH-STACK.md) for the refresh procedure and exact
composition rules.
