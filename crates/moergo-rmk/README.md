# Shared MoErgo firmware

This directory owns firmware services shared by Glove80 and Go60:

- the lighting engine and physical LED driver;
- Rynk lighting control and split-state replication;
- Magic-layer lighting actions;
- cross-half bootloader routing; and
- retained panic diagnostics used by Go60.

The board binaries include these modules with `#[path]`. They compile in the
board's crate, using its RMK-generated configuration, hardware constants, and
diagnostic hooks. This directory is a shared source tree, not an independent
Cargo package. The board workspaces own dependencies and build scripts.

`crates/xtask/tests/board_parity.rs` checks that both boards compile the shared
services and keep their RMK features aligned, with the documented Go60 battery
service exception. Formatting and compilation run through both board manifests.

Hardware constants live in each board's entry points:

| Board | LEDs per half | Channel ceiling | Maintenance LED |
| --- | ---: | ---: | ---: |
| Glove80 | 40 | 230 | 12 |
| Go60 | 30 | 102 | 8 |

Matrix wiring, GPIOs, Go60 trackpads, and device data stay in the board crates.
Shared services remain here so fixes apply to both keyboards.
