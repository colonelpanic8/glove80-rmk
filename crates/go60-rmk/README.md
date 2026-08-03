# Go60 RMK firmware (experimental)

This sibling crate is an initial RMK port for the MoErgo Go60. It currently
targets the board's two nRF52840 halves, matrix, BLE split, Rynk control, and
30-pixel-per-half RGB chains.

The hardware facts come from MoErgo's official `moergo-sc/zmk` Go60 board
definitions. The first milestone deliberately omits two features that need
generic RMK work before they can be safely enabled:

- the two SPI Cirque Pinnacle trackpads; and
- automatic inter-half switching between BLE and half-duplex UART/TRRS.

Until those land, this image is a keyboard-and-lighting bring-up build, not a
feature-complete replacement for the supported ZMK firmware. Hardware
qualification is required before relying on it. In particular, the official
board files document the left LED's electrical order; the right LED table is a
mirrored starting assumption that must be checked on hardware.

Build both halves from the repository root with `just go60-firmware`.
