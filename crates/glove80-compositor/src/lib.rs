//! Sparse lighting compositor for the Glove80 (docs/lighting-design.md).
//!
//! Pure logic, `no_std`, zero dependencies: time is an abstract `now_ms: u64`
//! supplied by the caller, the LED count is a const generic, and nothing here
//! touches hardware. `cargo test` runs the whole contract on the host; the
//! firmware (crates/glove80-rmk) consumes the crate by path and feeds it real
//! events, a real clock, and a WS2812 frame sink.
//!
//! Core model (from the design doc):
//!
//! - Every lighting definition is a sparse map **key -> [`Cell`]**; a cell is
//!   transparent or `{color, effect, params}` ([`Cell::Solid`],
//!   [`Cell::Blink`], [`Cell::Breathe`]).
//! - All definitions are the same [`Record`] type, differing only by
//!   activation predicate ([`Activation`]).
//! - Composition is bottom-to-top by class — base, layer, toggle, host,
//!   status — insertion order within a class. A defined cell replaces what is
//!   below; a transparent cell reveals it. A blinking cell's dark phase is
//!   BLACK (it occludes), not transparent.
//! - The host overlay is a live RAM-only slot with set/unset/clear/replace
//!   and optional per-cell TTL; expired cells revert to transparent.
//! - [`Compositor::render`] returns the frame plus the next instant the
//!   frame can change ([`RenderOutput::next_wake_ms`]). `None` means fully
//!   static: the caller arms no timer at all (the no-ticker-when-static
//!   guarantee). A `changed` flag lets the caller skip redundant frame
//!   writes.

#![cfg_attr(not(test), no_std)]

pub mod sync;

/// Compile-time per-channel ceiling: 80% of full scale.
///
/// SAFETY / WARRANTY: this is MoErgo's LED current limit for the Glove80.
/// Raising it can exceed the hardware's current budget and void the
/// warranty. The firmware's WS2812 driver clamps every encoded channel to
/// this value no matter what any caller asks; the compositor additionally
/// honors `min(CHANNEL_CEILING, runtime ceiling)` (see
/// [`Compositor::set_ceiling`]), where the runtime value can only lower the
/// ceiling, never raise it.
pub const CHANNEL_CEILING: u8 = 204;

/// Fixed capacity: configuration records the compositor can hold
/// (base + layer + toggle + status; the live host overlay has its own slot).
pub const MAX_RECORDS: usize = 16;

/// Fixed capacity: sparse cells per record. Sized for a whole-board scene on
/// one 40-LED half.
pub const MAX_CELLS_PER_RECORD: usize = 40;

/// Fixed capacity: live host-overlay cells (at most one per key; sized for a
/// whole 40-LED half).
pub const MAX_HOST_CELLS: usize = 40;

/// Animation tick for effects with continuously varying output (breathe).
/// Blink wakes exactly at its edges instead; TTLs wake exactly at expiry.
pub const ANIM_TICK_MS: u64 = 32;

/// One RGB color, pre-ceiling. `(0,0,0)` is off/black.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const OFF: Self = Self::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Scale every channel by `level / 255` (integer floor).
    const fn scaled(self, level: u8) -> Self {
        const fn s(c: u8, level: u8) -> u8 {
            ((c as u16 * level as u16) / 255) as u8
        }
        Self::new(s(self.r, level), s(self.g, level), s(self.b, level))
    }

    /// Clamp every channel to `ceiling`.
    const fn clamped(self, ceiling: u8) -> Self {
        const fn c(v: u8, ceiling: u8) -> u8 {
            if v < ceiling { v } else { ceiling }
        }
        Self::new(c(self.r, ceiling), c(self.g, ceiling), c(self.b, ceiling))
    }
}

/// One sparse lighting cell.
///
/// Times are milliseconds. New effects must be addable as new variants
/// without changing the meaning of existing ones.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Cell {
    /// Reveals whatever is composed below. In a record, a transparent cell
    /// is a no-op during composition (kept representable so the host
    /// protocol can round-trip it).
    Transparent,
    /// Static color.
    Solid { color: Rgb },
    /// Hard on/off square wave. `duty_pct` percent of the period is ON
    /// (`color`); the rest is BLACK — the dark phase occludes what is
    /// below, it does not become see-through. `phase_ms` shifts the
    /// waveform: the cell behaves as if the clock read `now + phase_ms`.
    /// Degenerate params are static: `period_ms == 0` or `duty_pct >= 100`
    /// is always-on, `duty_pct == 0` is always-black.
    Blink { color: Rgb, period_ms: u16, phase_ms: u16, duty_pct: u8 },
    /// Triangle-wave fade black -> `color` -> black over `period_ms`,
    /// peaking at the half period. `phase_ms` shifts the waveform as in
    /// [`Cell::Blink`]. `period_ms < 2` renders as static `color`.
    Breathe { color: Rgb, period_ms: u16, phase_ms: u16 },
}

impl Cell {
    /// The color this cell shows at `now_ms`, or `None` for transparent.
    fn color_at(&self, now_ms: u64) -> Option<Rgb> {
        match *self {
            Cell::Transparent => None,
            Cell::Solid { color } => Some(color),
            Cell::Blink { color, period_ms, phase_ms, duty_pct } => {
                if period_ms == 0 || duty_pct >= 100 {
                    return Some(color);
                }
                if duty_pct == 0 {
                    return Some(Rgb::OFF);
                }
                let t = phase_local(now_ms, period_ms, phase_ms);
                if t < on_ms(period_ms, duty_pct) { Some(color) } else { Some(Rgb::OFF) }
            }
            Cell::Breathe { color, period_ms, phase_ms } => {
                if period_ms < 2 {
                    return Some(color);
                }
                let t = phase_local(now_ms, period_ms, phase_ms);
                let half = period_ms as u32 / 2;
                let level = if (t as u32) < half {
                    t as u32 * 255 / half
                } else {
                    (period_ms as u32 - t as u32) * 255 / (period_ms as u32 - half)
                };
                Some(color.scaled(level as u8))
            }
        }
    }

