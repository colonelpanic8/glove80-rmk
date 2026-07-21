# RMK branch stack

`dependencies/rmk` follows the fork's release branch, `master`, and pins one
exact commit from it. A candidate integration branch is rebuilt from the
upstream Rynk base and independently reviewable topic branches, retains the
existing post-composition Rynk fixes, and is promoted to `master` after the
full composed tree passes verification.

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
| `colonelpanic8/rmk:glove80-rmk/lighting` | `aac695ad5438d2b987f6465d972f647e0d567eab` |
| `colonelpanic8/rmk:glove80-rmk/runtime-hooks` | `47922960a9d9ef1c3b088a655d03b986ec78badc` |
| `colonelpanic8/rmk:glove80-rmk/rynk-usb-hid` | `902c9d630d3b6d10afbd9fe8527a8806f648bf8b` |
| `colonelpanic8/rmk:glove80-rmk/build-info` | `8b5dd4d00e96e1cceed41d5a8977879c4879673c` |
| `colonelpanic8/rmk:master` | `27b8bf38444acddfe3d7d6a408ac0f2b102103cb` |

## Refresh procedure

1. Fetch `HaoboGu/rmk` and the fork. Record the old base and branch tips.
2. Rebase `split-app`, `runtime-hooks`, and `rynk-usb-hid` independently onto
   the selected `origin/feat/rynk` tip.
3. Rebase `lighting` onto the refreshed `split-app` tip. Resolve only the
   documented overlap in split routing; do not copy Glove80 hardware policy
   into RMK.
4. Run `scripts/format_all.sh`, the full RMK feature matrix, and the Rynk host
   test/WASM/clippy sequence on the topic tips.
5. Recreate `glove80-rmk/integration` at the selected Rynk base, then compose
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
   split-bootloader routing and unlock-policy fixes already carried by the
   integration branch.

6. Push rewritten topic refs with `--force-with-lease`, then push the rebuilt
   integration ref. After the composed tree passes verification, fast-forward
   `master` to that exact commit. Never update the superproject pin before the
   remote `master` ref is reachable.
7. Move `dependencies/rmk` to the new integration commit, regenerate
   `ui/src/vendor/rynk-wasm`, update its provenance hash, and run `make check`
   plus both release firmware builds.
8. Commit the new submodule gitlink only after every gate passes. The previous
   superproject commit remains the rollback pin.

When an upstream PR lands, remove that topic from the downstream set instead
of carrying a duplicate cherry-pick. If `feat/rynk` itself lands on `main`,
select the merged upstream commit as the new base and retarget any remaining
topic PRs.
