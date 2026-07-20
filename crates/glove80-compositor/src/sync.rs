//! Split lighting transfer (Phase 3 of `docs/implementation-plan.md`):
//! the pure-logic half of forwarding host-overlay lighting from the split
//! central to the peripheral.
//!
//! Two things live here, both host-tested:
//!
//! - [`SyncMessage`]: the bounded, versioned payload codec for the split
//!   application channel (`rmk::split_app` in the pinned fork). Every
//!   encoded payload fits [`MAX_SYNC_PAYLOAD`] bytes; keys are LOCAL chain
//!   indices on the receiving half (the central remaps protocol keys 40..80
//!   to 0..40 before encoding). Messages are absolute state ("cell k is now
//!   X", "the toggle bitmap is now Y"), so receiving any of them twice — or
//!   receiving a full resync after deltas — is harmless.
//! - [`RemoteOverlay`]: the central's authoritative store for the
//!   peripheral's host-overlay cells, including TTL bookkeeping. TTL expiry
//!   authority stays on the central: cells forwarded to the peripheral carry
//!   no TTL; when a cell expires here the central sends the unset.
//!
//! Versioning: every payload starts with `[SYNC_VERSION, tag]`. New message
//! kinds are new tags (old receivers must ignore unknown tags); breaking
//! layout changes bump the version byte (receivers must ignore other
//! versions). Both cases decode to a distinct error so firmware can drop
//! them silently-but-logged.

use crate::{Activation, Cell, Condition, MAX_CELLS_PER_RECORD, MAX_RECORDS, Record};

/// Version byte carried by every sync payload.
pub const SYNC_VERSION: u8 = 1;

/// Upper bound of one encoded sync payload. Must match the vendored split
/// pipe's `SPLIT_APP_MSG_MAX` (asserted in the firmware crate); kept small
/// because every split transfer, key events included, is sized by the
/// largest split message (which itself must stay ≤ 32 bytes — see the pipe).
pub const MAX_SYNC_PAYLOAD: usize = 26;

/// Cells per [`SyncMessage::SetCells`] batch:
/// `2 (header) + 1 (count) + 2 * 10 (cell entries) == 23 ≤ 26`.
pub const MAX_CELLS_PER_SYNC: usize = 2;

/// Keys per [`SyncMessage::UnsetKeys`] batch (3 + 16 = 19 ≤ 33).
pub const MAX_UNSETS_PER_SYNC: usize = 16;

const TAG_SET_CELLS: u8 = 0x01;
const TAG_UNSET_KEYS: u8 = 0x02;
const TAG_CLEAR: u8 = 0x03;
const TAG_STATE: u8 = 0x04;
// Persistent-config record transfer (Phase 4). Additive tags: an old
// receiver ignores them (UnknownTag), which is safe — it simply keeps its
// compiled default records.
const TAG_CONFIG_RESET: u8 = 0x05;
const TAG_CONFIG_RECORD: u8 = 0x06;
const TAG_CONFIG_CELLS: u8 = 0x07;
const TAG_CONFIG_COMMIT: u8 = 0x08;
const TAG_ENTER_BOOTLOADER: u8 = 0x09;
// Peripheral → central build identity, announced once per link-up (host
// protocol v1.3 GET_VERSION). Additive: an old central ignores it.
const TAG_PERIPHERAL_VERSION: u8 = 0x0A;
// Per-record gate for a staged persistent-config record (conditional
// lighting). Additive: an old peripheral ignores it (UnknownTag) and stages
// the record ungated — the record still composes, just without the extra
// suppression the gate would apply. Sent after the record's `ConfigRecord`.
const TAG_CONFIG_GATE: u8 = 0x0B;
// Shared lighting state, extended with the central's usb-connected flag
// (conditional lighting). Distinct tag from `TAG_STATE` so an old peripheral
// falls back to ignoring it rather than mis-decoding; a new peripheral still
// accepts the legacy 8-byte `TAG_STATE` (usb_connected defaults false) from an
// old central.
const TAG_STATE2: u8 = 0x0C;

/// Wire magic carried by [`SyncMessage::EnterBootloader`] so a corrupted or
/// truncated payload can never reboot the peripheral (same value as the
/// host protocol's bootloader magic).
pub const BOOTLOADER_SYNC_MAGIC: u32 = 0xB007_10AD;

/// Bytes of one `key + cell` entry on the wire.
const CELL_ENTRY_LEN: usize = 10;

/// Fixed-capacity cell batch for [`SyncMessage::SetCells`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SyncCells {
    len: u8,
    cells: [(u8, Cell); MAX_CELLS_PER_SYNC],
}

impl Default for SyncCells {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncCells {
    pub const fn new() -> Self {
        Self { len: 0, cells: [(0, Cell::Transparent); MAX_CELLS_PER_SYNC] }
    }

    /// Append an entry; `false` when full (entry not added).
    pub fn push(&mut self, key: u8, cell: Cell) -> bool {
        if (self.len as usize) == MAX_CELLS_PER_SYNC {
            return false;
        }
        self.cells[self.len as usize] = (key, cell);
        self.len += 1;
        true
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len as usize == MAX_CELLS_PER_SYNC
    }

    pub fn entries(&self) -> &[(u8, Cell)] {
        &self.cells[..self.len as usize]
    }
}

/// Fixed-capacity key batch for [`SyncMessage::UnsetKeys`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SyncKeys {
    len: u8,
    keys: [u8; MAX_UNSETS_PER_SYNC],
}

impl Default for SyncKeys {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncKeys {
    pub const fn new() -> Self {
        Self { len: 0, keys: [0; MAX_UNSETS_PER_SYNC] }
    }

    /// Append a key; `false` when full (key not added).
    pub fn push(&mut self, key: u8) -> bool {
        if (self.len as usize) == MAX_UNSETS_PER_SYNC {
            return false;
        }
        self.keys[self.len as usize] = key;
        self.len += 1;
        true
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len as usize == MAX_UNSETS_PER_SYNC
    }

