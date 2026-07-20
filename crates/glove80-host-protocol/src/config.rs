//! Persistent lighting configuration blob (protocol v1.1).
//!
//! The blob is the unit of persistence and transfer: hosts assemble it, ship
//! it via CONFIG_BEGIN/DATA/COMMIT, firmware validates it with
//! [`decode_lighting_config`] (or just [`validate_lighting_config`]), persists
//! the bytes verbatim, and serves them back byte-stable via CONFIG_READ.
//!
//! Layout (all little-endian):
//!
//! ```text
//! header (16 bytes):
//!   magic      u32 = 0x4C303847 ("G80L")
//!   version    u16 = 1
//!   reserved   u16 (encode 0)
//!   body_len   u32
//!   body_crc32 u32   CRC-32/ISO-HDLC over the body bytes
//! body:
//!   record_count         u8   (<= 16)
//!   toggle_persist_mask  u32
//!   toggle_initial_state u32
//!   reserved             [u8; 3] (encode 0)
//!   records: record_count x {
//!     activation     u8   (0 always, 1 layer-active, 2 toggle)
//!     activation_arg u8   (layer < 8 / toggle id < 32; 0 for always)
//!     gate_kind      u8   (0 no gate, else a condition kind; was reserved)
//!     gate_arg       u8   (layer/toggle id, or 0; was reserved)
//!     cell_count     u8   (<= 40)
//!     cells: cell_count x { key u8 (< 80), effect (10-byte v1 record) }
//!   }
//! ```
//!
//! Record order in the blob = composition order within each activation
//! class. Sparse maps: a key absent from a record is transparent; a key may
//! appear at most once per record. Host-overlay and status records are not
//! persistable.
//!
//! ## Gates (conditional lighting)
//!
//! The per-record `gate_kind`/`gate_arg` bytes occupy what was a reserved
//! `u16` (the low byte is `gate_kind`, the high byte `gate_arg`, matching the
//! little-endian `u16`). An optional **gate** is a second condition that must
//! also hold for the record to compose (a logical AND with the activation).
//! `gate_kind == 0` is "no gate" — every pre-gate blob wrote 0 into these
//! bytes, so old blobs decode unchanged and re-encode byte-identically, and
//! the blob format version is NOT bumped. Old firmware accepts the all-zero
//! reserved value but rejects a nonzero (unknown) gate kind as
//! `INVALID_CONFIG`; firmware that advertises gates validates and honors the
//! mapping below. Hosts learn whether a keyboard understands gates from the
//! `CONFIG_GATES` capability feature bit.
//!
//! Gate kinds mirror the compositor's condition wire mapping:
//! `1` layer-active(arg `< 8`), `2` toggle(arg `< 32`), `3` usb-connected,
//! `4` charging, `5` split-link-up (kinds 3–5 require `gate_arg == 0`).

use heapless::Vec;

use crate::error::{DecodeError, EncodeError};
use crate::io::{Reader, Writer};
use crate::message::{CellWrite, Effect};

/// Blob magic ("G80L" read as a little-endian u32).
pub const CONFIG_MAGIC: u32 = 0x4C30_3847;
/// Blob format version this codec reads and writes.
pub const CONFIG_VERSION: u16 = 1;
/// Fixed header: magic, version, reserved, body_len, body_crc32.
pub const CONFIG_HEADER_LEN: usize = 16;
/// Fixed body prefix: record_count, masks, reserved.
pub const CONFIG_BODY_HEADER_LEN: usize = 12;
/// Fixed per-record prefix: activation, arg, reserved, cell_count.
pub const CONFIG_RECORD_HEADER_LEN: usize = 5;
/// Persistable records per config (mirrors the compositor's capacity).
pub const MAX_CONFIG_RECORDS: usize = 16;
/// Cells per record (mirrors the compositor's capacity).
pub const MAX_CELLS_PER_RECORD: usize = 40;
/// Key index space: `0..80` (left half `0..40`, right half `40..80`).
pub const CONFIG_KEY_COUNT: u8 = 80;
/// Layer arg space for layer-active records.
pub const CONFIG_LAYER_COUNT: u8 = 8;
/// Toggle id space for toggle records.
pub const CONFIG_TOGGLE_COUNT: u8 = 32;
/// Largest possible blob: header + body prefix + 16 full records.
pub const MAX_CONFIG_BLOB_LEN: usize = CONFIG_HEADER_LEN
    + CONFIG_BODY_HEADER_LEN
    + MAX_CONFIG_RECORDS
        * (CONFIG_RECORD_HEADER_LEN + MAX_CELLS_PER_RECORD * (1 + Effect::ENCODED_LEN));

