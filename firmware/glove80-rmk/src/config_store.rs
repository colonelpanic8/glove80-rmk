//! Transactional persistent-config store in the reserved runtime-config
//! flash partition (Phase 4 of docs/implementation-plan.md).
//!
//! Partition: `0xdc000..0xec000` (64 KiB, sixteen 4 KiB nRF52840 pages) —
//! the partition the ZMK-era flash map reserved for exactly this. Layout:
//! two generation slots of 32 KiB each (A at `0xdc000`, B at `0xe4000`).
//! Each slot holds:
//!
//! ```text
//! +0   magic        u32  = "G80C" — written LAST (the commit word)
//! +4   generation   u32  monotonically increasing across saves
//! +8   blob_len     u32
//! +12  blob_crc32   u32  CRC-32 (IEEE) of the blob bytes
//! +16  header_crc32 u32  CRC-32 of bytes 4..16 (generation..blob_crc)
//! +32  blob bytes (opaque, validated above this layer)
//! ```
//!
//! Atomicity against power loss at any byte (design-goals.md: "a malformed
//! config or interrupted write can never strand the keyboard"):
//!
//! - A save only ever touches the INACTIVE slot: erase → write blob →
//!   read-back CRC verify → write header fields → write the magic word last.
//! - Until the magic word is programmed, the slot fails validation (erased
//!   flash reads `0xFFFFFFFF`), so an interruption anywhere leaves the
//!   previously active slot untouched and still winning.
//! - The 4-byte magic program is a single NVMC word write; there is no state
//!   in which a torn save validates.
//! - Boot picks the valid slot with the highest generation; corrupt or empty
//!   slots are simply ignored (falling back to the compiled defaults when
//!   neither validates).
//!
//! All flash traffic goes through `rmk::shared_flash`:
//! bounded chunk/page requests executed by a service task that shares the
//! radio-safe `nrf_mpsl::Flash` with RMK's storage task under an async
//! mutex — every operation yields, nothing ever blocks key scanning.
//!
//! This layer treats the blob as opaque bytes plus CRC — including the v1.4
//! gate bytes in each record header — so store/read-back stays byte-stable.
//! Decoding/validating the lighting config happens above
//! (`lighting_config.rs`).

use rmk::crc32::Crc32;
use rmk::shared_flash::{self, SharedFlash};

/// Reserved runtime-config partition bounds (see `memory.x`).
pub const PARTITION_START: u32 = 0xdc000;
pub const PARTITION_END: u32 = 0xec000;

/// nRF52840 flash page (erase unit).
const PAGE_SIZE: u32 = 4096;

/// Scratch-buffer size for streaming CRC validation. RMK's shared-flash
/// client chunks larger requests internally, so this only bounds our stack.
const FLASH_CHUNK: usize = 256;

/// Two generation slots of 32 KiB each.
const SLOT_SIZE: u32 = 0x8000;
const SLOT_ADDRS: [u32; 2] = [PARTITION_START, PARTITION_START + SLOT_SIZE];

/// Slot-relative offset of the blob (header padded to a round 32 bytes).
const BLOB_OFFSET: u32 = 32;

/// Commit word. LE bytes spell "G80C" (Glove80 Config).
const MAGIC: u32 = u32::from_le_bytes(*b"G80C");

/// Largest storable blob. Fits the full compositor capacity (16 records of
/// 40 cells) with ample headroom, and is what the session layer buffers in
/// RAM.
pub const CONFIG_BLOB_MAX: usize = 8192;
const _: () = assert!(CONFIG_BLOB_MAX as u32 <= SLOT_SIZE - BLOB_OFFSET);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum StoreError {
    /// Flash service reported a failure (driver error or window violation).
    Flash,
    /// Blob larger than [`CONFIG_BLOB_MAX`].
    TooLarge,
    /// Read-back verification failed (the save was not committed).
    VerifyFailed,
    /// The active slot no longer validates (e.g. bit rot since boot).
    Corrupt,
}

impl From<shared_flash::FlashOpError> for StoreError {
    fn from(_: shared_flash::FlashOpError) -> Self {
        StoreError::Flash
    }
}

