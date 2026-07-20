# RMK upstreaming verification summary

The first verification pass below was completed on 2026-07-19 without remote
mutations. Since then, the feature branches were consolidated on upstream
`a0ebb564`, the `glove80` integration branch was pushed to
`colonelpanic8/rmk`, and this repository first cut over at `8089822e`. It now
uses the Rynk integration `glove80-rynk` at `67f444b2`, while `8089822e`
remains the rollback. `PATCHES.md` is the authoritative current inventory; the older
branch hashes and test narrative retained here are provenance.

## Document index

- `PATCHES.md` — all 49 current `GLOVE80 PATCH` marker occurrences, their fork
  disposition, the crc32 keep-local exception, and five round-2 sites.
- `RENAME-MAP.md` — verified old-to-new names and snapshot equivalence verdict.
- `PR-split-app-messages.md` — proposed upstream PR body.
- `PR-host-transport-hooks.md` — proposed upstream PR body.
- `PR-shared-flash.md` — proposed upstream PR body.
- `PR-keymap-ops.md` — proposed upstream PR body.
- `BUG-trouble-host.md` — silent-success notification bug at trouble-host
  `c21b1239` and proposed `NotSubscribed` result.
- `BUG-rmk-dfu-hash.md` — RMK proactive split-DFU hash startup race and proposed
  first-inbound-message gate.
- Historical branch-test totals are summarized below. The disposable
  `rmk-upstreaming/test-logs` copies were removed with that workspace after
  consolidation; current tests are reproducible from the fork flake.

## Branches

| Role | Branch | Tip | Base / intended use |
| --- | --- | --- | --- |
| Feature | `split-app-messages` | `8f80acb5` | historical base `1156f82b` |
| Feature | `host-transport-hooks` | `ffe05dee` | historical base `1156f82b` |
| Feature | `shared-flash` | `66fb9a62` | historical base `1156f82b` |
| Feature | `keymap-ops` | `c0e8f60d` | historical base `1156f82b` |
| Integration | `glove80` | `165e4720` | merges all four, plus keep-local crc32 ungating |
| PR source | `split-app-messages-main` | `565a5d05` | rebased on upstream `main` `5feaf8b1` |
| PR source | `host-transport-hooks-main` | `a85d4265` | rebased on upstream `main` `5feaf8b1` |
| PR source | `shared-flash-main` | `9c54e35e` | rebased on upstream `main` `5feaf8b1` |
| PR source | `keymap-ops-main` | `cb0e03a5` | rebased on upstream `main` `5feaf8b1` |

The table above is historical provenance, not the current PR campaign. Do not
publish `glove80` or `glove80-rynk` as upstream proposals. The current
per-change dispositions and ordering are in `RMK-UPSTREAMING-PROPOSAL.md`.

## Rynk integration addendum

The active fork branch `glove80-rynk` contains:

- `b13e6dd7` — merge upstream `feat/rynk` into the Glove80 integration;
- `4136f2ee` — retire the downstream keymap-operation bridge; and
- `67f444b2` — cover Rynk plus shared flash in the RMK feature matrix.

Firmware and both host clients now use Rynk for keymaps. The Glove80 vendor
transport remains for product-specific lighting/config/version/bootloader
commands until Rynk has an accepted application extension seam.

## Test results

| Branch | Historical status |
| --- | --- |
| `split-app-messages` | **PASS** — all 17 harness commands |
| `host-transport-hooks` | **PASS** — all 17 harness commands |
| `shared-flash` | **PASS** — all 17 harness commands |
| `keymap-ops` | **PASS** — all 17 harness commands |
| `glove80` | **PASS** — all 17 harness commands |

The initial `run-tests.sh` invocation could not execute because the file lacks
an executable bit. Invoking it with Bash then hit sandbox infrastructure:
first the read-only Nix user cache, then the unavailable network/flake registry,
and the Nix daemon socket is not permitted. The valid runs therefore executed
the script's exact 15 nextest and 2 doctest cargo commands directly with:

- `RUSTUP_TOOLCHAIN=1.97.0`
- `RUSTFLAGS="-C link-self-contained=no"`
- the already installed Nix-store GCC, LLD, and cargo-nextest 0.9.140 binaries
- writable `/tmp` overrides for both Cargo target and global build directories

