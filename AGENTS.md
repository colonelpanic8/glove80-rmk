# Glove80 RMK development notes

## Nested RMK repository

`dependencies/rmk` is an independent Git repository, not ordinary vendored
source. Inspect it explicitly before making changes:

```bash
git status --short --branch
git submodule status
git -C dependencies/rmk status --short --branch
git -C dependencies/rmk log --graph --oneline --decorate -20
```

Commit RMK changes inside `dependencies/rmk` first. Push that commit to a
durable ref in `colonelpanic8/rmk` before committing the outer gitlink. Never
commit only a dirty submodule pointer or assume an uncommitted nested worktree
is part of an outer commit.

## The composed RMK line

The fork's `origin/master` is the composed Glove80/Rynk/lighting line. It is
not equivalent to upstream `main`, and the old local/remote
`glove80-rmk/integration` branch is stale. Local branch names are not an
authority: inspect the live commit graph and remote refs before choosing a
base.

The relevant 2026-07-21 history has two sibling tips with different status:

- `6bcf2d94` is the then-current composed `origin/master` tip.
- `228f9bcd` (`origin/glove80-rmk/scene-master-merge`) is its child and removes
  downstream-minted protocol versions; the outer repository was pinned here.
- `e4976e38` (`origin/wip/lighting-docs-20260721`) is a different child of
  `6bcf2d94` containing uncommitted-design material that is intentionally kept
  out of the composed branch.

Use `228f9bcd` as the baseline for new protocol and firmware work. Do not merge
or cherry-pick `e4976e38` unless the user explicitly asks to commit those
documents; the files may exist locally as untracked notes. Put new
Glove80-dependent patches above the normalized composed tip. Prefer additive
overlay commits for changes that depend on the full downstream stack. Keep
generic upstream candidates independently reviewable before composing them
into this line.

## Shared Rynk protocol coordination

Read the append-only [`PROTOCOL_NEEDS.md`](PROTOCOL_NEEDS.md) before allocating
commands, topics, feature bits, or changing shared payloads. Do not rewrite or
remove another task's entry. Coordinate overlapping requirements in one
endpoint-table change instead of landing colliding WIP branches.

- Keep existing postcard layouts and endpoint meanings stable; prefer new
  commands and new types.
- Do not mint `ProtocolVersion` values downstream. Discover downstream support
  through capability bits and/or command probing; older firmware must answer
  `UnknownCmd` safely.
- Regenerate wire values, wire frames, and the generated protocol reference
  for intentional protocol additions, while retaining the upstream-owned
  protocol version established by the normalization commit.
- Treat worktrees named in `PROTOCOL_NEEDS.md` as patch/reference sources only.
  Their dirty gitlinks and outer branches are not landed work and should not be
  merged wholesale.

## Moving the outer pin

Before updating `dependencies/rmk` in this repository:

1. Format RMK and run its protocol snapshots, native Rynk tests/doctests,
   relevant `cargo nextest` suites, clippy/no-std checks, and WASM build/type
   checks.
2. Build both Glove80 firmware halves from this repository. Protocol and
   compositor changes require hardware qualification before release.
3. Push the nested RMK commit to a durable fork ref.
4. Update the outer gitlink and any generated WASM/provenance artifacts in the
   same logical change. Keep the previous gitlink SHA in history as the
   rollback point.