    pub fn keys(&self) -> &[u8] {
        &self.keys[..self.len as usize]
    }
}

/// One split application sync message. All but
/// [`SyncMessage::PeripheralVersion`] flow central → peripheral; that one
/// flows peripheral → central. All state is absolute and idempotent; keys
/// are local chain indices on the receiving half.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SyncMessage {
    /// Set (or, for [`Cell::Transparent`], unset) host-overlay cells. Cells
    /// carry no TTL — TTL authority stays with the central, which sends
    /// [`SyncMessage::UnsetKeys`] on expiry.
    SetCells(SyncCells),
    /// Remove host-overlay cells.
    UnsetKeys(SyncKeys),
    /// Clear the whole host overlay.
    Clear,
    /// Shared lighting state snapshot: brightness scalar, effective ceiling
    /// (still bounded by the receiver's compiled `CHANNEL_CEILING`), the full
    /// toggle bitmap, and the central's usb-connected truth (the
    /// [`Condition::UsbConnected`] gate input, mirrored to the peripheral so
    /// both halves gate identically). Encodes under [`TAG_STATE2`]; a legacy
    /// 8-byte [`TAG_STATE`] payload still decodes with `usb_connected = false`.
    State { brightness: u8, ceiling: u8, toggles: u32, usb_connected: bool },
    /// Begin staging a persistent-config record set of `record_count`
    /// records (Phase 4). Discards any half-staged set; the live records
    /// stay untouched until [`SyncMessage::ConfigCommit`].
    ConfigReset { record_count: u8 },
    /// Declare record `index`: its activation predicate and how many cells
    /// [`SyncMessage::ConfigCells`] messages will deliver for it.
    ConfigRecord { index: u8, activation: Activation, cell_count: u8 },
    /// Attach a [`gate`](Record::gate) [`Condition`] to staged record
    /// `record_index` (conditional lighting). Additive tag: an old peripheral
    /// ignores it and stages the record ungated.
    ConfigGate { record_index: u8, gate: Condition },
    /// Append cells to staged record `record_index`.
    ConfigCells { record_index: u8, cells: SyncCells },
    /// Atomically swap the staged set (which must be complete and match
    /// `record_count`) into the live compositor records. An incomplete stage
    /// is discarded instead — the previous records keep rendering, and the
    /// next resync retransmits from `ConfigReset`.
    ConfigCommit { record_count: u8 },
    /// Reboot the peripheral into its UF2 bootloader. Guarded by
    /// [`BOOTLOADER_SYNC_MAGIC`] on the wire (checked at decode).
    EnterBootloader,
    /// The peripheral's build identity, sent peripheral → central once per
    /// link-up edge. The central caches it and serves it through the host
    /// protocol's GET_VERSION.
    PeripheralVersion(PeripheralVersion),
}

/// Build identity of the peripheral half, as carried by
/// [`SyncMessage::PeripheralVersion`] (14 wire bytes including the header).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PeripheralVersion {
    /// Firmware crate semver.
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    /// ASCII git short hash, zero-padded on the right.
    pub git_hash: [u8; 8],
    /// Built from a tree with uncommitted changes.
    pub dirty: bool,
}

/// Wire encoding of an [`Activation`] for config-record transfer: matches
/// the persistent blob's activation kinds (only base/layer/toggle records
/// are persistable).
fn put_activation(a: Activation) -> Option<(u8, u8)> {
    match a {
        Activation::Always => Some((0, 0)),
        Activation::LayerActive(n) => Some((1, n)),
        Activation::Toggle(id) => Some((2, id)),
        // Not persistable; never encoded.
        Activation::HostOverlay | Activation::Status => None,
    }
}

fn get_activation(kind: u8, arg: u8) -> Result<Activation, SyncDecodeError> {
    match kind {
        0 => Ok(Activation::Always),
        1 => Ok(Activation::LayerActive(arg)),
        2 => Ok(Activation::Toggle(arg)),
        k => Err(SyncDecodeError::UnknownActivation(k)),
    }
}

/// Why a payload failed to decode. `UnsupportedVersion` / `UnknownTag` are
/// the forward-compatibility cases receivers must silently ignore.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SyncDecodeError {
    UnsupportedVersion(u8),
    UnknownTag(u8),
    UnknownCellKind(u8),
    UnknownActivation(u8),
    /// A [`SyncMessage::ConfigGate`] named an unknown gate kind (or the
    /// "no gate" kind 0, which is never sent as a gate message).
    UnknownGate(u8),
    /// An [`SyncMessage::EnterBootloader`] payload without the wire magic.
    BadMagic,
    /// Payload shorter or longer than the tag's layout requires.
    BadLength,
}

fn put_u16(out: &mut [u8], at: usize, v: u16) {
    out[at] = (v & 0xff) as u8;
    out[at + 1] = (v >> 8) as u8;
}

fn get_u16(bytes: &[u8], at: usize) -> u16 {
    bytes[at] as u16 | ((bytes[at + 1] as u16) << 8)
}

/// Encode one `key + cell` entry (10 bytes) at `at`.
fn put_cell(out: &mut [u8], at: usize, key: u8, cell: &Cell) {
    let (kind, color, period_ms, phase_ms, duty) = match *cell {
        Cell::Solid { color } => (0u8, color, 0, 0, 0),
        Cell::Blink { color, period_ms, phase_ms, duty_pct } => (1, color, period_ms, phase_ms, duty_pct),
        Cell::Breathe { color, period_ms, phase_ms } => (2, color, period_ms, phase_ms, 0),
        Cell::Transparent => (3, crate::Rgb::OFF, 0, 0, 0),
    };
    out[at] = key;
    out[at + 1] = kind;
    out[at + 2] = color.r;
    out[at + 3] = color.g;
    out[at + 4] = color.b;
    put_u16(out, at + 5, period_ms);
    put_u16(out, at + 7, phase_ms);
    out[at + 9] = duty;
}