    /// The next `now_ms` strictly after `now_ms` at which this cell's output
    /// can differ, or `None` if the cell is static.
    fn next_change_after(&self, now_ms: u64) -> Option<u64> {
        match *self {
            Cell::Transparent | Cell::Solid { .. } => None,
            Cell::Blink { period_ms, phase_ms, duty_pct, .. } => {
                if period_ms == 0 || duty_pct == 0 || duty_pct >= 100 {
                    return None; // degenerate blink is static
                }
                let t = phase_local(now_ms, period_ms, phase_ms);
                let on = on_ms(period_ms, duty_pct);
                let delta = if t < on { on - t } else { period_ms - t };
                Some(now_ms + delta as u64)
            }
            Cell::Breathe { period_ms, .. } => {
                if period_ms < 2 {
                    return None;
                }
                Some(now_ms + ANIM_TICK_MS)
            }
        }
    }
}

/// Waveform-local time in `[0, period_ms)` for a cell at `now_ms`.
fn phase_local(now_ms: u64, period_ms: u16, phase_ms: u16) -> u16 {
    ((now_ms + phase_ms as u64) % period_ms as u64) as u16
}

/// ON span of a blink period, in ms.
fn on_ms(period_ms: u16, duty_pct: u8) -> u16 {
    (period_ms as u32 * duty_pct.min(100) as u32 / 100) as u16
}

/// Activation predicate: when a record participates in composition. The
/// predicate also fixes the record's composition class (bottom to top:
/// `Always` < `LayerActive` < `Toggle` < `HostOverlay` < `Status`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Activation {
    /// Base lighting, always composed.
    Always,
    /// Composed while this keymap layer is the active layer.
    LayerActive(u8),
    /// Composed while the named toggle (id `0..32`) is on.
    Toggle(u8),
    /// Host-overlay class. The live overlay slot ([`Compositor::host_set`]
    /// etc.) always composes in this class; a config [`Record`] with this
    /// predicate composes just below the live slot.
    HostOverlay,
    /// Status & safety class, always on top. Phase 1 composes these
    /// unconditionally; firmware-state predicates (low battery, ...) plug in
    /// here later.
    Status,
}

/// A firmware-evaluable condition (docs/lighting-design.md, "Conditions and
/// gates"). Two flavors of condition sit on top of the same primitive:
///
/// - **layer/toggle** conditions mirror the two activation predicates a
///   record can already carry, and
/// - **firmware-state** conditions ([`Condition::UsbConnected`],
///   [`Condition::Charging`], [`Condition::SplitLinkUp`]) read the runtime
///   inputs the caller feeds the compositor ([`Compositor::set_usb_connected`]
///   etc.).
///
/// A [`Condition`] is used as a record's optional **gate**: a second predicate
/// that must ALSO hold (a logical AND with the activation) for the record to
/// compose. One gate primitive covers the stock "Magic shows status" behavior
/// — a layer-indicator record gated on the Magic layer is press-and-hold; the
/// same record ungated is permanent.
///
/// The `(kind, arg)` wire encoding is shared by the split-sync codec
/// ([`crate::sync`]) and mirrored by the persistent blob (protocol crate),
/// so the same gate survives config transfer and split forwarding unchanged.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Condition {
    /// Holds while this keymap layer is the active layer.
    LayerActive(u8),
    /// Holds while the named toggle (id `0..32`) is on.
    Toggle(u8),
    /// Holds while the central reports an active USB data connection
    /// (central-truth, mirrored to the peripheral over the split link — the
    /// right half's own port is charge-only).
    UsbConnected,
    /// Holds while THIS half sees USB bus power (local truth per half).
    Charging,
    /// Holds while this half's split link to the other half is up.
    SplitLinkUp,
}

/// A gate wire pair `(kind, arg)` named an unknown condition kind.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct UnknownCondition(pub u8);

impl Condition {
    /// Wire `(kind, arg)` for this condition. `kind == 0` is reserved for
    /// "no gate" ([`Condition::from_gate_wire`]).
    pub const fn to_wire(self) -> (u8, u8) {
        match self {
            Condition::LayerActive(n) => (1, n),
            Condition::Toggle(id) => (2, id),
            Condition::UsbConnected => (3, 0),
            Condition::Charging => (4, 0),
            Condition::SplitLinkUp => (5, 0),
        }
    }

    /// Decode a gate wire pair into an optional gate: `kind == 0` is no gate
    /// (`Ok(None)`); a known kind yields `Ok(Some(_))`; an unknown kind is
    /// rejected. Argument ranges are NOT re-checked here (the persistent blob
    /// validates them before a config is ever accepted; the split link only
    /// carries already-validated conditions), so this is deliberately lenient
    /// about `arg`.
    pub const fn from_gate_wire(kind: u8, arg: u8) -> Result<Option<Condition>, UnknownCondition> {
        Ok(match kind {
            0 => None,
            1 => Some(Condition::LayerActive(arg)),
            2 => Some(Condition::Toggle(arg)),
            3 => Some(Condition::UsbConnected),
            4 => Some(Condition::Charging),
            5 => Some(Condition::SplitLinkUp),
            k => return Err(UnknownCondition(k)),
        })
    }
}

/// Wire `(kind, arg)` for an optional gate: `None` encodes as `(0, 0)`, the
/// same bytes every ungated record has always written into the reserved
/// field — so ungated configs are byte-identical across this change.
pub const fn gate_to_wire(gate: Option<Condition>) -> (u8, u8) {
    match gate {
        None => (0, 0),
        Some(c) => c.to_wire(),
    }
}

/// Composition class order (bottom to top). Derived from [`Activation`].
fn class(a: Activation) -> u8 {
    match a {
        Activation::Always => 0,
        Activation::LayerActive(_) => 1,
        Activation::Toggle(_) => 2,
        Activation::HostOverlay => 3,
        Activation::Status => 4,
    }
}

const NUM_CLASSES: u8 = 5;
const HOST_CLASS: u8 = 3;

/// Error: a fixed capacity ([`MAX_RECORDS`], [`MAX_CELLS_PER_RECORD`],
/// [`MAX_HOST_CELLS`]) would be exceeded. The operation left state unchanged.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CapacityError;

/// A sparse lighting record: an activation predicate plus up to
/// [`MAX_CELLS_PER_RECORD`] `(key -> Cell)` entries. Keys are chain indices
/// (`0..N` on the target half); setting an existing key replaces its cell.
///
/// A record may also carry one optional [`gate`](Self::gate): a [`Condition`]
/// that must hold for the record to compose, on top of its activation
/// predicate (a logical AND). `gate == None` is the default and the common
/// case — an ungated record behaves exactly as before gates existed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    activation: Activation,
    gate: Option<Condition>,
    len: usize,
    cells: [(u8, Cell); MAX_CELLS_PER_RECORD],
}

