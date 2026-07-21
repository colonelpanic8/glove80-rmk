//! Codec for the commit manifest at the start of a runtime-configuration slot.
//!
//! The manifest is deliberately independent of the runtime payload encoding. It
//! authenticates an opaque payload and carries only the metadata needed to
//! decide whether a slot can be loaded.

use std::fmt;

/// Number of bytes in an encoded runtime slot manifest.
pub const MANIFEST_LEN: usize = 64;
/// Maximum payload space in a 32 KiB slot after its 4 KiB commit page.
pub const MAX_PAYLOAD_LEN: usize = 28 * 1024;
/// Manifest magic, including its trailing NUL byte.
pub const MAGIC: [u8; 8] = *b"G80RCFG\0";
/// Manifest format understood by this codec.
pub const FORMAT_MAJOR: u16 = 1;
pub const FORMAT_MINOR: u16 = 0;
/// Binding representation understood by the first runtime payload codec.
pub const BINDING_ENCODING_VERSION: u16 = 1;
/// Glove80 key positions across both halves.
pub const KEY_POSITION_COUNT: u16 = 80;
/// ZMK's layer-state bitset currently limits the runtime layer capacity.
pub const MAX_LAYER_CAPACITY: u16 = 32;

const HEADER_CRC_OFFSET: usize = 60;

/// Metadata committed alongside one opaque runtime-configuration payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeManifest {
    pub generation: u32,
    pub payload_len: u32,
    pub payload_crc32: u32,
    pub required_layer_capacity: u16,
    pub key_position_count: u16,
    pub active_layer_count: u16,
    pub binding_encoding_version: u16,
}

impl RuntimeManifest {
    /// Build a manifest for `payload`, computing its length and CRC32.
    pub fn for_payload(
        generation: u32,
        payload: &[u8],
        required_layer_capacity: u16,
        active_layer_count: u16,
    ) -> Result<Self, ManifestError> {
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| ManifestError::PayloadTooLarge {
                actual: payload.len(),
            })?;
        let manifest = Self {
            generation,
            payload_len,
            payload_crc32: crc32fast::hash(payload),
            required_layer_capacity,
            key_position_count: KEY_POSITION_COUNT,
            active_layer_count,
            binding_encoding_version: BINDING_ENCODING_VERSION,
        };
        manifest.validate_metadata()?;
        Ok(manifest)
    }

    /// Encode this manifest as the fixed 64-byte little-endian wire format.
    pub fn encode(&self) -> Result<[u8; MANIFEST_LEN], ManifestError> {
        self.validate_metadata()?;

        let mut bytes = [0_u8; MANIFEST_LEN];
        bytes[0..8].copy_from_slice(&MAGIC);
        write_u16(&mut bytes, 8, FORMAT_MAJOR);
        write_u16(&mut bytes, 10, FORMAT_MINOR);
        write_u16(&mut bytes, 12, MANIFEST_LEN as u16);
        // 14..16 is reserved and remains zero.
        write_u32(&mut bytes, 16, self.generation);
        write_u32(&mut bytes, 20, self.payload_len);
        write_u32(&mut bytes, 24, self.payload_crc32);
        write_u16(&mut bytes, 28, self.required_layer_capacity);
        write_u16(&mut bytes, 30, self.key_position_count);
        write_u16(&mut bytes, 32, self.active_layer_count);
        write_u16(&mut bytes, 34, self.binding_encoding_version);
        // 36..60 is reserved and remains zero.
        let header_crc = header_crc32(&bytes);
        write_u32(&mut bytes, HEADER_CRC_OFFSET, header_crc);
        Ok(bytes)
    }

    /// Decode and strictly validate an encoded manifest header.
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() != MANIFEST_LEN {
            return Err(ManifestError::InvalidHeaderLength {
                expected: MANIFEST_LEN,
                actual: bytes.len(),
            });
        }
        if bytes[0..8] != MAGIC {
            return Err(ManifestError::InvalidMagic);
        }

        let major = read_u16(bytes, 8);
        let minor = read_u16(bytes, 10);
        if major != FORMAT_MAJOR || minor != FORMAT_MINOR {
            return Err(ManifestError::UnsupportedFormat { major, minor });
        }

        let declared_header_len = read_u16(bytes, 12) as usize;
        if declared_header_len != MANIFEST_LEN {
            return Err(ManifestError::InvalidDeclaredHeaderLength {
                expected: MANIFEST_LEN,
                actual: declared_header_len,
            });
        }
        if bytes[14..16]
            .iter()
            .chain(&bytes[36..60])
            .any(|byte| *byte != 0)
        {
            return Err(ManifestError::NonZeroReservedBytes);
        }

        let expected_crc = read_u32(bytes, HEADER_CRC_OFFSET);
        let actual_crc = header_crc32(bytes);
        if actual_crc != expected_crc {
            return Err(ManifestError::HeaderCrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        let manifest = Self {
            generation: read_u32(bytes, 16),
            payload_len: read_u32(bytes, 20),
            payload_crc32: read_u32(bytes, 24),
            required_layer_capacity: read_u16(bytes, 28),
            key_position_count: read_u16(bytes, 30),
            active_layer_count: read_u16(bytes, 32),
            binding_encoding_version: read_u16(bytes, 34),
        };
        manifest.validate_metadata()?;
        Ok(manifest)
    }

    /// Decode a manifest and authenticate the opaque payload it describes.
    pub fn decode_with_payload(bytes: &[u8], payload: &[u8]) -> Result<Self, ManifestError> {
        let manifest = Self::decode(bytes)?;
        manifest.validate_payload(payload)?;
        Ok(manifest)
    }

    /// Check an opaque payload against the committed length and CRC32.
    pub fn validate_payload(&self, payload: &[u8]) -> Result<(), ManifestError> {
        let expected_len = self.payload_len as usize;
        if payload.len() != expected_len {
            return Err(ManifestError::PayloadLengthMismatch {
                expected: expected_len,
                actual: payload.len(),
            });
        }
        let actual_crc = crc32fast::hash(payload);
        if actual_crc != self.payload_crc32 {
            return Err(ManifestError::PayloadCrcMismatch {
                expected: self.payload_crc32,
                actual: actual_crc,
            });
        }
        Ok(())
    }

    fn validate_metadata(&self) -> Result<(), ManifestError> {
        let payload_len = self.payload_len as usize;
        if payload_len == 0 {
            return Err(ManifestError::EmptyPayload);
        }
        if payload_len > MAX_PAYLOAD_LEN {
            return Err(ManifestError::PayloadTooLarge {
                actual: payload_len,
            });
        }
        if !(1..=MAX_LAYER_CAPACITY).contains(&self.required_layer_capacity) {
            return Err(ManifestError::InvalidLayerCapacity(
                self.required_layer_capacity,
            ));
        }
        if self.key_position_count != KEY_POSITION_COUNT {
            return Err(ManifestError::InvalidKeyPositionCount(
                self.key_position_count,
            ));
        }
        if self.active_layer_count == 0 || self.active_layer_count > self.required_layer_capacity {
            return Err(ManifestError::InvalidActiveLayerCount {
                active: self.active_layer_count,
                capacity: self.required_layer_capacity,
            });
        }
        if self.binding_encoding_version != BINDING_ENCODING_VERSION {
            return Err(ManifestError::UnsupportedBindingEncoding(
                self.binding_encoding_version,
            ));
        }
        Ok(())
    }
}

