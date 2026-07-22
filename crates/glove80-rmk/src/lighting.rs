//! Shared Glove80 LED hardware and half-local standard lighting processors.

use core::cell::Cell;
use core::num::NonZeroU32;

use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_nrf::pwm::{DutyCycle, Prescaler, SimpleConfig, SimplePwm};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::{Duration, Timer};
use rmk::core_traits::Runnable;
use rmk::event::{KeyboardEvent, KeyboardEventPos};
use rmk::lighting::topology::MatrixPosition;
use rmk::lighting::{
    BatteryStatusProvider, BuiltinEffect, ConditionalScenes, IndicatorState, LayerState,
    LightingContext, LightingMailbox, LightingOutput, LightingProcessor, LightingService,
    LogicalFrame, Rgb8, SnapshotProvider, StandardCommand, StandardError, StandardLightingEngine,
    StandardReplicaSlot, StandardReply,
};
use rmk::types::battery::BatteryStatus;
use rmk_palettefx::rmk_lighting::{HitQueue, PaletteFxConfig, PaletteFxSource, TopologyLayout};

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

pub const LEDS_PER_HALF: usize = 40;
pub const TOTAL_LEDS: usize = LEDS_PER_HALF * 2;
pub const OVERLAY_CAPACITY: usize = 64;
pub const SCENE_CAPACITY: usize = 64;
pub const COMMAND_CAPACITY: usize = 4;

/// Number of simultaneous key hits the Reactive effect remembers between
/// frames. Each hit fades over ~1.3 s at the default speed, so 16 covers
/// sustained fast typing on one half.
pub const REACTIVE_HITS: usize = 16;

pub type Engine = StandardLightingEngine<
    'static,
    PaletteFxSource<TopologyLayout<TOTAL_LEDS>, TOTAL_LEDS, REACTIVE_HITS>,
    ConditionalScenes<'static, BuiltinEffect, GloveBatteryProvider>,
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
pub static REPLICA_SLOT: StandardReplicaSlot<OVERLAY_CAPACITY, SCENE_CAPACITY> =
    StandardReplicaSlot::new();

/// Pending Reactive key hits for this half's own engine instance. Each
/// binary drains its local queue on its next rendered frame.
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

pub struct GloveBatteryProvider;

pub static GLOVE_BATTERIES: GloveBatteryProvider = GloveBatteryProvider;

impl BatteryStatusProvider for GloveBatteryProvider {
    fn battery_status(&self, node: u8) -> BatteryStatus {
        let batteries = battery_statuses();
        match node {
            0 => batteries.left,
            1 => batteries.right,
            _ => BatteryStatus::Unavailable,
        }
    }
}

/// MoErgo's documented 80% channel ceiling. This remains in the hardware
/// driver below every user-controlled transform and protocol path.
const CHANNEL_CEILING: u8 = 204;
const ONE_FRAME: u8 = 0x70;
const ZERO_FRAME: u8 = 0x40;
const RESET_BYTES: usize = 48;
const ENCODED_LEN: usize = LEDS_PER_HALF * 24 + RESET_BYTES;
const CHAIN_POWER_SETTLE: Duration = Duration::from_millis(120);
const STATUS_PWM_TOP: u16 = 320;
const STATUS_PWM_DUTY: u16 = 16;

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
                let channel = channel.min(CHANNEL_CEILING);
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
    _chain_power: Output<'static>,
    _status_pwm: SimplePwm<'static>,
}

impl LightingHardware {
    pub(crate) fn new(
        spi: Peri<'static, SPI3>,
        data_pin: Peri<'static, impl Pin>,
        chain_power_pin: Peri<'static, impl Pin>,
        pwm: Peri<'static, PWM0>,
        status_led_pin: Peri<'static, impl Pin>,
    ) -> Self {
        let chain_power = Output::new(chain_power_pin, Level::High, OutputDrive::Standard);
        let mut pwm_config = SimpleConfig::default();
        pwm_config.prescaler = Prescaler::Div1;
        pwm_config.max_duty = STATUS_PWM_TOP;
        let mut status_pwm = SimplePwm::new_1ch(pwm, status_led_pin, &pwm_config);
        status_pwm.set_duty(0, DutyCycle::inverted(STATUS_PWM_DUTY));
        Self {
            chain: Ws2812Chain::new(spi, data_pin),
            _chain_power: chain_power,
            _status_pwm: status_pwm,
        }
    }

    pub(crate) async fn initialize(&mut self) {
        Timer::after(CHAIN_POWER_SETTLE).await;
    }

