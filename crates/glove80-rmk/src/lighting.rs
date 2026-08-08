//! Shared MoErgo LED hardware and half-local standard lighting processors.

use core::cell::Cell;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_nrf::pwm::{DutyCycle, Prescaler, SimpleConfig, SimplePwm};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::{Duration, Timer};
use rmk::core_traits::Runnable;
use rmk::event::{
    EventSubscriber, KeyboardEvent, KeyboardEventPos, LayerChangeEvent, LightingChangedEvent,
    MaintenanceModeEvent, SleepStateEvent, SubscribableEvent,
};
use rmk::lighting::compositor::{Contribution, LightingSource, RenderInput};
use rmk::lighting::topology::{LedSlot, MatrixPosition};
use rmk::lighting::{
    BatteryStatusProvider, BuiltinEffect, ConditionalScenes, IndicatorState, LayerState,
    LightingContext, LightingEffect, LightingMailbox, LightingOutput, LightingProcessor,
    LightingService, LogicalFrame, Rgb8, SnapshotProvider, StandardCommand, StandardError,
    StandardLightingEngine, StandardReplicaSlot, StandardReply,
};
use rmk::storage::{LightingExtensionOverlayRecord, LightingExtensionRecord};
use rmk::types::battery::BatteryStatus;
use rmk_palettefx::effects::{CrosshairParams, Effect};
use rmk_palettefx::palette::id as palette_id;
use rmk_palettefx::rmk_lighting::{
    HitQueue, MAX_INITIAL_PARAMS, PaletteFxConfig, PaletteFxSource, TopologyLayout,
};

/// Board-wide lighting topology for both binaries. `#[rmk_central]` emits
/// `crate::LIGHTING_TOPOLOGY` for the central, but the peripheral macro only
/// emits renderer configuration; the standalone macro reads the same
/// `KEYBOARD_TOML_PATH` and makes identical statics available to both halves.
/// The central binary carries a duplicate flash copy under this namespace,
/// which the nRF52840's 1 MB flash absorbs without contortions.
pub mod topology_config {
    rmk::macros::rmk_lighting_config!();
}

bind_interrupts!(struct Irqs {
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
});

pub const LEDS_PER_HALF: usize = crate::BOARD_LEDS_PER_HALF;
pub const TOTAL_LEDS: usize = LEDS_PER_HALF * 2;
pub const OVERLAY_CAPACITY: usize = 64;
/// Bounds the runtime scene table and, separately, the runtime conditional
/// table, so a host can carry every lighting rule this board would otherwise
/// compile in and still have room to grow. The compiled config currently uses
/// 104 scene cells across seven layers and 65 conditional rules.
///
/// This is a total across all layers, not a per-layer budget: a cell carries
/// its own layer, and every layer shares the one table.
///
/// It is not a cheap constant. Measured on the central binary, each unit of
/// capacity is multiplied across the live tables, atomic-replace staging,
/// replica snapshot in `REPLICA_SLOT`, and the `StandardCommand` payloads
/// queued in a `COMMAND_CAPACITY`-deep mailbox. Both runtime tables intern
/// effects so repeated styles do not pay that multiplier per cell. Raising
/// this value still requires measuring the final central binary.
///
/// Widening a cell costs the same as raising the capacity. Adding the
/// connection and effects predicates to `ConditionSet` grew every conditional
/// cell, and at 160 the central ran out of stack: opening a conditional
/// replace transaction faulted and reset the board. 112 restored the RAM
/// budget the board shipped with before those predicates; the bonded-slot
/// and usb-connected predicates widened the cell again, so 100 gives back
/// that growth with margin. `config/glove80.toml` currently carries 95
/// scene cells and 47 conditional rules.
pub const SCENE_CAPACITY: usize = 100;
pub const COMMAND_CAPACITY: usize = 4;

/// Number of simultaneous key hits each typing-reactive effect can remember.
/// Sixteen covers sustained fast typing on one half.
pub const REACTIVE_HITS: usize = 16;

const MAGIC_LAYER: u8 = 2;
const MAINTENANCE_LED: LedSlot = LedSlot(12);
const MAINTENANCE_ENABLED: Rgb8 = Rgb8::new(0, 128, 0);
const MAINTENANCE_DISABLED: Rgb8 = Rgb8::new(128, 0, 0);

pub struct BoardStatus {
    compiled: ConditionalScenes<'static, BuiltinEffect, BoardBatteryProvider>,
}

impl BoardStatus {
    pub const fn new(
        compiled: ConditionalScenes<'static, BuiltinEffect, BoardBatteryProvider>,
    ) -> Self {
        Self { compiled }
    }

    fn maintenance_visible(input: &RenderInput<'_, LightingContext>) -> bool {
        input.context.layers.is_active(MAGIC_LAYER)
    }
}

