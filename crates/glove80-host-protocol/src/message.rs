//! Message types and the transport-independent encode/decode.
//!
//! Byte layouts are specified exhaustively in `PROTOCOL.md`.

use heapless::Vec;

use crate::error::{DecodeError, EncodeError};
use crate::io::{Reader, Writer};
use crate::{
    MAX_CELLS_PER_MESSAGE, MAX_CONFIG_DATA_PER_MESSAGE, MAX_KEYMAP_ENTRIES_PER_MESSAGE,
    MAX_PING_LEN, REQUEST_HEADER_LEN, RESPONSE_FLAG, RESPONSE_HEADER_LEN,
};

/// Feature bits advertised in [`Capabilities::feature_bits`].
pub mod feature {
    /// Per-write TTL supported.
    pub const TTL: u32 = 1 << 0;
    /// Toggle overlays reachable via GET/SET_TOGGLE.
    pub const TOGGLES: u32 = 1 << 1;
    /// Programmatic bootloader entry.
    pub const BOOTLOADER_ENTRY: u32 = 1 << 2;
    /// REPLACE_OVERLAY supported.
    pub const ATOMIC_REPLACE: u32 = 1 << 3;
    /// READ_OVERLAY supported.
    pub const OVERLAY_READBACK: u32 = 1 << 4;
    /// PARTIAL_APPLY reporting (peripheral offline is reported, not hidden).
    pub const PARTIAL_APPLY: u32 = 1 << 5;
    /// Persistent lighting configuration (CONFIG_* commands, v1.1). When set,
    /// the capability payload carries the `max_config_blob_len` extension.
    pub const PERSISTENT_CONFIG: u32 = 1 << 6;
    /// Keymap editing (KEYMAP_* commands, v1.2). When set, the capability
    /// payload carries the keymap extension (`keymap_rows`, `keymap_cols`,
    /// `max_keymap_entries_per_op`).
    pub const KEYMAP: u32 = 1 << 7;
    /// Build-identity reporting (GET_VERSION, v1.3). Adds no capability
    /// extension — the bit only gates the command.
    pub const VERSION_REPORT: u32 = 1 << 8;
    /// Per-record gates in the config blob (conditional lighting, v1.4). Adds
    /// no capability extension — the bit tells a host the firmware validates
    /// and honors record gates. Older firmware accepts the all-zero reserved
    /// value and rejects nonzero unknown gate kinds as an invalid config.
    pub const CONFIG_GATES: u32 = 1 << 9;
}

/// Command opcodes (always < 0x80; responses set [`RESPONSE_FLAG`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    GetCapabilities = 0x01,
    Ping = 0x02,
    GetVersion = 0x03,
    SetCells = 0x10,
    UnsetCells = 0x11,
    ClearOverlay = 0x12,
    ReadOverlay = 0x13,
    ReplaceOverlay = 0x14,
    GetBrightness = 0x20,
    SetBrightness = 0x21,
    GetToggle = 0x30,
    SetToggle = 0x31,
    ConfigBegin = 0x40,
    ConfigData = 0x41,
    ConfigCommit = 0x42,
    ConfigAbort = 0x43,
    ConfigRead = 0x44,
    KeymapRead = 0x50,
    KeymapWrite = 0x51,
    EnterBootloader = 0x7F,
}

impl Command {
    pub fn opcode(self) -> u8 {
        self as u8
    }

    pub fn from_opcode(op: u8) -> Option<Command> {
        Some(match op {
            0x01 => Command::GetCapabilities,
            0x02 => Command::Ping,
            0x03 => Command::GetVersion,
            0x10 => Command::SetCells,
            0x11 => Command::UnsetCells,
            0x12 => Command::ClearOverlay,
            0x13 => Command::ReadOverlay,
            0x14 => Command::ReplaceOverlay,
            0x20 => Command::GetBrightness,
            0x21 => Command::SetBrightness,
            0x30 => Command::GetToggle,
            0x31 => Command::SetToggle,
            0x40 => Command::ConfigBegin,
            0x41 => Command::ConfigData,
            0x42 => Command::ConfigCommit,
            0x43 => Command::ConfigAbort,
            0x44 => Command::ConfigRead,
            0x50 => Command::KeymapRead,
            0x51 => Command::KeymapWrite,
            0x7F => Command::EnterBootloader,
            _ => return None,
        })
    }

    /// The four overlay-write commands that ack with an overlay ack and may
    /// report PARTIAL_APPLY.
    pub fn is_overlay_write(self) -> bool {
        matches!(
            self,
            Command::SetCells | Command::UnsetCells | Command::ClearOverlay | Command::ReplaceOverlay
        )
    }
}

/// Response status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0x00,
    UnknownCommand = 0x01,
    Malformed = 0x02,
    OutOfRange = 0x03,
    CapacityExceeded = 0x04,
    PartialApply = 0x05,
    Busy = 0x06,
    UnknownToggle = 0x07,
    BadMagic = 0x08,
    UnsupportedVersion = 0x09,
    /// CONFIG_DATA / CONFIG_COMMIT without an open transfer session.
    NoSession = 0x0A,
    /// CONFIG_DATA offset is not the number of bytes received so far, or the
    /// chunk would run past the announced total length.
    BadOffset = 0x0B,
    /// CONFIG_COMMIT before all announced bytes arrived.
    ConfigIncomplete = 0x0C,
    /// Assembled blob's CRC (announced in CONFIG_BEGIN, or the header's
    /// body CRC) does not match.
    CrcMismatch = 0x0D,
    /// The complete blob failed validation; nothing was changed.
    InvalidConfig = 0x0E,
}

impl Status {
    pub fn from_u8(v: u8) -> Option<Status> {
        Some(match v {
            0x00 => Status::Ok,
            0x01 => Status::UnknownCommand,
            0x02 => Status::Malformed,
            0x03 => Status::OutOfRange,
            0x04 => Status::CapacityExceeded,
            0x05 => Status::PartialApply,
            0x06 => Status::Busy,
            0x07 => Status::UnknownToggle,
            0x08 => Status::BadMagic,
            0x09 => Status::UnsupportedVersion,
            0x0A => Status::NoSession,
            0x0B => Status::BadOffset,
            0x0C => Status::ConfigIncomplete,
            0x0D => Status::CrcMismatch,
            0x0E => Status::InvalidConfig,
            _ => return None,
        })
    }
}

/// Effect kinds; bit positions in [`Capabilities::effect_mask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EffectKind {
    Solid = 0,
    Blink = 1,
    Breathe = 2,
}

impl EffectKind {
    pub fn from_u8(v: u8) -> Option<EffectKind> {
        Some(match v {
            0 => EffectKind::Solid,
            1 => EffectKind::Blink,
            2 => EffectKind::Breathe,
            _ => return None,
        })
    }
}

/// A fixed 10-byte effect record. Fields not applicable to `kind` should be
/// encoded as 0 but round-trip verbatim either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub kind: EffectKind,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub period_ms: u16,
    pub phase_ms: u16,
    pub duty_percent: u8,
}