impl Record {
    pub const fn new(activation: Activation) -> Self {
        Self {
            activation,
            gate: None,
            len: 0,
            cells: [(0, Cell::Transparent); MAX_CELLS_PER_RECORD],
        }
    }

    /// Build a record from `(key, cell)` pairs (later duplicates replace
    /// earlier ones, as with [`set`](Self::set)).
    pub fn with_cells(activation: Activation, cells: &[(u8, Cell)]) -> Result<Self, CapacityError> {
        let mut r = Self::new(activation);
        for &(key, cell) in cells {
            r.set(key, cell)?;
        }
        Ok(r)
    }

    /// Builder: attach a [`gate`](Self::gate) condition to this record.
    pub const fn gated(mut self, gate: Condition) -> Self {
        self.gate = Some(gate);
        self
    }

    /// The record's optional gate condition.
    pub fn gate(&self) -> Option<Condition> {
        self.gate
    }

    /// Set or clear the record's gate condition.
    pub fn set_gate(&mut self, gate: Option<Condition>) {
        self.gate = gate;
    }

    pub fn activation(&self) -> Activation {
        self.activation
    }

    /// Set or replace the cell for `key`.
    pub fn set(&mut self, key: u8, cell: Cell) -> Result<(), CapacityError> {
        for slot in &mut self.cells[..self.len] {
            if slot.0 == key {
                slot.1 = cell;
                return Ok(());
            }
        }
        if self.len == MAX_CELLS_PER_RECORD {
            return Err(CapacityError);
        }
        self.cells[self.len] = (key, cell);
        self.len += 1;
        Ok(())
    }

    pub fn get(&self, key: u8) -> Option<&Cell> {
        self.cells[..self.len].iter().find(|(k, _)| *k == key).map(|(_, c)| c)
    }

    pub fn cells(&self) -> impl Iterator<Item = (u8, &Cell)> {
        self.cells[..self.len].iter().map(|(k, c)| (*k, c))
    }
}

/// One live host-overlay entry.
#[derive(Copy, Clone, Debug)]
struct HostSlot {
    key: u8,
    cell: Cell,
    /// Absolute expiry (`now_ms` scale); `None` = lives until explicit
    /// clear/unset or reboot.
    expires_at_ms: Option<u64>,
}

/// Output of one [`Compositor::render`] call.
#[derive(Copy, Clone, Debug)]
pub struct RenderOutput<const N: usize> {
    /// The composed frame, brightness-scaled and ceiling-clamped, in chain
    /// order. Keys with nothing composed are [`Rgb::OFF`].
    pub frame: [Rgb; N],
    /// Whether `frame` differs from the previous render (always `true` on
    /// the first). Callers can skip the LED write when `false`.
    pub changed: bool,
    /// The earliest `now_ms` strictly after this render at which the frame
    /// can change on its own (blink edge, breathe tick, TTL expiry of a
    /// visible cell). `None` = fully static: arm no timer.
    pub next_wake_ms: Option<u64>,
}

/// The sparse lighting compositor for one `N`-LED half.
///
/// Owns configuration records, the runtime inputs (active layer, toggles,
/// brightness, effective ceiling), and the live host overlay. All state
/// changes are plain setters; [`render`](Self::render) is the only place
/// output is produced. Single-owner by design: the firmware task that holds
/// the compositor is the atomicity boundary (e.g.
/// [`host_replace`](Self::host_replace) is atomic because nothing else can
/// observe the overlay mid-update).
pub struct Compositor<const N: usize> {
    records: [Record; MAX_RECORDS],
    record_count: usize,
    active_layer: u8,
    /// Bitmask of toggle ids `0..32`. Non-persistent (lighting-design.md).
    toggles: u32,
    /// Global brightness scalar `0..=255`, applied at composition output.
    brightness: u8,
    /// Runtime effective ceiling; always `<=` [`CHANNEL_CEILING`].
    ceiling: u8,
    /// Firmware-state condition inputs (docs/lighting-design.md). Fed by the
    /// caller; read by gates ([`Condition::UsbConnected`] etc.). All default
    /// off so an unconfigured or freshly-booted compositor gates nothing on.
    usb_connected: bool,
    charging: bool,
    split_link: bool,
    host: [Option<HostSlot>; MAX_HOST_CELLS],
    last_frame: Option<[Rgb; N]>,
}

impl<const N: usize> Default for Compositor<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Compositor<N> {
    pub const fn new() -> Self {
        Self {
            records: [Record::new(Activation::Always); MAX_RECORDS],
            record_count: 0,
            active_layer: 0,
            toggles: 0,
            brightness: 255,
            ceiling: CHANNEL_CEILING,
            usb_connected: false,
            charging: false,
            split_link: false,
            host: [None; MAX_HOST_CELLS],
            last_frame: None,
        }
    }

    /// Append a configuration record. Insertion order is composition order
    /// within a class (later records compose on top).
    pub fn add_record(&mut self, record: Record) -> Result<(), CapacityError> {
        if self.record_count == MAX_RECORDS {
            return Err(CapacityError);
        }
        self.records[self.record_count] = record;
        self.record_count += 1;
        Ok(())
    }

    /// Atomically replace the whole configuration record set (Phase 4: a
    /// persistent-config apply swaps every base/layer/toggle record in one
    /// step). Validate-first: on capacity error nothing changes, so the
    /// caller keeps rendering the previous config. The live host overlay and
    /// runtime inputs (layer, toggles, brightness, ceiling) are untouched.
    pub fn replace_records(&mut self, records: &[Record]) -> Result<(), CapacityError> {
        if records.len() > MAX_RECORDS {
            return Err(CapacityError);
        }
        self.records[..records.len()].copy_from_slice(records);
        self.record_count = records.len();
        Ok(())
    }

    /// The current configuration record set, in composition order.
    pub fn records(&self) -> &[Record] {
        &self.records[..self.record_count]
    }

    pub fn set_active_layer(&mut self, layer: u8) {
        self.active_layer = layer;
    }

    pub fn active_layer(&self) -> u8 {
        self.active_layer
    }

    /// Switch toggle `id` (`0..32`) on or off. Out-of-range ids are ignored.
    pub fn set_toggle(&mut self, id: u8, on: bool) {
        if id >= 32 {
            return;
        }
        if on {
            self.toggles |= 1 << id;
        } else {
            self.toggles &= !(1 << id);
        }
    }

    pub fn toggle(&self, id: u8) -> bool {
        id < 32 && self.toggles & (1 << id) != 0
    }