/// Activation predicate of a persistable record. Host-overlay and status
/// records are firmware/runtime state and deliberately not representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigActivation {
    /// Base lighting, always composed.
    Always,
    /// Composed while this keymap layer (`< 8`) is active.
    LayerActive(u8),
    /// Composed while this toggle (`< 32`) is on.
    Toggle(u8),
}

impl ConfigActivation {
    /// Wire `(activation, activation_arg)` pair.
    pub fn to_wire(self) -> (u8, u8) {
        match self {
            ConfigActivation::Always => (0, 0),
            ConfigActivation::LayerActive(layer) => (1, layer),
            ConfigActivation::Toggle(id) => (2, id),
        }
    }

    /// Parse and range-check a wire `(activation, activation_arg)` pair.
    pub fn from_wire(kind: u8, arg: u8) -> Result<ConfigActivation, ConfigError> {
        match kind {
            0 => Ok(ConfigActivation::Always),
            1 if arg < CONFIG_LAYER_COUNT => Ok(ConfigActivation::LayerActive(arg)),
            1 => Err(ConfigError::LayerOutOfRange(arg)),
            2 if arg < CONFIG_TOGGLE_COUNT => Ok(ConfigActivation::Toggle(arg)),
            2 => Err(ConfigError::ToggleOutOfRange(arg)),
            k => Err(ConfigError::UnknownActivation(k)),
        }
    }
}

/// An optional per-record gate: a second condition that must also hold for
/// the record to compose (a logical AND with the activation). The wire
/// `(kind, arg)` mapping mirrors the compositor's `Condition` so the same
/// gate survives config transfer and split forwarding unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigGate {
    /// Holds while this keymap layer (`< 8`) is active.
    LayerActive(u8),
    /// Holds while this toggle (`< 32`) is on.
    Toggle(u8),
    /// Holds while the central reports an active USB data connection.
    UsbConnected,
    /// Holds while this half sees USB bus power.
    Charging,
    /// Holds while this half's split link is up.
    SplitLinkUp,
}

impl ConfigGate {
    /// Wire `(gate_kind, gate_arg)` pair. `kind == 0` is reserved for
    /// "no gate" (see [`gate_to_wire`](ConfigGate::gate_to_wire)).
    pub fn to_wire(self) -> (u8, u8) {
        match self {
            ConfigGate::LayerActive(layer) => (1, layer),
            ConfigGate::Toggle(id) => (2, id),
            ConfigGate::UsbConnected => (3, 0),
            ConfigGate::Charging => (4, 0),
            ConfigGate::SplitLinkUp => (5, 0),
        }
    }

    /// Wire `(gate_kind, gate_arg)` for an optional gate; `None` is `(0, 0)`,
    /// the all-zero bytes every ungated (and pre-gate) record writes.
    pub fn gate_to_wire(gate: Option<ConfigGate>) -> (u8, u8) {
        match gate {
            None => (0, 0),
            Some(g) => g.to_wire(),
        }
    }

    /// Parse and range-check a wire `(gate_kind, gate_arg)` pair. `kind == 0`
    /// is no gate (`Ok(None)`); a known kind with an in-range arg is
    /// `Ok(Some(_))`; anything else is a [`ConfigError`] so the whole blob is
    /// rejected before it can be applied.
    pub fn from_wire(kind: u8, arg: u8) -> Result<Option<ConfigGate>, ConfigError> {
        Ok(match kind {
            0 => None,
            1 if arg < CONFIG_LAYER_COUNT => Some(ConfigGate::LayerActive(arg)),
            1 => return Err(ConfigError::GateLayerOutOfRange(arg)),
            2 if arg < CONFIG_TOGGLE_COUNT => Some(ConfigGate::Toggle(arg)),
            2 => return Err(ConfigError::GateToggleOutOfRange(arg)),
            3..=5 if arg != 0 => return Err(ConfigError::GateArgNonZero(arg)),
            3 => Some(ConfigGate::UsbConnected),
            4 => Some(ConfigGate::Charging),
            5 => Some(ConfigGate::SplitLinkUp),
            k => return Err(ConfigError::UnknownGate(k)),
        })
    }
}