impl Effect {
    pub const ENCODED_LEN: usize = 10;

    pub fn solid(r: u8, g: u8, b: u8) -> Effect {
        Effect { kind: EffectKind::Solid, r, g, b, period_ms: 0, phase_ms: 0, duty_percent: 0 }
    }

    pub fn blink(r: u8, g: u8, b: u8, period_ms: u16, phase_ms: u16, duty_percent: u8) -> Effect {
        Effect { kind: EffectKind::Blink, r, g, b, period_ms, phase_ms, duty_percent }
    }

    pub fn breathe(r: u8, g: u8, b: u8, period_ms: u16, phase_ms: u16) -> Effect {
        Effect { kind: EffectKind::Breathe, r, g, b, period_ms, phase_ms, duty_percent: 0 }
    }

    pub(crate) fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        w.u8(self.kind as u8)?;
        w.u8(self.r)?;
        w.u8(self.g)?;
        w.u8(self.b)?;
        w.u16(self.period_ms)?;
        w.u16(self.phase_ms)?;
        w.u8(self.duty_percent)?;
        w.u8(0) // reserved
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Effect, DecodeError> {
        let kind_byte = r.u8()?;
        let kind = EffectKind::from_u8(kind_byte).ok_or(DecodeError::UnknownEffectKind(kind_byte))?;
        let red = r.u8()?;
        let green = r.u8()?;
        let blue = r.u8()?;
        let period_ms = r.u16()?;
        let phase_ms = r.u16()?;
        let duty_percent = r.u8()?;
        let _reserved = r.u8()?; // ignored for forward compatibility
        Ok(Effect { kind, r: red, g: green, b: blue, period_ms, phase_ms, duty_percent })
    }
}

/// One cell in a SET/REPLACE batch: 11 bytes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellWrite {
    pub key: u8,
    pub effect: Effect,
}

/// One cell in a READ_OVERLAY response: 15 bytes on the wire.
/// `remaining_ttl_ms == 0` means the cell has no TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellState {
    pub key: u8,
    pub effect: Effect,
    pub remaining_ttl_ms: u32,
}

/// One entry in a KEYMAP_WRITE batch: 4 bytes on the wire (v1.2). `keycode`
/// is the VIA/Vial 16-bit keycode encoding — the same values Vial reads and
/// writes over its own protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeymapEntry {
    pub layer: u8,
    /// Grid position: `row * keymap_cols + col` in the advertised keymap
    /// grid (independent of the LED key space).
    pub key: u8,
    pub keycode: u16,
}

/// Build identity of one keyboard half: 16 bytes on the wire (v1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HalfVersion {
    /// Whether this half is currently reachable. The central is always
    /// present in its own response; the peripheral entry keeps its
    /// last-known version fields with `present = false` while the split
    /// link is down (all-zero fields = never seen).
    pub present: bool,
    /// Firmware semver, from the firmware crate's `CARGO_PKG_VERSION`.
    pub fw_major: u8,
    pub fw_minor: u8,
    pub fw_patch: u8,
    /// Git short hash of the build tree (`git rev-parse --short=8 HEAD`),
    /// ASCII, zero-padded on the right. `b"unknown0"` when the build had no
    /// git available; all-zero when this half was never seen.
    pub git_hash: [u8; 8],
    /// Whether the build tree had uncommitted changes
    /// (`git status --porcelain` non-empty).
    pub dirty: bool,
}

impl HalfVersion {
    pub const ENCODED_LEN: usize = 16;

    fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        w.u8(self.present as u8)?;
        w.u8(self.fw_major)?;
        w.u8(self.fw_minor)?;
        w.u8(self.fw_patch)?;
        w.bytes(&self.git_hash)?;
        w.u8(self.dirty as u8)?;
        w.bytes(&[0; 3]) // reserved
    }

    fn read(r: &mut Reader<'_>) -> Result<HalfVersion, DecodeError> {
        let present = read_flag(r)?;
        let fw_major = r.u8()?;
        let fw_minor = r.u8()?;
        let fw_patch = r.u8()?;
        let mut git_hash = [0u8; 8];
        git_hash.copy_from_slice(r.bytes(8)?);
        let dirty = read_flag(r)?;
        let _reserved = r.bytes(3)?; // ignored for forward compatibility
        Ok(HalfVersion { present, fw_major, fw_minor, fw_patch, git_hash, dirty })
    }
}

/// GET_VERSION response payload: both halves' build identity plus the
/// firmware-computed mismatch flag (v1.3). 33 bytes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VersionInfo {
    pub central: HalfVersion,
    pub peripheral: HalfVersion,
    /// Set by the firmware when both halves are present and their git hash
    /// or firmware semver differ.
    pub halves_mismatch: bool,
}

impl VersionInfo {
    pub const ENCODED_LEN: usize = 2 * HalfVersion::ENCODED_LEN + 1;
}

fn read_flag(r: &mut Reader<'_>) -> Result<bool, DecodeError> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        v => Err(DecodeError::BadFlag(v)),
    }
}

/// Capability response payload (16 bytes; +4 with the v1.1 persistent-config
/// extension, +4 more with the v1.2 keymap extension). Tools must never
/// assume capacities; everything they rely on is advertised here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub protocol_major: u8,
    pub protocol_minor: u8,
    pub led_count_left: u8,
    pub led_count_right: u8,
    pub layer_capacity: u8,
    pub max_cells_per_op: u8,
    /// Bit n set ⇔ effect kind n supported.
    pub effect_mask: u16,
    pub overlay_cell_capacity: u16,
    pub max_message_len: u16,
    pub feature_bits: u32,
    /// Largest config blob the device accepts (v1.1). On the wire this u32 is
    /// present **iff** `feature_bits` has [`feature::PERSISTENT_CONFIG`] set;
    /// otherwise it is absent and decodes as 0.
    pub max_config_blob_len: u32,
    /// Keymap grid rows (v1.2). With `keymap_cols` and the trailing reserved
    /// byte this 4-byte extension is on the wire **iff** `feature_bits` has
    /// [`feature::KEYMAP`] set; otherwise absent and decodes as 0.
    pub keymap_rows: u8,
    /// Keymap grid columns (v1.2; see `keymap_rows`).
    pub keymap_cols: u8,
    /// Max entries per KEYMAP_READ/KEYMAP_WRITE (v1.2; see `keymap_rows`).
    pub max_keymap_entries_per_op: u8,
}

/// Bootloader entry target half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootTarget {
    Central = 0,
    Peripheral = 1,
}

impl BootTarget {
    pub fn from_u8(v: u8) -> Option<BootTarget> {
        Some(match v {
            0 => BootTarget::Central,
            1 => BootTarget::Peripheral,
            _ => return None,
        })
    }
}