    /// The whole toggle bitmask (bit n ⇔ toggle id n). Used to mirror toggle
    /// state across the split link in one message.
    pub fn toggles_mask(&self) -> u32 {
        self.toggles
    }

    /// Replace the whole toggle bitmask (the split-sync counterpart of
    /// [`toggles_mask`](Self::toggles_mask)). Idempotent.
    pub fn set_toggles_mask(&mut self, mask: u32) {
        self.toggles = mask;
    }

    /// Global brightness scalar (255 = full). Applied to the composed
    /// output; the effective ceiling and the driver clamp still bound the
    /// result.
    pub fn set_brightness(&mut self, brightness: u8) {
        self.brightness = brightness;
    }

    pub fn brightness(&self) -> u8 {
        self.brightness
    }

    /// Lower the runtime per-channel ceiling. The stored value is
    /// `min(requested, CHANNEL_CEILING)`: a host can lower the ceiling at
    /// runtime but can never raise it above the compiled safety value.
    pub fn set_ceiling(&mut self, ceiling: u8) {
        self.ceiling = ceiling.min(CHANNEL_CEILING);
    }

    /// The effective per-channel ceiling currently applied
    /// (`<=` [`CHANNEL_CEILING`]).
    pub fn ceiling(&self) -> u8 {
        self.ceiling
    }

    // --- Firmware-state condition inputs ----------------------------------

    /// Report whether the central has an active USB data connection (the
    /// [`Condition::UsbConnected`] gate input). Central-truth: the firmware
    /// mirrors it to the peripheral over the split link so both halves gate
    /// identically.
    pub fn set_usb_connected(&mut self, connected: bool) {
        self.usb_connected = connected;
    }

    pub fn usb_connected(&self) -> bool {
        self.usb_connected
    }

    /// Report whether THIS half sees USB bus power (the [`Condition::Charging`]
    /// gate input). Local truth per half — deliberately a different source
    /// from [`set_usb_connected`](Self::set_usb_connected).
    pub fn set_charging(&mut self, charging: bool) {
        self.charging = charging;
    }

    pub fn charging(&self) -> bool {
        self.charging
    }

    /// Report whether this half's split link is up (the
    /// [`Condition::SplitLinkUp`] gate input).
    pub fn set_split_link(&mut self, up: bool) {
        self.split_link = up;
    }

    pub fn split_link(&self) -> bool {
        self.split_link
    }

    /// Whether a bare condition holds against the current runtime inputs.
    /// This is the gate evaluator; the activation predicate is checked
    /// separately (see [`record_active`](Self::record_active)).
    fn condition_holds(&self, c: Condition) -> bool {
        match c {
            Condition::LayerActive(layer) => layer == self.active_layer,
            Condition::Toggle(id) => self.toggle(id),
            Condition::UsbConnected => self.usb_connected,
            Condition::Charging => self.charging,
            Condition::SplitLinkUp => self.split_link,
        }
    }

    // --- Host overlay (live, RAM-only) ------------------------------------

    /// Set or replace the live overlay cell for `key`, optionally expiring
    /// `ttl_ms` after `now_ms`. On expiry the cell reverts to transparent.
    pub fn host_set(&mut self, key: u8, cell: Cell, ttl_ms: Option<u32>, now_ms: u64) -> Result<(), CapacityError> {
        let slot = HostSlot { key, cell, expires_at_ms: ttl_ms.map(|t| now_ms + t as u64) };
        let mut free = None;
        for (i, s) in self.host.iter_mut().enumerate() {
            match s {
                Some(existing) if existing.key == key => {
                    *existing = slot;
                    return Ok(());
                }
                None if free.is_none() => free = Some(i),
                _ => {}
            }
        }
        match free {
            Some(i) => {
                self.host[i] = Some(slot);
                Ok(())
            }
            None => Err(CapacityError),
        }
    }

    /// Remove the live overlay cell for `key` (no-op if absent).
    pub fn host_unset(&mut self, key: u8) {
        for s in &mut self.host {
            if matches!(s, Some(slot) if slot.key == key) {
                *s = None;
            }
        }
    }

    /// Clear the whole live overlay.
    pub fn host_clear(&mut self) {
        self.host = [None; MAX_HOST_CELLS];
    }

