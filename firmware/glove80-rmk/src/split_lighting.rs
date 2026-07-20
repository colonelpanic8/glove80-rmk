//! Split lighting transfer (Phase 3 of docs/implementation-plan.md): the
//! firmware glue between the pure-logic sync layer
//! (`glove80_compositor::sync`) and RMK's split application channel
//! (`rmk::split_app`).
//!
//! Roles (both compiled into `lighting.rs`'s single loop; the binary picks
//! its role in `central.rs` / `peripheral.rs`):
//!
//! - **Central** ([`CentralSplit`]): owns the authoritative store for the
//!   right half's host-overlay cells ([`RemoteOverlay`], protocol keys
//!   `40..80` remapped to local `0..40`), including all TTL bookkeeping.
//!   Mutations are mirrored to the peripheral as bounded [`SyncMessage`]
//!   deltas via `try_send` — NEVER a blocking send, so lighting can never
//!   stall key traffic. If the queue overflows, or on every link-up edge,
//!   the central schedules a full **resync** (clear + every live cell +
//!   shared state), which is idempotent and therefore safe to repeat.
//! - **Peripheral** ([`PeripheralSplit`]): applies received messages to its
//!   own compositor's host overlay (the lighting task stays the compositor's
//!   single owner — messages are handed to it, never applied elsewhere).
//!   Link-loss policy: when the central link drops, the peripheral clears
//!   its host overlay after [`LINK_LOSS_GRACE_MS`] — the TTL/authority for
//!   those cells is gone, so they must not outlive it; the grace period
//!   avoids visible flicker across transient reconnects (which end in a
//!   resync anyway). Brightness/ceiling/toggles are kept across link loss,
//!   like the synced layer state.

use glove80_compositor::sync::{
    ConfigPush, ConfigStage, MAX_SYNC_PAYLOAD, PeripheralVersion, RemoteOverlay, SyncCells,
    SyncKeys, SyncMessage,
};
use glove80_compositor::{Cell, Compositor, MAX_RECORDS, Record};
use rmk::split_app::{SPLIT_APP_MSG_MAX, SPLIT_APP_PERIPH_TX, SPLIT_APP_TX, SplitAppData};

use crate::lighting::NUM_LEDS;

// The sync codec's payload bound and RMK's split-app buffer size must
// agree; both are deliberately small (they size every split transfer).
const _: () = assert!(MAX_SYNC_PAYLOAD == SPLIT_APP_MSG_MAX);

/// How long the peripheral keeps host-overlay cells lit after losing the
/// central. Long enough to ride out a routine reconnect without flicker,
/// short enough that authority-less indicators cannot linger.
pub const LINK_LOSS_GRACE_MS: u64 = 5_000;

/// Retry cadence for a resync that could not be queued in one go.
const RESYNC_RETRY_MS: u64 = 50;

/// Persistent-config push pacing (Phase 4): at most this many messages per
/// tick, one tick per [`PUSH_TICK_MS`]. Sized so the peripheral's bounded
/// split inbox (capacity 8, drained promptly by its lighting task) can never
/// overflow, while a full 16-record / 640-cell set still transfers in under
/// two seconds.
const PUSH_MSGS_PER_TICK: usize = 4;
const PUSH_TICK_MS: u64 = 20;

/// Encode and queue one central → peripheral message; `false` if the
/// (bounded) queue is full.
fn try_queue(msg: &SyncMessage) -> bool {
    let mut buf = [0u8; MAX_SYNC_PAYLOAD];
    let len = msg.encode(&mut buf);
    // `new` cannot fail: len <= MAX_SYNC_PAYLOAD == SPLIT_APP_MSG_MAX.
    let Some(data) = SplitAppData::new(&buf[..len]) else {
        return false;
    };
    SPLIT_APP_TX.try_send(data).is_ok()
}

/// Encode and queue one peripheral → central message; `false` if the
/// (bounded) queue is full.
fn try_queue_to_central(msg: &SyncMessage) -> bool {
    let mut buf = [0u8; MAX_SYNC_PAYLOAD];
    let len = msg.encode(&mut buf);
    let Some(data) = SplitAppData::new(&buf[..len]) else {
        return false;
    };
    SPLIT_APP_PERIPH_TX.try_send(data).is_ok()
}