/// A request message (payload part; `request_id` travels alongside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    GetCapabilities { client_major: u8, client_minor: u8 },
    Ping { data: Vec<u8, MAX_PING_LEN> },
    /// Query both halves' firmware build identity (v1.3, empty payload).
    GetVersion,
    SetCells { ttl_ms: u32, cells: Vec<CellWrite, MAX_CELLS_PER_MESSAGE> },
    UnsetCells { keys: Vec<u8, MAX_CELLS_PER_MESSAGE> },
    ClearOverlay,
    ReadOverlay,
    ReplaceOverlay { ttl_ms: u32, cells: Vec<CellWrite, MAX_CELLS_PER_MESSAGE> },
    GetBrightness,
    SetBrightness { level: u8 },
    GetToggle { id: u8 },
    SetToggle { id: u8, state: bool },
    /// Open a config transfer session (a new BEGIN replaces any open one).
    /// `total_len`/`blob_crc32` describe the **entire** blob (header + body).
    ConfigBegin { total_len: u32, blob_crc32: u32 },
    /// One contiguous chunk of the blob; `offset` must equal the number of
    /// bytes received so far.
    ConfigData { offset: u32, data: Vec<u8, MAX_CONFIG_DATA_PER_MESSAGE> },
    /// Validate, atomically activate, and persist the assembled blob.
    ConfigCommit,
    /// Discard any open transfer session (idempotent).
    ConfigAbort,
    /// Read `max_len` bytes of the active config blob starting at `offset`.
    ConfigRead { offset: u32, max_len: u16 },
    /// Read up to `max_count` key actions on `layer`, starting at grid
    /// position `start_key` (v1.2). The device answers with consecutive
    /// positions; iterate to walk the whole grid.
    KeymapRead { layer: u8, start_key: u8, max_count: u8 },
    /// Write a batch of key actions (v1.2). Validated as a whole
    /// (all-or-nothing): any out-of-range entry rejects the batch.
    KeymapWrite { entries: Vec<KeymapEntry, MAX_KEYMAP_ENTRIES_PER_MESSAGE> },
    EnterBootloader { magic: u32, target: BootTarget },
}

impl Request {
    pub fn command(&self) -> Command {
        match self {
            Request::GetCapabilities { .. } => Command::GetCapabilities,
            Request::Ping { .. } => Command::Ping,
            Request::GetVersion => Command::GetVersion,
            Request::SetCells { .. } => Command::SetCells,
            Request::UnsetCells { .. } => Command::UnsetCells,
            Request::ClearOverlay => Command::ClearOverlay,
            Request::ReadOverlay => Command::ReadOverlay,
            Request::ReplaceOverlay { .. } => Command::ReplaceOverlay,
            Request::GetBrightness => Command::GetBrightness,
            Request::SetBrightness { .. } => Command::SetBrightness,
            Request::GetToggle { .. } => Command::GetToggle,
            Request::SetToggle { .. } => Command::SetToggle,
            Request::ConfigBegin { .. } => Command::ConfigBegin,
            Request::ConfigData { .. } => Command::ConfigData,
            Request::ConfigCommit => Command::ConfigCommit,
            Request::ConfigAbort => Command::ConfigAbort,
            Request::ConfigRead { .. } => Command::ConfigRead,
            Request::KeymapRead { .. } => Command::KeymapRead,
            Request::KeymapWrite { .. } => Command::KeymapWrite,
            Request::EnterBootloader { .. } => Command::EnterBootloader,
        }
    }
}

/// Response payload, discriminated by command + status on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsePayload {
    /// Error statuses and ENTER_BOOTLOADER ok.
    Empty,
    Capabilities(Capabilities),
    Echo { data: Vec<u8, MAX_PING_LEN> },
    /// GET_VERSION ok (v1.3): both halves' build identity.
    Version(VersionInfo),
    /// Ack for the four overlay writes; `pending_keys` lists keys accepted on
    /// the central but not yet applied on the peripheral.
    OverlayAck { pending_keys: Vec<u8, MAX_CELLS_PER_MESSAGE> },
    OverlayState { cells: Vec<CellState, MAX_CELLS_PER_MESSAGE> },
    Brightness { level: u8 },
    Toggle { id: u8, state: bool },
    /// CONFIG_READ ok: `total_len` is the full blob length; `data` is the
    /// slice at the requested offset (empty at end of blob).
    ConfigData { total_len: u32, data: Vec<u8, MAX_CONFIG_DATA_PER_MESSAGE> },
    /// KEYMAP_READ ok (v1.2): VIA 16-bit keycodes at consecutive grid
    /// positions `start_key..start_key + keycodes.len()` on `layer`.
    KeymapActions { layer: u8, start_key: u8, keycodes: Vec<u16, MAX_KEYMAP_ENTRIES_PER_MESSAGE> },
    /// KEYMAP_WRITE ok (v1.2): per-entry canonical read-back — the keycode
    /// now stored at each written position, in request order (differs from
    /// the request when the encoding is not exactly representable).
    KeymapWritten { keycodes: Vec<u16, MAX_KEYMAP_ENTRIES_PER_MESSAGE> },
}

/// A full response message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub request_id: u8,
    pub command: Command,
    pub status: Status,
    pub payload: ResponsePayload,
}

fn write_cells(
    w: &mut Writer<'_>,
    ttl_ms: u32,
    cells: &[CellWrite],
) -> Result<(), EncodeError> {
    w.u32(ttl_ms)?;
    w.u8(cells.len() as u8)?;
    for cell in cells {
        w.u8(cell.key)?;
        cell.effect.write(w)?;
    }
    Ok(())
}

fn read_cells(
    r: &mut Reader<'_>,
) -> Result<(u32, Vec<CellWrite, MAX_CELLS_PER_MESSAGE>), DecodeError> {
    let ttl_ms = r.u32()?;
    let count = r.u8()? as usize;
    let mut cells = Vec::new();
    for _ in 0..count {
        let key = r.u8()?;
        let effect = Effect::read(r)?;
        cells.push(CellWrite { key, effect }).map_err(|_| DecodeError::CapacityExceeded)?;
    }
    Ok((ttl_ms, cells))
}

