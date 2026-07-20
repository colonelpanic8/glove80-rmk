# Deglove rename map and equivalence check

The comparison source is monorepo commit `b1ae2b2c`, path `rmk/vendor/rmk`.
The historical comparison target was fork branch `glove80` at `165e4720`.
The pre-Rynk rollback is pinned at `8089822e`, which additionally contains the
polished shared-flash API and VBUS hook. The active candidate is
`glove80-rynk` at `67f444b2`; it merges Rynk and removes `keymap_ops`.

## Verified renames

These mappings were verified from the actual snapshot-to-fork tree diff, not
only from the handoff notes.

### Modules and files

| Vendored name | Fork name |
| --- | --- |
| `split_app_pipe.rs` / `split_app_pipe` | `split_app.rs` / `split_app` |
| `host_proto_pipe.rs` / `host_proto_pipe` | `vendor_transport.rs` / `vendor_transport` |
| `config_flash.rs` / `config_flash` | `shared_flash.rs` / `shared_flash` |
| `keymap_ops_pipe.rs` / `keymap_ops_pipe` | `keymap_ops.rs` / `keymap_ops` |

### Public and generated symbols

| Vendored symbol | Fork symbol |
| --- | --- |
| `HostProtocolReport` | `VendorHidReport` |
| `HostProtoService` | `VendorGattService` |
| `HOSTP_USB_RX` | `VENDOR_USB_RX` |
| `HOSTP_USB_TX` | `VENDOR_USB_TX` |
| `HOSTP_BLE_RX` | `VENDOR_BLE_RX` |
| `HOSTP_BLE_TX` | `VENDOR_BLE_TX` |
| `HOSTP_BLE_ATT_PAYLOAD` | `VENDOR_BLE_ATT_PAYLOAD` |
| `run_usb_host_proto` | `run_usb_vendor` |
| `GLOVE80_SHARED_FLASH` | `SHARED_FLASH` |
| `glove80_config_flash_service` | `shared_flash_service` |

The snapshot actually uses `glove80_config_flash_service`; the handoff spelling
was therefore correct.

### Private fields and local variables

| Vendored symbol | Fork symbol |
| --- | --- |
| `host_proto_service` | `vendor_gatt_service` |
| `host_proto_request` | `vendor_request` |
| `host_proto_response` | `vendor_response` |
| `host_proto_task` | `vendor_task` |
| `host_proto_rw` | `vendor_rw` |

Log text using `host-proto` was likewise generalized to `vendor`. The split
application public APIs (`SplitAppData`, `SPLIT_APP_*`) did not change names.
The historical keymap-operation APIs (`KeymapOp`, `KEYMAP_OPS`, and
`KEYMAP_OP_RESULTS`) were renamed during extraction and later removed from the
active integration after the Rynk migration.

## Comment changes

The fork removes the literal `GLOVE80 PATCH` / `END GLOVE80 PATCH` markers and
rewrites Glove80-specific module documentation as generic keyboard-firmware
documentation. These are prose-only changes. Examples and UUID/usage-page text
are explicitly described as consumer-selectable defaults in the fork.

## Equivalence verdict

**PASS for snapshot equivalence, with one documented keep-local code
difference.** After applying the verified renames above and disregarding
marker/comment generalization, the snapshot and fork `glove80` trees have the
same functional patch code. The one intentional non-rename code difference is
the original fork commit `165e4720` (rebased as `e26faf69`), which exposes
`crc32` without `dfu_split`; it exists only on `glove80` and is documented in
`PATCHES.md`. The historical monorepo vendor tree also contained the separately
enumerated post-snapshot VBUS hook. That hook was subsequently ported to the
fork in `8089822e` before the submodule cutover.

Evidence:

- Snapshot vendored tree object: `e323cec0abfbdb4a635e7b0a542df65ec762c871`.
- Current monorepo vendored tree object at HEAD `2bac75ac`:
  `7f868cc1c57f72e82ff2f5962f0bf29fe736c59f`.
- `git log b1ae2b2c..HEAD -- rmk/vendor/rmk` at current monorepo HEAD
  returns exactly `7a56c997 firmware: wire conditional lighting state`.
- A normalized file-by-file diff accounts for module/public/local renames,
  generic prose, removed marker text, and the crc32 keep-local commit; no
  unexplained functional delta remains between the snapshot and fork. The
  normalization removes comments and applies every rename in this document;
  after normalizing the crc32 keep-local gate, `diff -qr` returns zero.
- `git grep "GLOVE80 PATCH" glove80 -- rmk rmk-macro` returns no matches.

## Post-snapshot / round 2

There is exactly **one post-snapshot commit to `rmk/vendor/rmk`**:

- `7a56c997` — `firmware: wire conditional lighting state`; adds
  `ReportingVbusDetect`, `USB_VBUS_DETECTED`, and generated nRF wrappers in
  `rmk-macro/src/codegen/chip/comm.rs`,
  `rmk-macro/src/codegen/split/peripheral.rs`, and `rmk/src/usb/mod.rs`.

It added five historical marker sites, listed in `PATCHES.md`, and was later
ported to the fork as `8089822e`. The handoff's example `c7e53891` is real, but
its only changed path is `crates/glove80-compositor/src/sync.rs`; it does not
modify vendored RMK.

Other post-snapshot monorepo commits are downstream protocol, compositor, CLI,
example, or documentation changes rather than additional vendored-RMK commits:

- `755d008d` — `Compositor: per-record gates and firmware-state conditions`
- `c7e53891` — `Compositor sync: usb-connected in State, per-record gate message`
- `ca61cc4a` — `Add Phase 7 qualification run-sheet (Sol-authored)`
- `f0cf7849` — `protocol: add conditional lighting config gates`
- `dcfb7003` — `cli: support conditional lighting gates`
- `2bac75ac` — `examples: show gated status indicators`
