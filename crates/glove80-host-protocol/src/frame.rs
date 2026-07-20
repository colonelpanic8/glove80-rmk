//! Per-transport segmentation: splits an encoded message into transport
//! chunks (32-byte HID reports, ATT payloads) and reassembles them.
//!
//! Chunk layout: `[seq | FINAL flag, payload_len, payload...]`, optionally
//! zero-padded to a fixed report size by the transport.

use crate::error::FrameError;

/// Two bytes: control (seq + FINAL) and payload length.
pub const FRAME_HEADER_LEN: usize = 2;
/// Set on the last frame of a message.
pub const FRAME_FINAL_FLAG: u8 = 0x80;
/// Sequence number mask (0..=127).
pub const FRAME_SEQ_MASK: u8 = 0x7F;
/// A message may span at most this many frames.
pub const MAX_FRAMES_PER_MESSAGE: usize = 128;
/// Smallest usable chunk: header + one payload byte.
pub const MIN_CHUNK_LEN: usize = FRAME_HEADER_LEN + 1;

fn payload_per_frame(chunk_len: usize) -> Result<usize, FrameError> {
    if chunk_len < MIN_CHUNK_LEN {
        return Err(FrameError::ChunkTooSmall);
    }
    // The per-frame length byte caps the payload at 255.
    Ok((chunk_len - FRAME_HEADER_LEN).min(255))
}

/// Number of frames a message needs at the given chunk size.
pub fn frame_count(message_len: usize, chunk_len: usize) -> Result<usize, FrameError> {
    if message_len == 0 {
        return Err(FrameError::EmptyMessage);
    }
    let per = payload_per_frame(chunk_len)?;
    let n = message_len.div_ceil(per);
    if n > MAX_FRAMES_PER_MESSAGE {
        return Err(FrameError::MessageTooLong);
    }
    Ok(n)
}

/// Write frame `index` of `message` into `out`. Returns the frame length
/// (header + payload, without padding); fixed-size transports should send
/// `out` zero-padded to their report size.
pub fn write_frame(
    message: &[u8],
    chunk_len: usize,
    index: usize,
    out: &mut [u8],
) -> Result<usize, FrameError> {
    let n = frame_count(message.len(), chunk_len)?;
    if index >= n {
        return Err(FrameError::IndexOutOfRange);
    }
    let per = payload_per_frame(chunk_len)?;
    let start = index * per;
    let end = (start + per).min(message.len());
    let payload = &message[start..end];
    if out.len() < FRAME_HEADER_LEN + payload.len() {
        return Err(FrameError::BufferTooSmall);
    }
    let mut control = index as u8;
    if index == n - 1 {
        control |= FRAME_FINAL_FLAG;
    }
    out[0] = control;
    out[1] = payload.len() as u8;
    out[FRAME_HEADER_LEN..FRAME_HEADER_LEN + payload.len()].copy_from_slice(payload);
    Ok(FRAME_HEADER_LEN + payload.len())
}

/// Reassembles one message at a time from incoming frames.
///
/// `N` is the message buffer capacity (use [`crate::MAX_MESSAGE_LEN`]).
/// A frame with sequence 0 always starts a new message, dropping any
/// incomplete one; errors reset the reassembler.
pub struct Reassembler<const N: usize> {
    buf: [u8; N],
    len: usize,
    next_seq: u8,
}

impl<const N: usize> Default for Reassembler<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Reassembler<N> {
    pub fn new() -> Self {
        Reassembler { buf: [0; N], len: 0, next_seq: 0 }
    }

    pub fn reset(&mut self) {
        self.len = 0;
        self.next_seq = 0;
    }