/// This build's identity as the peripheral announces it over the split link
/// (and as the central reports itself over the host protocol).
pub fn own_version() -> PeripheralVersion {
    PeripheralVersion {
        major: crate::version::FW_MAJOR,
        minor: crate::version::FW_MINOR,
        patch: crate::version::FW_PATCH,
        git_hash: crate::version::GIT_HASH,
        dirty: crate::version::GIT_DIRTY,
    }
}

/// Central-side split lighting state. Owned by the lighting task alongside
/// the compositor; see the module docs for the model.
pub struct CentralSplit {
    remote: RemoteOverlay<NUM_LEDS>,
    link_up: bool,
    /// `Some(t)` = a full resync is owed and should run at/after `t`
    /// (link-up edge or delta-queue overflow). While owed, delta queueing is
    /// suppressed — the resync will carry the final state.
    resync_at_ms: Option<u64>,
    /// The peripheral's persistent lighting records (Phase 4): right-half
    /// cells of the applied config, remapped to local keys. `None` until a
    /// stored config is applied — while running compiled defaults nothing is
    /// pushed and the peripheral renders its own identical compiled
    /// defaults.
    persist: Option<PersistState>,
    /// The peripheral's build identity, announced once per link-up (host
    /// protocol v1.3 GET_VERSION). Kept across link-down as the last-known
    /// version; `None` = never seen since this central booted.
    peripheral_version: Option<PeripheralVersion>,
}

/// Streaming state for the peripheral's persistent record set.
struct PersistState {
    count: usize,
    records: [Record; MAX_RECORDS],
    /// Active push cursor; `None` when the peripheral is up to date (or the
    /// link is down — every link-up edge restarts the push).
    push: Option<ConfigPush>,
    /// Earliest time the next push tick may run.
    push_at_ms: u64,
}