/// Decode one `key + cell` entry at `at` (bounds already checked).
fn get_cell(bytes: &[u8], at: usize) -> Result<(u8, Cell), SyncDecodeError> {
    let key = bytes[at];
    let color = crate::Rgb::new(bytes[at + 2], bytes[at + 3], bytes[at + 4]);
    let period_ms = get_u16(bytes, at + 5);
    let phase_ms = get_u16(bytes, at + 7);
    let duty_pct = bytes[at + 9];
    let cell = match bytes[at + 1] {
        0 => Cell::Solid { color },
        1 => Cell::Blink { color, period_ms, phase_ms, duty_pct },
        2 => Cell::Breathe { color, period_ms, phase_ms },
        3 => Cell::Transparent,
        k => return Err(SyncDecodeError::UnknownCellKind(k)),
    };
    Ok((key, cell))
}

impl SyncMessage {
    /// Encode into `out`; returns the encoded length (≤ [`MAX_SYNC_PAYLOAD`]).
    pub fn encode(&self, out: &mut [u8; MAX_SYNC_PAYLOAD]) -> usize {
        out[0] = SYNC_VERSION;
        match self {
            SyncMessage::SetCells(cells) => {
                out[1] = TAG_SET_CELLS;
                out[2] = cells.len;
                for (i, (key, cell)) in cells.entries().iter().enumerate() {
                    put_cell(out, 3 + i * CELL_ENTRY_LEN, *key, cell);
                }
                3 + cells.entries().len() * CELL_ENTRY_LEN
            }
            SyncMessage::UnsetKeys(keys) => {
                out[1] = TAG_UNSET_KEYS;
                out[2] = keys.len;
                out[3..3 + keys.keys().len()].copy_from_slice(keys.keys());
                3 + keys.keys().len()
            }
            SyncMessage::Clear => {
                out[1] = TAG_CLEAR;
                2
            }
            SyncMessage::State { brightness, ceiling, toggles, usb_connected } => {
                out[1] = TAG_STATE2;
                out[2] = *brightness;
                out[3] = *ceiling;
                out[4] = (*toggles & 0xff) as u8;
                out[5] = ((*toggles >> 8) & 0xff) as u8;
                out[6] = ((*toggles >> 16) & 0xff) as u8;
                out[7] = ((*toggles >> 24) & 0xff) as u8;
                out[8] = *usb_connected as u8;
                9
            }
            SyncMessage::ConfigReset { record_count } => {
                out[1] = TAG_CONFIG_RESET;
                out[2] = *record_count;
                3
            }
            SyncMessage::ConfigRecord { index, activation, cell_count } => {
                out[1] = TAG_CONFIG_RECORD;
                out[2] = *index;
                // Only persistable activations reach the wire (the config
                // producers never build HostOverlay/Status records); map a
                // programming error to Always rather than corrupt the frame.
                let (kind, arg) = put_activation(*activation).unwrap_or((0, 0));
                out[3] = kind;
                out[4] = arg;
                out[5] = *cell_count;
                6
            }
            SyncMessage::ConfigGate { record_index, gate } => {
                out[1] = TAG_CONFIG_GATE;
                out[2] = *record_index;
                let (kind, arg) = gate.to_wire();
                out[3] = kind;
                out[4] = arg;
                5
            }
            SyncMessage::ConfigCells { record_index, cells } => {
                out[1] = TAG_CONFIG_CELLS;
                out[2] = *record_index;
                out[3] = cells.len;
                for (i, (key, cell)) in cells.entries().iter().enumerate() {
                    put_cell(out, 4 + i * CELL_ENTRY_LEN, *key, cell);
                }
                4 + cells.entries().len() * CELL_ENTRY_LEN
            }
            SyncMessage::ConfigCommit { record_count } => {
                out[1] = TAG_CONFIG_COMMIT;
                out[2] = *record_count;
                3
            }
            SyncMessage::EnterBootloader => {
                out[1] = TAG_ENTER_BOOTLOADER;
                out[2..6].copy_from_slice(&BOOTLOADER_SYNC_MAGIC.to_le_bytes());
                6
            }
            SyncMessage::PeripheralVersion(v) => {
                out[1] = TAG_PERIPHERAL_VERSION;
                out[2] = v.major;
                out[3] = v.minor;
                out[4] = v.patch;
                out[5..13].copy_from_slice(&v.git_hash);
                out[13] = v.dirty as u8;
                14
            }
        }
    }

