//! Minimal little-endian cursor helpers over byte slices.

use crate::error::{DecodeError, EncodeError};

pub(crate) struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Writer { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn u8(&mut self, v: u8) -> Result<(), EncodeError> {
        self.bytes(&[v])
    }

    pub fn u16(&mut self, v: u16) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u32(&mut self, v: u32) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }

    pub fn bytes(&mut self, src: &[u8]) -> Result<(), EncodeError> {
        let end = self.pos.checked_add(src.len()).ok_or(EncodeError::BufferTooSmall)?;
        if end > self.buf.len() {
            return Err(EncodeError::BufferTooSmall);
        }
        self.buf[self.pos..end].copy_from_slice(src);
        self.pos = end;
        Ok(())
    }

    /// Overwrite two bytes at an absolute position (for back-patching lengths).
    pub fn patch_u16(&mut self, at: usize, v: u16) {
        self.buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Overwrite four bytes at an absolute position (for back-patching
    /// lengths/checksums).
    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// The bytes written so far.
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
}

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        if end > self.buf.len() {
            return Err(DecodeError::Truncated);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// Payloads must be fully consumed.
    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(DecodeError::LengthMismatch)
        }
    }
}