/// Encode a request into `out`; returns the number of bytes written.
pub fn encode_request(request_id: u8, req: &Request, out: &mut [u8]) -> Result<usize, EncodeError> {
    let mut w = Writer::new(out);
    w.u8(req.command().opcode())?;
    w.u8(request_id)?;
    w.u16(0)?; // payload_len, patched below
    match req {
        Request::GetCapabilities { client_major, client_minor } => {
            w.u8(*client_major)?;
            w.u8(*client_minor)?;
        }
        Request::Ping { data } => w.bytes(data)?,
        Request::GetVersion => {}
        Request::SetCells { ttl_ms, cells } | Request::ReplaceOverlay { ttl_ms, cells } => {
            write_cells(&mut w, *ttl_ms, cells)?;
        }
        Request::UnsetCells { keys } => {
            w.u8(keys.len() as u8)?;
            w.bytes(keys)?;
        }
        Request::ClearOverlay | Request::ReadOverlay | Request::GetBrightness => {}
        Request::SetBrightness { level } => w.u8(*level)?,
        Request::GetToggle { id } => w.u8(*id)?,
        Request::SetToggle { id, state } => {
            w.u8(*id)?;
            w.u8(*state as u8)?;
        }
        Request::ConfigBegin { total_len, blob_crc32 } => {
            w.u32(*total_len)?;
            w.u32(*blob_crc32)?;
        }
        Request::ConfigData { offset, data } => {
            w.u32(*offset)?;
            w.bytes(data)?;
        }
        Request::ConfigCommit | Request::ConfigAbort => {}
        Request::ConfigRead { offset, max_len } => {
            w.u32(*offset)?;
            w.u16(*max_len)?;
        }
        Request::KeymapRead { layer, start_key, max_count } => {
            w.u8(*layer)?;
            w.u8(*start_key)?;
            w.u8(*max_count)?;
        }
        Request::KeymapWrite { entries } => {
            w.u8(entries.len() as u8)?;
            for e in entries {
                w.u8(e.layer)?;
                w.u8(e.key)?;
                w.u16(e.keycode)?;
            }
        }
        Request::EnterBootloader { magic, target } => {
            w.u32(*magic)?;
            w.u8(*target as u8)?;
        }
    }
    let len = w.pos();
    w.patch_u16(2, (len - REQUEST_HEADER_LEN) as u16);
    Ok(len)
}

/// Decode a complete request message. Returns `(request_id, request)`.
pub fn decode_request(bytes: &[u8]) -> Result<(u8, Request), DecodeError> {
    let mut r = Reader::new(bytes);
    let opcode = r.u8()?;
    let command = Command::from_opcode(opcode).ok_or(DecodeError::UnknownOpcode(opcode))?;
    let request_id = r.u8()?;
    let payload_len = r.u16()? as usize;
    if r.remaining() != payload_len {
        return Err(DecodeError::LengthMismatch);
    }
    let request = match command {
        Command::GetCapabilities => {
            let client_major = r.u8()?;
            let client_minor = r.u8()?;
            Request::GetCapabilities { client_major, client_minor }
        }
        Command::Ping => {
            if payload_len > MAX_PING_LEN {
                return Err(DecodeError::CapacityExceeded);
            }
            let mut data = Vec::new();
            data.extend_from_slice(r.bytes(payload_len)?)
                .map_err(|_| DecodeError::CapacityExceeded)?;
            Request::Ping { data }
        }
        Command::GetVersion => Request::GetVersion,
        Command::SetCells => {
            let (ttl_ms, cells) = read_cells(&mut r)?;
            Request::SetCells { ttl_ms, cells }
        }
        Command::ReplaceOverlay => {
            let (ttl_ms, cells) = read_cells(&mut r)?;
            Request::ReplaceOverlay { ttl_ms, cells }
        }
        Command::UnsetCells => {
            let count = r.u8()? as usize;
            if count > MAX_CELLS_PER_MESSAGE {
                return Err(DecodeError::CapacityExceeded);
            }
            let mut keys = Vec::new();
            keys.extend_from_slice(r.bytes(count)?)
                .map_err(|_| DecodeError::CapacityExceeded)?;
            Request::UnsetCells { keys }
        }
        Command::ClearOverlay => Request::ClearOverlay,
        Command::ReadOverlay => Request::ReadOverlay,
        Command::GetBrightness => Request::GetBrightness,
        Command::SetBrightness => Request::SetBrightness { level: r.u8()? },
        Command::GetToggle => Request::GetToggle { id: r.u8()? },
        Command::SetToggle => {
            let id = r.u8()?;
            let state = match r.u8()? {
                0 => false,
                1 => true,
                v => return Err(DecodeError::BadToggleState(v)),
            };
            Request::SetToggle { id, state }
        }
        Command::ConfigBegin => {
            let total_len = r.u32()?;
            let blob_crc32 = r.u32()?;
            Request::ConfigBegin { total_len, blob_crc32 }
        }
        Command::ConfigData => {
            let offset = r.u32()?;
            let len = r.remaining();
            if len > MAX_CONFIG_DATA_PER_MESSAGE {
                return Err(DecodeError::CapacityExceeded);
            }
            let mut data = Vec::new();
            data.extend_from_slice(r.bytes(len)?).map_err(|_| DecodeError::CapacityExceeded)?;
            Request::ConfigData { offset, data }
        }
        Command::ConfigCommit => Request::ConfigCommit,
        Command::ConfigAbort => Request::ConfigAbort,
        Command::ConfigRead => {
            let offset = r.u32()?;
            let max_len = r.u16()?;
            Request::ConfigRead { offset, max_len }
        }
        Command::KeymapRead => {
            let layer = r.u8()?;
            let start_key = r.u8()?;
            let max_count = r.u8()?;
            Request::KeymapRead { layer, start_key, max_count }
        }
        Command::KeymapWrite => {
            let count = r.u8()? as usize;
            if count > MAX_KEYMAP_ENTRIES_PER_MESSAGE {
                return Err(DecodeError::CapacityExceeded);
            }
            let mut entries = Vec::new();
            for _ in 0..count {
                let layer = r.u8()?;
                let key = r.u8()?;
                let keycode = r.u16()?;
                entries
                    .push(KeymapEntry { layer, key, keycode })
                    .map_err(|_| DecodeError::CapacityExceeded)?;
            }
            Request::KeymapWrite { entries }
        }
        Command::EnterBootloader => {
            let magic = r.u32()?;
            let target_byte = r.u8()?;
            let target =
                BootTarget::from_u8(target_byte).ok_or(DecodeError::UnknownBootTarget(target_byte))?;
            Request::EnterBootloader { magic, target }
        }
    };
    r.finish()?;
    Ok((request_id, request))
}

fn payload_matches(command: Command, status: Status, payload: &ResponsePayload) -> bool {
    match status {
        Status::Ok => match (command, payload) {
            (Command::GetCapabilities, ResponsePayload::Capabilities(_)) => true,
            (Command::Ping, ResponsePayload::Echo { .. }) => true,
            (Command::GetVersion, ResponsePayload::Version(_)) => true,
            (c, ResponsePayload::OverlayAck { .. }) if c.is_overlay_write() => true,
            (Command::ReadOverlay, ResponsePayload::OverlayState { .. }) => true,
            (Command::GetBrightness | Command::SetBrightness, ResponsePayload::Brightness { .. }) => {
                true
            }
            (Command::GetToggle | Command::SetToggle, ResponsePayload::Toggle { .. }) => true,
            (
                Command::ConfigBegin
                | Command::ConfigData
                | Command::ConfigCommit
                | Command::ConfigAbort,
                ResponsePayload::Empty,
            ) => true,
            (Command::ConfigRead, ResponsePayload::ConfigData { .. }) => true,
            (Command::KeymapRead, ResponsePayload::KeymapActions { .. }) => true,
            (Command::KeymapWrite, ResponsePayload::KeymapWritten { .. }) => true,
            (Command::EnterBootloader, ResponsePayload::Empty) => true,
            _ => false,
        },
        Status::PartialApply => {
            command.is_overlay_write() && matches!(payload, ResponsePayload::OverlayAck { .. })
        }
        _ => matches!(payload, ResponsePayload::Empty),
    }
}

