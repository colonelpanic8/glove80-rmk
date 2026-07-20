//! Error types for the message codec and the frame layer.

use core::fmt;

/// Errors while encoding a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer is too small for the encoded message.
    BufferTooSmall,
    /// A `Response`'s payload variant does not match its command/status.
    PayloadMismatch,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::BufferTooSmall => write!(f, "output buffer too small"),
            EncodeError::PayloadMismatch => {
                write!(f, "response payload does not match command/status")
            }
        }
    }
}

/// Errors while decoding a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Ran out of bytes while parsing.
    Truncated,
    /// Buffer length disagrees with the header's payload_len, or a payload
    /// had leftover/missing bytes for its declared counts.
    LengthMismatch,
    /// Opcode is not a known command (or has the wrong response-flag bit).
    UnknownOpcode(u8),
    /// Status byte is not a known status.
    UnknownStatus(u8),
    /// Effect kind byte is not a known effect.
    UnknownEffectKind(u8),
    /// Boot target byte is not a known target.
    UnknownBootTarget(u8),
    /// Toggle state byte is not 0 or 1.
    BadToggleState(u8),
    /// A boolean flag byte (present / dirty / mismatch, v1.3) is not 0 or 1.
    BadFlag(u8),
    /// A count exceeds the codec's compile-time capacity.
    CapacityExceeded,
    /// Status/payload combination is not valid for the command.
    InvalidStatusForCommand,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "message truncated"),
            DecodeError::LengthMismatch => write!(f, "length field disagrees with buffer"),
            DecodeError::UnknownOpcode(op) => write!(f, "unknown opcode 0x{op:02x}"),
            DecodeError::UnknownStatus(s) => write!(f, "unknown status 0x{s:02x}"),
            DecodeError::UnknownEffectKind(k) => write!(f, "unknown effect kind {k}"),
            DecodeError::UnknownBootTarget(t) => write!(f, "unknown boot target {t}"),
            DecodeError::BadToggleState(v) => write!(f, "toggle state must be 0 or 1, got {v}"),
            DecodeError::BadFlag(v) => write!(f, "flag byte must be 0 or 1, got {v}"),
            DecodeError::CapacityExceeded => write!(f, "count exceeds codec capacity"),
            DecodeError::InvalidStatusForCommand => {
                write!(f, "status not valid for this command")
            }
        }
    }
}

/// Errors in the frame (segmentation) layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Chunk size below the 3-byte minimum (2-byte header + 1 payload byte).
    ChunkTooSmall,
    /// Cannot frame an empty message.
    EmptyMessage,
    /// Message needs more than 128 frames at this chunk size.
    MessageTooLong,
    /// Frame index past the end of the message.
    IndexOutOfRange,
    /// Output buffer too small for the frame.
    BufferTooSmall,
    /// Frame shorter than its header + declared payload length.
    Truncated,
    /// Frame declares a zero-length payload.
    EmptyFrame,
    /// Frame sequence number is not the expected one.
    UnexpectedSequence { expected: u8, got: u8 },
    /// Reassembled message would exceed the reassembler's buffer.
    Overflow,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::ChunkTooSmall => write!(f, "chunk size below minimum (3)"),
            FrameError::EmptyMessage => write!(f, "cannot frame an empty message"),
            FrameError::MessageTooLong => write!(f, "message exceeds 128 frames"),
            FrameError::IndexOutOfRange => write!(f, "frame index out of range"),
            FrameError::BufferTooSmall => write!(f, "output buffer too small"),
            FrameError::Truncated => write!(f, "frame shorter than declared payload"),
            FrameError::EmptyFrame => write!(f, "frame has zero-length payload"),
            FrameError::UnexpectedSequence { expected, got } => {
                write!(f, "expected frame sequence {expected}, got {got}")
            }
            FrameError::Overflow => write!(f, "reassembly buffer overflow"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EncodeError {}
#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}
#[cfg(feature = "std")]
impl std::error::Error for FrameError {}