/// One persistable lighting record: an activation predicate, an optional
/// [`gate`](Self::gate), and a sparse key → effect map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRecord {
    pub activation: ConfigActivation,
    /// Optional gate condition (conditional lighting); `None` = ungated, the
    /// default and the byte-compatible pre-gate case.
    pub gate: Option<ConfigGate>,
    pub cells: Vec<CellWrite, MAX_CELLS_PER_RECORD>,
}

/// A complete persistent lighting configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LightingConfig {
    /// Bit n set ⇔ toggle n's runtime state is persisted across reboots
    /// (toggle persistence is opt-in).
    pub toggle_persist_mask: u32,
    /// Bit n = toggle n's state on boot (for toggles without a persisted
    /// runtime state).
    pub toggle_initial_state: u32,
    pub records: Vec<ConfigRecord, MAX_CONFIG_RECORDS>,
}

impl Default for ConfigActivation {
    fn default() -> Self {
        ConfigActivation::Always
    }
}

/// Why a config blob was rejected. Any error means the blob must not be
/// applied; the previous configuration stays in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// Blob shorter than its header or a declared count.
    Truncated,
    /// Bytes left over after the declared records, or `body_len` disagrees
    /// with the actual blob length.
    LengthMismatch,
    /// Header magic is not `CONFIG_MAGIC`.
    BadMagic(u32),
    /// Header version is not `CONFIG_VERSION`.
    UnsupportedVersion(u16),
    /// `body_crc32` does not match the body bytes.
    CrcMismatch { expected: u32, actual: u32 },
    /// `record_count` exceeds [`MAX_CONFIG_RECORDS`].
    TooManyRecords(u8),
    /// A record's `cell_count` exceeds [`MAX_CELLS_PER_RECORD`].
    TooManyCells(u8),
    /// Activation byte is not a known persistable predicate.
    UnknownActivation(u8),
    /// Layer arg of a layer-active record is `>= 8`.
    LayerOutOfRange(u8),
    /// Toggle arg of a toggle record is `>= 32`.
    ToggleOutOfRange(u8),
    /// A cell key is `>= 80`.
    KeyOutOfRange(u8),
    /// A key appears more than once in the same record.
    DuplicateKey(u8),
    /// A cell's effect kind byte is unknown.
    UnknownEffectKind(u8),
    /// A record's `gate_kind` is not a known condition kind (old firmware
    /// rejects a future gate this way).
    UnknownGate(u8),
    /// Layer arg of a layer-active gate is `>= 8`.
    GateLayerOutOfRange(u8),
    /// Toggle arg of a toggle gate is `>= 32`.
    GateToggleOutOfRange(u8),
    /// A firmware-state gate (usb/charging/split-link) carried a nonzero arg.
    GateArgNonZero(u8),
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Truncated => write!(f, "config blob truncated"),
            ConfigError::LengthMismatch => write!(f, "config length fields disagree with blob"),
            ConfigError::BadMagic(m) => write!(f, "bad config magic 0x{m:08x}"),
            ConfigError::UnsupportedVersion(v) => write!(f, "unsupported config version {v}"),
            ConfigError::CrcMismatch { expected, actual } => {
                write!(f, "config body crc mismatch: header 0x{expected:08x}, body 0x{actual:08x}")
            }
            ConfigError::TooManyRecords(n) => {
                write!(f, "record count {n} exceeds {MAX_CONFIG_RECORDS}")
            }
            ConfigError::TooManyCells(n) => {
                write!(f, "cell count {n} exceeds {MAX_CELLS_PER_RECORD}")
            }
            ConfigError::UnknownActivation(k) => write!(f, "unknown activation {k}"),
            ConfigError::LayerOutOfRange(a) => {
                write!(f, "layer {a} out of range (< {CONFIG_LAYER_COUNT})")
            }
            ConfigError::ToggleOutOfRange(a) => {
                write!(f, "toggle {a} out of range (< {CONFIG_TOGGLE_COUNT})")
            }
            ConfigError::KeyOutOfRange(k) => {
                write!(f, "key {k} out of range (< {CONFIG_KEY_COUNT})")
            }
            ConfigError::DuplicateKey(k) => write!(f, "key {k} appears twice in one record"),
            ConfigError::UnknownEffectKind(k) => write!(f, "unknown effect kind {k}"),
            ConfigError::UnknownGate(k) => write!(f, "unknown gate kind {k}"),
            ConfigError::GateLayerOutOfRange(a) => {
                write!(f, "gate layer {a} out of range (< {CONFIG_LAYER_COUNT})")
            }
            ConfigError::GateToggleOutOfRange(a) => {
                write!(f, "gate toggle {a} out of range (< {CONFIG_TOGGLE_COUNT})")
            }
            ConfigError::GateArgNonZero(a) => {
                write!(f, "firmware-state gate carried nonzero arg {a}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ConfigError {}

impl From<DecodeError> for ConfigError {
    fn from(e: DecodeError) -> ConfigError {
        match e {
            DecodeError::UnknownEffectKind(k) => ConfigError::UnknownEffectKind(k),
            _ => ConfigError::Truncated,
        }
    }
}

/// CRC-32/ISO-HDLC (the zlib/PNG CRC: poly 0xEDB88320 reflected, init and
/// xorout 0xFFFFFFFF). `crc32(b"123456789") == 0xCBF43926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    // Half-byte lookup table for the reflected polynomial 0xEDB88320.
    const TABLE: [u32; 16] = [
        0x0000_0000, 0x1DB7_1064, 0x3B6E_20C8, 0x26D9_30AC, 0x76DC_4190, 0x6B6B_51F4, 0x4DB2_6158,
        0x5005_713C, 0xEDB8_8320, 0xF00F_9344, 0xD6D6_A3E8, 0xCB61_B38C, 0x9B64_C2B0, 0x86D3_D2D4,
        0xA00A_E278, 0xBDBD_F21C,
    ];
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc = (crc >> 4) ^ TABLE[((crc ^ b as u32) & 0xF) as usize];
        crc = (crc >> 4) ^ TABLE[((crc ^ (b as u32 >> 4)) & 0xF) as usize];
    }
    !crc
}