/// Encode a response into `out`; returns the number of bytes written.
pub fn encode_response(resp: &Response, out: &mut [u8]) -> Result<usize, EncodeError> {
    if !payload_matches(resp.command, resp.status, &resp.payload) {
        return Err(EncodeError::PayloadMismatch);
    }
    let mut w = Writer::new(out);
    w.u8(resp.command.opcode() | RESPONSE_FLAG)?;
    w.u8(resp.request_id)?;
    w.u8(resp.status as u8)?;
    w.u16(0)?; // payload_len, patched below
    match &resp.payload {
        ResponsePayload::Empty => {}
        ResponsePayload::Capabilities(c) => {
            w.u8(c.protocol_major)?;
            w.u8(c.protocol_minor)?;
            w.u8(c.led_count_left)?;
            w.u8(c.led_count_right)?;
            w.u8(c.layer_capacity)?;
            w.u8(c.max_cells_per_op)?;
            w.u16(c.effect_mask)?;
            w.u16(c.overlay_cell_capacity)?;
            w.u16(c.max_message_len)?;
            w.u32(c.feature_bits)?;
            if c.feature_bits & feature::PERSISTENT_CONFIG != 0 {
                w.u32(c.max_config_blob_len)?;
            }
            if c.feature_bits & feature::KEYMAP != 0 {
                w.u8(c.keymap_rows)?;
                w.u8(c.keymap_cols)?;
                w.u8(c.max_keymap_entries_per_op)?;
                w.u8(0)?; // reserved
            }
        }
        ResponsePayload::Echo { data } => w.bytes(data)?,
        ResponsePayload::Version(v) => {
            v.central.write(&mut w)?;
            v.peripheral.write(&mut w)?;
            w.u8(v.halves_mismatch as u8)?;
        }
        ResponsePayload::OverlayAck { pending_keys } => {
            w.u8(pending_keys.len() as u8)?;
            w.bytes(pending_keys)?;
        }
        ResponsePayload::OverlayState { cells } => {
            w.u8(cells.len() as u8)?;
            for cell in cells {
                w.u8(cell.key)?;
                cell.effect.write(&mut w)?;
                w.u32(cell.remaining_ttl_ms)?;
            }
        }
        ResponsePayload::Brightness { level } => w.u8(*level)?,
        ResponsePayload::Toggle { id, state } => {
            w.u8(*id)?;
            w.u8(*state as u8)?;
        }
        ResponsePayload::ConfigData { total_len, data } => {
            w.u32(*total_len)?;
            w.bytes(data)?;
        }
        ResponsePayload::KeymapActions { layer, start_key, keycodes } => {
            w.u8(*layer)?;
            w.u8(*start_key)?;
            w.u8(keycodes.len() as u8)?;
            for kc in keycodes {
                w.u16(*kc)?;
            }
        }
        ResponsePayload::KeymapWritten { keycodes } => {
            w.u8(keycodes.len() as u8)?;
            for kc in keycodes {
                w.u16(*kc)?;
            }
        }
    }
    let len = w.pos();
    w.patch_u16(3, (len - RESPONSE_HEADER_LEN) as u16);
    Ok(len)
}

/// Decode a complete response message.
pub fn decode_response(bytes: &[u8]) -> Result<Response, DecodeError> {
    let mut r = Reader::new(bytes);
    let opcode = r.u8()?;
    if opcode & RESPONSE_FLAG == 0 {
        return Err(DecodeError::UnknownOpcode(opcode));
    }
    let command = Command::from_opcode(opcode & !RESPONSE_FLAG)
        .ok_or(DecodeError::UnknownOpcode(opcode))?;
    let request_id = r.u8()?;
    let status_byte = r.u8()?;
    let status = Status::from_u8(status_byte).ok_or(DecodeError::UnknownStatus(status_byte))?;
    let payload_len = r.u16()? as usize;
    if r.remaining() != payload_len {
        return Err(DecodeError::LengthMismatch);
    }
    let payload = match status {
        Status::Ok => match command {
            Command::GetCapabilities => {
                let mut caps = Capabilities {
                    protocol_major: r.u8()?,
                    protocol_minor: r.u8()?,
                    led_count_left: r.u8()?,
                    led_count_right: r.u8()?,
                    layer_capacity: r.u8()?,
                    max_cells_per_op: r.u8()?,
                    effect_mask: r.u16()?,
                    overlay_cell_capacity: r.u16()?,
                    max_message_len: r.u16()?,
                    feature_bits: r.u32()?,
                    max_config_blob_len: 0,
                    keymap_rows: 0,
                    keymap_cols: 0,
                    max_keymap_entries_per_op: 0,
                };
                if caps.feature_bits & feature::PERSISTENT_CONFIG != 0 {
                    caps.max_config_blob_len = r.u32()?;
                }
                if caps.feature_bits & feature::KEYMAP != 0 {
                    caps.keymap_rows = r.u8()?;
                    caps.keymap_cols = r.u8()?;
                    caps.max_keymap_entries_per_op = r.u8()?;
                    let _reserved = r.u8()?;
                }
                ResponsePayload::Capabilities(caps)
            }
            Command::Ping => {
                if payload_len > MAX_PING_LEN {
                    return Err(DecodeError::CapacityExceeded);
                }
                let mut data = Vec::new();
                data.extend_from_slice(r.bytes(payload_len)?)
                    .map_err(|_| DecodeError::CapacityExceeded)?;
                ResponsePayload::Echo { data }
            }
            Command::GetVersion => {
                let central = HalfVersion::read(&mut r)?;
                let peripheral = HalfVersion::read(&mut r)?;
                let halves_mismatch = read_flag(&mut r)?;
                ResponsePayload::Version(VersionInfo { central, peripheral, halves_mismatch })
            }
            c if c.is_overlay_write() => read_overlay_ack(&mut r)?,
            Command::ReadOverlay => {
                let count = r.u8()? as usize;
                let mut cells = Vec::new();
                for _ in 0..count {
                    let key = r.u8()?;
                    let effect = Effect::read(&mut r)?;
                    let remaining_ttl_ms = r.u32()?;
                    cells
                        .push(CellState { key, effect, remaining_ttl_ms })
                        .map_err(|_| DecodeError::CapacityExceeded)?;
                }
                ResponsePayload::OverlayState { cells }
            }
            Command::GetBrightness | Command::SetBrightness => {
                ResponsePayload::Brightness { level: r.u8()? }
            }
            Command::GetToggle | Command::SetToggle => {
                let id = r.u8()?;
                let state = match r.u8()? {
                    0 => false,
                    1 => true,
                    v => return Err(DecodeError::BadToggleState(v)),
                };
                ResponsePayload::Toggle { id, state }
            }
            Command::ConfigBegin
            | Command::ConfigData
            | Command::ConfigCommit
            | Command::ConfigAbort => ResponsePayload::Empty,
            Command::ConfigRead => {
                let total_len = r.u32()?;
                let len = r.remaining();
                if len > MAX_CONFIG_DATA_PER_MESSAGE {
                    return Err(DecodeError::CapacityExceeded);
                }
                let mut data = Vec::new();
                data.extend_from_slice(r.bytes(len)?)
                    .map_err(|_| DecodeError::CapacityExceeded)?;
                ResponsePayload::ConfigData { total_len, data }
            }
            Command::KeymapRead => {
                let layer = r.u8()?;
                let start_key = r.u8()?;
                let count = r.u8()? as usize;
                if count > MAX_KEYMAP_ENTRIES_PER_MESSAGE {
                    return Err(DecodeError::CapacityExceeded);
                }
                let mut keycodes = Vec::new();
                for _ in 0..count {
                    keycodes.push(r.u16()?).map_err(|_| DecodeError::CapacityExceeded)?;
                }
                ResponsePayload::KeymapActions { layer, start_key, keycodes }
            }
            Command::KeymapWrite => {
                let count = r.u8()? as usize;
                if count > MAX_KEYMAP_ENTRIES_PER_MESSAGE {
                    return Err(DecodeError::CapacityExceeded);
                }
                let mut keycodes = Vec::new();
                for _ in 0..count {
                    keycodes.push(r.u16()?).map_err(|_| DecodeError::CapacityExceeded)?;
                }
                ResponsePayload::KeymapWritten { keycodes }
            }
            Command::EnterBootloader => ResponsePayload::Empty,
            // All commands are covered above; this arm is unreachable.
            _ => return Err(DecodeError::InvalidStatusForCommand),
        },
        Status::PartialApply => {
            if !command.is_overlay_write() {
                return Err(DecodeError::InvalidStatusForCommand);
            }
            read_overlay_ack(&mut r)?
        }
        _ => ResponsePayload::Empty,
    };
    r.finish()?;
    Ok(Response { request_id, command, status, payload })
}