impl CentralSplit {
    // Constructed via `SplitRole::central()`, which only the central binary
    // calls; the peripheral binary compiles this as dead code.
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            remote: RemoteOverlay::new(),
            link_up: false,
            resync_at_ms: None,
            persist: None,
            peripheral_version: None,
        }
    }

    /// The peripheral's build identity for GET_VERSION: `(last-known
    /// version, currently connected)`. `None` = never seen since boot.
    pub fn peripheral_version(&self) -> (Option<PeripheralVersion>, bool) {
        (self.peripheral_version, self.link_up)
    }

    /// Apply one application message received FROM the peripheral (today
    /// only its build-identity announcement).
    fn apply_from_peripheral(&mut self, payload: &[u8]) {
        match SyncMessage::decode(payload) {
            Ok(SyncMessage::PeripheralVersion(v)) => {
                defmt::info!(
                    "split-lighting: peripheral is v{}.{}.{} ({=[u8]:a}{})",
                    v.major,
                    v.minor,
                    v.patch,
                    v.git_hash,
                    if v.dirty { "-dirty" } else { "" }
                );
                self.peripheral_version = Some(v);
            }
            Ok(_) => defmt::warn!("split-lighting: unexpected app message on the central"),
            Err(e) => defmt::warn!(
                "split-lighting: dropped message: {}",
                defmt::Debug2Format(&e)
            ),
        }
    }

    /// Live right-half cells as `(local key, cell, absolute expiry)`.
    pub fn remote_cells(&self) -> impl Iterator<Item = (u8, Cell, Option<u64>)> + '_ {
        self.remote.cells()
    }

    fn mark_resync(&mut self, at_ms: u64) {
        self.resync_at_ms = Some(match self.resync_at_ms {
            Some(cur) => cur.min(at_ms),
            None => at_ms,
        });
    }

    /// Whether deltas can be queued right now (link up, no resync owed).
    fn deltas_flow(&self) -> bool {
        self.link_up && self.resync_at_ms.is_none()
    }

    /// Queue a batch of cell writes; on overflow fall back to a resync.
    /// Returns `false` if the cells did not go out as deltas.
    fn queue_cells<'a>(
        &mut self,
        cells: impl Iterator<Item = &'a (u8, Cell)>,
        now_ms: u64,
    ) -> bool {
        if !self.deltas_flow() {
            return false;
        }
        let mut batch = SyncCells::new();
        for &(key, cell) in cells {
            batch.push(key, cell);
            if batch.is_full() {
                if !try_queue(&SyncMessage::SetCells(batch)) {
                    self.mark_resync(now_ms + RESYNC_RETRY_MS);
                    return false;
                }
                batch = SyncCells::new();
            }
        }
        if !batch.is_empty() && !try_queue(&SyncMessage::SetCells(batch)) {
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
            return false;
        }
        true
    }

    /// Queue a batch of unsets; same overflow fallback as [`queue_cells`].
    fn queue_unsets<'a>(&mut self, keys: impl Iterator<Item = &'a u8>, now_ms: u64) -> bool {
        if !self.deltas_flow() {
            return false;
        }
        let mut batch = SyncKeys::new();
        for &key in keys {
            batch.push(key);
            if batch.is_full() {
                if !try_queue(&SyncMessage::UnsetKeys(batch)) {
                    self.mark_resync(now_ms + RESYNC_RETRY_MS);
                    return false;
                }
                batch = SyncKeys::new();
            }
        }
        if !batch.is_empty() && !try_queue(&SyncMessage::UnsetKeys(batch)) {
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
            return false;
        }
        true
    }

    /// Store + forward right-half cell writes (`cells` in LOCAL keys, shared
    /// `ttl_ms` per the protocol). Returns `true` when the write reached a
    /// connected peripheral (⇒ protocol `OK`); `false` means the cells are
    /// held authoritatively here and will land via resync (⇒ `PARTIAL_APPLY`).
    pub fn write_cells(&mut self, cells: &[(u8, Cell)], ttl_ms: Option<u32>, now_ms: u64) -> bool {
        for &(key, cell) in cells {
            self.remote.set(key, cell, ttl_ms, now_ms);
        }
        self.queue_cells(cells.iter(), now_ms)
    }

    /// Store + forward right-half unsets (LOCAL keys).
    pub fn unset_keys(&mut self, keys: &[u8], now_ms: u64) -> bool {
        for &key in keys {
            self.remote.unset(key);
        }
        self.queue_unsets(keys.iter(), now_ms)
    }

    /// Clear the right half's overlay (store + forward).
    pub fn clear(&mut self, now_ms: u64) -> bool {
        self.remote.clear();
        if !self.deltas_flow() {
            return false;
        }
        if !try_queue(&SyncMessage::Clear) {
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
            return false;
        }
        true
    }

    /// Atomically replace the right half's overlay with `cells` (store +
    /// forward as clear-then-set).
    pub fn replace_cells(
        &mut self,
        cells: &[(u8, Cell)],
        ttl_ms: Option<u32>,
        now_ms: u64,
    ) -> bool {
        self.remote.clear();
        for &(key, cell) in cells {
            self.remote.set(key, cell, ttl_ms, now_ms);
        }
        if !self.deltas_flow() {
            return false;
        }
        if !try_queue(&SyncMessage::Clear) {
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
            return false;
        }
        self.queue_cells(cells.iter(), now_ms)
    }

    /// Ask the connected peripheral to reboot into its UF2 bootloader.
    /// `true` when the request was dispatched (⇒ protocol `OK`); `false`
    /// when the peripheral is offline or the queue is full (⇒ `BUSY` — the
    /// host retries). Magic-guarded on the wire.
    pub fn request_peripheral_bootloader(&mut self) -> bool {
        self.link_up && try_queue(&SyncMessage::EnterBootloader)
    }

    /// Best-effort forward of the shared state snapshot (brightness,
    /// effective ceiling, toggle bitmap). Falls back to resync on overflow.
    pub fn notify_state(&mut self, comp: &Compositor<NUM_LEDS>, now_ms: u64) {
        if !self.deltas_flow() {
            return; // resync (or the next link-up resync) carries it
        }
        let msg = SyncMessage::State {
            brightness: comp.brightness(),
            ceiling: comp.ceiling(),
            toggles: comp.toggles_mask(),
            usb_connected: comp.usb_connected(),
        };
        if !try_queue(&msg) {
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
        }
    }

    /// Install the peripheral's persistent record set (Phase 4: right-half
    /// cells of an applied config, already remapped to local keys) and start
    /// streaming it if the link is up. Also called on boot load, so a
    /// link-up may precede or follow it — both orders end in a push.
    pub fn set_persistent_records(&mut self, records: &[Record], now_ms: u64) {
        let mut state = PersistState {
            count: records.len().min(MAX_RECORDS),
            records: [Record::new(glove80_compositor::Activation::Always); MAX_RECORDS],
            push: None,
            push_at_ms: now_ms,
        };
        state.records[..state.count].copy_from_slice(&records[..state.count]);
        if self.link_up {
            state.push = Some(ConfigPush::new());
        }
        self.persist = Some(state);
    }

    /// React to a split-link edge. A `false → true` edge schedules the
    /// reconnect resync immediately and restarts the persistent-record push
    /// from the top (idempotent on the peripheral: a commit only lands when
    /// the staged set is complete).
    pub fn on_link_change(&mut self, up: bool, now_ms: u64) {
        self.link_up = up;
        self.resync_at_ms = if up { Some(now_ms) } else { None };
        if let Some(persist) = &mut self.persist {
            persist.push = up.then(ConfigPush::new);
            persist.push_at_ms = now_ms;
        }
    }

    /// The next moment this state machine needs the loop to wake: a pending
    /// right-half TTL expiry, an owed resync, or a persistent-push tick.
    pub fn next_deadline(&self) -> Option<u64> {
        let push_at = self
            .persist
            .as_ref()
            .filter(|p| p.push.is_some() && self.link_up)
            .map(|p| p.push_at_ms);
        [self.remote.next_expiry(), self.resync_at_ms, push_at]
            .into_iter()
            .flatten()
            .min()
    }

    /// Deadline housekeeping: expire right-half TTLs (forwarding the unsets
    /// — expiry authority lives here), run an owed resync, and stream the
    /// next slice of a pending persistent-record push.
    pub fn service(&mut self, comp: &Compositor<NUM_LEDS>, now_ms: u64) {
        let expired = self.remote.expire(now_ms);
        if !expired.as_slice().is_empty() {
            self.queue_unsets(expired.as_slice().iter(), now_ms);
        }
        if matches!(self.resync_at_ms, Some(at) if at <= now_ms) && self.link_up {
            self.resync(comp, now_ms);
        }
        self.service_push(now_ms);
    }

    /// One paced tick of the persistent-record push: peek → queue → advance,
    /// at most [`PUSH_MSGS_PER_TICK`] messages, retrying the same message
    /// later when the split queue is full.
    fn service_push(&mut self, now_ms: u64) {
        if !self.link_up {
            return;
        }
        let Some(persist) = &mut self.persist else {
            return;
        };
        let Some(push) = &mut persist.push else {
            return;
        };
        if now_ms < persist.push_at_ms {
            return;
        }
        for _ in 0..PUSH_MSGS_PER_TICK {
            let records = &persist.records[..persist.count];
            let Some(msg) = push.next_message(records) else {
                break;
            };
            if !try_queue(&msg) {
                // Queue full: keep the cursor, retry the same message soon.
                persist.push_at_ms = now_ms + RESYNC_RETRY_MS;
                return;
            }
            push.advance(records);
        }
        if push.done() {
            defmt::info!("split-lighting: persistent records pushed to the peripheral");
            persist.push = None;
        } else {
            persist.push_at_ms = now_ms + PUSH_TICK_MS;
        }
    }

    /// Push the complete right-half picture: clear, every live cell, shared
    /// state. Idempotent; re-queued in full on any overflow.
    fn resync(&mut self, comp: &Compositor<NUM_LEDS>, now_ms: u64) {
        self.resync_at_ms = None;
        let mut ok = try_queue(&SyncMessage::Clear);
        let mut batch = SyncCells::new();
        for (key, cell, expires_at) in self.remote.cells() {
            if !ok {
                break;
            }
            if matches!(expires_at, Some(at) if at <= now_ms) {
                continue; // expired while disconnected; expire() will purge
            }
            batch.push(key, cell);
            if batch.is_full() {
                ok = try_queue(&SyncMessage::SetCells(batch));
                batch = SyncCells::new();
            }
        }
        if ok && !batch.is_empty() {
            ok = try_queue(&SyncMessage::SetCells(batch));
        }
        if ok {
            ok = try_queue(&SyncMessage::State {
                brightness: comp.brightness(),
                ceiling: comp.ceiling(),
                toggles: comp.toggles_mask(),
                usb_connected: comp.usb_connected(),
            });
        }
        if !ok {
            defmt::debug!("split-lighting: resync overflowed the queue, retrying");
            self.mark_resync(now_ms + RESYNC_RETRY_MS);
        }
    }
}

