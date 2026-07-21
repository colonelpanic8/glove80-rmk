//! Device-identification constants for the RMK host-protocol transports.
//!
//! # Single source for transport identifiers
//!
//! Everything the transports match on lives in this file and nowhere else.
//! The values mirror the firmware transport definitions and must be kept
//! in sync with them:
//!
//! - `USB_VENDOR_USAGE_PAGE` / `USB_VENDOR_USAGE` — the vendor usage pair
//!   of the host-protocol raw-HID interface: `HostProtocolReport` in
//!   `dependencies/rmk/rmk/src/hid.rs` (page 0xFF88, usage 0x01 — distinct
//!   from Via/Vial's 0xFF60/0x61 so the two raw-HID interfaces are
//!   unambiguous).
//! - `BLE_SERVICE_UUID`, `BLE_REQUEST_CHAR_UUID`, `BLE_RESPONSE_CHAR_UUID`
//!   — `HostProtoService` in `dependencies/rmk/rmk/src/ble/ble_server.rs`
//!   (request: write-without-response; response: notify).
//!
//! Everything else (VID/PID, report size, framing) is settled by
//! `PROTOCOL.md` or the Glove80 hardware.

/// Glove80 USB vendor ID (MoErgo, pid.codes allocation).
pub const USB_VID: u16 = 0x16c0;
/// Glove80 USB product ID.
pub const USB_PID: u16 = 0x27db;

/// Vendor usage page of the host-protocol HID interface.
pub const USB_VENDOR_USAGE_PAGE: u16 = 0xFF88;
/// Vendor usage of the host-protocol HID interface.
pub const USB_VENDOR_USAGE: u32 = 0x0001;

/// Fixed HID report size (PROTOCOL.md: USB raw HID chunk size 32).
pub const USB_REPORT_LEN: usize = 32;
/// Report ID prepended to hidraw writes. 0 = the interface uses unnumbered
/// reports (the kernel strips the leading zero byte). If the firmware ever
/// switches to numbered reports, set the real ID here and flip
/// `USB_INPUT_HAS_REPORT_ID`.
pub const USB_OUTPUT_REPORT_ID: u8 = 0x00;
/// Whether input reports arrive with a leading report-ID byte to strip.
pub const USB_INPUT_HAS_REPORT_ID: bool = false;

/// Custom GATT service UUID advertised by the firmware.
pub const BLE_SERVICE_UUID: &str = "fc550001-f8e0-459f-b421-c254fc42b138";
/// Request characteristic (host writes, write-without-response).
pub const BLE_REQUEST_CHAR_UUID: &str = "fc550002-f8e0-459f-b421-c254fc42b138";
/// Response characteristic (host subscribes to notifications).
pub const BLE_RESPONSE_CHAR_UUID: &str = "fc550003-f8e0-459f-b421-c254fc42b138";