    /// Read back the live overlay: `(key, cell, absolute expiry)` per stored
    /// cell. Entries already past their expiry may still appear until the
    /// next [`render`](Self::render) purges them.
    pub fn host_cells(&self) -> impl Iterator<Item = (u8, Cell, Option<u64>)> + '_ {
        self.host.iter().flatten().map(|s| (s.key, s.cell, s.expires_at_ms))
    }

    /// Atomically replace the whole live overlay (the force-sync primitive).
    /// Either every entry is applied or, on capacity error, nothing changes.
    /// Later duplicates of a key replace earlier ones.
    pub fn host_replace(&mut self, cells: &[(u8, Cell, Option<u32>)], now_ms: u64) -> Result<(), CapacityError> {
        // Validate first so failure leaves the overlay untouched.
        let mut seen = [false; 256];
        let mut unique = 0usize;
        for &(key, _, _) in cells {
            if !seen[key as usize] {
                seen[key as usize] = true;
                unique += 1;
            }
        }
        if unique > MAX_HOST_CELLS {
            return Err(CapacityError);
        }
        self.host_clear();
        for &(key, cell, ttl) in cells {
            // Cannot fail: unique keys fit, host_set replaces duplicates.
            self.host_set(key, cell, ttl, now_ms)?;
        }
        Ok(())
    }

    /// Drop host cells whose expiry has passed.
    fn purge_expired(&mut self, now_ms: u64) {
        for s in &mut self.host {
            if matches!(s, Some(slot) if matches!(slot.expires_at_ms, Some(at) if at <= now_ms)) {
                *s = None;
            }
        }
    }

    // --- Rendering --------------------------------------------------------

    /// Compose everything visible at `now_ms` into a frame.
    ///
    /// Also computes when the frame can next change on its own; a `None`
    /// wake with `changed == false` means the caller has nothing to do until
    /// the next external event.
    pub fn render(&mut self, now_ms: u64) -> RenderOutput<N> {
        self.purge_expired(now_ms);

        // 1. Effective cell per key, bottom-to-top. `expiry` remembers a
        //    TTL when the winning cell is a host cell that carries one.
        let mut effective: [Option<Cell>; N] = [None; N];
        let mut expiry: [Option<u64>; N] = [None; N];
        let mut place = |key: u8, cell: &Cell, exp: Option<u64>| {
            if matches!(cell, Cell::Transparent) {
                return; // reveals what is below
            }
            if let Some(slot) = effective.get_mut(key as usize) {
                *slot = Some(*cell);
                expiry[key as usize] = exp;
            }
        };
        for cls in 0..NUM_CLASSES {
            for record in &self.records[..self.record_count] {
                if class(record.activation) == cls && self.record_participates(record) {
                    for (key, cell) in record.cells() {
                        place(key, cell, None);
                    }
                }
            }
            if cls == HOST_CLASS {
                // The live overlay slot composes on top of any HostOverlay
                // config records.
                for s in self.host.iter().flatten() {
                    place(s.key, &s.cell, s.expires_at_ms);
                }
            }
        }

        // 2. Evaluate effects, apply brightness, apply the effective
        //    ceiling. The driver's compile-time clamp remains the hard
        //    backstop.
        let mut frame = [Rgb::OFF; N];
        for (i, cell) in effective.iter().enumerate() {
            if let Some(cell) = cell
                && let Some(color) = cell.color_at(now_ms)
            {
                frame[i] = color.scaled(self.brightness).clamped(self.ceiling);
            }
        }

        // 3. Next wake: the earliest change among visible animated cells and
        //    visible TTLs. Occluded cells cannot change the frame, so they
        //    contribute nothing (their effects re-enter whenever a real
        //    change re-renders).
        let mut next_wake_ms: Option<u64> = None;
        let mut consider = |candidate: u64| {
            let candidate = candidate.max(now_ms + 1); // strictly in the future
            next_wake_ms = Some(match next_wake_ms {
                Some(cur) => cur.min(candidate),
                None => candidate,
            });
        };
        for (i, cell) in effective.iter().enumerate() {
            if let Some(cell) = cell {
                if let Some(at) = cell.next_change_after(now_ms) {
                    consider(at);
                }
                if let Some(at) = expiry[i] {
                    consider(at);
                }
            }
        }

        // 4. Frame diffing.
        let changed = self.last_frame != Some(frame);
        self.last_frame = Some(frame);

        RenderOutput { frame, changed, next_wake_ms }
    }

    /// Whether a record composes right now: its activation predicate holds
    /// AND its gate (if any) holds. The gate is a pure AND — it can only
    /// suppress a record, never activate one whose class predicate is false.
    fn record_participates(&self, record: &Record) -> bool {
        self.record_active(record.activation)
            && record.gate.is_none_or(|g| self.condition_holds(g))
    }

    fn record_active(&self, a: Activation) -> bool {
        match a {
            Activation::Always => true,
            Activation::LayerActive(layer) => layer == self.active_layer,
            Activation::Toggle(id) => self.toggle(id),
            Activation::HostOverlay => true,
            Activation::Status => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 8;
    // Composition tests use colors below CHANNEL_CEILING so the (always-on)
    // safety clamp is a no-op for them; the ceiling tests exercise the clamp
    // explicitly with FULL.
    const RED: Rgb = Rgb::new(200, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 200, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 200);
    const WHITE: Rgb = Rgb::new(200, 200, 200);
    const FULL: Rgb = Rgb::new(255, 255, 255);

    fn solid(color: Rgb) -> Cell {
        Cell::Solid { color }
    }

    fn comp_with(records: &[Record]) -> Compositor<N> {
        let mut c = Compositor::<N>::new();
        for r in records {
            c.add_record(*r).unwrap();
        }
        c
    }

    fn record(activation: Activation, cells: &[(u8, Cell)]) -> Record {
        Record::with_cells(activation, cells).unwrap()
    }

    // --- Composition ------------------------------------------------------

    #[test]
    fn base_fills_and_unset_keys_are_off() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(RED)), (3, solid(GREEN))])]);
        let out = c.render(0);
        assert_eq!(out.frame[0], RED);
        assert_eq!(out.frame[3], GREEN);
        for i in [1, 2, 4, 5, 6, 7] {
            assert_eq!(out.frame[i], Rgb::OFF, "key {i} should be off");
        }
        assert!(out.changed);
        assert_eq!(out.next_wake_ms, None, "static config must arm no timer");
    }

    #[test]
    fn class_order_is_base_layer_toggle_host_status() {
        let mut c = comp_with(&[
            // Added in reverse class order: class, not insertion order, wins.
            record(Activation::Status, &[(0, solid(WHITE))]),
            record(Activation::Toggle(1), &[(0, solid(BLUE)), (1, solid(BLUE)), (2, solid(BLUE))]),
            record(Activation::LayerActive(0), &[(0, solid(GREEN)), (1, solid(GREEN)), (2, solid(GREEN)), (3, solid(GREEN))]),
            record(Activation::Always, &[(0, solid(RED)), (1, solid(RED)), (2, solid(RED)), (3, solid(RED)), (4, solid(RED))]),
        ]);
        c.set_toggle(1, true);
        c.host_set(1, solid(Rgb::new(9, 9, 9)), None, 0).unwrap();
        let out = c.render(0);
        assert_eq!(out.frame[0], WHITE, "status beats everything");
        assert_eq!(out.frame[1], Rgb::new(9, 9, 9), "host beats toggle");
        assert_eq!(out.frame[2], BLUE, "toggle beats layer");
        assert_eq!(out.frame[3], GREEN, "layer beats base");
        assert_eq!(out.frame[4], RED, "base shows where nothing is above");
    }

    #[test]
    fn transparent_reveals_and_defined_replaces() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, solid(RED)), (1, solid(RED))]),
            record(Activation::LayerActive(0), &[(0, Cell::Transparent), (1, solid(GREEN))]),
        ]);
        let out = c.render(0);
        assert_eq!(out.frame[0], RED, "transparent cell reveals base");
        assert_eq!(out.frame[1], GREEN, "defined cell replaces base");
    }

    #[test]
    fn insertion_order_breaks_ties_within_a_class() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, solid(RED))]),
            record(Activation::Always, &[(0, solid(GREEN))]),
        ]);
        assert_eq!(c.render(0).frame[0], GREEN, "later record composes on top");
    }

    #[test]
    fn inactive_records_do_not_compose() {
        let mut c = comp_with(&[
            record(Activation::LayerActive(2), &[(0, solid(GREEN))]),
            record(Activation::Toggle(3), &[(1, solid(BLUE))]),
        ]);
        let out = c.render(0);
        assert_eq!(out.frame[0], Rgb::OFF);
        assert_eq!(out.frame[1], Rgb::OFF);

        c.set_active_layer(2);
        c.set_toggle(3, true);
        let out = c.render(1);
        assert_eq!(out.frame[0], GREEN);
        assert_eq!(out.frame[1], BLUE);

        c.set_toggle(3, false);
        assert_eq!(c.render(2).frame[1], Rgb::OFF, "toggle off removes overlay");
    }

    #[test]
    fn record_set_replaces_existing_key() {
        let mut r = Record::new(Activation::Always);
        r.set(5, solid(RED)).unwrap();
        r.set(5, solid(GREEN)).unwrap();
        assert_eq!(r.get(5), Some(&solid(GREEN)));
        assert_eq!(r.cells().count(), 1);
    }

    #[test]
    fn capacities_are_enforced() {
        let mut r = Record::new(Activation::Always);
        for k in 0..MAX_CELLS_PER_RECORD {
            r.set(k as u8, solid(RED)).unwrap();
        }
        assert_eq!(r.set(200, solid(RED)), Err(CapacityError));

        let mut c = Compositor::<N>::new();
        for _ in 0..MAX_RECORDS {
            c.add_record(Record::new(Activation::Always)).unwrap();
        }
        assert_eq!(c.add_record(Record::new(Activation::Always)), Err(CapacityError));
    }

    #[test]
    fn replace_records_swaps_the_whole_set_atomically() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, solid(RED))]),
            record(Activation::LayerActive(0), &[(1, solid(GREEN))]),
        ]);
        c.host_set(2, solid(WHITE), None, 0).unwrap();
        assert_eq!(c.render(0).frame[0], RED);

        // Replace with a different set: old records are gone, new ones
        // compose; the live host overlay and runtime inputs are untouched.
        let new_set = [record(Activation::Always, &[(3, solid(BLUE))])];
        c.replace_records(&new_set).unwrap();
        assert_eq!(c.records().len(), 1);
        let out = c.render(1);
        assert_eq!(out.frame[0], Rgb::OFF);
        assert_eq!(out.frame[1], Rgb::OFF);
        assert_eq!(out.frame[2], WHITE, "host overlay survives the swap");
        assert_eq!(out.frame[3], BLUE);

        // Oversized set: error, nothing changes.
        let too_many = [record(Activation::Always, &[]); MAX_RECORDS + 1];
        assert_eq!(c.replace_records(&too_many), Err(CapacityError));
        assert_eq!(c.records().len(), 1);
        assert_eq!(c.render(2).frame[3], BLUE);

        // Empty set is valid (config with no records).
        c.replace_records(&[]).unwrap();
        assert_eq!(c.records().len(), 0);
        assert_eq!(c.render(3).frame[3], Rgb::OFF);
    }

    // --- Conditions and gates ---------------------------------------------

    #[test]
    fn ungated_record_is_unchanged_by_state() {
        // Back-compat: a record with no gate composes exactly as before,
        // regardless of the firmware-state inputs.
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(RED))])]);
        assert_eq!(c.render(0).frame[0], RED);
        c.set_usb_connected(true);
        c.set_charging(true);
        c.set_split_link(true);
        let out = c.render(1);
        assert_eq!(out.frame[0], RED);
        assert!(!out.changed, "an ungated record does not react to state flips");
    }

    #[test]
    fn gate_suppresses_until_condition_holds() {
        // A gate is a pure AND with the activation: the base record only
        // composes while USB is connected.
        let gated = Record::with_cells(Activation::Always, &[(0, solid(RED))])
            .unwrap()
            .gated(Condition::UsbConnected);
        let mut c = comp_with(&[gated]);
        assert_eq!(c.render(0).frame[0], Rgb::OFF, "gate false -> record suppressed");
        assert_eq!(c.render(0).next_wake_ms, None);

        c.set_usb_connected(true);
        let out = c.render(1);
        assert_eq!(out.frame[0], RED, "gate true -> record composes");
        assert!(out.changed, "flipping a gate's input re-renders the frame");

        c.set_usb_connected(false);
        assert_eq!(c.render(2).frame[0], Rgb::OFF, "gate false again -> suppressed");
    }

    #[test]
    fn every_gate_kind_gates_its_input() {
        let cases: &[(Condition, fn(&mut Compositor<N>, bool))] = &[
            (Condition::Charging, |c, on| c.set_charging(on)),
            (Condition::SplitLinkUp, |c, on| c.set_split_link(on)),
            (Condition::UsbConnected, |c, on| c.set_usb_connected(on)),
            (Condition::Toggle(4), |c, on| c.set_toggle(4, on)),
            (Condition::LayerActive(2), |c, on| c.set_active_layer(if on { 2 } else { 0 })),
        ];
        for (gate, set) in cases {
            let r = Record::with_cells(Activation::Always, &[(0, solid(GREEN))]).unwrap().gated(*gate);
            let mut c = comp_with(&[r]);
            assert_eq!(c.render(0).frame[0], Rgb::OFF, "{gate:?} starts unmet");
            set(&mut c, true);
            assert_eq!(c.render(1).frame[0], GREEN, "{gate:?} met -> composes");
            set(&mut c, false);
            assert_eq!(c.render(2).frame[0], Rgb::OFF, "{gate:?} unmet again -> suppressed");
        }
    }

    #[test]
    fn gate_ands_with_activation() {
        // Layer-indicator-style: a layer record gated on the Magic layer.
        // Composes only when BOTH the record's own layer is active AND the
        // gate layer is active. Gate and activation on the same layer is the
        // press-and-hold status pattern.
        let magic = 2u8;
        let indicator = Record::with_cells(Activation::LayerActive(magic), &[(0, solid(BLUE))])
            .unwrap()
            .gated(Condition::LayerActive(magic));
        let mut c = comp_with(&[indicator]);
        assert_eq!(c.render(0).frame[0], Rgb::OFF, "neither active");
        c.set_active_layer(magic);
        assert_eq!(c.render(1).frame[0], BLUE, "both active -> composes");
        c.set_active_layer(1);
        assert_eq!(c.render(2).frame[0], Rgb::OFF, "left the layer -> suppressed");
    }

    #[test]
    fn gate_wire_roundtrips_and_rejects_unknown() {
        for cond in [
            Condition::LayerActive(3),
            Condition::Toggle(31),
            Condition::UsbConnected,
            Condition::Charging,
            Condition::SplitLinkUp,
        ] {
            let (kind, arg) = cond.to_wire();
            assert_eq!(Condition::from_gate_wire(kind, arg), Ok(Some(cond)));
        }
        assert_eq!(gate_to_wire(None), (0, 0), "no gate is the all-zero reserved field");
        assert_eq!(Condition::from_gate_wire(0, 0), Ok(None));
        assert_eq!(Condition::from_gate_wire(9, 0), Err(UnknownCondition(9)));
    }

    // --- Blink ------------------------------------------------------------

    #[test]
    fn blink_duty_and_phase() {
        let cell = Cell::Blink { color: RED, period_ms: 1000, phase_ms: 0, duty_pct: 25 };
        assert_eq!(cell.color_at(0), Some(RED));
        assert_eq!(cell.color_at(249), Some(RED));
        assert_eq!(cell.color_at(250), Some(Rgb::OFF), "dark phase is black, not transparent");
        assert_eq!(cell.color_at(999), Some(Rgb::OFF));
        assert_eq!(cell.color_at(1000), Some(RED), "wraps to next period");

        // phase shifts the waveform: with phase 250 the cell is already dark at t=0.
        let shifted = Cell::Blink { color: RED, period_ms: 1000, phase_ms: 250, duty_pct: 25 };
        assert_eq!(shifted.color_at(0), Some(Rgb::OFF));
        assert_eq!(shifted.color_at(750), Some(RED), "750 + 250 wraps into the ON span");
    }

    #[test]
    fn blink_dark_phase_occludes_below() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, solid(GREEN))]),
            record(Activation::LayerActive(0), &[(0, Cell::Blink { color: RED, period_ms: 100, phase_ms: 0, duty_pct: 50 })]),
        ]);
        assert_eq!(c.render(0).frame[0], RED);
        assert_eq!(c.render(50).frame[0], Rgb::OFF, "dark phase renders black over the base");
    }

    #[test]
    fn degenerate_blinks_are_static() {
        for cell in [
            Cell::Blink { color: RED, period_ms: 0, phase_ms: 0, duty_pct: 50 },
            Cell::Blink { color: RED, period_ms: 100, phase_ms: 0, duty_pct: 100 },
            Cell::Blink { color: RED, period_ms: 100, phase_ms: 0, duty_pct: 0 },
        ] {
            assert_eq!(cell.next_change_after(0), None, "{cell:?} must not tick");
        }
    }

    // --- Breathe ----------------------------------------------------------

    #[test]
    fn breathe_rises_then_falls_monotonically() {
        let cell = Cell::Breathe { color: FULL, period_ms: 1000, phase_ms: 0 };
        let level = |t: u64| cell.color_at(t).unwrap().r;
        assert_eq!(level(0), 0, "starts black");
        assert_eq!(level(500), 255, "peaks at half period");
        let mut prev = level(0);
        for t in (0..=500).step_by(20) {
            let cur = level(t);
            assert!(cur >= prev, "rising half must be monotonic at t={t}");
            prev = cur;
        }
        let mut prev = level(500);
        for t in (500..1000).step_by(20) {
            let cur = level(t);
            assert!(cur <= prev, "falling half must be monotonic at t={t}");
            prev = cur;
        }
        assert_eq!(level(1000), 0, "wraps to black");
        // phase shift moves the peak.
        let shifted = Cell::Breathe { color: FULL, period_ms: 1000, phase_ms: 500 };
        assert_eq!(shifted.color_at(0).unwrap().r, 255);
    }

    #[test]
    fn breathe_scales_all_channels() {
        let cell = Cell::Breathe { color: Rgb::new(200, 100, 0), period_ms: 1000, phase_ms: 0 };
        let c = cell.color_at(250).unwrap(); // level ~127
        assert_eq!(c, Rgb::new((200 * 127 / 255) as u8, (100 * 127 / 255) as u8, 0));
    }

    // --- TTL --------------------------------------------------------------

    #[test]
    fn ttl_expiry_reverts_to_transparent() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(GREEN))])]);
        c.host_set(0, solid(RED), Some(1000), 0).unwrap();
        assert_eq!(c.render(0).frame[0], RED);
        assert_eq!(c.render(999).frame[0], RED);
        let out = c.render(1000);
        assert_eq!(out.frame[0], GREEN, "expired cell reveals the base");
        assert!(out.changed);
        assert_eq!(out.next_wake_ms, None, "nothing left to expire");
        assert_eq!(c.host_cells().count(), 0, "expired cell is purged");
    }

    #[test]
    fn no_ttl_means_no_expiry() {
        let mut c = Compositor::<N>::new();
        c.host_set(0, solid(RED), None, 0).unwrap();
        let out = c.render(u64::MAX / 2);
        assert_eq!(out.frame[0], RED);
        assert_eq!(out.next_wake_ms, None, "a TTL-less cell never wakes the renderer");
    }

    #[test]
    fn host_set_unset_clear_replace_roundtrip() {
        let mut c = Compositor::<N>::new();
        c.host_set(1, solid(RED), None, 0).unwrap();
        c.host_set(2, solid(GREEN), Some(500), 0).unwrap();
        c.host_set(1, solid(BLUE), None, 0).unwrap(); // replace by key
        let mut cells: [_; 2] = [(0u8, Cell::Transparent, None); 2];
        for (i, entry) in c.host_cells().enumerate() {
            cells[i] = entry;
        }
        assert!(cells.contains(&(1, solid(BLUE), None)));
        assert!(cells.contains(&(2, solid(GREEN), Some(500))));

        c.host_unset(1);
        assert_eq!(c.host_cells().count(), 1);

        c.host_replace(&[(4, solid(WHITE), Some(100)), (4, solid(RED), None), (5, solid(BLUE), None)], 10)
            .unwrap();
        let mut got: [_; 2] = [(0u8, Cell::Transparent, None); 2];
        for (i, entry) in c.host_cells().enumerate() {
            got[i] = entry;
        }
        assert!(got.contains(&(4, solid(RED), None)), "later duplicate wins");
        assert!(got.contains(&(5, solid(BLUE), None)));

        c.host_clear();
        assert_eq!(c.host_cells().count(), 0);
    }

    #[test]
    fn host_replace_is_atomic_on_capacity_error() {
        let mut c = Compositor::<N>::new();
        c.host_set(7, solid(RED), None, 0).unwrap();
        // 41 unique keys > MAX_HOST_CELLS.
        let mut too_many = [(0u8, solid(WHITE), None); MAX_HOST_CELLS + 1];
        for (i, e) in too_many.iter_mut().enumerate() {
            e.0 = i as u8;
        }
        assert_eq!(c.host_replace(&too_many, 0), Err(CapacityError));
        let mut cells = c.host_cells();
        assert_eq!(cells.next(), Some((7, solid(RED), None)), "failed replace left the overlay untouched");
        assert_eq!(cells.next(), None);
    }

    // --- Brightness and ceiling -------------------------------------------

    #[test]
    fn brightness_scales_output() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(Rgb::new(200, 100, 50)))])]);
        assert_eq!(c.render(0).frame[0], Rgb::new(200, 100, 50), "255 is identity");
        c.set_brightness(128);
        assert_eq!(
            c.render(1).frame[0],
            Rgb::new((200 * 128 / 255) as u8, (100 * 128 / 255) as u8, (50 * 128 / 255) as u8)
        );
        c.set_brightness(0);
        assert_eq!(c.render(2).frame[0], Rgb::OFF);
    }

    #[test]
    fn ceiling_defaults_to_compiled_value_and_only_lowers() {
        let mut c = Compositor::<N>::new();
        assert_eq!(c.ceiling(), CHANNEL_CEILING);
        c.add_record(record(Activation::Always, &[(0, solid(FULL))])).unwrap();
        assert_eq!(c.render(0).frame[0], Rgb::new(CHANNEL_CEILING, CHANNEL_CEILING, CHANNEL_CEILING));

        c.set_ceiling(100);
        assert_eq!(c.ceiling(), 100);
        assert_eq!(c.render(1).frame[0], Rgb::new(100, 100, 100));

        c.set_ceiling(255); // attempt to raise above the compiled safety value
        assert_eq!(c.ceiling(), CHANNEL_CEILING, "runtime ceiling can never exceed CHANNEL_CEILING");
        assert_eq!(c.render(2).frame[0], Rgb::new(CHANNEL_CEILING, CHANNEL_CEILING, CHANNEL_CEILING));
    }

    // --- next_wake --------------------------------------------------------

    #[test]
    fn next_wake_none_when_static() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(RED)), (1, solid(GREEN))])]);
        assert_eq!(c.render(0).next_wake_ms, None);
    }

    #[test]
    fn next_wake_hits_blink_edges_exactly() {
        let mut c = comp_with(&[record(
            Activation::Always,
            &[(0, Cell::Blink { color: RED, period_ms: 1000, phase_ms: 0, duty_pct: 25 })],
        )]);
        assert_eq!(c.render(0).next_wake_ms, Some(250), "ON now, next edge at duty boundary");
        assert_eq!(c.render(250).next_wake_ms, Some(1000), "OFF now, next edge at period end");
        assert_eq!(c.render(600).next_wake_ms, Some(1000));
    }

    #[test]
    fn next_wake_ticks_while_breathing() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, Cell::Breathe { color: RED, period_ms: 1000, phase_ms: 0 })])]);
        assert_eq!(c.render(100).next_wake_ms, Some(100 + ANIM_TICK_MS));
    }

    #[test]
    fn next_wake_includes_visible_ttl_boundary() {
        let mut c = Compositor::<N>::new();
        c.host_set(0, solid(RED), Some(5000), 0).unwrap();
        assert_eq!(c.render(0).next_wake_ms, Some(5000), "static cell with TTL wakes exactly at expiry");

        c.host_set(1, Cell::Blink { color: RED, period_ms: 200, phase_ms: 0, duty_pct: 50 }, None, 0).unwrap();
        assert_eq!(c.render(0).next_wake_ms, Some(100), "min of blink edge and TTL");
    }

    #[test]
    fn occluded_ttl_does_not_wake() {
        let mut c = comp_with(&[record(Activation::Status, &[(0, solid(WHITE))])]);
        c.host_set(0, solid(RED), Some(1000), 0).unwrap();
        let out = c.render(0);
        assert_eq!(out.frame[0], WHITE, "status occludes the host cell");
        assert_eq!(out.next_wake_ms, None, "occluded TTL cannot change the frame");
    }

    #[test]
    fn occluded_animation_does_not_wake() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, Cell::Blink { color: RED, period_ms: 100, phase_ms: 0, duty_pct: 50 })]),
            record(Activation::Status, &[(0, solid(WHITE))]),
        ]);
        assert_eq!(c.render(0).next_wake_ms, None);
    }

    #[test]
    fn next_wake_returns_to_none_after_animation_leaves() {
        let mut c = comp_with(&[
            record(Activation::Always, &[(0, solid(GREEN))]),
            record(Activation::LayerActive(1), &[(0, Cell::Breathe { color: RED, period_ms: 1000, phase_ms: 0 })]),
        ]);
        assert_eq!(c.render(0).next_wake_ms, None);
        c.set_active_layer(1);
        assert!(c.render(1).next_wake_ms.is_some());
        c.set_active_layer(0);
        assert_eq!(c.render(2).next_wake_ms, None, "leaving the layer stops the ticker");
    }

    #[test]
    fn next_wake_is_strictly_in_the_future() {
        let mut c = Compositor::<N>::new();
        c.host_set(0, solid(RED), Some(100), 0).unwrap();
        // Render exactly at an edge-adjacent time: the wake must never be <= now.
        for now in [0u64, 50, 99] {
            let out = c.render(now);
            if let Some(wake) = out.next_wake_ms {
                assert!(wake > now, "wake {wake} not after now {now}");
            }
        }
    }

    // --- Frame diffing ----------------------------------------------------

    #[test]
    fn unchanged_frames_report_changed_false() {
        let mut c = comp_with(&[record(Activation::Always, &[(0, solid(RED))])]);
        assert!(c.render(0).changed, "first frame always reports changed");
        assert!(!c.render(1).changed, "identical frame is a no-op");
        c.set_brightness(10);
        assert!(c.render(2).changed);
        assert!(!c.render(3).changed);
    }

    #[test]
    fn blink_frames_change_only_at_edges() {
        let mut c = comp_with(&[record(
            Activation::Always,
            &[(0, Cell::Blink { color: RED, period_ms: 100, phase_ms: 0, duty_pct: 50 })],
        )]);
        assert!(c.render(0).changed);
        assert!(!c.render(10).changed, "still ON, no visual change");
        assert!(c.render(50).changed, "edge: ON -> black");
        assert!(!c.render(60).changed);
        assert!(c.render(100).changed, "edge: black -> ON");
    }
}