/// Peripheral-side split lighting state (see the module docs for the
/// link-loss policy).
pub struct PeripheralSplit {
    /// When set, the host overlay is cleared at/after this time unless the
    /// central link comes back first.
    clear_at_ms: Option<u64>,
    /// Staging area for an incoming persistent-record set (Phase 4). The
    /// live records swap only on a complete commit; a partial transfer
    /// (link drop, lost message) is discarded and the previous records —
    /// compiled defaults or the last committed set — keep rendering. The
    /// set is NOT persisted here: the central is authoritative and
    /// re-streams it on every link-up.
    stage: ConfigStage,
    /// `Some(t)` = this half's build identity is owed to the central at/after
    /// `t` (set on every link-up edge; host protocol v1.3 GET_VERSION).
    /// Retried while the bounded peripheral → central queue is full.
    announce_at_ms: Option<u64>,
}

impl PeripheralSplit {
    // See CentralSplit::new: only the peripheral binary constructs this.
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            clear_at_ms: None,
            stage: ConfigStage::new(),
            announce_at_ms: None,
        }
    }

    pub fn on_link_change(&mut self, up: bool, now_ms: u64) {
        self.clear_at_ms = if up {
            None
        } else {
            Some(now_ms + LINK_LOSS_GRACE_MS)
        };
        // Announce the build identity once per link-up edge; a stale owed
        // announcement is dropped on link-down (the next link-up re-arms it).
        self.announce_at_ms = up.then_some(now_ms);
    }

    pub fn next_deadline(&self) -> Option<u64> {
        [self.clear_at_ms, self.announce_at_ms]
            .into_iter()
            .flatten()
            .min()
    }

    /// Deadline housekeeping: drop the authority-less host overlay once the
    /// link-loss grace expires, and send the owed build-identity
    /// announcement.
    pub fn service(&mut self, comp: &mut Compositor<NUM_LEDS>, now_ms: u64) {
        if matches!(self.clear_at_ms, Some(at) if at <= now_ms) {
            defmt::info!("split-lighting: central link lost, clearing host overlay");
            comp.host_clear();
            self.clear_at_ms = None;
        }
        if matches!(self.announce_at_ms, Some(at) if at <= now_ms) {
            if try_queue_to_central(&SyncMessage::PeripheralVersion(own_version())) {
                self.announce_at_ms = None;
            } else {
                // Queue momentarily full: retry shortly.
                self.announce_at_ms = Some(now_ms + RESYNC_RETRY_MS);
            }
        }
    }

    /// Apply one received sync message to the local compositor. Cells carry
    /// no TTL by design; expiry arrives as an unset from the central.
    pub fn apply(&mut self, comp: &mut Compositor<NUM_LEDS>, payload: &[u8], now_ms: u64) {
        match SyncMessage::decode(payload) {
            Ok(SyncMessage::SetCells(cells)) => {
                for &(key, cell) in cells.entries() {
                    match cell {
                        // Transparent means "reveal what is below" — same as
                        // not having a host cell at all.
                        Cell::Transparent => comp.host_unset(key),
                        // Cannot overflow: one slot per key, keys < NUM_LEDS
                        // == the overlay capacity; guarded anyway.
                        cell => {
                            if comp.host_set(key, cell, None, now_ms).is_err() {
                                defmt::warn!(
                                    "split-lighting: host overlay full, dropping key {}",
                                    key
                                );
                            }
                        }
                    }
                }
            }
            Ok(SyncMessage::UnsetKeys(keys)) => {
                for &key in keys.keys() {
                    comp.host_unset(key);
                }
            }
            Ok(SyncMessage::Clear) => comp.host_clear(),
            Ok(SyncMessage::ConfigReset { record_count }) => {
                if !self.stage.reset(record_count) {
                    defmt::warn!("split-lighting: config push exceeds record capacity");
                }
            }
            Ok(SyncMessage::ConfigRecord {
                index,
                activation,
                cell_count,
            }) => {
                self.stage.record(index, activation, cell_count);
            }
            Ok(SyncMessage::ConfigGate { record_index, gate }) => {
                self.stage.gate(record_index, gate);
            }
            Ok(SyncMessage::ConfigCells {
                record_index,
                cells,
            }) => {
                self.stage.cells(record_index, cells.entries());
            }
            Ok(SyncMessage::ConfigCommit { record_count }) => match self.stage.commit(record_count)
            {
                Some(records) => {
                    // Cannot fail: the stage capacity equals the
                    // compositor's; guarded anyway.
                    if comp.replace_records(records).is_err() {
                        defmt::warn!("split-lighting: config commit exceeded compositor capacity");
                    } else {
                        defmt::info!(
                            "split-lighting: persistent records applied ({})",
                            record_count
                        );
                    }
                }
                // Incomplete stage: keep the previous records; the central
                // restarts the push on the next link-up edge.
                None => defmt::warn!("split-lighting: incomplete config push discarded"),
            },
            Ok(SyncMessage::EnterBootloader) => {
                // Magic-checked at decode; requested by the central on the
                // host's behalf (ENTER_BOOTLOADER target 1).
                defmt::warn!("split-lighting: entering bootloader by central request");
                rmk::boot::jump_to_bootloader();
            }
            Ok(SyncMessage::State {
                brightness,
                ceiling,
                toggles,
                usb_connected,
            }) => {
                comp.set_brightness(brightness);
                // set_ceiling re-clamps to this half's compiled CHANNEL_CEILING.
                comp.set_ceiling(ceiling);
                comp.set_toggles_mask(toggles);
                comp.set_usb_connected(usb_connected);
            }
            // Peripheral → central only; a central would never echo it back.
            Ok(SyncMessage::PeripheralVersion(_)) => {
                defmt::warn!("split-lighting: unexpected version announcement from the central");
            }
            // Unknown version/tag: a newer central talking to an older
            // peripheral — ignore by contract. Anything else is a framing
            // bug worth a log line; dropping is always safe (state heals on
            // the next resync).
            Err(e) => defmt::warn!(
                "split-lighting: dropped message: {}",
                defmt::Debug2Format(&e)
            ),
        }
    }
}