/// Exact encoded size of a config, header included.
pub fn encoded_config_len(config: &LightingConfig) -> usize {
    CONFIG_HEADER_LEN
        + CONFIG_BODY_HEADER_LEN
        + config
            .records
            .iter()
            .map(|rec| CONFIG_RECORD_HEADER_LEN + rec.cells.len() * (1 + Effect::ENCODED_LEN))
            .sum::<usize>()
}

/// Encode a config as a complete blob (header + body, CRC filled in).
/// Returns the number of bytes written. The output is canonical: encoding
/// the result of [`decode_lighting_config`] reproduces the input bytes.
pub fn encode_lighting_config(
    config: &LightingConfig,
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    let mut w = Writer::new(out);
    w.u32(CONFIG_MAGIC)?;
    w.u16(CONFIG_VERSION)?;
    w.u16(0)?; // reserved
    w.u32(0)?; // body_len, patched below
    w.u32(0)?; // body_crc32, patched below
    w.u8(config.records.len() as u8)?;
    w.u32(config.toggle_persist_mask)?;
    w.u32(config.toggle_initial_state)?;
    w.bytes(&[0u8; 3])?; // reserved
    for record in &config.records {
        let (kind, arg) = record.activation.to_wire();
        w.u8(kind)?;
        w.u8(arg)?;
        let (gate_kind, gate_arg) = ConfigGate::gate_to_wire(record.gate);
        w.u8(gate_kind)?; // was reserved (low byte)
        w.u8(gate_arg)?; // was reserved (high byte)
        w.u8(record.cells.len() as u8)?;
        for cell in &record.cells {
            w.u8(cell.key)?;
            cell.effect.write(&mut w)?;
        }
    }
    let total = w.pos();
    let body_len = (total - CONFIG_HEADER_LEN) as u32;
    let body_crc = crc32(&w.written()[CONFIG_HEADER_LEN..]);
    w.patch_u32(8, body_len);
    w.patch_u32(12, body_crc);
    Ok(total)
}

