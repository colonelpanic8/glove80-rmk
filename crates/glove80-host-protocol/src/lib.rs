//! Transport-independent wire codec for the Glove80 host protocol.
//!
//! Byte-level spec: `PROTOCOL.md` next to this crate. Golden vectors shared
//! with the TypeScript codec: `crates/glove80-host-protocol/vectors/host-protocol-v1.json`.
//!
//! The crate is `no_std` (core + `heapless`) so the firmware can embed it;
//! the `std` feature only adds `std::error::Error` impls.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod config;
pub mod error;
pub mod frame;
mod io;
mod message;

pub use config::{
    crc32, decode_lighting_config, encode_lighting_config, ConfigActivation, ConfigError,
    ConfigGate, ConfigRecord, LightingConfig, CONFIG_BODY_HEADER_LEN, CONFIG_HEADER_LEN,
    CONFIG_KEY_COUNT, CONFIG_LAYER_COUNT, CONFIG_MAGIC, CONFIG_RECORD_HEADER_LEN,
    CONFIG_TOGGLE_COUNT, CONFIG_VERSION, MAX_CELLS_PER_RECORD, MAX_CONFIG_BLOB_LEN,
    MAX_CONFIG_RECORDS,
};
pub use error::{DecodeError, EncodeError, FrameError};
pub use message::{
    decode_request, decode_response, encode_request, encode_response, feature, BootTarget,
    Capabilities, CellState, CellWrite, Command, Effect, EffectKind, HalfVersion, KeymapEntry,
    Request, Response, ResponsePayload, Status, VersionInfo,
};

/// Protocol major version. A major bump is a breaking change.
pub const PROTOCOL_VERSION_MAJOR: u8 = 1;
/// Protocol minor version. Minor bumps are additive. 1.1 adds persistent
/// lighting configuration (CONFIG_* commands, the config blob format); 1.2
/// adds keymap editing (KEYMAP_* commands, VIA 16-bit keycodes); 1.3 adds
/// build-identity reporting (GET_VERSION); 1.4 adds per-record gates to the
/// config blob (conditional lighting, `CONFIG_GATES` feature bit).
pub const PROTOCOL_VERSION_MINOR: u8 = 4;

/// Bit 7 of the opcode byte marks a response.
pub const RESPONSE_FLAG: u8 = 0x80;

/// Request header: opcode, request_id, payload_len (u16 LE).
pub const REQUEST_HEADER_LEN: usize = 4;
/// Response header: opcode|0x80, request_id, status, payload_len (u16 LE).
pub const RESPONSE_HEADER_LEN: usize = 5;

/// Upper bound on a whole message (header + payload).
pub const MAX_MESSAGE_LEN: usize = 1536;
/// Codec-side bound on cells/keys per message. Devices advertise their own
/// (possibly smaller) `max_cells_per_op` in the capability response.
pub const MAX_CELLS_PER_MESSAGE: usize = 80;
/// Maximum PING/echo payload.
pub const MAX_PING_LEN: usize = 64;
/// Maximum config bytes carried by one CONFIG_DATA request or one
/// CONFIG_READ response (fits comfortably under [`MAX_MESSAGE_LEN`]).
pub const MAX_CONFIG_DATA_PER_MESSAGE: usize = 1024;
/// Codec-side bound on keymap entries per KEYMAP_READ/KEYMAP_WRITE message
/// (v1.2). Devices advertise their own (possibly smaller)
/// `max_keymap_entries_per_op` in the capability response.
pub const MAX_KEYMAP_ENTRIES_PER_MESSAGE: usize = 128;

/// Required magic for `ENTER_BOOTLOADER`.
pub const BOOTLOADER_MAGIC: u32 = 0xB007_10AD;