    /// Decode one payload. Known tags require exactly their layout's length
    /// (additive evolution uses new tags, never trailing bytes).
    pub fn decode(bytes: &[u8]) -> Result<SyncMessage, SyncDecodeError> {
        if bytes.len() < 2 {
            return Err(SyncDecodeError::BadLength);
        }
        if bytes[0] != SYNC_VERSION {
            return Err(SyncDecodeError::UnsupportedVersion(bytes[0]));
        }
        match bytes[1] {
            TAG_SET_CELLS => {
                if bytes.len() < 3 {
                    return Err(SyncDecodeError::BadLength);
                }
                let count = bytes[2] as usize;
                if count > MAX_CELLS_PER_SYNC || bytes.len() != 3 + count * CELL_ENTRY_LEN {
                    return Err(SyncDecodeError::BadLength);
                }
                let mut cells = SyncCells::new();
                for i in 0..count {
                    let (key, cell) = get_cell(bytes, 3 + i * CELL_ENTRY_LEN)?;
                    cells.push(key, cell);
                }
                Ok(SyncMessage::SetCells(cells))
            }
            TAG_UNSET_KEYS => {
                if bytes.len() < 3 {
                    return Err(SyncDecodeError::BadLength);
                }
                let count = bytes[2] as usize;
                if count > MAX_UNSETS_PER_SYNC || bytes.len() != 3 + count {
                    return Err(SyncDecodeError::BadLength);
                }
                let mut keys = SyncKeys::new();
                for &key in &bytes[3..3 + count] {
                    keys.push(key);
                }
                Ok(SyncMessage::UnsetKeys(keys))
            }
            TAG_CLEAR => {
                if bytes.len() != 2 {
                    return Err(SyncDecodeError::BadLength);
                }
                Ok(SyncMessage::Clear)
            }
            TAG_STATE | TAG_STATE2 => {
                let extended = bytes[1] == TAG_STATE2;
                let want = if extended { 9 } else { 8 };
                if bytes.len() != want {
                    return Err(SyncDecodeError::BadLength);
                }
                let toggles = bytes[4] as u32
                    | ((bytes[5] as u32) << 8)
                    | ((bytes[6] as u32) << 16)
                    | ((bytes[7] as u32) << 24);
                let usb_connected = extended && bytes[8] != 0;
                Ok(SyncMessage::State {
                    brightness: bytes[2],
                    ceiling: bytes[3],
                    toggles,
                    usb_connected,
                })
            }
            TAG_CONFIG_RESET => {
                if bytes.len() != 3 {
                    return Err(SyncDecodeError::BadLength);
                }
                Ok(SyncMessage::ConfigReset { record_count: bytes[2] })
            }
            TAG_CONFIG_RECORD => {
                if bytes.len() != 6 {
                    return Err(SyncDecodeError::BadLength);
                }
                Ok(SyncMessage::ConfigRecord {
                    index: bytes[2],
                    activation: get_activation(bytes[3], bytes[4])?,
                    cell_count: bytes[5],
                })
            }
            TAG_CONFIG_GATE => {
                if bytes.len() != 5 {
                    return Err(SyncDecodeError::BadLength);
                }
                let gate = match Condition::from_gate_wire(bytes[3], bytes[4]) {
                    Ok(Some(c)) => c,
                    // Kind 0 ("no gate") is never a valid gate message.
                    Ok(None) => return Err(SyncDecodeError::UnknownGate(0)),
                    Err(crate::UnknownCondition(k)) => {
                        return Err(SyncDecodeError::UnknownGate(k));
                    }
                };
                Ok(SyncMessage::ConfigGate { record_index: bytes[2], gate })
            }
            TAG_CONFIG_CELLS => {
                if bytes.len() < 4 {
                    return Err(SyncDecodeError::BadLength);
                }
                let count = bytes[3] as usize;
                if count > MAX_CELLS_PER_SYNC || bytes.len() != 4 + count * CELL_ENTRY_LEN {
                    return Err(SyncDecodeError::BadLength);
                }
                let mut cells = SyncCells::new();
                for i in 0..count {
                    let (key, cell) = get_cell(bytes, 4 + i * CELL_ENTRY_LEN)?;
                    cells.push(key, cell);
                }
                Ok(SyncMessage::ConfigCells { record_index: bytes[2], cells })
            }
            TAG_CONFIG_COMMIT => {
                if bytes.len() != 3 {
                    return Err(SyncDecodeError::BadLength);
                }
                Ok(SyncMessage::ConfigCommit { record_count: bytes[2] })
            }
            TAG_ENTER_BOOTLOADER => {
                if bytes.len() != 6 {
                    return Err(SyncDecodeError::BadLength);
                }
                let magic =
                    u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                if magic != BOOTLOADER_SYNC_MAGIC {
                    return Err(SyncDecodeError::BadMagic);
                }
                Ok(SyncMessage::EnterBootloader)
            }
            TAG_PERIPHERAL_VERSION => {
                if bytes.len() != 14 {
                    return Err(SyncDecodeError::BadLength);
                }
                let mut git_hash = [0u8; 8];
                git_hash.copy_from_slice(&bytes[5..13]);
                Ok(SyncMessage::PeripheralVersion(PeripheralVersion {
                    major: bytes[2],
                    minor: bytes[3],
                    patch: bytes[4],
                    git_hash,
                    dirty: bytes[13] != 0,
                }))
            }
            tag => Err(SyncDecodeError::UnknownTag(tag)),
        }
    }
}

/// The central's authoritative store for the peripheral half's host-overlay
/// cells, indexed by LOCAL key (`0..N`). Mirrors the semantics of the
/// compositor's own host overlay (set replaces, TTL expiry reverts to
/// transparent) without rendering anything.
pub struct RemoteOverlay<const N: usize> {
    /// `cell, absolute expiry (now_ms scale)` per local key.
    cells: [Option<(Cell, Option<u64>)>; N],
}

impl<const N: usize> Default for RemoteOverlay<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Keys removed by [`RemoteOverlay::expire`].
pub struct ExpiredKeys<const N: usize> {
    len: usize,
    keys: [u8; N],
}

impl<const N: usize> ExpiredKeys<N> {
    pub fn as_slice(&self) -> &[u8] {
        &self.keys[..self.len]
    }
}

impl<const N: usize> RemoteOverlay<N> {
    pub const fn new() -> Self {
        Self { cells: [None; N] }
    }

    /// Set or replace the cell for local `key`, optionally expiring `ttl_ms`
    /// after `now_ms`. Out-of-range keys are ignored (`false`).
    pub fn set(&mut self, key: u8, cell: Cell, ttl_ms: Option<u32>, now_ms: u64) -> bool {
        match self.cells.get_mut(key as usize) {
            Some(slot) => {
                *slot = Some((cell, ttl_ms.map(|t| now_ms + t as u64)));
                true
            }
            None => false,
        }
    }

    pub fn unset(&mut self, key: u8) {
        if let Some(slot) = self.cells.get_mut(key as usize) {
            *slot = None;
        }
    }

    pub fn clear(&mut self) {
        self.cells = [None; N];
    }