#[derive(Copy, Clone, Debug)]
struct ActiveSlot {
    index: usize,
    generation: u32,
    blob_len: u32,
}

/// The store handle. Owned by the central's lighting task; `open` scans the
/// partition once at boot, `save` runs the transactional apply.
pub struct ConfigStore {
    flash: SharedFlash,
    active: Option<ActiveSlot>,
}

/// Decoded fixed-size slot header (fields after the magic).
struct Header {
    generation: u32,
    blob_len: u32,
    blob_crc32: u32,
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Parse + CRC-check the 20 header bytes; `None` when the slot is not a
/// committed config.
fn parse_header(bytes: &[u8; 20]) -> Option<Header> {
    if read_u32(bytes, 0) != MAGIC {
        return None;
    }
    let mut crc = Crc32::new();
    crc.update(&bytes[4..16]);
    if crc.finalize() != read_u32(bytes, 16) {
        return None;
    }
    let header = Header {
        generation: read_u32(bytes, 4),
        blob_len: read_u32(bytes, 8),
        blob_crc32: read_u32(bytes, 12),
    };
    (header.blob_len as usize <= CONFIG_BLOB_MAX).then_some(header)
}

/// Stream the blob of `slot` through CRC-32 without a large buffer.
async fn blob_crc(
    flash: &mut SharedFlash,
    slot_addr: u32,
    blob_len: u32,
) -> Result<u32, StoreError> {
    let mut crc = Crc32::new();
    let mut chunk = [0u8; FLASH_CHUNK];
    let mut done = 0u32;
    while done < blob_len {
        let want = ((blob_len - done) as usize).min(chunk.len());
        flash
            .read(slot_addr + BLOB_OFFSET + done, &mut chunk[..want])
            .await?;
        crc.update(&chunk[..want]);
        done += want as u32;
    }
    Ok(crc.finalize())
}

/// Fully validate one slot: header, then streamed blob CRC.
async fn validate_slot(flash: &mut SharedFlash, slot_addr: u32) -> Option<Header> {
    let mut header_bytes = [0u8; 20];
    flash.read(slot_addr, &mut header_bytes).await.ok()?;
    let header = parse_header(&header_bytes)?;
    match blob_crc(flash, slot_addr, header.blob_len).await {
        Ok(crc) if crc == header.blob_crc32 => Some(header),
        _ => None,
    }
}

impl ConfigStore {
    /// Acquire the partition-scoped flash client and scan both
    /// slots. The newest fully valid slot becomes active; if neither
    /// validates the store is empty (boot then keeps the compiled defaults).
    pub async fn open() -> Self {
        let mut flash = shared_flash::take(PARTITION_START..PARTITION_END)
            .await
            .expect("runtime-config shared-flash client initialization failed");
        let mut active: Option<ActiveSlot> = None;
        for (index, &addr) in SLOT_ADDRS.iter().enumerate() {
            if let Some(h) = validate_slot(&mut flash, addr).await {
                let newer = match &active {
                    // Plain comparison: generations only ever move forward
                    // (u32 exhaustion would take billions of saves).
                    Some(a) => h.generation > a.generation,
                    None => true,
                };
                if newer {
                    active = Some(ActiveSlot {
                        index,
                        generation: h.generation,
                        blob_len: h.blob_len,
                    });
                }
            }
        }
        match &active {
            Some(a) => defmt::info!(
                "config-store: active slot {} generation {} ({} bytes)",
                a.index,
                a.generation,
                a.blob_len
            ),
            None => defmt::info!("config-store: no stored config (compiled defaults apply)"),
        }
        Self { flash, active }
    }

    /// Length of the active blob, or `None` when nothing is stored.
    pub fn active_len(&self) -> Option<usize> {
        self.active.map(|a| a.blob_len as usize)
    }

