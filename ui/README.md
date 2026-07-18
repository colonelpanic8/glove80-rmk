# Glove80 Lightbench

Lightbench is a standalone browser interface for the experimental Glove80
host-lighting protocol. It connects directly to the keyboard and has no Codex
integration or daemon dependency.

## Run locally

```sh
cd ui
npm ci
npm run dev
```

Open the local URL printed by Vite in Chrome or Edge. `localhost` is important:
browser hardware APIs require a secure context, and browsers treat localhost as
secure for local development.

Use **Connect USB** for the Studio CDC/ACM serial endpoint. **Connect BLE** is
available when the browser and operating system expose Web Bluetooth for the
ZMK Studio GATT service. The keyboard must be running this repository's custom
firmware, and its active output should match the chosen connection transport.

Only one program can normally own the USB serial port at a time. Close ZMK
Studio or any future lighting service before using the direct USB connection.

## Architecture

The application is intentionally split into four pieces:

- `App.tsx` owns the manual editor and never imports a concrete transport.
- `lighting-client.ts` exposes the generic `LightingClient` interface and
  implements bounded frames, capability negotiation, rate limiting, and color
  safety.
- `transports.ts` adapts the official ZMK Studio USB and BLE transports.
- `protobuf.ts` is the language-neutral host-lighting wire contract encoded for
  the browser.

The browser currently uses TypeScript because it is a browser application, not
because ZMK Studio or the firmware requires JavaScript. A future native daemon
can be written in Rust, Python, or another language and can expose a separate
adapter implementing the same conceptual lighting operations. Neither the
manual editor nor the firmware protocol contains Codex-specific state.

## Behavior

- Clicking or dragging paints individual keys.
- Complete scenes, fill, blackout, and left-to-right mirroring are supported.
- The current canvas is stored only in browser local storage.
- Live frames are kept in keyboard RAM and refreshed while connected.
- **Release** clears the host override without erasing the local canvas.
- Disconnecting clears the override; an unexpected disconnect falls back when
  the firmware timeout expires.

The visible key legends come from the current base layer. The LED mapping uses
the Glove80 hardware chain order, including the mirrored right half; the tests
verify that all 80 logical keys and all 80 LED indices are represented exactly
once.
