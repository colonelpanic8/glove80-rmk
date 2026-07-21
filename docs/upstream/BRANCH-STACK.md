# RMK branch stack

`dependencies/rmk` follows the fork's release branch, `master`, and pins one
exact commit from it. A throwaway candidate branch is rebuilt from the
upstream Rynk base and independently reviewable topic branches, retains the
existing post-composition Rynk fixes, and is proposed as a fork PR against
`master`; the branch is promoted after the full composed tree passes
verification. The long-lived `glove80-rmk/integration` branch that previously
carried the composition was deleted on 2026-07-20; its final tip `52eb0dec`
remains a historical provenance ref only.

The current pin, `228f9bcd`, is the `glove80-rmk/scene-master-merge` tip
(fork PR #6): `master` plus the protocol-versioning normalization below. When
the PR merges, the pin moves to the resulting `master` commit.

## Branch graph

```text
HaoboGu/rmk:feat/rynk
├── glove80-rmk/split-app ──┐
│                           ├── glove80-rmk/lighting
├── glove80-rmk/runtime-hooks
├── glove80-rmk/rynk-usb-hid
└── glove80-rmk/build-info

feat/rynk + lighting + runtime-hooks + rynk-usb-hid
    └── octopus merge + build-info merge
        └── split-bootloader routing + unlock policy
            └── scene-lighting merge
                └── master
```

`glove80-rmk/lighting` includes `glove80-rmk/split-app`, so Git records the
split tip through the lighting parent rather than adding a redundant parent to
the octopus merge.

## Current composed set

| Ref | Commit |
| --- | --- |
| `HaoboGu/rmk:feat/rynk` | `8bfc94f715fbb9d68feb5d6f2dc1137800869f03` |
| `colonelpanic8/rmk:glove80-rmk/split-app` | `6f436cf103929760a3c03ff335cd713856fe7182` |
| `colonelpanic8/rmk:glove80-rmk/lighting` | `c7c090ca24070bcfd59f673d65d4418d8d8a7524` |
| `colonelpanic8/rmk:glove80-rmk/runtime-hooks` | `47922960a9d9ef1c3b088a655d03b986ec78badc` |
| `colonelpanic8/rmk:glove80-rmk/rynk-usb-hid` | `902c9d630d3b6d10afbd9fe8527a8806f648bf8b` |
| `colonelpanic8/rmk:glove80-rmk/build-info` | `8b5dd4d00e96e1cceed41d5a8977879c4879673c` |
| `colonelpanic8/rmk:glove80-rmk/scene-lighting` | `c7c090ca24070bcfd59f673d65d4418d8d8a7524` |
| `colonelpanic8/rmk:master` | `6bcf2d94f07f04669fb7e99045edb218faa79df0` |
| `colonelpanic8/rmk:glove80-rmk/scene-master-merge` (PR #6, pinned) | `228f9bcdfa012512f89a8bc1b48f2a3daa0a8d53` |

`glove80-rmk/lighting` was fast-forwarded to include the scene topic, so it
and `glove80-rmk/scene-lighting` currently share a tip. Earlier revisions of
this table recorded the pre-scene lighting tip `aac695ad`; treat the live
branch refs, not this table, as authoritative between refreshes.

## Refresh procedure

1. Fetch `HaoboGu/rmk` and the fork. Record the old base and branch tips.
2. Rebase `split-app`, `runtime-hooks`, and `rynk-usb-hid` independently onto
   the selected `origin/feat/rynk` tip.
3. Rebase `lighting` onto the refreshed `split-app` tip. Resolve only the
   documented overlap in split routing; do not copy Glove80 hardware policy
   into RMK.
4. Run `scripts/format_all.sh`, the full RMK feature matrix, and the Rynk host
   test/WASM/clippy sequence on the topic tips.
5. Create a throwaway candidate branch at the selected Rynk base, then compose
   the topics in one command:

   ```sh
   git merge --no-ff \
     -m 'glove80-rmk: integrate required upstream topics' \
     glove80-rmk/split-app \
     glove80-rmk/lighting \
     glove80-rmk/runtime-hooks \
     glove80-rmk/rynk-usb-hid
   ```

   Then merge `glove80-rmk/build-info` with the dedicated
   `glove80-rmk: integrate Rynk build info` merge commit and replay the
   split-bootloader routing and unlock-policy fixes already carried by
   `master`. Finally merge `glove80-rmk/scene-lighting`, which adds the
   runtime scene wire surface, engine/storage support, and native/WASM host
   APIs, with its own merge commit.

6. Push rewritten topic refs with `--force-with-lease`, then push the
   candidate branch and open a fork PR against `master`. After the composed
   tree passes verification, merge the PR (or fast-forward `master` to the
   exact candidate commit) and delete the candidate branch. Never update the
   superproject pin before the commit is reachable from a pushed fork ref.
7. Move `dependencies/rmk` to the new composed commit, regenerate
   `ui/src/vendor/rynk-wasm`, update its provenance hash, and run `make check`
   plus both release firmware builds.
8. Commit the new submodule gitlink only after every gate passes. The previous
   superproject commit remains the rollback pin.

## Protocol versioning rule

The fork never mints Rynk `ProtocolVersion` numbers. Version numbers belong to
upstream (`HaoboGu/rmk:feat/rynk`); a fork-minted minor would eventually
collide with upstream reusing it for different semantics. `CURRENT` stays at
the upstream base's value (v0.1 today), and every downstream feature must be
discoverable through capability negotiation, never a version check:

- lighting endpoints: `DeviceCapabilities.lighting_enabled`, then
  `GetLightingCapabilities.features`
- layer scenes: `LightingFeatureFlags::LAYER_SCENES` plus
  `GetLightingSceneStatus.capacity`
- `GetBuildInfo` / `PeripheralBootloaderJump`: per-command probing — firmware
  without them answers `UnknownCmd`, which hosts see as `Rejected(UnknownCmd)`

The handshake only rejects `major` mismatches, so firmware already in the
field that reported fork-minted minors (v0.2–v0.5) interoperates with
normalized hosts and vice versa.

When an upstream PR lands, remove that topic from the downstream set instead
of carrying a duplicate cherry-pick. If `feat/rynk` itself lands on `main`,
select the merged upstream commit as the new base and retarget any remaining
topic PRs.
