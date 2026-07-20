# RMK extension inventory

Refreshed after the Rynk migration on 2026-07-19. The active candidate pins
`dependencies/rmk` to `glove80-rynk` at `67f444b2`; `glove80` at `8089822e`
remains the pre-Rynk rollback. Both are published at `colonelpanic8/rmk`.

The old subtree carried 49 `GLOVE80 PATCH` marker occurrences. The active
submodule carries **zero** marker comments and the Glove80 firmware contains
no references to the old module names. The historical marker counts below are
kept only to make the extraction auditable.

## Active fork components

| Component | Fork branch / tip | Active API | Historical markers |
| --- | --- | --- | ---: |
| Split application messages | `split-app-messages` / `f84ac245` | `rmk::split_app` | 14 |
| Host transport hooks | `host-transport-hooks` / `722ddcdf` | `rmk::vendor_transport` | 19 |
| Shared flash | `shared-flash` / `ed6bd38d` | `rmk::shared_flash` | 4 |
| Keymap operations | `keymap-ops` / `b0b89891` | **Retired; superseded by Rynk** | 6 |
| CRC-32 ungating | `glove80` / `e26faf69` | `rmk::crc32` | 1 |
| nRF VBUS state hook | `glove80` / `8089822e` | `rmk::usb::USB_VBUS_DETECTED` | 5 |
| **Historical total** | | | **49** |

The pre-Rynk integration commit `9b0e53c0` merges the four generic feature
branches on upstream `main` `a0ebb564`. `b13e6dd7` merges upstream
`feat/rynk`; `4136f2ee` then removes `keymap_ops`, and `67f444b2` adds the
shared-flash/Rynk CI matrix. CRC-32 and VBUS remain integration-only changes.

## Component notes

### Split application messages

A bounded opaque application channel in both directions plus split-link state.
The Glove80 sync codec uses it for lighting, firmware identity, shared state,
and magic-guarded peripheral bootloader entry. The containing module was
renamed from `split_app_pipe` to `split_app` during extraction.

### Host transport hooks

Opaque application-defined transport over a dedicated USB raw-HID interface
and custom BLE GATT service. The fork generalizes the former
`host_proto_pipe` module and `HOSTP_*` symbols as `vendor_transport` and
`VENDOR_*`.

### Shared flash

The opt-in `shared_flash` feature serializes RMK storage and application-owned
flash access through the same radio-safe nRF driver. The polished API uniquely
acquires `SharedFlash` with `take(window)`; every operation requires `&mut
self` and is confined to the immutable validated window. This replaces the
old free-function `config_flash` API.

### Keymap operations

A historical one-operation-at-a-time channel into the Vial task. It is still
preserved on its feature branch for provenance, but is absent from the active
integration because firmware, CLI, and Lightbench now use Rynk.

### Keep-local integration changes

- `e26faf69` exposes RMK's CRC-32 implementation without requiring split DFU;
  Glove80 runtime-configuration headers reuse it.
- `8089822e` wraps the nRF VBUS detector and exposes
  `USB_VBUS_DETECTED`, allowing conditional lighting to distinguish physical
  VBUS/charging state from configured USB HID state.

## Cutover verification

- `.gitmodules` tracks `dependencies/rmk`, branch `glove80-rynk`, at `67f444b2`.
- `firmware/glove80-rmk/Cargo.toml` opts into `shared_flash` and uses the submodule path.
- The firmware uses `split_app`, `vendor_transport`, `shared_flash`, and Rynk;
  no old module names or production `keymap_ops` use remain.
- The transactional config store owns the unique partition-scoped
  `SharedFlash` client.
- Both release UF2 images build under the repository's pinned Rust 1.97.0 Nix
  shell after the cutover.

Do not propose either Glove80 integration branch upstream. Publish and review
the generic feature branches independently, rebasing them onto the current
upstream base as required by the upstream-alignment plan.