    /// Live entries as `(local key, cell, absolute expiry)`. Entries past
    /// their expiry may linger until [`expire`](Self::expire) runs; callers
    /// comparing against a clock must filter, as the compositor's read-back
    /// path does.
    pub fn cells(&self) -> impl Iterator<Item = (u8, Cell, Option<u64>)> + '_ {
        self.cells
            .iter()
            .enumerate()
            .filter_map(|(k, slot)| slot.map(|(cell, exp)| (k as u8, cell, exp)))
    }

    /// Drop every cell whose expiry has passed and return their keys (so the
    /// central can forward the unsets).
    pub fn expire(&mut self, now_ms: u64) -> ExpiredKeys<N> {
        let mut expired = ExpiredKeys { len: 0, keys: [0; N] };
        for (k, slot) in self.cells.iter_mut().enumerate() {
            if matches!(slot, Some((_, Some(at))) if *at <= now_ms) {
                *slot = None;
                expired.keys[expired.len] = k as u8;
                expired.len += 1;
            }
        }
        expired
    }

    /// The earliest pending expiry, or `None` when nothing can expire.
    pub fn next_expiry(&self) -> Option<u64> {
        self.cells.iter().flatten().filter_map(|(_, exp)| *exp).min()
    }
}

// --- Persistent-config record transfer (Phase 4) ---------------------------

/// Peripheral-side staging area for a persistent-config record set arriving
/// as `ConfigReset` / `ConfigRecord` / `ConfigCells` / `ConfigCommit`
/// messages. The live compositor records are only touched when a COMPLETE
/// set commits: any gap (lost message, link drop mid-transfer, stray
/// out-of-order commit) discards the stage and keeps the previous records —
/// the central retransmits from `ConfigReset` on the next resync.
pub struct ConfigStage {
    /// Declared record count; `None` = no transfer in progress.
    expected: Option<u8>,
    records: [Record; MAX_RECORDS],
    declared: [bool; MAX_RECORDS],
    expected_cells: [u8; MAX_RECORDS],
}

impl Default for ConfigStage {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigStage {
    pub const fn new() -> Self {
        Self {
            expected: None,
            records: [Record::new(Activation::Always); MAX_RECORDS],
            declared: [false; MAX_RECORDS],
            expected_cells: [0; MAX_RECORDS],
        }
    }

    /// Begin staging `record_count` records; discards any half-staged set.
    /// `false` (stage inactive) when the count exceeds [`MAX_RECORDS`].
    pub fn reset(&mut self, record_count: u8) -> bool {
        self.declared = [false; MAX_RECORDS];
        self.expected_cells = [0; MAX_RECORDS];
        if record_count as usize > MAX_RECORDS {
            self.expected = None;
            return false;
        }
        self.expected = Some(record_count);
        true
    }

    /// Declare record `index`. Out-of-range or out-of-transfer declarations
    /// abort the stage.
    pub fn record(&mut self, index: u8, activation: Activation, cell_count: u8) {
        let ok = matches!(self.expected, Some(n) if index < n)
            && cell_count as usize <= MAX_CELLS_PER_RECORD;
        if !ok {
            self.expected = None;
            return;
        }
        self.records[index as usize] = Record::new(activation);
        self.declared[index as usize] = true;
        self.expected_cells[index as usize] = cell_count;
    }

    /// Attach a gate to staged record `record_index` (conditional lighting).
    /// A gate for an undeclared or out-of-transfer record aborts the stage.
    pub fn gate(&mut self, record_index: u8, gate: Condition) {
        let declared = matches!(self.expected, Some(n) if record_index < n)
            && self.declared[record_index as usize];
        if !declared {
            self.expected = None;
            return;
        }
        self.records[record_index as usize].set_gate(Some(gate));
    }

    /// Append cells to staged record `record_index`. Any inconsistency
    /// (undeclared record, overflow past the declared cell count) aborts the
    /// stage.
    pub fn cells(&mut self, record_index: u8, cells: &[(u8, Cell)]) {
        let declared = matches!(self.expected, Some(n) if record_index < n)
            && self.declared[record_index as usize];
        if !declared {
            self.expected = None;
            return;
        }
        let record = &mut self.records[record_index as usize];
        for &(key, cell) in cells {
            let over = record.cells().count() >= self.expected_cells[record_index as usize] as usize;
            if over || record.set(key, cell).is_err() {
                self.expected = None;
                return;
            }
        }
    }

    /// Commit: when the staged set is complete and matches `record_count`,
    /// return the records (and deactivate the stage); otherwise discard.
    pub fn commit(&mut self, record_count: u8) -> Option<&[Record]> {
        let expected = self.expected.take()?;
        if expected != record_count {
            return None;
        }
        let n = expected as usize;
        for i in 0..n {
            if !self.declared[i] || self.records[i].cells().count() != self.expected_cells[i] as usize {
                return None;
            }
        }
        Some(&self.records[..n])
    }

    /// Whether a transfer is currently staged (for diagnostics).
    pub fn in_progress(&self) -> bool {
        self.expected.is_some()
    }
}

/// Central-side cursor producing the message stream that transfers a record
/// set: `ConfigReset`, then per record `ConfigRecord` + its `ConfigCells`
/// batches, then `ConfigCommit`. Pure and restartable: the firmware peeks
/// [`next_message`](Self::next_message), tries to queue it, and only then
/// [`advance`](Self::advance)s — so a full split queue simply retries the
/// same message later. The record slice must not change mid-push (the
/// firmware restarts the push whenever it swaps record sets).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConfigPush {
    Reset,
    /// About to declare record `record`.
    Record { record: u8 },
    /// About to send record `record`'s gate (only for gated records).
    Gate { record: u8 },
    /// About to send the batch starting at cell `cell` of record `record`.
    Cells { record: u8, cell: u8 },
    Commit,
    Done,
}

impl ConfigPush {
    pub const fn new() -> Self {
        ConfigPush::Reset
    }