/// Which side of the split this binary is, plus that side's lighting-sync
/// state. Owned by [`crate::lighting::LightingProcessor`] so the compositor
/// keeps exactly one owner.
// Each binary constructs exactly one variant (in `central.rs` /
// `peripheral.rs`), so the other variant and its constructor are dead code
// in that binary by design.
#[allow(dead_code)]
pub enum SplitRole {
    Central(CentralSplit),
    Peripheral(PeripheralSplit),
}

impl SplitRole {
    #[allow(dead_code)] // see the enum note
    pub const fn central() -> Self {
        SplitRole::Central(CentralSplit::new())
    }

    #[allow(dead_code)] // see the enum note
    pub const fn peripheral() -> Self {
        SplitRole::Peripheral(PeripheralSplit::new())
    }

    /// The central state, when this is the central (used by the host
    /// protocol semantics; `None` on the peripheral, which never receives
    /// host requests).
    pub fn central_mut(&mut self) -> Option<&mut CentralSplit> {
        match self {
            SplitRole::Central(c) => Some(c),
            SplitRole::Peripheral(_) => None,
        }
    }

    pub fn as_central(&self) -> Option<&CentralSplit> {
        match self {
            SplitRole::Central(c) => Some(c),
            SplitRole::Peripheral(_) => None,
        }
    }