/// Decode and fully validate a config blob. Every check the firmware needs
/// before an atomic apply happens here: magic, version, lengths, body CRC,
/// record/cell counts, activation and effect kinds, key/layer/toggle ranges,
/// and per-record key uniqueness.
pub fn decode_lighting_config(bytes: &[u8]) -> Result<LightingConfig, ConfigError> {
    let mut r = Reader::new(bytes);
    let magic = r.u32()?;
    if magic != CONFIG_MAGIC {
        return Err(ConfigError::BadMagic(magic));
    }
    let version = r.u16()?;
    if version != CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion(version));
    }
    let _reserved = r.u16()?;
    let body_len = r.u32()? as usize;
    let expected_crc = r.u32()?;
    if r.remaining() != body_len {
        return Err(ConfigError::LengthMismatch);
    }
    let actual_crc = crc32(&bytes[CONFIG_HEADER_LEN..]);
    if actual_crc != expected_crc {
        return Err(ConfigError::CrcMismatch { expected: expected_crc, actual: actual_crc });
    }
    let record_count = r.u8()?;
    if record_count as usize > MAX_CONFIG_RECORDS {
        return Err(ConfigError::TooManyRecords(record_count));
    }
    let toggle_persist_mask = r.u32()?;
    let toggle_initial_state = r.u32()?;
    let _reserved = r.bytes(3)?;
    let mut records = Vec::new();
    for _ in 0..record_count {
        let kind = r.u8()?;
        let arg = r.u8()?;
        let activation = ConfigActivation::from_wire(kind, arg)?;
        let gate_kind = r.u8()?;
        let gate_arg = r.u8()?;
        let gate = ConfigGate::from_wire(gate_kind, gate_arg)?;
        let cell_count = r.u8()?;
        if cell_count as usize > MAX_CELLS_PER_RECORD {
            return Err(ConfigError::TooManyCells(cell_count));
        }
        let mut seen: u128 = 0;
        let mut cells = Vec::new();
        for _ in 0..cell_count {
            let key = r.u8()?;
            if key >= CONFIG_KEY_COUNT {
                return Err(ConfigError::KeyOutOfRange(key));
            }
            if seen & (1u128 << key) != 0 {
                return Err(ConfigError::DuplicateKey(key));
            }
            seen |= 1u128 << key;
            let effect = Effect::read(&mut r)?;
            // cell_count <= MAX_CELLS_PER_RECORD, so this cannot overflow.
            let _ = cells.push(CellWrite { key, effect });
        }
        // record_count <= MAX_CONFIG_RECORDS, so this cannot overflow.
        let _ = records.push(ConfigRecord { activation, gate, cells });
    }
    if r.remaining() != 0 {
        return Err(ConfigError::LengthMismatch);
    }
    Ok(LightingConfig { toggle_persist_mask, toggle_initial_state, records })
}