fn read_overlay_ack(r: &mut Reader<'_>) -> Result<ResponsePayload, DecodeError> {
    let count = r.u8()? as usize;
    if count > MAX_CELLS_PER_MESSAGE {
        return Err(DecodeError::CapacityExceeded);
    }
    let mut pending_keys = Vec::new();
    pending_keys
        .extend_from_slice(r.bytes(count)?)
        .map_err(|_| DecodeError::CapacityExceeded)?;
    Ok(ResponsePayload::OverlayAck { pending_keys })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BOOTLOADER_MAGIC, MAX_MESSAGE_LEN};

    fn roundtrip_request(req: Request) {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let len = encode_request(0x42, &req, &mut buf).unwrap();
        let (id, decoded) = decode_request(&buf[..len]).unwrap();
        assert_eq!(id, 0x42);
        assert_eq!(decoded, req);
    }

    fn roundtrip_response(resp: Response) {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let len = encode_response(&resp, &mut buf).unwrap();
        assert_eq!(decode_response(&buf[..len]).unwrap(), resp);
    }

    fn sample_cells(n: usize) -> Vec<CellWrite, MAX_CELLS_PER_MESSAGE> {
        let mut cells = Vec::new();
        for i in 0..n {
            cells
                .push(CellWrite {
                    key: i as u8,
                    effect: Effect::blink(i as u8, 0x80, 0xFF - i as u8, 500, 100, 50),
                })
                .unwrap();
        }
        cells
    }

    #[test]
    fn requests_roundtrip() {
        roundtrip_request(Request::GetCapabilities { client_major: 1, client_minor: 0 });
        roundtrip_request(Request::Ping { data: Vec::from_slice(&[1, 2, 3]).unwrap() });
        roundtrip_request(Request::Ping { data: Vec::new() });
        roundtrip_request(Request::GetVersion);
        roundtrip_request(Request::SetCells { ttl_ms: 12345, cells: sample_cells(3) });
        roundtrip_request(Request::SetCells { ttl_ms: 0, cells: Vec::new() });
        roundtrip_request(Request::UnsetCells { keys: Vec::from_slice(&[0, 40, 79]).unwrap() });
        roundtrip_request(Request::ClearOverlay);
        roundtrip_request(Request::ReadOverlay);
        roundtrip_request(Request::ReplaceOverlay { ttl_ms: 0, cells: sample_cells(80) });
        roundtrip_request(Request::GetBrightness);
        roundtrip_request(Request::SetBrightness { level: 200 });
        roundtrip_request(Request::GetToggle { id: 7 });
        roundtrip_request(Request::SetToggle { id: 7, state: true });
        roundtrip_request(Request::EnterBootloader {
            magic: BOOTLOADER_MAGIC,
            target: BootTarget::Peripheral,
        });
        roundtrip_request(Request::ConfigBegin { total_len: 7148, blob_crc32: 0xDEAD_BEEF });
        roundtrip_request(Request::ConfigData {
            offset: 1024,
            data: Vec::from_slice(&[0xAA; 100]).unwrap(),
        });
        roundtrip_request(Request::ConfigData { offset: 0, data: Vec::new() });
        roundtrip_request(Request::ConfigCommit);
        roundtrip_request(Request::ConfigAbort);
        roundtrip_request(Request::ConfigRead { offset: 4096, max_len: 1024 });
        roundtrip_request(Request::KeymapRead { layer: 3, start_key: 42, max_count: 84 });
        roundtrip_request(Request::KeymapWrite {
            entries: Vec::from_slice(&[
                KeymapEntry { layer: 0, key: 0, keycode: 0x0004 },
                KeymapEntry { layer: 7, key: 83, keycode: 0x5220 },
            ])
            .unwrap(),
        });
        roundtrip_request(Request::KeymapWrite { entries: Vec::new() });
    }

    fn sample_half(present: bool, hash: &[u8; 8], dirty: bool) -> HalfVersion {
        HalfVersion {
            present,
            fw_major: 0,
            fw_minor: 1,
            fw_patch: 0,
            git_hash: *hash,
            dirty,
        }
    }

    #[test]
    fn version_responses_roundtrip() {
        roundtrip_response(Response {
            request_id: 60,
            command: Command::GetVersion,
            status: Status::Ok,
            payload: ResponsePayload::Version(VersionInfo {
                central: sample_half(true, b"1a2b3c4d", true),
                peripheral: sample_half(false, b"unknown0", false),
                halves_mismatch: false,
            }),
        });
        roundtrip_response(Response {
            request_id: 61,
            command: Command::GetVersion,
            status: Status::Ok,
            payload: ResponsePayload::Version(VersionInfo {
                central: sample_half(true, b"1a2b3c4d", false),
                peripheral: sample_half(true, b"9f8e7d6c", false),
                halves_mismatch: true,
            }),
        });
        roundtrip_response(Response {
            request_id: 62,
            command: Command::GetVersion,
            status: Status::UnknownCommand,
            payload: ResponsePayload::Empty,
        });
        // The payload only pairs with GET_VERSION.
        let resp = Response {
            request_id: 63,
            command: Command::Ping,
            status: Status::Ok,
            payload: ResponsePayload::Version(VersionInfo::default()),
        };
        let mut buf = [0u8; 64];
        assert_eq!(encode_response(&resp, &mut buf), Err(EncodeError::PayloadMismatch));
        // Flag bytes must be 0 or 1.
        let resp = Response {
            request_id: 64,
            command: Command::GetVersion,
            status: Status::Ok,
            payload: ResponsePayload::Version(VersionInfo::default()),
        };
        let len = encode_response(&resp, &mut buf).unwrap();
        assert_eq!(len, RESPONSE_HEADER_LEN + VersionInfo::ENCODED_LEN);
        buf[RESPONSE_HEADER_LEN] = 2; // central present flag
        assert_eq!(decode_response(&buf[..len]), Err(DecodeError::BadFlag(2)));
    }

    #[test]
    fn keymap_responses_roundtrip() {
        roundtrip_response(Response {
            request_id: 50,
            command: Command::KeymapRead,
            status: Status::Ok,
            payload: ResponsePayload::KeymapActions {
                layer: 2,
                start_key: 10,
                keycodes: Vec::from_slice(&[0x0000, 0x0001, 0x7C00, 0x0F00 | 0x04]).unwrap(),
            },
        });
        roundtrip_response(Response {
            request_id: 51,
            command: Command::KeymapWrite,
            status: Status::Ok,
            payload: ResponsePayload::KeymapWritten {
                keycodes: Vec::from_slice(&[0x0004, 0x5220]).unwrap(),
            },
        });
        for status in [Status::OutOfRange, Status::CapacityExceeded] {
            roundtrip_response(Response {
                request_id: 52,
                command: Command::KeymapWrite,
                status,
                payload: ResponsePayload::Empty,
            });
        }
        // Keymap payloads only pair with their own commands.
        let resp = Response {
            request_id: 53,
            command: Command::KeymapRead,
            status: Status::Ok,
            payload: ResponsePayload::KeymapWritten { keycodes: Vec::new() },
        };
        let mut buf = [0u8; 64];
        assert_eq!(encode_response(&resp, &mut buf), Err(EncodeError::PayloadMismatch));
    }

    #[test]
    fn max_keymap_batches_fit_in_max_message_len() {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let mut entries = Vec::new();
        for i in 0..MAX_KEYMAP_ENTRIES_PER_MESSAGE {
            entries
                .push(KeymapEntry { layer: (i % 8) as u8, key: (i % 84) as u8, keycode: i as u16 })
                .unwrap();
        }
        let len = encode_request(1, &Request::KeymapWrite { entries }, &mut buf).unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
        let mut keycodes = Vec::new();
        for i in 0..MAX_KEYMAP_ENTRIES_PER_MESSAGE {
            keycodes.push(i as u16).unwrap();
        }
        let len = encode_response(
            &Response {
                request_id: 1,
                command: Command::KeymapRead,
                status: Status::Ok,
                payload: ResponsePayload::KeymapActions { layer: 0, start_key: 0, keycodes },
            },
            &mut buf,
        )
        .unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
    }

    #[test]
    fn keymap_write_count_mismatch_rejected() {
        // Says 2 entries, carries 1 (payload_len covers only one entry).
        let mut buf = [0u8; 64];
        let entries = Vec::from_slice(&[KeymapEntry { layer: 0, key: 1, keycode: 0x0004 }]).unwrap();
        let len = encode_request(9, &Request::KeymapWrite { entries }, &mut buf).unwrap();
        buf[4] = 2; // entry_count
        assert_eq!(decode_request(&buf[..len]), Err(DecodeError::Truncated));
    }

    #[test]
    fn config_responses_roundtrip() {
        for (command, status) in [
            (Command::ConfigBegin, Status::Ok),
            (Command::ConfigBegin, Status::CapacityExceeded),
            (Command::ConfigData, Status::Ok),
            (Command::ConfigData, Status::NoSession),
            (Command::ConfigData, Status::BadOffset),
            (Command::ConfigCommit, Status::Ok),
            (Command::ConfigCommit, Status::ConfigIncomplete),
            (Command::ConfigCommit, Status::CrcMismatch),
            (Command::ConfigCommit, Status::InvalidConfig),
            (Command::ConfigAbort, Status::Ok),
        ] {
            roundtrip_response(Response {
                request_id: 20,
                command,
                status,
                payload: ResponsePayload::Empty,
            });
        }
        roundtrip_response(Response {
            request_id: 21,
            command: Command::ConfigRead,
            status: Status::Ok,
            payload: ResponsePayload::ConfigData {
                total_len: 7148,
                data: Vec::from_slice(&[1, 2, 3, 4, 5]).unwrap(),
            },
        });
        roundtrip_response(Response {
            request_id: 22,
            command: Command::ConfigRead,
            status: Status::Ok,
            payload: ResponsePayload::ConfigData { total_len: 64, data: Vec::new() },
        });
        // ConfigData payload only pairs with CONFIG_READ.
        let resp = Response {
            request_id: 23,
            command: Command::ConfigCommit,
            status: Status::Ok,
            payload: ResponsePayload::ConfigData { total_len: 0, data: Vec::new() },
        };
        let mut buf = [0u8; 64];
        assert_eq!(encode_response(&resp, &mut buf), Err(EncodeError::PayloadMismatch));
    }

    #[test]
    fn max_config_chunks_fit_in_max_message_len() {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let data: Vec<u8, MAX_CONFIG_DATA_PER_MESSAGE> =
            Vec::from_slice(&[0x5A; MAX_CONFIG_DATA_PER_MESSAGE]).unwrap();
        let len = encode_request(
            1,
            &Request::ConfigData { offset: 0, data: data.clone() },
            &mut buf,
        )
        .unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
        let len = encode_response(
            &Response {
                request_id: 1,
                command: Command::ConfigRead,
                status: Status::Ok,
                payload: ResponsePayload::ConfigData { total_len: 7148, data },
            },
            &mut buf,
        )
        .unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
    }

    #[test]
    fn responses_roundtrip() {
        roundtrip_response(Response {
            request_id: 1,
            command: Command::GetCapabilities,
            status: Status::Ok,
            payload: ResponsePayload::Capabilities(Capabilities {
                protocol_major: 1,
                protocol_minor: 0,
                led_count_left: 40,
                led_count_right: 40,
                layer_capacity: 8,
                max_cells_per_op: 80,
                effect_mask: 0b111,
                overlay_cell_capacity: 80,
                max_message_len: 1536,
                feature_bits: 0x3F,
                max_config_blob_len: 0,
                keymap_rows: 0,
                keymap_cols: 0,
                max_keymap_entries_per_op: 0,
            }),
        });
        // v1.1: the persistent-config bit gates the blob-length extension.
        roundtrip_response(Response {
            request_id: 1,
            command: Command::GetCapabilities,
            status: Status::Ok,
            payload: ResponsePayload::Capabilities(Capabilities {
                protocol_major: 1,
                protocol_minor: 1,
                led_count_left: 40,
                led_count_right: 40,
                layer_capacity: 8,
                max_cells_per_op: 80,
                effect_mask: 0b111,
                overlay_cell_capacity: 80,
                max_message_len: 1536,
                feature_bits: 0x7F,
                max_config_blob_len: 7148,
                keymap_rows: 0,
                keymap_cols: 0,
                max_keymap_entries_per_op: 0,
            }),
        });
        // v1.2: the keymap bit gates the keymap extension (here together
        // with the persistent-config extension, in feature-bit order).
        roundtrip_response(Response {
            request_id: 1,
            command: Command::GetCapabilities,
            status: Status::Ok,
            payload: ResponsePayload::Capabilities(Capabilities {
                protocol_major: 1,
                protocol_minor: 2,
                led_count_left: 40,
                led_count_right: 40,
                layer_capacity: 8,
                max_cells_per_op: 80,
                effect_mask: 0b111,
                overlay_cell_capacity: 80,
                max_message_len: 1536,
                feature_bits: 0xFF,
                max_config_blob_len: 7148,
                keymap_rows: 6,
                keymap_cols: 14,
                max_keymap_entries_per_op: 84,
            }),
        });
        roundtrip_response(Response {
            request_id: 2,
            command: Command::Ping,
            status: Status::Ok,
            payload: ResponsePayload::Echo { data: Vec::from_slice(&[9, 8, 7]).unwrap() },
        });
        roundtrip_response(Response {
            request_id: 3,
            command: Command::SetCells,
            status: Status::PartialApply,
            payload: ResponsePayload::OverlayAck {
                pending_keys: Vec::from_slice(&[41, 42]).unwrap(),
            },
        });
        roundtrip_response(Response {
            request_id: 4,
            command: Command::ClearOverlay,
            status: Status::Ok,
            payload: ResponsePayload::OverlayAck { pending_keys: Vec::new() },
        });
        let mut cells = Vec::new();
        cells
            .push(CellState { key: 12, effect: Effect::solid(1, 2, 3), remaining_ttl_ms: 0 })
            .unwrap();
        cells
            .push(CellState {
                key: 60,
                effect: Effect::breathe(0, 0, 255, 3000, 1500),
                remaining_ttl_ms: 4200,
            })
            .unwrap();
        roundtrip_response(Response {
            request_id: 5,
            command: Command::ReadOverlay,
            status: Status::Ok,
            payload: ResponsePayload::OverlayState { cells },
        });
        roundtrip_response(Response {
            request_id: 6,
            command: Command::SetBrightness,
            status: Status::Ok,
            payload: ResponsePayload::Brightness { level: 128 },
        });
        roundtrip_response(Response {
            request_id: 7,
            command: Command::SetToggle,
            status: Status::Ok,
            payload: ResponsePayload::Toggle { id: 2, state: false },
        });
        roundtrip_response(Response {
            request_id: 8,
            command: Command::EnterBootloader,
            status: Status::BadMagic,
            payload: ResponsePayload::Empty,
        });
    }

    #[test]
    fn rejects_length_mismatch() {
        let mut buf = [0u8; 64];
        let len = encode_request(1, &Request::GetBrightness, &mut buf).unwrap();
        // Trailing garbage.
        assert_eq!(decode_request(&buf[..len + 1]), Err(DecodeError::LengthMismatch));
        // Truncated header.
        assert_eq!(decode_request(&buf[..2]), Err(DecodeError::Truncated));
        // Payload_len larger than buffer.
        assert_eq!(
            decode_request(&[0x12u8, 0x01, 0x05, 0x00]),
            Err(DecodeError::LengthMismatch)
        );
        // Inner count disagrees with payload length: says 3 keys, has 1.
        assert_eq!(
            decode_request(&[0x11, 0x01, 0x02, 0x00, 0x03, 0x05]),
            Err(DecodeError::Truncated)
        );
    }

    #[test]
    fn rejects_unknown_discriminants() {
        assert_eq!(decode_request(&[0x77, 0, 0, 0]), Err(DecodeError::UnknownOpcode(0x77)));
        // Response flag missing on a response decode.
        assert_eq!(decode_response(&[0x12, 0, 0, 0, 0]), Err(DecodeError::UnknownOpcode(0x12)));
        assert_eq!(
            decode_response(&[0x92, 0, 0xEE, 0, 0]),
            Err(DecodeError::UnknownStatus(0xEE))
        );
        // Unknown effect kind inside a cell.
        let mut buf = [0u8; 64];
        let mut cells = Vec::new();
        cells.push(CellWrite { key: 0, effect: Effect::solid(0, 0, 0) }).unwrap();
        let len = encode_request(1, &Request::SetCells { ttl_ms: 0, cells }, &mut buf).unwrap();
        buf[10] = 9; // effect kind byte of the first cell (4 header + 4 ttl + 1 count + 1 key)
        assert_eq!(decode_request(&buf[..len]), Err(DecodeError::UnknownEffectKind(9)));
    }

    #[test]
    fn rejects_mismatched_response_payload() {
        let resp = Response {
            request_id: 1,
            command: Command::Ping,
            status: Status::Ok,
            payload: ResponsePayload::Brightness { level: 1 },
        };
        let mut buf = [0u8; 64];
        assert_eq!(encode_response(&resp, &mut buf), Err(EncodeError::PayloadMismatch));
        // Error statuses must carry an empty payload.
        let resp = Response {
            request_id: 1,
            command: Command::Ping,
            status: Status::Busy,
            payload: ResponsePayload::Echo { data: Vec::new() },
        };
        assert_eq!(encode_response(&resp, &mut buf), Err(EncodeError::PayloadMismatch));
        // PartialApply only on overlay writes.
        assert_eq!(
            decode_response(&[0xA0, 1, 0x05, 1, 0, 0]),
            Err(DecodeError::InvalidStatusForCommand)
        );
    }

    #[test]
    fn rejects_small_buffers() {
        let mut buf = [0u8; 3];
        assert_eq!(
            encode_request(1, &Request::GetBrightness, &mut buf),
            Err(EncodeError::BufferTooSmall)
        );
    }

    #[test]
    fn max_batch_fits_in_max_message_len() {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let len = encode_request(
            1,
            &Request::ReplaceOverlay { ttl_ms: u32::MAX, cells: sample_cells(80) },
            &mut buf,
        )
        .unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
        // Full read-back with TTLs also fits.
        let mut cells = Vec::new();
        for i in 0..80u8 {
            cells
                .push(CellState {
                    key: i,
                    effect: Effect::solid(i, i, i),
                    remaining_ttl_ms: u32::MAX,
                })
                .unwrap();
        }
        let resp = Response {
            request_id: 1,
            command: Command::ReadOverlay,
            status: Status::Ok,
            payload: ResponsePayload::OverlayState { cells },
        };
        let len = encode_response(&resp, &mut buf).unwrap();
        assert!(len <= MAX_MESSAGE_LEN);
    }
}