    pub(crate) async fn write(&mut self, frame: &[Rgb8; LEDS_PER_HALF]) -> Result<(), spim::Error> {
        self.chain.write(frame).await
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OutputError {
    Spi,
}

/// One physical half's sink for an otherwise board-wide logical frame.
/// Keeping the same 80 stable slots in both engines avoids a second topology
/// or layer-scene mapping while all animation sampling remains local.
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
        self.hardware.initialize().await;
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

pub fn engine() -> Engine {
    // Half brightness (0x80) as the initial value: the hardware driver's 204
    // channel ceiling and the per-key diffusors make full-scale PaletteFx
    // output harsher than useful; the default speed and first palette match
    // the effect pack's own boot state.
    let palettefx = PaletteFxSource::new(
        TopologyLayout::new(&topology_config::LIGHTING_TOPOLOGY),
        &HIT_QUEUE,
        PaletteFxConfig {
            initial_val: 0x80,
            initial_palette: 0,
            ..PaletteFxConfig::default()
        },
    );
    Engine::new(
        crate::LIGHTING_BACKGROUND,
        crate::LIGHTING_LAYER_SCENES,
        palettefx,
        ConditionalScenes::new(&crate::LIGHTING_CONDITIONAL_SCENE_CELLS, &GLOVE_BATTERIES),
    )
    .with_controls(crate::LIGHTING_CONTROLS)
}

/// Feed pressed keys to the Reactive PaletteFx effect on this half's own
/// engine. Key positions arrive in the local event bus's coordinates:
/// board-wide on the central (the split driver re-publishes peripheral keys
/// with their `[[split.peripheral]]` offsets applied), half-local on the
/// peripheral (its matrix scanner publishes unshifted scan positions).
/// Offsets shift them into the board-wide lighting matrix and the column
/// bounds keep each engine's hits on its physical half. Recording is
/// render-neutral unless Reactive is active; the source drains the queue
/// either way and timestamps hits in the engine animation-clock domain.
#[rmk::macros::processor(subscribe = [KeyboardEvent])]
pub struct ReactiveKeyHits {
    row_offset: u8,
    col_offset: u8,
    first_col: u8,
    last_col: u8,
}

impl ReactiveKeyHits {
    /// Central event bus: positions are already board-wide, including
    /// re-published peripheral events. Keep only this engine's left half.
    pub const fn central() -> Self {
        Self {
            row_offset: 0,
            col_offset: 0,
            first_col: 0,
            last_col: 7,
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
        }
    }

    async fn on_keyboard_event(&mut self, event: KeyboardEvent) {
        if !event.pressed {
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
    }));

#[derive(Clone, Copy)]
pub struct PeripheralState;

impl PeripheralState {
    fn set(context: LightingContext) {
        PERIPHERAL_CONTEXT.lock(|current| current.set(context));
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
        context
    }
}

fn local_vbus_present() -> bool {
    embassy_nrf::pac::POWER.usbregstatus().read().vbusdetect()
}

/// Poll the peripheral's local VBUS bit and invalidate static lighting when it
/// changes. The central USB connection state is deliberately not involved.
pub struct PeripheralPowerMonitor {
    powered: bool,
}

pub fn peripheral_power_monitor() -> PeripheralPowerMonitor {
    PeripheralPowerMonitor {
        powered: local_vbus_present(),
    }
}

impl Runnable for PeripheralPowerMonitor {
    async fn run(&mut self) -> ! {
        loop {
            Timer::after_millis(100).await;
            let powered = local_vbus_present();
            if matches!(
                crate::LIGHTING_CONTROLS.powered_only_scope,
                rmk::lighting::PoweredOnlyScope::Local
            ) && powered != self.powered
            {
                self.powered = powered;
                CORE_MAILBOX.snapshot_changed();
            }
        }
    }
}

pub fn init_peripheral(
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> LightingProcessor<'static, PeripheralState, Engine, HalfOutput, COMMAND_CAPACITY> {
    let service = LightingService::new(PeripheralState, engine(), LogicalFrame::new(Rgb8::BLACK));
    let output = HalfOutput::right(LightingHardware::new(
        spi,
        data_pin,
        chain_power_pin,
        pwm,
        status_led_pin,
    ));
    LightingProcessor::new(service, output, &CORE_MAILBOX)
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
        let Some((generation, snapshot, batteries)) = self.stage.apply(message) else {
            return;
        };
        set_battery_statuses(batteries);
        PeripheralState::set(snapshot.context);
        let revision = snapshot.revision;
        if REPLICA_SLOT.put(snapshot).is_err() {
            defmt::warn!("lighting: peripheral replica slot busy");
            return;
        }
        match CORE_MAILBOX
            .request(StandardCommand::ApplyReplica(&REPLICA_SLOT))
            .await
        {
            Ok(_) => {
                let ack = crate::split_lighting::Message::Ack {
                    generation,
                    revision,
                }
                .encode();
                if rmk::split_app::SPLIT_APP_PERIPH_TX.try_send(ack).is_err() {
                    defmt::warn!("lighting: peripheral replica ack queue full");
                }
            }
            Err(_) => defmt::warn!("lighting: peripheral rejected replica"),
        }
    }
}

impl Runnable for PeripheralReplication {
    async fn run(&mut self) -> ! {
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("lighting replication owns one split-link receiver");
        loop {
            match embassy_futures::select::select(
                link.changed(),
                rmk::split_app::SPLIT_APP_RX.receive(),
            )
            .await
            {
                embassy_futures::select::Either::First(_) => {
                    self.stage.reset();
                    while rmk::split_app::SPLIT_APP_RX.try_receive().is_ok() {}
                }
                embassy_futures::select::Either::Second(message) => self.process(message).await,
            }
        }
    }
}