impl LightingSource<Rgb8, LightingContext> for BoardStatus {
    fn len(&self, input: &RenderInput<'_, LightingContext>) -> usize {
        self.compiled.len(input) + usize::from(Self::maintenance_visible(input))
    }

    fn slot(&self, index: usize, input: &RenderInput<'_, LightingContext>) -> LedSlot {
        let compiled_len = self.compiled.len(input);
        if index < compiled_len {
            self.compiled.slot(index, input)
        } else if index == compiled_len && Self::maintenance_visible(input) {
            MAINTENANCE_LED
        } else {
            panic!("LightingSource index must be below len")
        }
    }

    fn contribution(
        &mut self,
        index: usize,
        input: &RenderInput<'_, LightingContext>,
    ) -> Contribution<Rgb8> {
        let compiled_len = self.compiled.len(input);
        if index < compiled_len {
            self.compiled.contribution(index, input)
        } else if index == compiled_len && Self::maintenance_visible(input) {
            let color = if rmk::state::maintenance_mode_enabled() {
                MAINTENANCE_ENABLED
            } else {
                MAINTENANCE_DISABLED
            };
            Contribution::Opaque(BuiltinEffect::solid(color).sample(input.now_ms))
        } else {
            panic!("LightingSource index must be below len")
        }
    }
}

pub type Engine = StandardLightingEngine<
    'static,
    PaletteFxSource<TopologyLayout<TOTAL_LEDS>, TOTAL_LEDS, REACTIVE_HITS>,
    BoardStatus,
    TOTAL_LEDS,
    OVERLAY_CAPACITY,
    SCENE_CAPACITY,
>;
pub type CoreMailbox = LightingMailbox<
    StandardCommand<OVERLAY_CAPACITY, SCENE_CAPACITY>,
    StandardReply,
    StandardError,
    COMMAND_CAPACITY,
>;

pub static CORE_MAILBOX: CoreMailbox = LightingMailbox::new();
static LIGHTING_OUTPUT_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static REPLICA_SLOT: StandardReplicaSlot<OVERLAY_CAPACITY, SCENE_CAPACITY> =
    StandardReplicaSlot::new();

#[derive(Clone, Copy)]
struct ApplyRequest {
    generation: u8,
    revision: u32,
    digests: rmk::host::ReplicaDigests,
}

#[derive(Clone, Copy)]
enum DiagnosticRequest {
    Status { request_id: u8 },
    FrameChunk { request_id: u8, offset: u16 },
}

static APPLY_REQUESTS: embassy_sync::signal::Signal<rmk::RawMutex, ApplyRequest> =
    embassy_sync::signal::Signal::new();
static DIAGNOSTIC_REQUESTS: embassy_sync::channel::Channel<rmk::RawMutex, DiagnosticRequest, 1> =
    embassy_sync::channel::Channel::new();
static LAST_APPLIED_REVISION: BlockingMutex<rmk::RawMutex, Cell<Option<u32>>> =
    BlockingMutex::new(Cell::new(None));
static LAST_DIGESTS: BlockingMutex<rmk::RawMutex, Cell<Option<rmk::host::ReplicaDigests>>> =
    BlockingMutex::new(Cell::new(None));

/// Pending key-reactive effect hits for this half's own engine instance. Each
/// binary drains its local queue on its next rendered frame. Central-half hits
/// are also mirrored to the peripheral so spatial effects span the seam.
static HIT_QUEUE: HitQueue<REACTIVE_HITS> = HitQueue::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryPair {
    pub left: BatteryStatus,
    pub right: BatteryStatus,
}

impl BatteryPair {
    pub const UNAVAILABLE: Self = Self {
        left: BatteryStatus::Unavailable,
        right: BatteryStatus::Unavailable,
    };
}

static BATTERIES: BlockingMutex<rmk::RawMutex, Cell<BatteryPair>> =
    BlockingMutex::new(Cell::new(BatteryPair::UNAVAILABLE));

pub fn battery_statuses() -> BatteryPair {
    BATTERIES.lock(Cell::get)
}

pub fn set_battery_statuses(statuses: BatteryPair) {
    BATTERIES.lock(|current| current.set(statuses));
}

pub fn set_left_battery(status: BatteryStatus) {
    BATTERIES.lock(|current| {
        let mut statuses = current.get();
        statuses.left = status;
        current.set(statuses);
    });
}

pub fn set_right_battery(status: BatteryStatus) {
    BATTERIES.lock(|current| {
        let mut statuses = current.get();
        statuses.right = status;
        current.set(statuses);
    });
}

pub struct BoardBatteryProvider;

pub static BOARD_BATTERIES: BoardBatteryProvider = BoardBatteryProvider;