Each valid log ends in `ALL GREEN` and contains no `!!! FAILED` line.

## Equivalence verdict

**PASS for the requested snapshot, with one intentional keep-local delta.** Fork
branch `glove80` carries the same functional patch set as monorepo snapshot
`b1ae2b2c` after the verified deglove renames and marker/prose generalization.
The only extra fork change is commit `165e4720`, which ungates `crc32` for
Glove80 runtime-config headers and is not intended upstream. A normalized
comment-free/rename-aware tree comparison returns no unexplained delta. See
`RENAME-MAP.md` for the detailed evidence.

The snapshot vendored tree object is
`e323cec0abfbdb4a635e7b0a542df65ec762c871`. Current monorepo HEAD `2bac75ac`
has vendor tree `7f868cc1c57f72e82ff2f5962f0bf29fe736c59f` because of the round-2 VBUS hook.

## Round 2 / post-snapshot work

`git log b1ae2b2c..HEAD -- rmk/vendor/rmk` returns exactly one commit:

- `7a56c997` — `firmware: wire conditional lighting state`.

Its vendored-RMK delta is a generic nRF VBUS state hook:

- `rmk/vendor/rmk/rmk/src/usb/mod.rs:4,36,90`
- `rmk/vendor/rmk/rmk-macro/src/codegen/chip/comm.rs:68`
- `rmk/vendor/rmk/rmk-macro/src/codegen/split/peripheral.rs:357`

The current vendor directory therefore has 49 marker occurrences in 16 files:
the snapshot's 44 plus these five. This commit is the complete **round-2 RMK
list**. It was not ported or tested in the fork.

For clarity, other post-snapshot commits include downstream work outside the
vendor tree:

- `755d008d` — compositor per-record gates and firmware-state conditions.
- `c7e53891` — compositor sync adds USB-connected state and per-record gate
  messages; its only path is `crates/glove80-compositor/src/sync.rs`.
- `ca61cc4a` — qualification documentation.
- `f0cf7849` — protocol support for conditional-lighting gates.
- `dcfb7003` — CLI support for conditional-lighting gates.
- `2bac75ac` — gated-status examples.

Those downstream changes should not be ported onto the fork feature branches as
RMK patches. Only `7a56c997` touched the vendor tree, and it remains explicitly
deferred to round 2.

## Publish runbook

The fork and integration branch now exist. The remaining manual publication
work is to push the final generic feature branches as needed and open the
upstream issue/PR waves described in `docs/upstream-alignment.md`.

1. Re-fetch upstream and confirm upstream `main` has not advanced in a way that
   requires another rebase. The four `-main` branches currently target
   `5feaf8b1`.
2. Push only one feature branch at a time, beginning with
   `split-app-messages-main`. Never push `glove80` as an upstream PR branch.
3. Open the split-app-messages PR first using
   `PR-split-app-messages.md`. This change touches the shared split driver and
   peripheral loop, so landing/reviewing it first reduces adjacent-code churn
   for later work.
4. After it lands—or after the maintainer establishes the desired base—refresh
   the remaining `-main` branches onto the then-current upstream `main`, rerun
   the harness, and push/open PRs in this order:
   `host-transport-hooks-main`, `shared-flash-main`, `keymap-ops-main`.
5. Use the corresponding `PR-*.md` body for each PR. Keep the four proposals
   logically separate; do not include the crc32 ungating commit.
6. File the trouble-host and RMK DFU reports separately with their respective
   upstreams. The RMK report can cite split-app-messages' first-inbound-message
   gate as an implementation model without making the feature PR depend on DFU
   changes.

### TL;DR

Create `colonelpanic8/rmk`; publish the four `-main` branches only; submit
`split-app-messages-main` first; then refresh, retest, and submit host transport,
shared flash, and keymap ops. Keep `glove80` and its crc32 commit local.

## Workspace note

The disposable `/home/imalison/Projects/rmk-upstreaming` workspace was removed
after its durable documents moved here and its branch work was consolidated in
`/home/imalison/Projects/rmk`. The fork repository and this monorepo are now the
two authoritative local workspaces.