/// Validate a blob without keeping the decoded form.
pub fn validate_lighting_config(bytes: &[u8]) -> Result<(), ConfigError> {
    decode_lighting_config(bytes).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::EffectKind;

    fn sample_config() -> LightingConfig {
        let mut records = Vec::new();
        records
            .push(ConfigRecord {
                activation: ConfigActivation::Always,
                gate: None,
                cells: Vec::from_slice(&[
                    CellWrite { key: 0, effect: Effect::solid(10, 20, 30) },
                    CellWrite { key: 79, effect: Effect::blink(255, 0, 64, 1000, 250, 50) },
                ])
                .unwrap(),
            })
            .unwrap();
        records
            .push(ConfigRecord {
                activation: ConfigActivation::LayerActive(3),
                gate: None,
                cells: Vec::from_slice(&[CellWrite {
                    key: 40,
                    effect: Effect::breathe(16, 32, 48, 3000, 0),
                }])
                .unwrap(),
            })
            .unwrap();
        records
            .push(ConfigRecord {
                activation: ConfigActivation::Toggle(31),
                gate: None,
                cells: Vec::new(),
            })
            .unwrap();
        LightingConfig {
            toggle_persist_mask: 0x8000_0001,
            toggle_initial_state: 0x0000_0001,
            records,
        }
    }

    fn encode(config: &LightingConfig) -> std::vec::Vec<u8> {
        let mut buf = [0u8; MAX_CONFIG_BLOB_LEN];
        let len = encode_lighting_config(config, &mut buf).unwrap();
        assert_eq!(len, encoded_config_len(config));
        buf[..len].to_vec()
    }

    #[test]
    fn crc32_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn roundtrips() {
        for config in [LightingConfig::default(), sample_config()] {
            let bytes = encode(&config);
            let decoded = decode_lighting_config(&bytes).unwrap();
            assert_eq!(decoded, config);
            // Byte-stable: re-encoding reproduces the blob exactly.
            assert_eq!(encode(&decoded), bytes);
        }
    }

    #[test]
    fn max_config_matches_max_blob_len() {
        let mut records = Vec::new();
        for i in 0..MAX_CONFIG_RECORDS {
            let mut cells = Vec::new();
            for k in 0..MAX_CELLS_PER_RECORD {
                cells
                    .push(CellWrite {
                        key: k as u8,
                        effect: Effect::blink(i as u8, k as u8, 7, 500, 250, 50),
                    })
                    .unwrap();
            }
            records
                .push(ConfigRecord {
                    activation: ConfigActivation::Always,
                    gate: None,
                    cells,
                })
                .unwrap();
        }
        let config = LightingConfig {
            toggle_persist_mask: u32::MAX,
            toggle_initial_state: u32::MAX,
            records,
        };
        let bytes = encode(&config);
        assert_eq!(bytes.len(), MAX_CONFIG_BLOB_LEN);
        assert_eq!(decode_lighting_config(&bytes).unwrap(), config);
    }

    #[test]
    fn rejects_header_corruption() {
        let good = encode(&sample_config());

        let mut bad = good.clone();
        bad[0] = 0x00;
        assert!(matches!(decode_lighting_config(&bad), Err(ConfigError::BadMagic(_))));

        let mut bad = good.clone();
        bad[4] = 2;
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::UnsupportedVersion(2)));

        // Flip one body byte (a reserved byte, so parsing still succeeds
        // structurally): CRC must catch it.
        let mut bad = good.clone();
        bad[CONFIG_HEADER_LEN + 9] ^= 0xFF;
        assert!(matches!(decode_lighting_config(&bad), Err(ConfigError::CrcMismatch { .. })));

        // body_len disagreeing with the actual length.
        let mut bad = good.clone();
        bad[8] = bad[8].wrapping_add(1);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::LengthMismatch));

        // Truncated header.
        assert_eq!(decode_lighting_config(&good[..10]), Err(ConfigError::Truncated));

        // Truncated body (with a "fixed up" length + CRC the parser trusts):
        // reconstruct a blob whose declared counts overrun the body.
        let mut body = good[CONFIG_HEADER_LEN..].to_vec();
        body.truncate(body.len() - 4);
        let bad = reheader(&body);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::Truncated));

        // Trailing bytes after the declared records.
        let mut body = good[CONFIG_HEADER_LEN..].to_vec();
        body.push(0);
        let bad = reheader(&body);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::LengthMismatch));
    }

    /// Wrap arbitrary body bytes in a valid header (correct len + CRC).
    fn reheader(body: &[u8]) -> std::vec::Vec<u8> {
        let mut blob = std::vec::Vec::new();
        blob.extend_from_slice(&CONFIG_MAGIC.to_le_bytes());
        blob.extend_from_slice(&CONFIG_VERSION.to_le_bytes());
        blob.extend_from_slice(&[0, 0]);
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&crc32(body).to_le_bytes());
        blob.extend_from_slice(body);
        blob
    }

    fn mutated_body(mutate: impl FnOnce(&mut std::vec::Vec<u8>)) -> std::vec::Vec<u8> {
        let good = encode(&sample_config());
        let mut body = good[CONFIG_HEADER_LEN..].to_vec();
        mutate(&mut body);
        reheader(&body)
    }

    #[test]
    fn rejects_invalid_body_fields() {
        // record_count > 16.
        let bad = mutated_body(|b| b[0] = 17);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::TooManyRecords(17)));

        // Body layout: 12-byte prefix, then record 0 header at offset 12:
        // activation, arg, reserved u16, cell_count, cells at offset 17.
        let bad = mutated_body(|b| b[12] = 3);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::UnknownActivation(3)));

        let bad = mutated_body(|b| {
            b[12] = 1;
            b[13] = 8;
        });
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::LayerOutOfRange(8)));

        let bad = mutated_body(|b| {
            b[12] = 2;
            b[13] = 32;
        });
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::ToggleOutOfRange(32)));

        let bad = mutated_body(|b| b[16] = 41);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::TooManyCells(41)));

        // Record 0 cell 0 key at offset 17.
        let bad = mutated_body(|b| b[17] = 80);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::KeyOutOfRange(80)));

        // Cell 1 of record 0 is at 17 + 11 = 28; make it collide with cell 0.
        let bad = mutated_body(|b| b[28] = b[17]);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::DuplicateKey(0)));

        // Effect kind byte of record 0 cell 0 is at offset 18.
        let bad = mutated_body(|b| b[18] = 9);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::UnknownEffectKind(9)));
    }

    fn gated_config() -> LightingConfig {
        let mut records = Vec::new();
        // Layer-indicator gated on the Magic layer (press-and-hold status).
        records
            .push(ConfigRecord {
                activation: ConfigActivation::LayerActive(2),
                gate: Some(ConfigGate::LayerActive(2)),
                cells: Vec::from_slice(&[CellWrite { key: 3, effect: Effect::solid(0, 0, 255) }])
                    .unwrap(),
            })
            .unwrap();
        // One record per firmware-state gate.
        for (i, gate) in [
            ConfigGate::UsbConnected,
            ConfigGate::Charging,
            ConfigGate::SplitLinkUp,
            ConfigGate::Toggle(31),
        ]
        .into_iter()
        .enumerate()
        {
            records
                .push(ConfigRecord {
                    activation: ConfigActivation::Always,
                    gate: Some(gate),
                    cells: Vec::from_slice(&[CellWrite {
                        key: i as u8,
                        effect: Effect::solid(10, 10, 10),
                    }])
                    .unwrap(),
                })
                .unwrap();
        }
        LightingConfig { toggle_persist_mask: 0, toggle_initial_state: 0, records }
    }

    #[test]
    fn gated_config_roundtrips() {
        let config = gated_config();
        let bytes = encode(&config);
        assert_eq!(decode_lighting_config(&bytes).unwrap(), config);
        assert_eq!(encode(&decode_lighting_config(&bytes).unwrap()), bytes);
    }

    #[test]
    fn ungated_blob_is_byte_identical_to_pre_gate() {
        // The gate bytes are the old reserved u16: an ungated record still
        // writes (0, 0) there, so a no-gate config is unchanged by this
        // feature. Record 0 header starts at CONFIG_HEADER_LEN + 12.
        let bytes = encode(&sample_config());
        let rec0 = CONFIG_HEADER_LEN + CONFIG_BODY_HEADER_LEN;
        assert_eq!(bytes[rec0 + 2], 0, "gate_kind byte is 0 for an ungated record");
        assert_eq!(bytes[rec0 + 3], 0, "gate_arg byte is 0 for an ungated record");
    }

    #[test]
    fn rejects_invalid_gates() {
        // Record 0 header at body offset 12: activation(12), arg(13),
        // gate_kind(14), gate_arg(15), cell_count(16).
        let bad = mutated_body(|b| b[14] = 9);
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::UnknownGate(9)));

        let bad = mutated_body(|b| {
            b[14] = 1; // layer gate
            b[15] = 8; // >= CONFIG_LAYER_COUNT
        });
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::GateLayerOutOfRange(8)));

        let bad = mutated_body(|b| {
            b[14] = 2; // toggle gate
            b[15] = 32; // >= CONFIG_TOGGLE_COUNT
        });
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::GateToggleOutOfRange(32)));

        let bad = mutated_body(|b| {
            b[14] = 3; // usb-connected gate
            b[15] = 1; // must be 0
        });
        assert_eq!(decode_lighting_config(&bad), Err(ConfigError::GateArgNonZero(1)));
    }

    #[test]
    fn gate_wire_maps_all_kinds() {
        for gate in [
            ConfigGate::LayerActive(7),
            ConfigGate::Toggle(31),
            ConfigGate::UsbConnected,
            ConfigGate::Charging,
            ConfigGate::SplitLinkUp,
        ] {
            let (k, a) = gate.to_wire();
            assert_eq!(ConfigGate::from_wire(k, a), Ok(Some(gate)));
        }
        assert_eq!(ConfigGate::gate_to_wire(None), (0, 0));
        assert_eq!(ConfigGate::from_wire(0, 0), Ok(None));
    }

    #[test]
    fn effect_fields_roundtrip_verbatim() {
        // Ignored effect fields keep their bytes (byte-stable export).
        let mut records = Vec::new();
        records
            .push(ConfigRecord {
                activation: ConfigActivation::Always,
                gate: None,
                cells: Vec::from_slice(&[CellWrite {
                    key: 5,
                    effect: Effect {
                        kind: EffectKind::Solid,
                        r: 1,
                        g: 2,
                        b: 3,
                        period_ms: 1234,
                        phase_ms: 777,
                        duty_percent: 33,
                    },
                }])
                .unwrap(),
            })
            .unwrap();
        let config =
            LightingConfig { toggle_persist_mask: 0, toggle_initial_state: 0, records };
        let bytes = encode(&config);
        assert_eq!(decode_lighting_config(&bytes).unwrap(), config);
    }
}
