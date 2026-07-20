# Repository extraction record

This repository was created as an independent local clone of
`/home/imalison/Projects/glove80-config` on 2026-07-19.

## Source pins

- Source repository commit: `80de42394459de885617967cd0fa6ca6d01ac89f`
- RMK submodule commit: `75d0edc315ea759ba667deb4148e389bc016120c`
- RMK submodule URL: `https://github.com/colonelpanic8/rmk`
- RMK update branch: `glove80-rynk`

The clone was created with independent Git objects. Its clone-time local
remote was removed, and no network remote was added.

The source working tree also contained two untracked planning documents. The
repository-design document is imported later with its provenance recorded;
the `DO-NOT-COMMIT` agent brief is deliberately not imported.

## Imported paths

- `rmk/glove80` became `firmware/glove80-rmk`.
- `rmk/glove80-compositor` became `crates/glove80-compositor`.
- `protocol/glove80-host-protocol` and all five active golden-vector files
  became `crates/glove80-host-protocol`.
- `tools/glove80-control`, `ui`, `dependencies/rmk`, Rust/Nix/npm pins,
  `scripts/elf-to-uf2.mjs`, licensing, active documentation, and upstream RMK
  notes were retained.
- The untracked source document `docs/glove80-rmk-repository-design.md` was
  imported in the documentation commit with its pre-extraction status made
  explicit.

Generated `target`, `node_modules`, `ui/dist`, UF2, and recovery-result files
were not copied.

## Deliberate exclusions

- `zmk`, `config`, `host-lighting`, and `maintenance`;
- legacy ZMK protobuf inputs under `protocol/proto`;
- the ZMK-only keymap generator and workflow;
- the source-session state file and `DO-NOT-COMMIT` agent briefs; and
- all generated build and release artifacts.

No excluded tree was required by an active RMK build. Hardware, keymap, memory,
and lighting facts needed by RMK were already represented in version-controlled
firmware inputs.

## Logical extraction commits

1. `579bf85` — record the independent repository baseline.
2. `2d7b115` — remove legacy ZMK product trees.
3. `3b8e76c` — organize the active RMK product stack and repair paths.
4. `cd4bcbc` — add repository-level verification and release packaging.
5. The documentation commit containing this completed record and README.

## Verification record

Before extraction, the source layout passed product-protocol golden tests,
45 compositor tests, 89 control tests, 184 UI tests, a UI production build,
and both release cross-builds. The baseline images had the correct family IDs
and remained inside the application range.

After reorganization, the same test counts and UI build passed. Both halves
cross-built successfully from `firmware/glove80-rmk`, Cargo metadata resolved
for all four workspaces, and every product path dependency stayed beneath this
repository root.

The repository-level `make check` passed its submodule, path-boundary, Rynk
WASM provenance, Rust static/test, and UI gates. `make dist` produced and
validated both UF2s, retained ELFs, checksums, and a machine-readable manifest.
Final artifact checksums are recorded in the generated `dist/SHA256SUMS`, not
committed here.

The source repository status was rechecked after extraction and remained the
same two untracked planning documents recorded at the start. No source file,
submodule checkout, or worktree was changed by the extraction.

## Qualification limit

No physical hardware was available during extraction. Typing, split reconnect,
USB/BLE Rynk and product-protocol configuration, lighting, persistence, and
bootloader recovery remain unqualified on both halves.

No GitHub repository or network remote was created, and nothing was pushed or
published.