impl BatteryStatusProvider for BoardBatteryProvider {
    fn battery_status(&self, node: u8) -> BatteryStatus {
        let batteries = battery_statuses();
        match node {
            0 => batteries.left,
            1 => batteries.right,
            _ => BatteryStatus::Unavailable,
        }
    }
}

/// The board-specific hardware-output limit. This remains below every
/// user-controlled transform and protocol path. Scale rather than clamp so
/// RMK's global brightness has no dead zone and RGB ratios remain intact.
const CHANNEL_CEILING: u8 = crate::BOARD_CHANNEL_CEILING;

const fn limit_channel(channel: u8) -> u8 {
    ((channel as u16 * CHANNEL_CEILING as u16 + u8::MAX as u16 / 2) / u8::MAX as u16) as u8
}
const ONE_FRAME: u8 = 0x70;
const ZERO_FRAME: u8 = 0x40;
const RESET_BYTES: usize = 48;
const ENCODED_LEN: usize = LEDS_PER_HALF * 24 + RESET_BYTES;
const CHAIN_POWER_SETTLE: Duration = Duration::from_millis(120);
const STATUS_PWM_TOP: u16 = 320;
const STATUS_PWM_DUTY: u16 = 16;
const POWER_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SLEEP_POWER_POLL_INTERVAL: Duration = Duration::from_secs(10);
const ATTESTATION_INTERVAL: Duration = Duration::from_secs(300);

pub(crate) const BOOTLOADER_TAG: u8 = 0xb0;

struct Ws2812Chain {
    spim: Spim<'static>,
    buf: [u8; ENCODED_LEN],
}

impl Ws2812Chain {
    fn new(spi: Peri<'static, SPI3>, data_pin: Peri<'static, impl Pin>) -> Self {
        let mut config = spim::Config::default();
        config.frequency = spim::Frequency::M4;
        config.mode = spim::MODE_0;
        config.orc = 0;
        Self {
            spim: Spim::new_txonly_nosck(spi, Irqs, data_pin, config),
            buf: [0; ENCODED_LEN],
        }
    }

    async fn write(&mut self, frame: &[Rgb8; LEDS_PER_HALF]) -> Result<(), spim::Error> {
        let mut encoded = 0;
        for pixel in frame {
            for channel in [pixel.g, pixel.r, pixel.b] {
                let channel = limit_channel(channel);
                for bit in (0..8).rev() {
                    self.buf[encoded] = if channel & (1 << bit) == 0 {
                        ZERO_FRAME
                    } else {
                        ONE_FRAME
                    };
                    encoded += 1;
                }
            }
        }
        self.buf[encoded..].fill(0);
        self.spim.write_from_ram(&self.buf).await
    }
}

pub(crate) struct LightingHardware {
    chain: Ws2812Chain,
    chain_power: Output<'static>,
    chain_powered: bool,
}

impl LightingHardware {
    pub(crate) fn new(
        spi: Peri<'static, SPI3>,
        data_pin: Peri<'static, impl Pin>,
        chain_power_pin: Peri<'static, impl Pin>,
    ) -> Self {
        Self {
            chain: Ws2812Chain::new(spi, data_pin),
            chain_power: Output::new(chain_power_pin, Level::Low, OutputDrive::Standard),
            chain_powered: false,
        }
    }