    /// The next message to send, or `None` when the push is complete.
    pub fn next_message(&self, records: &[Record]) -> Option<SyncMessage> {
        match *self {
            ConfigPush::Reset => Some(SyncMessage::ConfigReset { record_count: records.len() as u8 }),
            ConfigPush::Record { record } => {
                let r = records.get(record as usize)?;
                Some(SyncMessage::ConfigRecord {
                    index: record,
                    activation: r.activation(),
                    cell_count: r.cells().count() as u8,
                })
            }
            ConfigPush::Gate { record } => {
                let r = records.get(record as usize)?;
                // Only reached for gated records; a missing gate is skipped.
                r.gate().map(|gate| SyncMessage::ConfigGate { record_index: record, gate })
            }
            ConfigPush::Cells { record, cell } => {
                let r = records.get(record as usize)?;
                let mut batch = SyncCells::new();
                for (key, c) in r.cells().skip(cell as usize).take(MAX_CELLS_PER_SYNC) {
                    batch.push(key, *c);
                }
                Some(SyncMessage::ConfigCells { record_index: record, cells: batch })
            }
            ConfigPush::Commit => Some(SyncMessage::ConfigCommit { record_count: records.len() as u8 }),
            ConfigPush::Done => None,
        }
    }

    /// Step the cursor past the message [`next_message`](Self::next_message)
    /// just produced (call only after that message was queued successfully).
    pub fn advance(&mut self, records: &[Record]) {
        let next_record_or_commit = |record: u8| {
            if (record as usize + 1) < records.len() {
                ConfigPush::Record { record: record + 1 }
            } else {
                ConfigPush::Commit
            }
        };
        // After a record's header (and gate) are sent, either stream its
        // cells or move on if it has none.
        let cells_or_next = |record: u8| {
            let cells = records.get(record as usize).map_or(0, |r| r.cells().count());
            if cells == 0 {
                next_record_or_commit(record)
            } else {
                ConfigPush::Cells { record, cell: 0 }
            }
        };
        *self = match *self {
            ConfigPush::Reset => {
                if records.is_empty() {
                    ConfigPush::Commit
                } else {
                    ConfigPush::Record { record: 0 }
                }
            }
            ConfigPush::Record { record } => {
                // A gated record sends its gate before its cells.
                if records.get(record as usize).is_some_and(|r| r.gate().is_some()) {
                    ConfigPush::Gate { record }
                } else {
                    cells_or_next(record)
                }
            }
            ConfigPush::Gate { record } => cells_or_next(record),
            ConfigPush::Cells { record, cell } => {
                let cells = records.get(record as usize).map_or(0, |r| r.cells().count());
                let next = cell as usize + MAX_CELLS_PER_SYNC;
                if next < cells {
                    ConfigPush::Cells { record, cell: next as u8 }
                } else {
                    next_record_or_commit(record)
                }
            }
            ConfigPush::Commit => ConfigPush::Done,
            ConfigPush::Done => ConfigPush::Done,
        };
    }