    /// Read the active blob into `buf` (which must hold `active_len`).
    /// Verifies the CRC again on the way out.
    pub async fn read_active(&mut self, buf: &mut [u8]) -> Result<usize, StoreError> {
        let active = self.active.ok_or(StoreError::Corrupt)?;
        let len = active.blob_len as usize;
        if buf.len() < len {
            return Err(StoreError::TooLarge);
        }
        let addr = SLOT_ADDRS[active.index];
        self.flash.read(addr + BLOB_OFFSET, &mut buf[..len]).await?;
        let mut header_bytes = [0u8; 20];
        self.flash.read(addr, &mut header_bytes).await?;
        let header = parse_header(&header_bytes).ok_or(StoreError::Corrupt)?;
        let mut crc = Crc32::new();
        crc.update(&buf[..len]);
        if crc.finalize() != header.blob_crc32 {
            return Err(StoreError::Corrupt);
        }
        Ok(len)
    }

    /// Read part of the active blob: `buf.len()` bytes at `offset` (clamped
    /// to the blob end; returns the byte count). Serves CONFIG_READ straight
    /// from flash — exactly the committed bytes, so export is byte-stable —
    /// without needing a second RAM buffer next to the session's.
    pub async fn read_active_at(
        &mut self,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<usize, StoreError> {
        let active = self.active.ok_or(StoreError::Corrupt)?;
        let len = active.blob_len as usize;
        if offset > len {
            return Err(StoreError::Corrupt);
        }
        let n = buf.len().min(len - offset);
        self.flash
            .read(
                SLOT_ADDRS[active.index] + BLOB_OFFSET + offset as u32,
                &mut buf[..n],
            )
            .await?;
        Ok(n)
    }

    /// Transactional apply: write `blob` to the inactive slot, verify it by
    /// read-back, then commit it with a newer generation. On ANY failure or
    /// power loss the previously active config remains active.
    pub async fn save(&mut self, blob: &[u8]) -> Result<(), StoreError> {
        if blob.len() > CONFIG_BLOB_MAX {
            return Err(StoreError::TooLarge);
        }
        let target = match self.active {
            Some(a) => 1 - a.index,
            None => 0,
        };
        let generation = self.active.map_or(1, |a| a.generation + 1);
        let addr = SLOT_ADDRS[target];

        // 1. Erase the pages the header + blob will occupy.
        let used = BLOB_OFFSET + blob.len() as u32;
        let erase_end = addr + used.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        self.flash.erase(addr, erase_end).await?;

        // 2. Program the blob (padded to the 4-byte write unit with 0xFF,
        //    the erased state).
        let full = blob.len() & !3;
        self.flash.write(addr + BLOB_OFFSET, &blob[..full]).await?;
        if full < blob.len() {
            let mut tail = [0xFFu8; 4];
            tail[..blob.len() - full].copy_from_slice(&blob[full..]);
            self.flash
                .write(addr + BLOB_OFFSET + full as u32, &tail)
                .await?;
        }

        // 3. Read back and verify before anything can make the slot valid.
        let mut crc = Crc32::new();
        crc.update(blob);
        let expected_crc = crc.finalize();
        if blob_crc(&mut self.flash, addr, blob.len() as u32).await? != expected_crc {
            return Err(StoreError::VerifyFailed);
        }

        // 4. Header fields (still no magic: the slot stays invalid).
        let mut fields = [0u8; 16];
        fields[0..4].copy_from_slice(&generation.to_le_bytes());
        fields[4..8].copy_from_slice(&(blob.len() as u32).to_le_bytes());
        fields[8..12].copy_from_slice(&expected_crc.to_le_bytes());
        let mut hcrc = Crc32::new();
        hcrc.update(&fields[..12]);
        fields[12..16].copy_from_slice(&hcrc.finalize().to_le_bytes());
        self.flash.write(addr + 4, &fields).await?;

        // 5. Commit: the magic word, one atomic NVMC word program. From this
        //    write on, this slot validates and out-generations the old one.
        self.flash.write(addr, &MAGIC.to_le_bytes()).await?;

        // Belt and braces: confirm the commit actually validates before
        // updating our in-RAM notion of the active slot.
        if validate_slot(&mut self.flash, addr).await.is_none() {
            return Err(StoreError::VerifyFailed);
        }

        self.active = Some(ActiveSlot {
            index: target,
            generation,
            blob_len: blob.len() as u32,
        });
        defmt::info!(
            "config-store: committed generation {} to slot {} ({} bytes)",
            generation,
            target,
            blob.len()
        );
        Ok(())
    }
}