    pub(crate) async fn write(&mut self, frame: &[Rgb8; LEDS_PER_HALF]) -> Result<(), spim::Error> {
        let visible = frame.iter().any(|pixel| *pixel != Rgb8::BLACK);
        if !visible {
            if self.chain_powered {
                self.chain_power.set_low();
                self.chain_powered = false;
            }
            return Ok(());
        }
        if !self.chain_powered {
            self.chain_power.set_high();
            self.chain_powered = true;
            Timer::after(CHAIN_POWER_SETTLE).await;
        }
        match self.chain.write(frame).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.chain_power.set_low();
                self.chain_powered = false;
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OutputError {
    Spi,
}

/// One physical half's sink for an otherwise board-wide logical frame.
/// Keeping the same board-wide stable slots in both engines avoids a second
/// topology or layer-scene mapping while all animation sampling remains local.
pub struct HalfOutput {
    hardware: LightingHardware,
    first_slot: usize,
}

impl HalfOutput {
    pub fn left(hardware: LightingHardware) -> Self {
        Self {
            hardware,
            first_slot: 0,
        }
    }

    pub fn right(hardware: LightingHardware) -> Self {
        Self {
            hardware,
            first_slot: LEDS_PER_HALF,
        }
    }

    async fn present_frame(
        &mut self,
        frame: &LogicalFrame<Rgb8, TOTAL_LEDS>,
    ) -> Result<(), OutputError> {
        let mut local = [Rgb8::BLACK; LEDS_PER_HALF];
        local.copy_from_slice(&frame.as_slice()[self.first_slot..self.first_slot + LEDS_PER_HALF]);
        self.hardware
            .write(&local)
            .await
            .map_err(|_| OutputError::Spi)
    }
}

impl LightingOutput<LogicalFrame<Rgb8, TOTAL_LEDS>> for HalfOutput {
    type Error = OutputError;

    async fn initialize(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn present(&mut self, frame: &LogicalFrame<Rgb8, TOTAL_LEDS>) -> Result<(), Self::Error> {
        self.present_frame(frame).await
    }

    async fn suspend(&mut self) -> Result<(), Self::Error> {
        self.present_frame(&LogicalFrame::new(Rgb8::BLACK)).await
    }

    async fn resume(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn retry_after(
        &self,
        _operation: rmk::lighting::OutputOperation,
        _error: &Self::Error,
    ) -> Option<NonZeroU32> {
        NonZeroU32::new(50)
    }
}

/// Compiled-in effect defaults, used on a board that has never persisted a
/// selection. Keep these aligned with `config/glove80.toml` in the outer
/// configuration repository so its release artifacts and runtime profile boot
/// into the same tuned Crosshair/Amber setup.
const DEFAULT_EFFECT: u8 = Effect::<REACTIVE_HITS>::CROSSHAIR_INDEX;
const DEFAULT_EFFECT_VAL: u8 = 0xff;
const DEFAULT_EFFECT_SPEED: u8 = 108;
const DEFAULT_EFFECT_PARAMS: [u8; MAX_INITIAL_PARAMS] = [0, 90, 11, 170, 1, 0, 173, 0];

/// Effect index used when migrating a persisted selection of the retired
/// combined Storm effect into its Rain plus Reactive representation.
const LEGACY_STORM_OVERLAY: u8 = 6;

pub fn engine(
    persisted_extension: Option<LightingExtensionRecord>,
    persisted_overlay: Option<LightingExtensionOverlayRecord>,
) -> Engine {
    // Effect index 7 used to mean the combined Storm effect and now means
    // Crosshair. Old Storm advertised at most six parameters, while every
    // Crosshair record stores its seven-parameter row. Together with the
    // absence of a separately persisted overlay record, that distinguishes
    // old selections without sacrificing Crosshair's stable index.
    let legacy_storm = persisted_overlay.is_none()
        && persisted_extension.as_ref().is_some_and(|record| {
            record.effect == Effect::<REACTIVE_HITS>::LEGACY_STORM_INDEX && record.param_len <= 6
        });
    // A persisted selection wins over the compiled defaults, so changing what
    // the board boots into never means rebuilding firmware. Every index is
    // re-validated downstream against the live effect/palette/parameter lists,
    // because a record written before an effect was inserted would otherwise
    // name a different one.
    let mut config = PaletteFxConfig {
        initial_enabled: true,
        initial_val: DEFAULT_EFFECT_VAL,
        initial_speed: DEFAULT_EFFECT_SPEED,
        initial_palette: palette_id::AMBER,
        initial_effect: DEFAULT_EFFECT,
        initial_overlay: None,
        initial_params: DEFAULT_EFFECT_PARAMS,
        initial_param_len: CrosshairParams::COUNT,
        ..PaletteFxConfig::default()
    };
    if let Some(record) = persisted_extension {
        config.initial_effect = if legacy_storm {
            Effect::<REACTIVE_HITS>::RAIN_INDEX
        } else {
            record.effect
        };
        // Pre-layering records had no overlay key. Only the retired Storm
        // selection implies one; every other saved effect remains standalone.
        config.initial_overlay = legacy_storm.then_some(LEGACY_STORM_OVERLAY);
        config.initial_palette = record.palette as usize;
        config.initial_speed = record.speed;
        // A persisted zero means the user left the effects toggled off; keep
        // them off, and let RgbTog come back at the compiled brightness.
        config.initial_enabled = record.value != 0;
        config.initial_val = if record.value != 0 {
            record.value
        } else {
            DEFAULT_EFFECT_VAL
        };
        let restored = record.params();
        config.initial_param_len = restored.len() as u8;
        config.initial_params[..restored.len()].copy_from_slice(restored);
    }
    if let Some(record) = persisted_overlay {
        config.initial_overlay = record.effect;
        let restored = record.params();
        config.initial_overlay_param_len = restored.len() as u8;
        config.initial_overlay_params[..restored.len()].copy_from_slice(restored);
    }
    let palettefx = PaletteFxSource::new(
        TopologyLayout::new(&topology_config::LIGHTING_TOPOLOGY),
        &HIT_QUEUE,
        config,
    );
    Engine::new(
        crate::LIGHTING_BACKGROUND,
        crate::LIGHTING_LAYER_SCENES,
        palettefx,
        BoardStatus::new(ConditionalScenes::new(
            &crate::LIGHTING_CONDITIONAL_SCENE_CELLS,
            &BOARD_BATTERIES,
        )),
    )
    .with_controls(crate::LIGHTING_CONTROLS)
    .with_battery_status_provider(&BOARD_BATTERIES)
}

#[rmk::macros::processor(subscribe = [MaintenanceModeEvent])]
pub struct MaintenanceLightingState;

impl MaintenanceLightingState {
    async fn on_maintenance_mode_event(&mut self, _event: MaintenanceModeEvent) {
        CORE_MAILBOX.snapshot_changed();
    }
}

pub struct LightingOutputActivity;

pub const fn lighting_output_activity() -> LightingOutputActivity {
    LightingOutputActivity
}

impl LightingOutputActivity {
    async fn refresh(&self) {
        if let Ok(StandardReply::State(state)) =
            CORE_MAILBOX.request(StandardCommand::ReadState).await
        {
            LIGHTING_OUTPUT_ACTIVE.store(state.output_enabled, Ordering::Relaxed);
        }
    }
}

impl Runnable for LightingOutputActivity {
    async fn run(&mut self) -> ! {
        let mut changes = LightingChangedEvent::subscriber();
        self.refresh().await;
        loop {
            changes.next_event().await;
            self.refresh().await;
        }
    }
}

/// Feed pressed keys to the typing-reactive PaletteFx effects. Key
/// positions arrive in the local event bus's coordinates:
/// board-wide on the central (the split driver re-publishes peripheral keys
/// with their `[[split.peripheral]]` offsets applied), half-local on the
/// peripheral (its matrix scanner publishes unshifted scan positions).
/// Offsets shift them into the board-wide lighting matrix. The central records
/// both halves locally, while its own left-half hits are mirrored over the
/// split application channel. The peripheral records right-half scans locally
/// and mirrored left-half slots remotely, giving both engines the same spatial
/// hit while avoiding a duplicate for right-half keys.
///
/// Recording is render-neutral unless a key-reactive effect is active; the
/// source drains the queue either way and timestamps hits in the engine
/// animation-clock domain.
#[rmk::macros::processor(subscribe = [KeyboardEvent])]
pub struct ReactiveKeyHits {
    row_offset: u8,
    col_offset: u8,
    first_col: u8,
    last_col: u8,
    mirror_left_hits: bool,
}

impl ReactiveKeyHits {
    /// Central event bus: positions are already board-wide, including
    /// re-published peripheral events. Render every hit locally and mirror
    /// left-half hits to the peripheral.
    pub const fn central() -> Self {
        Self {
            row_offset: 0,
            col_offset: 0,
            first_col: 0,
            last_col: 14,
            mirror_left_hits: true,
        }
    }

    /// Peripheral event bus: shift local scans by the right half's
    /// `[[split.peripheral]]` offsets from keyboard.toml.
    pub const fn peripheral() -> Self {
        Self {
            row_offset: 0,
            col_offset: 7,
            first_col: 7,
            last_col: 14,
            mirror_left_hits: false,
        }
    }

    async fn on_keyboard_event(&mut self, event: KeyboardEvent) {
        if !event.pressed || !LIGHTING_OUTPUT_ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        let KeyboardEventPos::Key(pos) = event.pos else {
            return;
        };
        let (Some(row), Some(col)) = (
            pos.row.checked_add(self.row_offset),
            pos.col.checked_add(self.col_offset),
        ) else {
            return;
        };
        if !(self.first_col..self.last_col).contains(&col) {
            return;
        }
        let key = MatrixPosition::new(row, col);
        let mut queued = false;
        for (slot, _) in topology_config::LIGHTING_TOPOLOGY.leds_for_key(key) {
            queued |= HIT_QUEUE.record(slot.0 as u8);
            if self.mirror_left_hits {
                crate::split_lighting::try_queue_effect_hit(slot);
            }
        }
        if queued {
            CORE_MAILBOX.snapshot_changed();
        }
    }
}

static PERIPHERAL_CONTEXT: BlockingMutex<rmk::RawMutex, Cell<LightingContext>> =
    BlockingMutex::new(Cell::new(LightingContext {
        layers: LayerState::new(0, 0, 1),
        indicators: IndicatorState {
            num_lock: false,
            caps_lock: false,
            scroll_lock: false,
            compose: false,
            kana: false,
        },
        powered: false,
        // Replaced by the central's bitmap on the first replicated context;
        // until then the peripheral knows of no bonds.
        bonded_slots: 0,
        connection: rmk::types::connection::ConnectionStatus::new(),
    }));

#[derive(Clone, Copy)]
pub struct PeripheralState;

impl PeripheralState {
    fn set(context: LightingContext) {
        PERIPHERAL_CONTEXT.lock(|current| current.set(context));
    }

    fn apply_replicated(context: crate::split_lighting::ReplicatedContext) {
        PERIPHERAL_CONTEXT.lock(|current| {
            let mut merged = current.get();
            context.apply_to(&mut merged);
            current.set(merged);
        });
    }

    fn merge_ephemeral(context: &mut LightingContext) {
        context.layers = PERIPHERAL_CONTEXT.lock(Cell::get).layers;
        context.connection = rmk::state::current_connection_status();
    }

    /// Apply RMK's native high-priority effective-layer edge immediately.
    /// The local bitmap is conservatively adjusted without semantic sync.
    fn set_effective_layer(layer: u8) {
        PERIPHERAL_CONTEXT.lock(|current| {
            let mut context = current.get();
            let previous = context.layers;
            let mut active = previous.active_bits();
            if previous.effective != previous.default && previous.effective != layer {
                active &= !(1_u64 << previous.effective);
            }
            active |= (1_u64 << previous.default) | (1_u64 << layer);
            context.layers = LayerState::new(layer, previous.default, active);
            current.set(context);
        });
    }
}

/// Bridge RMK's native priority layer edge directly into the renderer.
#[rmk::macros::processor(subscribe = [LayerChangeEvent])]
pub struct FastPeripheralLayerLighting;

impl FastPeripheralLayerLighting {
    async fn on_layer_change_event(&mut self, event: LayerChangeEvent) {
        PeripheralState::set_effective_layer(event.0);
        CORE_MAILBOX.snapshot_changed();
    }
}

impl SnapshotProvider for PeripheralState {
    type Snapshot = LightingContext;

    fn snapshot(&self) -> Self::Snapshot {
        let mut context = PERIPHERAL_CONTEXT.lock(Cell::get);
        if matches!(
            crate::LIGHTING_CONTROLS.powered_only_scope,
            rmk::lighting::PoweredOnlyScope::Local
        ) {
            context.powered = local_vbus_present();
        }
        // The split link replicates the central's connection status into this
        // half's own global; the lighting context packet does not carry it.
        context.connection = rmk::state::current_connection_status();
        context
    }
}

fn local_vbus_present() -> bool {
    embassy_nrf::pac::POWER.usbregstatus().read().vbusdetect()
}

/// Poll this half's local VBUS bit; on a change, invalidate static lighting
/// (under the local powered-only scope) and publish the charge state.
///
/// The board has no charger-status line configured, so VBUS presence stands
/// in for the charge state: plugged is reported as charging, unplugged as
/// discharging. There is no charge-termination detection, so a full battery
/// still reads as charging while wired. Without this proxy no charge state
/// is ever produced at all -- rmk's `ChargingStateReader` needs a
/// charger-detect GPIO this board does not wire up -- and every
/// `charge`-gated lighting rule is dead.
pub struct PowerMonitor {
    powered: bool,
    sleeping: bool,
    status_pwm: SimplePwm<'static>,
}

pub fn power_monitor(
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> PowerMonitor {
    let powered = local_vbus_present();
    let mut pwm_config = SimpleConfig::default();
    pwm_config.prescaler = Prescaler::Div1;
    pwm_config.max_duty = STATUS_PWM_TOP;
    let mut status_pwm = SimplePwm::new_1ch(pwm, status_led_pin, &pwm_config);
    status_pwm.set_duty(
        0,
        DutyCycle::inverted(if powered { STATUS_PWM_DUTY } else { 0 }),
    );
    PowerMonitor {
        powered,
        sleeping: false,
        status_pwm,
    }
}

impl PowerMonitor {
    fn update_status_led(&mut self) {
        let duty = if self.powered && !self.sleeping {
            STATUS_PWM_DUTY
        } else {
            0
        };
        self.status_pwm.set_duty(0, DutyCycle::inverted(duty));
    }

    fn refresh_power(&mut self) {
        let powered = local_vbus_present();
        if powered == self.powered {
            return;
        }
        self.powered = powered;
        self.update_status_led();
        rmk::event::publish_event(rmk::event::ChargingStateEvent { charging: powered });
        if matches!(
            crate::LIGHTING_CONTROLS.powered_only_scope,
            rmk::lighting::PoweredOnlyScope::Local
        ) {
            CORE_MAILBOX.snapshot_changed();
        }
    }
}

impl Runnable for PowerMonitor {
    async fn run(&mut self) -> ! {
        // Mirror rmk's ChargingStateReader settle: give the first battery ADC
        // reading a head start, then announce the boot-time state so the
        // charge state is defined without waiting for a plug/unplug edge.
        Timer::after_secs(2).await;
        self.powered = local_vbus_present();
        rmk::event::publish_event(rmk::event::ChargingStateEvent {
            charging: self.powered,
        });
        self.update_status_led();
        let mut sleep = SleepStateEvent::subscriber();
        loop {
            let interval = if self.sleeping {
                SLEEP_POWER_POLL_INTERVAL
            } else {
                POWER_POLL_INTERVAL
            };
            match embassy_futures::select::select(sleep.next_event(), Timer::after(interval)).await
            {
                embassy_futures::select::Either::First(event) => {
                    self.sleeping = event.0;
                    self.refresh_power();
                    self.update_status_led();
                }
                embassy_futures::select::Either::Second(()) => self.refresh_power(),
            }
        }
    }
}

pub fn init_peripheral(
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
) -> LightingProcessor<'static, PeripheralState, Engine, HalfOutput, COMMAND_CAPACITY> {
    // The peripheral never persists a selection: it renders whatever the
    // central replicates to it, so it boots on the compiled defaults.
    let service = LightingService::new(
        PeripheralState,
        engine(None, None),
        LogicalFrame::new(Rgb8::BLACK),
    );
    let output = HalfOutput::right(LightingHardware::new(spi, data_pin, chain_power_pin));
    LightingProcessor::new(service, output, &CORE_MAILBOX)
}

fn try_send_diagnostic(message: crate::split_lighting::Message) -> bool {
    // RMK's peripheral application queue has two slots. Diagnostics and
    // heartbeats only enter an empty queue, preserving the second slot for a
    // replication Ack that can arrive while the first packet is pending.
    crate::split_lighting::diagnostic_may_enqueue(
        rmk::split_app::SPLIT_APP_PERIPH_TX.free_capacity(),
    ) && rmk::split_app::SPLIT_APP_PERIPH_TX
        .try_send(message.encode())
        .is_ok()
}

fn try_send_attestation(generation: u8) -> bool {
    LAST_DIGESTS.lock(Cell::get).is_some_and(|digests| {
        try_send_diagnostic(crate::split_lighting::Message::Attestation {
            generation,
            digests,
        })
    })
}

pub struct PeripheralLightingWorker;

pub const fn peripheral_lighting_worker() -> PeripheralLightingWorker {
    PeripheralLightingWorker
}

impl PeripheralLightingWorker {
    async fn apply(&mut self, request: ApplyRequest) {
        match CORE_MAILBOX
            .request(StandardCommand::ApplyReplica(&REPLICA_SLOT))
            .await
        {
            Ok(_) => {
                LAST_APPLIED_REVISION.lock(|revision| revision.set(Some(request.revision)));
                LAST_DIGESTS.lock(|digests| digests.set(Some(request.digests)));
                let ack = crate::split_lighting::Message::Ack {
                    generation: request.generation,
                    revision: request.revision,
                }
                .encode();
                if rmk::split_app::SPLIT_APP_PERIPH_TX.try_send(ack).is_err() {
                    defmt::warn!("lighting: peripheral replica ack queue full");
                    return;
                }
                // The Ack is now the head packet. With no other diagnostics
                // admitted into a non-empty queue, the remaining slot is safe
                // for its additive attestation.
                if let Some(digests) = LAST_DIGESTS.lock(Cell::get) {
                    let _ = rmk::split_app::SPLIT_APP_PERIPH_TX.try_send(
                        crate::split_lighting::Message::Attestation {
                            generation: request.generation,
                            digests,
                        }
                        .encode(),
                    );
                }
            }
            Err(_) => defmt::warn!("lighting: peripheral rejected replica"),
        }
    }

    async fn diagnostic(&mut self, request: DiagnosticRequest) {
        let message = match request {
            DiagnosticRequest::Status { request_id } => {
                match CORE_MAILBOX.request(StandardCommand::ReadState).await {
                    Ok(StandardReply::State(state)) => {
                        let layers = state.presented.map_or_else(
                            || PeripheralState.snapshot().layers,
                            |presented| presented.context.layers,
                        );
                        Some(crate::split_lighting::Message::StatusReport {
                            request_id,
                            applied_revision: LAST_APPLIED_REVISION.lock(Cell::get),
                            engine_revision: state.revision,
                            layers,
                            powered: state.powered,
                            wake_active: state.wake_active,
                            effective_output_enabled: state.output_enabled,
                        })
                    }
                    _ => None,
                }
            }
            DiagnosticRequest::FrameChunk { request_id, offset } => {
                match CORE_MAILBOX
                    .request(StandardCommand::ReadFrame { offset })
                    .await
                {
                    Ok(StandardReply::FramePage(page)) => {
                        let mut cells = [Rgb8::BLACK; 4];
                        let len = page.cells().len().min(cells.len());
                        cells[..len].copy_from_slice(&page.cells()[..len]);
                        Some(crate::split_lighting::Message::FrameChunk {
                            request_id,
                            revision: page.revision,
                            total: page.total,
                            start: page.start,
                            len: len as u8,
                            cells,
                        })
                    }
                    _ => None,
                }
            }
        };
        if let Some(message) = message {
            let _ = try_send_diagnostic(message);
        }
    }
}

impl Runnable for PeripheralLightingWorker {
    async fn run(&mut self) -> ! {
        loop {
            match embassy_futures::select::select(
                APPLY_REQUESTS.wait(),
                DIAGNOSTIC_REQUESTS.receive(),
            )
            .await
            {
                embassy_futures::select::Either::First(request) => self.apply(request).await,
                embassy_futures::select::Either::Second(request) => self.diagnostic(request).await,
            }
        }
    }
}

pub struct PeripheralReplication {
    stage: crate::split_lighting::SnapshotStage,
}

pub const fn peripheral_replication() -> PeripheralReplication {
    PeripheralReplication {
        stage: crate::split_lighting::SnapshotStage::new(),
    }
}

impl PeripheralReplication {
    async fn process(&mut self, data: rmk::split_app::SplitAppData) {
        if data.payload() == [BOOTLOADER_TAG] {
            rmk::boot::jump_to_bootloader();
            return;
        }
        let Ok(message) = crate::split_lighting::Message::decode(data) else {
            return;
        };
        if let crate::split_lighting::Message::EffectHit { slot } = message {
            if LIGHTING_OUTPUT_ACTIVE.load(Ordering::Relaxed)
                && slot.index() < LEDS_PER_HALF
                && HIT_QUEUE.record(slot.0 as u8)
            {
                CORE_MAILBOX.snapshot_changed();
            }
            return;
        }
        match message {
            crate::split_lighting::Message::ReplicaProbe { generation } => {
                let _ = try_send_attestation(generation);
                return;
            }
            crate::split_lighting::Message::StatusRequest { request_id } => {
                let _ = DIAGNOSTIC_REQUESTS.try_send(DiagnosticRequest::Status { request_id });
                return;
            }
            crate::split_lighting::Message::FrameChunkRequest { request_id, offset } => {
                let _ = DIAGNOSTIC_REQUESTS
                    .try_send(DiagnosticRequest::FrameChunk { request_id, offset });
                return;
            }
            _ => {}
        }
        if let crate::split_lighting::Message::ContextUpdate {
            generation,
            revision,
            context,
            batteries,
        } = message
        {
            if LAST_APPLIED_REVISION.lock(Cell::get) == Some(revision) {
                set_battery_statuses(batteries);
                PeripheralState::apply_replicated(context);
                CORE_MAILBOX.snapshot_changed();
                let ack = crate::split_lighting::Message::Ack {
                    generation,
                    revision,
                }
                .encode();
                if rmk::split_app::SPLIT_APP_PERIPH_TX.try_send(ack).is_err() {
                    defmt::warn!("lighting: peripheral context ack queue full");
                }
            }
            return;
        }
        let Some((generation, mut snapshot, batteries)) = self.stage.apply(message) else {
            return;
        };
        PeripheralState::merge_ephemeral(&mut snapshot.context);
        let digests = crate::split_lighting::replica_digests(&snapshot);
        set_battery_statuses(batteries);
        PeripheralState::set(snapshot.context);
        let revision = snapshot.revision;
        if REPLICA_SLOT.put(snapshot).is_err() {
            defmt::warn!("lighting: peripheral replica slot busy");
            return;
        }
        APPLY_REQUESTS.signal(ApplyRequest {
            generation,
            revision,
            digests,
        });
    }
}

impl Runnable for PeripheralReplication {
    async fn run(&mut self) -> ! {
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("lighting replication owns one split-link receiver");
        let mut heartbeat_at = embassy_time::Instant::now() + ATTESTATION_INTERVAL;
        loop {
            match embassy_futures::select::select3(
                link.changed(),
                rmk::split_app::SPLIT_APP_RX.receive(),
                Timer::at(heartbeat_at),
            )
            .await
            {
                embassy_futures::select::Either3::First(up) => {
                    self.stage.reset();
                    // Drain the inbox only when the link went down. This half
                    // marks the link up on the first *inbound* message, so on
                    // the up edge the inbox already holds the head of the
                    // reconnect snapshot -- draining here threw that snapshot
                    // away and cost every reconnect a wasted burst plus the
                    // central's ack timeout. Anything genuinely stale that
                    // survives an up edge is harmless: the next Begin restarts
                    // staging, and generation/revision matching rejects the
                    // rest.
                    if !up {
                        while rmk::split_app::SPLIT_APP_RX.try_receive().is_ok() {}
                    }
                }
                embassy_futures::select::Either3::Second(message) => self.process(message).await,
                embassy_futures::select::Either3::Third(()) => {
                    heartbeat_at += ATTESTATION_INTERVAL;
                    if rmk::split_app::SPLIT_APP_LINK.try_get() == Some(true) {
                        let _ = try_send_attestation(0);
                    }
                }
            }
        }
    }
}