    pub fn done(&self) -> bool {
        matches!(self, ConfigPush::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgb;

    const RED: Rgb = Rgb::new(200, 0, 0);

    fn solid(color: Rgb) -> Cell {
        Cell::Solid { color }
    }

    fn roundtrip(msg: SyncMessage) {
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = msg.encode(&mut buf);
        assert!(len <= MAX_SYNC_PAYLOAD);
        assert_eq!(SyncMessage::decode(&buf[..len]), Ok(msg));
    }

    #[test]
    fn messages_roundtrip() {
        let mut cells = SyncCells::new();
        assert!(cells.push(17, Cell::Blink { color: RED, period_ms: 500, phase_ms: 100, duty_pct: 25 }));
        assert!(cells.push(39, Cell::Breathe { color: Rgb::new(0, 0, 255), period_ms: 3000, phase_ms: 1500 }));
        assert!(cells.is_full());
        assert!(!cells.push(5, solid(RED)), "capacity enforced");
        roundtrip(SyncMessage::SetCells(cells));
        roundtrip(SyncMessage::SetCells(SyncCells::new()));

        let mut transparent = SyncCells::new();
        transparent.push(4, Cell::Transparent);
        roundtrip(SyncMessage::SetCells(transparent));

        let mut keys = SyncKeys::new();
        for k in 0..MAX_UNSETS_PER_SYNC {
            assert!(keys.push(k as u8));
        }
        assert!(!keys.push(99), "capacity enforced");
        roundtrip(SyncMessage::UnsetKeys(keys));

        roundtrip(SyncMessage::Clear);
        roundtrip(SyncMessage::State {
            brightness: 128,
            ceiling: 204,
            toggles: 0xA5A5_5A5A,
            usb_connected: true,
        });
        roundtrip(SyncMessage::State {
            brightness: 0,
            ceiling: 10,
            toggles: 0,
            usb_connected: false,
        });
    }

    #[test]
    fn legacy_state_decodes_with_usb_disconnected() {
        // A pre-gate central sends the 8-byte TAG_STATE (0x04); a new
        // peripheral must accept it and assume usb disconnected.
        let legacy = [SYNC_VERSION, 0x04, 200, 100, 0x01, 0x00, 0x00, 0x00];
        assert_eq!(
            SyncMessage::decode(&legacy),
            Ok(SyncMessage::State { brightness: 200, ceiling: 100, toggles: 1, usb_connected: false })
        );
        // The extended encoding uses a distinct tag (0x0C).
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::State { brightness: 200, ceiling: 100, toggles: 1, usb_connected: true }
            .encode(&mut buf);
        assert_eq!(buf[1], 0x0C);
        assert_eq!(len, 9);
    }

    #[test]
    fn config_gate_roundtrips_and_rejects_unknown() {
        for gate in [
            Condition::LayerActive(2),
            Condition::Toggle(31),
            Condition::UsbConnected,
            Condition::Charging,
            Condition::SplitLinkUp,
        ] {
            roundtrip(SyncMessage::ConfigGate { record_index: 3, gate });
        }
        // Unknown gate kind and the reserved "no gate" kind 0 are rejected.
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::ConfigGate { record_index: 0, gate: Condition::UsbConnected }
            .encode(&mut buf);
        buf[3] = 9;
        assert_eq!(SyncMessage::decode(&buf[..len]), Err(SyncDecodeError::UnknownGate(9)));
        buf[3] = 0;
        assert_eq!(SyncMessage::decode(&buf[..len]), Err(SyncDecodeError::UnknownGate(0)));
    }

    #[test]
    fn full_batch_fits_the_payload() {
        let mut cells = SyncCells::new();
        for k in 0..MAX_CELLS_PER_SYNC {
            cells.push(k as u8, solid(RED));
        }
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::SetCells(cells).encode(&mut buf);
        assert_eq!(len, 3 + MAX_CELLS_PER_SYNC * 10);
        assert!(len <= MAX_SYNC_PAYLOAD);
        // The largest of the other kinds also fits.
        let mut keys = SyncKeys::new();
        for k in 0..MAX_UNSETS_PER_SYNC {
            keys.push(k as u8);
        }
        assert!(SyncMessage::UnsetKeys(keys).encode(&mut buf) <= MAX_SYNC_PAYLOAD);
    }

    #[test]
    fn decode_rejects_foreign_and_malformed_payloads() {
        // Version and tag are the tolerated forward-compat rejections.
        assert_eq!(
            SyncMessage::decode(&[SYNC_VERSION + 1, TAG_CLEAR]),
            Err(SyncDecodeError::UnsupportedVersion(SYNC_VERSION + 1))
        );
        assert_eq!(SyncMessage::decode(&[SYNC_VERSION, 0x7F]), Err(SyncDecodeError::UnknownTag(0x7F)));
        // Length must match the tag's layout exactly.
        assert_eq!(SyncMessage::decode(&[SYNC_VERSION]), Err(SyncDecodeError::BadLength));
        assert_eq!(SyncMessage::decode(&[SYNC_VERSION, TAG_CLEAR, 0]), Err(SyncDecodeError::BadLength));
        assert_eq!(SyncMessage::decode(&[SYNC_VERSION, TAG_SET_CELLS, 1, 0, 0]), Err(SyncDecodeError::BadLength));
        assert_eq!(
            SyncMessage::decode(&[SYNC_VERSION, TAG_SET_CELLS, 3]),
            Err(SyncDecodeError::BadLength),
            "count above MAX_CELLS_PER_SYNC"
        );
        assert_eq!(SyncMessage::decode(&[SYNC_VERSION, TAG_STATE, 0, 0]), Err(SyncDecodeError::BadLength));
        // Unknown cell kind inside an otherwise valid batch.
        let mut cells = SyncCells::new();
        cells.push(0, solid(RED));
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::SetCells(cells).encode(&mut buf);
        buf[4] = 9; // kind byte of entry 0
        assert_eq!(SyncMessage::decode(&buf[..len]), Err(SyncDecodeError::UnknownCellKind(9)));
    }

    #[test]
    fn config_messages_roundtrip() {
        roundtrip(SyncMessage::ConfigReset { record_count: 7 });
        roundtrip(SyncMessage::ConfigRecord {
            index: 3,
            activation: Activation::LayerActive(2),
            cell_count: 40,
        });
        roundtrip(SyncMessage::ConfigRecord { index: 0, activation: Activation::Toggle(31), cell_count: 0 });
        let mut cells = SyncCells::new();
        cells.push(4, solid(RED));
        cells.push(39, Cell::Breathe { color: RED, period_ms: 2000, phase_ms: 0 });
        roundtrip(SyncMessage::ConfigCells { record_index: 5, cells });
        roundtrip(SyncMessage::ConfigCommit { record_count: 7 });
        roundtrip(SyncMessage::EnterBootloader);
        roundtrip(SyncMessage::PeripheralVersion(PeripheralVersion {
            major: 0,
            minor: 1,
            patch: 0,
            git_hash: *b"1a2b3c4d",
            dirty: true,
        }));

        // Bootloader entry without the wire magic is rejected.
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::EnterBootloader.encode(&mut buf);
        buf[2] ^= 0xFF;
        assert_eq!(SyncMessage::decode(&buf[..len]), Err(SyncDecodeError::BadMagic));

        // Unknown activation kind is rejected (stage discards on the drop).
        let mut buf = [0u8; MAX_SYNC_PAYLOAD];
        let len = SyncMessage::ConfigRecord { index: 0, activation: Activation::Always, cell_count: 1 }
            .encode(&mut buf);
        buf[3] = 9;
        assert_eq!(SyncMessage::decode(&buf[..len]), Err(SyncDecodeError::UnknownActivation(9)));
    }

    fn stage_records() -> Vec<Record> {
        let mut base = Record::new(Activation::Always);
        for k in 0..5u8 {
            base.set(k, solid(RED)).unwrap();
        }
        // Gated layer record exercises the ConfigGate step in the push.
        let mut layer = Record::new(Activation::LayerActive(1));
        layer.set(7, Cell::Blink { color: RED, period_ms: 400, phase_ms: 0, duty_pct: 50 }).unwrap();
        let layer = layer.gated(Condition::LayerActive(1));
        // Gated zero-cell record: gate is sent, then straight to the next.
        let toggle = Record::new(Activation::Toggle(3)).gated(Condition::UsbConnected);
        vec![base, layer, toggle]
    }

    fn assert_same_records(a: &[Record], b: &[Record]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.activation(), y.activation());
            assert_eq!(x.gate(), y.gate());
            let xs: Vec<_> = x.cells().map(|(k, c)| (k, *c)).collect();
            let ys: Vec<_> = y.cells().map(|(k, c)| (k, *c)).collect();
            assert_eq!(xs, ys);
        }
    }

