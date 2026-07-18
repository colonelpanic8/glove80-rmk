# Experimental host-lighting protocol

This is a versioned extension to ZMK Studio RPC for temporary, per-key Glove80
lighting. It is intentionally separate from the generated keymap and standard
Studio configuration. A desktop client may use the same extension over Studio's
USB serial or Bluetooth GATT transport.

The implementation is experimental. It builds for both halves, but has not yet
been exercised on a physical keyboard.

## Transport and envelope

The extension adds `host_lighting` as field 6 of the Studio request and response
subsystem `oneof`. Its schema is in
[`protocol/proto/zmk/host_lighting.proto`](../protocol/proto/zmk/host_lighting.proto).
Protocol version 1 supports three requests:

- `get_capabilities` reports the limits compiled into the keyboard.
- `set_pixels` updates up to eight pixels and optionally clears the previous
  host frame first.
- `clear` removes the host override immediately.

These RPCs are not gated by Studio's physical unlock because they are ephemeral:
they cannot change bindings, persist settings, or write lighting frames to
flash. Standard Studio operations that modify configuration retain their normal
unlock requirement.

## Pixel and color model

Pixel indices are raw LED-chain indices, not ZMK key positions:

- `0` through `39` address the central/left half.
- `40` through `79` address the peripheral/right half.

The manual editor contains an explicit, tested logical-key-to-LED mapping.
Colors are encoded as `0xRRGGBB`. Each channel is clamped to the advertised
`max_channel_value`, currently 96, before rendering.

With `replace = true`, all host pixels are first set to black and the supplied
updates become the complete frame fragment. With `replace = false`, supplied
pixels update the existing host frame.

## Lifetime and fallback

`timeout_ms = 0` selects the five-second default. Other values are capped at the
advertised 30-second maximum. Each applied update refreshes that timeout. On
timeout or `clear`, the keyboard resumes its ordinary saved underglow state.

Host lighting is optional and RAM-only. A missing, stopped, or disconnected
daemon therefore affects only the temporary lighting overlay; typing, layers,
bindings, and Studio-saved settings continue to work independently.

Commands for the right half use ZMK's existing split behavior transport. A
response can report a partial application if one half is unavailable. Lighting
rendering runs on ZMK's low-priority work queue, below key scanning and HID work.
The advertised rate is currently 20 updates per second; firmware-side rate
enforcement and frame coalescing remain roadmap work.

## Compatibility

An ordinary ZMK Studio client does not use this custom subsystem. A host client
must use this repository's extended protobuf schema and should call
`get_capabilities` before sending frames. It must reject unsupported protocol
versions rather than assuming compatible semantics.

The current JavaScript file in `scripts/` is only a build-time converter from a
MoErgo JSON layout to a devicetree keymap. It is not part of Studio RPC, does not
run as a daemon, and is not required by this protocol.

The browser editor in [`ui/`](../ui/) is also independent of any daemon. Its
transport adapter uses the official ZMK Studio browser transports, while its
small custom codec implements only this versioned subsystem. A future native
service can implement the same protobuf contract in any language.