/// Compare wrapping 32-bit generation counters using serial-number arithmetic.
///
/// A candidate exactly half the sequence space away is ambiguous and is not
/// considered newer. This keeps slot selection deterministic for corrupt or
/// impossibly stale records.
pub fn is_generation_newer(candidate: u32, reference: u32) -> bool {
    let distance = candidate.wrapping_sub(reference);
    distance != 0 && distance < (1_u32 << 31)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    InvalidHeaderLength { expected: usize, actual: usize },
    InvalidMagic,
    UnsupportedFormat { major: u16, minor: u16 },
    InvalidDeclaredHeaderLength { expected: usize, actual: usize },
    NonZeroReservedBytes,
    HeaderCrcMismatch { expected: u32, actual: u32 },
    EmptyPayload,
    PayloadTooLarge { actual: usize },
    InvalidLayerCapacity(u16),
    InvalidKeyPositionCount(u16),
    InvalidActiveLayerCount { active: u16, capacity: u16 },
    UnsupportedBindingEncoding(u16),
    PayloadLengthMismatch { expected: usize, actual: usize },
    PayloadCrcMismatch { expected: u32, actual: u32 },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeaderLength { expected, actual } => {
                write!(formatter, "manifest must be {expected} bytes, got {actual}")
            }
            Self::InvalidMagic => formatter.write_str("invalid runtime manifest magic"),
            Self::UnsupportedFormat { major, minor } => {
                write!(formatter, "unsupported runtime manifest format {major}.{minor}")
            }
            Self::InvalidDeclaredHeaderLength { expected, actual } => write!(
                formatter,
                "manifest declares a {actual}-byte header; expected {expected}"
            ),
            Self::NonZeroReservedBytes => {
                formatter.write_str("runtime manifest reserved bytes must be zero")
            }
            Self::HeaderCrcMismatch { expected, actual } => write!(
                formatter,
                "manifest CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}"
            ),
            Self::EmptyPayload => formatter.write_str("runtime payload must not be empty"),
            Self::PayloadTooLarge { actual } => write!(
                formatter,
                "runtime payload is {actual} bytes; maximum is {MAX_PAYLOAD_LEN}"
            ),
            Self::InvalidLayerCapacity(capacity) => write!(
                formatter,
                "required layer capacity must be between 1 and {MAX_LAYER_CAPACITY}, got {capacity}"
            ),
            Self::InvalidKeyPositionCount(count) => write!(
                formatter,
                "runtime manifest must describe {KEY_POSITION_COUNT} key positions, got {count}"
            ),
            Self::InvalidActiveLayerCount { active, capacity } => write!(
                formatter,
                "active layer count must be between 1 and required capacity {capacity}, got {active}"
            ),
            Self::UnsupportedBindingEncoding(version) => {
                write!(formatter, "unsupported binding encoding version {version}")
            }
            Self::PayloadLengthMismatch { expected, actual } => write!(
                formatter,
                "runtime payload length mismatch: expected {expected}, got {actual}"
            ),
            Self::PayloadCrcMismatch { expected, actual } => write!(
                formatter,
                "runtime payload CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

fn header_crc32(bytes: &[u8]) -> u32 {
    let mut crc_input = [0_u8; MANIFEST_LEN];
    crc_input.copy_from_slice(bytes);
    crc_input[HEADER_CRC_OFFSET..HEADER_CRC_OFFSET + 4].fill(0);
    crc32fast::hash(&crc_input)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &[u8] = b"opaque runtime payload";

    fn example_manifest() -> RuntimeManifest {
        RuntimeManifest::for_payload(0x1234_5678, PAYLOAD, 8, 6).unwrap()
    }

    #[test]
    fn golden_little_endian_encoding() {
        let bytes = example_manifest().encode().unwrap();
        let expected = [
            0x47, 0x38, 0x30, 0x52, 0x43, 0x46, 0x47, 0x00, 0x01, 0x00, 0x00, 0x00, 0x40, 0x00,
            0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x16, 0x00, 0x00, 0x00, 0xdf, 0x53, 0x0b, 0x6a,
            0x08, 0x00, 0x50, 0x00, 0x06, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xda, 0x4d, 0x7b, 0x3e,
        ];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn round_trip_and_payload_validation() {
        let manifest = example_manifest();
        let bytes = manifest.encode().unwrap();
        assert_eq!(
            RuntimeManifest::decode_with_payload(&bytes, PAYLOAD),
            Ok(manifest)
        );
    }

    #[test]
    fn generation_comparison_handles_wraparound() {
        assert!(is_generation_newer(11, 10));
        assert!(!is_generation_newer(10, 10));
        assert!(!is_generation_newer(9, 10));
        assert!(is_generation_newer(0, u32::MAX));
        assert!(is_generation_newer(1, u32::MAX));
        assert!(!is_generation_newer(u32::MAX, 0));
        assert!(!is_generation_newer(0x8000_0000, 0));
        assert!(!is_generation_newer(0, 0x8000_0000));
    }

    #[test]
    fn rejects_header_corruption() {
        let mut bytes = example_manifest().encode().unwrap();
        bytes[18] ^= 0x80;
        assert!(matches!(
            RuntimeManifest::decode(&bytes),
            Err(ManifestError::HeaderCrcMismatch { .. })
        ));
    }

    #[test]
    fn rejects_header_crc_corruption() {
        let mut bytes = example_manifest().encode().unwrap();
        bytes[HEADER_CRC_OFFSET] ^= 0x01;
        assert!(matches!(
            RuntimeManifest::decode(&bytes),
            Err(ManifestError::HeaderCrcMismatch { .. })
        ));
    }

    #[test]
    fn rejects_payload_corruption() {
        let manifest = example_manifest();
        let bytes = manifest.encode().unwrap();
        let mut payload = PAYLOAD.to_vec();
        payload[0] ^= 0x01;
        assert!(matches!(
            RuntimeManifest::decode_with_payload(&bytes, &payload),
            Err(ManifestError::PayloadCrcMismatch { .. })
        ));
    }

    #[test]
    fn rejects_nonzero_reserved_bytes_even_with_valid_crc() {
        let mut bytes = example_manifest().encode().unwrap();
        bytes[40] = 1;
        let crc = header_crc32(&bytes);
        write_u32(&mut bytes, HEADER_CRC_OFFSET, crc);
        assert_eq!(
            RuntimeManifest::decode(&bytes),
            Err(ManifestError::NonZeroReservedBytes)
        );
    }

    #[test]
    fn rejects_invalid_metadata_even_with_valid_crc() {
        let mut bytes = example_manifest().encode().unwrap();
        write_u16(&mut bytes, 32, 9);
        let crc = header_crc32(&bytes);
        write_u32(&mut bytes, HEADER_CRC_OFFSET, crc);
        assert_eq!(
            RuntimeManifest::decode(&bytes),
            Err(ManifestError::InvalidActiveLayerCount {
                active: 9,
                capacity: 8,
            })
        );
    }

    #[test]
    fn rejects_wrong_length_before_indexing() {
        assert_eq!(
            RuntimeManifest::decode(&[0; MANIFEST_LEN - 1]),
            Err(ManifestError::InvalidHeaderLength {
                expected: MANIFEST_LEN,
                actual: MANIFEST_LEN - 1,
            })
        );
    }
}