    #[test]
    fn config_push_drives_stage_to_commit() {
        let records = stage_records();
        let mut push = ConfigPush::new();
        let mut stage = ConfigStage::new();
        let mut committed = None;
        let mut msgs = 0;
        while let Some(msg) = push.next_message(&records) {
            msgs += 1;
            assert!(msgs < 100, "push must terminate");
            // Wire roundtrip on the way through.
            let mut buf = [0u8; MAX_SYNC_PAYLOAD];
            let len = msg.encode(&mut buf);
            let msg = SyncMessage::decode(&buf[..len]).unwrap();
            match msg {
                SyncMessage::ConfigReset { record_count } => {
                    stage.reset(record_count);
                }
                SyncMessage::ConfigRecord { index, activation, cell_count } => {
                    stage.record(index, activation, cell_count);
                }
                SyncMessage::ConfigGate { record_index, gate } => {
                    stage.gate(record_index, gate);
                }
                SyncMessage::ConfigCells { record_index, cells } => {
                    stage.cells(record_index, cells.entries());
                }
                SyncMessage::ConfigCommit { record_count } => {
                    committed = stage.commit(record_count).map(<[Record]>::to_vec);
                }
                other => panic!("unexpected message {other:?}"),
            }
            push.advance(&records);
        }
        assert!(push.done());
        assert_same_records(&committed.expect("commit must succeed"), &records);
        assert!(!stage.in_progress());
    }

    #[test]
    fn config_push_empty_set() {
        let records: Vec<Record> = vec![];
        let mut push = ConfigPush::new();
        let mut stage = ConfigStage::new();
        // Reset then commit, nothing else.
        assert_eq!(push.next_message(&records), Some(SyncMessage::ConfigReset { record_count: 0 }));
        push.advance(&records);
        assert_eq!(push.next_message(&records), Some(SyncMessage::ConfigCommit { record_count: 0 }));
        stage.reset(0);
        let committed = stage.commit(0).unwrap();
        assert!(committed.is_empty());
        push.advance(&records);
        assert!(push.done());
    }

    #[test]
    fn config_stage_discards_incomplete_or_inconsistent_sets() {
        let records = stage_records();

        // Missing cells: declared 5, delivered 2.
        let mut stage = ConfigStage::new();
        stage.reset(1);
        stage.record(0, Activation::Always, 5);
        stage.cells(0, &[(0, solid(RED)), (1, solid(RED))]);
        assert_eq!(stage.commit(1), None);

        // Commit without reset.
        assert_eq!(stage.commit(1), None);

        // Record count mismatch at commit.
        stage.reset(2);
        stage.record(0, Activation::Always, 0);
        stage.record(1, Activation::Always, 0);
        assert_eq!(stage.commit(3), None);

        // Cells for an undeclared record abort the stage.
        stage.reset(2);
        stage.record(0, Activation::Always, 1);
        stage.cells(1, &[(0, solid(RED))]);
        assert!(!stage.in_progress());
        assert_eq!(stage.commit(2), None);

        // A gate for an undeclared record aborts the stage.
        stage.reset(2);
        stage.record(0, Activation::Always, 0);
        stage.gate(1, Condition::UsbConnected);
        assert!(!stage.in_progress());

        // Cell overflow past the declared count aborts the stage.
        stage.reset(1);
        stage.record(0, Activation::Always, 1);
        stage.cells(0, &[(0, solid(RED)), (1, solid(RED))]);
        assert_eq!(stage.commit(1), None);

        // Reset above capacity deactivates.
        assert!(!stage.reset(MAX_RECORDS as u8 + 1));
        assert!(!stage.in_progress());

        // A fresh, complete run still works after all the aborts.
        let mut push = ConfigPush::new();
        stage.reset(records.len() as u8);
        push.advance(&records);
        while let Some(msg) = push.next_message(&records) {
            match msg {
                SyncMessage::ConfigRecord { index, activation, cell_count } => {
                    stage.record(index, activation, cell_count)
                }
                SyncMessage::ConfigGate { record_index, gate } => {
                    stage.gate(record_index, gate)
                }
                SyncMessage::ConfigCells { record_index, cells } => {
                    stage.cells(record_index, cells.entries())
                }
                SyncMessage::ConfigCommit { record_count } => {
                    assert!(stage.commit(record_count).is_some());
                }
                other => panic!("unexpected message {other:?}"),
            }
            push.advance(&records);
        }
    }

    #[test]
    fn remote_overlay_set_unset_clear() {
        let mut r = RemoteOverlay::<40>::new();
        assert!(r.set(3, solid(RED), None, 0));
        assert!(r.set(3, Cell::Transparent, None, 0), "set replaces by key");
        assert!(r.set(39, solid(RED), Some(100), 0));
        assert!(!r.set(40, solid(RED), None, 0), "out of range ignored");
        let mut got: Vec<_> = r.cells().collect();
        got.sort_by_key(|(k, _, _)| *k);
        assert_eq!(got, vec![(3, Cell::Transparent, None), (39, solid(RED), Some(100))]);

        r.unset(3);
        assert_eq!(r.cells().count(), 1);
        r.clear();
        assert_eq!(r.cells().count(), 0);
        assert_eq!(r.next_expiry(), None);
    }

    #[test]
    fn remote_overlay_ttl_expiry() {
        let mut r = RemoteOverlay::<40>::new();
        r.set(1, solid(RED), Some(1000), 0);
        r.set(2, solid(RED), Some(500), 0);
        r.set(3, solid(RED), None, 0);
        assert_eq!(r.next_expiry(), Some(500));

        assert_eq!(r.expire(499).as_slice(), &[] as &[u8]);
        assert_eq!(r.expire(500).as_slice(), &[2]);
        assert_eq!(r.next_expiry(), Some(1000));
        assert_eq!(r.expire(5000).as_slice(), &[1]);
        assert_eq!(r.next_expiry(), None, "TTL-less cell never expires");
        assert_eq!(r.cells().count(), 1);
    }
}
