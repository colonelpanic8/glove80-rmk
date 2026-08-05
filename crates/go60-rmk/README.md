# Go60 RMK firmware (experimental)

This sibling crate is an initial RMK port for the MoErgo Go60. It currently
targets the board's two nRF52840 halves, matrix, BLE split, Rynk control,
30-pixel-per-half RGB chains, and the two SPI Cirque Pinnacle trackpads
(relative mode with tap-to-click; the peripheral half's pad reaches the
central over the split link).

The hardware facts come from MoErgo's official `moergo-sc/zmk` Go60 board
definitions. One feature is still deliberately omitted: automatic inter-half
switching between BLE and half-duplex UART/TRRS.

Hardware qualification is required before relying on this image as a
replacement for the supported ZMK firmware. In particular, the official board
files document the left LED's electrical order; the right LED table is a
mirrored starting assumption that must be checked on hardware. The trackpad
driver is new and unqualified: motion direction, sensitivity, and tap
behavior on the physical pads need verification against the ZMK build.

Build both halves from the repository root with `just go60-firmware`.