    /// Feed one received chunk (padding after the declared payload is
    /// ignored). Returns the complete message when the FINAL frame arrives.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<&[u8]>, FrameError> {
        if frame.len() < FRAME_HEADER_LEN {
            self.reset();
            return Err(FrameError::Truncated);
        }
        let control = frame[0];
        let seq = control & FRAME_SEQ_MASK;
        let is_final = control & FRAME_FINAL_FLAG != 0;
        let payload_len = frame[1] as usize;
        if payload_len == 0 {
            self.reset();
            return Err(FrameError::EmptyFrame);
        }
        if frame.len() < FRAME_HEADER_LEN + payload_len {
            self.reset();
            return Err(FrameError::Truncated);
        }
        if seq == 0 {
            // Start of a new message; drop any incomplete one.
            self.len = 0;
        } else if seq != self.next_seq {
            let expected = self.next_seq;
            self.reset();
            return Err(FrameError::UnexpectedSequence { expected, got: seq });
        }
        if self.len + payload_len > N {
            self.reset();
            return Err(FrameError::Overflow);
        }
        self.buf[self.len..self.len + payload_len]
            .copy_from_slice(&frame[FRAME_HEADER_LEN..FRAME_HEADER_LEN + payload_len]);
        self.len += payload_len;
        if is_final {
            self.next_seq = 0;
            Ok(Some(&self.buf[..self.len]))
        } else if seq as usize == MAX_FRAMES_PER_MESSAGE - 1 {
            self.reset();
            Err(FrameError::MessageTooLong)
        } else {
            self.next_seq = seq + 1;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_MESSAGE_LEN;

    fn frames_of(message: &[u8], chunk_len: usize) -> std::vec::Vec<std::vec::Vec<u8>> {
        let n = frame_count(message.len(), chunk_len).unwrap();
        (0..n)
            .map(|i| {
                let mut out = std::vec![0u8; chunk_len];
                let used = write_frame(message, chunk_len, i, &mut out).unwrap();
                out.truncate(used);
                out
            })
            .collect()
    }

    #[test]
    fn splits_and_reassembles() {
        let message: std::vec::Vec<u8> = (0u8..=44).collect(); // 45 bytes
        for chunk_len in [3, 20, 32, 64] {
            let frames = frames_of(&message, chunk_len);
            let mut r: Reassembler<MAX_MESSAGE_LEN> = Reassembler::new();
            for (i, f) in frames.iter().enumerate() {
                let out = r.push(f).unwrap();
                if i == frames.len() - 1 {
                    assert_eq!(out.unwrap(), &message[..]);
                } else {
                    assert!(out.is_none());
                }
            }
        }
    }

    #[test]
    fn single_frame_message() {
        let frames = frames_of(&[1, 2, 3], 32);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], std::vec![0x80, 3, 1, 2, 3]);
    }

    #[test]
    fn tolerates_padding() {
        let mut padded = std::vec![0u8; 32];
        write_frame(&[9, 9], 32, 0, &mut padded).unwrap();
        let mut r: Reassembler<64> = Reassembler::new();
        assert_eq!(r.push(&padded).unwrap().unwrap(), &[9, 9]);
    }

    #[test]
    fn seq_zero_restarts() {
        let message: std::vec::Vec<u8> = (0u8..40).collect();
        let frames = frames_of(&message, 20);
        assert!(frames.len() > 1);
        let mut r: Reassembler<64> = Reassembler::new();
        assert!(r.push(&frames[0]).unwrap().is_none());
        // New message starts before the old one finished.
        let mut r2_frames = frames_of(&[7, 7, 7], 20);
        assert_eq!(r.push(&r2_frames.remove(0)).unwrap().unwrap(), &[7, 7, 7]);
    }

    #[test]
    fn rejects_gaps_and_bad_frames() {
        let message: std::vec::Vec<u8> = (0u8..60).collect();
        let frames = frames_of(&message, 20);
        assert_eq!(frames.len(), 4);
        let mut r: Reassembler<128> = Reassembler::new();
        assert!(r.push(&frames[0]).unwrap().is_none());
        assert_eq!(
            r.push(&frames[2]),
            Err(FrameError::UnexpectedSequence { expected: 1, got: 2 })
        );
        // Truncated frame.
        assert_eq!(r.push(&[0x00]), Err(FrameError::Truncated));
        assert_eq!(r.push(&[0x00, 5, 1, 2]), Err(FrameError::Truncated));
        // Empty payload.
        assert_eq!(r.push(&[0x80, 0]), Err(FrameError::EmptyFrame));
        // Overflow.
        let mut small: Reassembler<4> = Reassembler::new();
        assert_eq!(small.push(&frames[0]), Err(FrameError::Overflow));
    }

    #[test]
    fn split_errors() {
        assert_eq!(frame_count(0, 32), Err(FrameError::EmptyMessage));
        assert_eq!(frame_count(10, 2), Err(FrameError::ChunkTooSmall));
        assert_eq!(frame_count(129, 3), Err(FrameError::MessageTooLong));
        assert_eq!(frame_count(128, 3), Ok(128));
        let mut out = [0u8; 32];
        assert_eq!(write_frame(&[1], 32, 1, &mut out), Err(FrameError::IndexOutOfRange));
        let mut tiny = [0u8; 2];
        assert_eq!(write_frame(&[1], 32, 0, &mut tiny), Err(FrameError::BufferTooSmall));
    }
}
