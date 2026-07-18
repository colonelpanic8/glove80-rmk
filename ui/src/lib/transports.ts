import { connect as connectGatt } from "@zmkfirmware/zmk-studio-ts-client/transport/gatt";
import { connect as connectSerial } from "@zmkfirmware/zmk-studio-ts-client/transport/serial";

import { ZmkLightingClient, type LightingClient } from "./lighting-client";

export type TransportKind = "usb" | "ble";

export function transportSupported(kind: TransportKind): boolean {
  return kind === "usb" ? "serial" in navigator : "bluetooth" in navigator;
}

export async function connectLighting(kind: TransportKind): Promise<LightingClient> {
  const transport = kind === "usb" ? await connectSerial() : await connectGatt();
  try {
    return await ZmkLightingClient.connect(transport);
  } catch (error) {
    transport.abortController.abort();
    throw error;
  }
}