    pub fn on_link_change(&mut self, up: bool, now_ms: u64) {
        match self {
            SplitRole::Central(c) => c.on_link_change(up, now_ms),
            SplitRole::Peripheral(p) => p.on_link_change(up, now_ms),
        }
    }

    /// The next self-driven wake this role needs (merged with the
    /// compositor's `next_wake_ms` by the lighting loop).
    pub fn next_deadline(&self) -> Option<u64> {
        match self {
            SplitRole::Central(c) => c.next_deadline(),
            SplitRole::Peripheral(p) => p.next_deadline(),
        }
    }

    /// Deadline housekeeping for either role.
    pub fn service(&mut self, comp: &mut Compositor<NUM_LEDS>, now_ms: u64) {
        match self {
            SplitRole::Central(c) => c.service(comp, now_ms),
            SplitRole::Peripheral(p) => p.service(comp, now_ms),
        }
    }

    /// Apply one received split application message: the central receives
    /// the peripheral's build-identity announcement, the peripheral receives
    /// forwarded overlay/config/state traffic.
    pub fn apply_message(&mut self, comp: &mut Compositor<NUM_LEDS>, payload: &[u8], now_ms: u64) {
        match self {
            SplitRole::Central(c) => c.apply_from_peripheral(payload),
            SplitRole::Peripheral(p) => p.apply(comp, payload, now_ms),
        }
    }
}
