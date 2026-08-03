# Glove80 RMK development notes

## Experimental Go60 sibling

`crates/go60-rmk` is a sibling embedded workspace that reuses the Glove80
lighting/replication modules with board constants for 30 LEDs per half and a
40% hardware-output ceiling. Its hardware configuration is transcribed from
MoErgo's official `moergo-sc/zmk` Go60 board definitions.

The current port deliberately supports BLE split only. Do not claim feature
parity or release it as a ZMK replacement until RMK has a Cirque Pinnacle
driver, peripheral pointing forwarding is qualified, and the Go60's automatic
BLE/TRRS half-duplex split switching is implemented. Build and validate its
independent UF2 bundle with `just go60-firmware`; the official family IDs are
`0x9809B007` (left) and `0x980AB007` (right).

## Nested RMK repository

`dependencies/rmk` is an independent Git repository, not ordinary vendored
source. Inspect it explicitly before making changes:

```bash
git status --short --branch
git submodule status
git -C dependencies/rmk status --short --branch
git -C dependencies/rmk log --graph --oneline --decorate -20
```

The submodule tracks `colonelpanic8/rmk`'s `assembled` branch, which is
generated output. Never commit RMK changes onto it, and never commit only a
dirty submodule pointer or assume an uncommitted nested worktree is part of an
outer commit. RMK work goes on a topic branch and reaches this repository only
through a rebuild of the assembly (below).

## The assembled RMK line

`dependencies/rmk` is pinned to `colonelpanic8/rmk`'s `assembled` branch, which
is compiled by [fork-fold](https://github.com/colonelpanic8/fork-fold) from the
stack vendored as the `dependencies/rmk-assembly` submodule here (upstream
`colonelpanic8/rmk-assembly`). That repository's `manifest.toml` is the intent
(upstream `HaoboGu/rmk` `main` as the base, plus an ordered list of
`fork:fold/*` topic branches), `manifest.lock.json` is the fact (the OIDs and
tree hash of the last build), and `resolutions/` plus `patches/` carry the
tracked conflict resolutions and coherence fixups. Read
`dependencies/rmk-assembly/AGENTS.md` before operating on the stack; do not
work from memory of the workflow.

Consequences for work in this repository:

- The `assembled` history is a chain of `fork-fold: merge <branch>` commits. It
  is rewritten on every rebuild, so its commit IDs are not durable — only the
  lock's tree hash is. Do not base branches on it, cherry-pick from it, or
  merge it back into anything.
- Every RMK change belongs on the topic branch that owns it — currently
  `fold/macro-hooks`, `fold/split-reliability`, `fold/lighting-rynk`, and
  `fold/connection-selection`. Topic branches stay minimal diffs against
  upstream `main` so they remain upstreamable. Pick the branch by subject
  matter; if a change fits none of them, add a new topic branch and a manifest
  entry rather than widening an existing one.
- A change that only makes sense because of the full downstream stack is a
  cross-entry incoherence, not a topic commit. It belongs in the owning entry's
  `fixup` patch in the assembly repository.
- Older notes described `origin/master` as the composed line and named specific
  2026-07-21 tips (`6bcf2d94`, `228f9bcd`, `e4976e38`) as baselines. That line
  is superseded; `origin/archive/pre-fork-fold-master` preserves it. Local
  branch names are not an authority — inspect live remote refs and the manifest
  before choosing a base.

The loop for landing an RMK change here is:

1. Commit it on the owning `fold/*` branch in `dependencies/rmk` (or another
   checkout) and push that branch to `colonelpanic8/rmk`.
2. In `dependencies/rmk-assembly`, `fork-fold update <entry>` (or `update`
   alone to bump the base too), then `fork-fold build`, resolving any conflict
   the build stops on per that repository's AGENTS.md.
3. Push the rebuilt `assembled` branch to the fork, commit the assembly's
   manifest, lock, resolutions, and patches together inside
   `dependencies/rmk-assembly`, and push that commit — then commit the updated
   `dependencies/rmk-assembly` submodule pointer here.
4. Fetch in `dependencies/rmk`, check out the new `origin/assembled` tip, and
   move the outer pin as described below.

## Rynk protocol compatibility

- Keep existing postcard layouts and endpoint meanings stable; prefer new
  commands and new types.
- Do not mint `ProtocolVersion` values downstream. Discover downstream support
  through capability bits and/or command probing; older firmware must answer
  `UnknownCmd` safely.
- Regenerate wire values, wire frames, and the generated protocol reference
  for intentional protocol additions, while retaining the upstream-owned
  protocol version established by the normalization commit.

## Moving the outer pin

Before updating `dependencies/rmk` in this repository:

1. Format RMK and run its protocol snapshots, native Rynk tests/doctests,
   relevant `cargo nextest` suites, clippy/no-std checks, and WASM build/type
   checks.
2. Build both Glove80 firmware halves from this repository. Protocol and
   compositor changes require hardware qualification before release.
3. Push the owning `fold/*` branch and the rebuilt `assembled` branch to
   `colonelpanic8/rmk`, and commit the assembly repository's manifest, lock,
   resolutions, and patches. A pin to an unpushed or locally-built `assembled`
   commit is not reproducible.
4. Update the outer gitlink and any generated WASM/provenance artifacts in the
   same logical change. Keep the previous gitlink SHA in history as the
   rollback point; because `assembled` is rewritten on each build, that SHA is
   the only rollback target — recover it from this repository's history, not
   from the fork's reflog.
