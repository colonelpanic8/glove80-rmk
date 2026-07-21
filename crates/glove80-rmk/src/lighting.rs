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
use rmk::lighting::compositor::{Contribution, LightingSource, RenderInput};
use rmk::lighting::{
    EffectSample, EmptySource, IndicatorState, LayerState, LedSlot, LightingContext,
    LightingMailbox, LightingOutput, LightingProcessor, LightingService, LogicalFrame, Rgb8,
    SnapshotProvider, StandardCommand, StandardError, StandardLightingEngine, StandardReplicaSlot,
    StandardReply,
};
use rmk::types::battery::{BatteryStatus, ChargeState};

bind_interrupts!(struct Irqs {
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
});

pub const LEDS_PER_HALF: usize = 40;
pub const TOTAL_LEDS: usize = LEDS_PER_HALF * 2;
pub const OVERLAY_CAPACITY: usize = 64;
pub const SCENE_CAPACITY: usize = 64;
pub const COMMAND_CAPACITY: usize = 4;

pub type Engine = StandardLightingEngine<
    'static,
    EmptySource,
    InformationSource,
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

const MAGIC_LAYER: u8 = 2;
const GAMES_LAYER: u8 = 3;

// F1-F5, corresponding to layers 0-4.
const LAYER_STATUS_SLOTS: [LedSlot; 5] = [
    LedSlot(34),
    LedSlot(28),
    LedSlot(22),
    LedSlot(16),
    LedSlot(10),
];
// Five outer-column keys below each top-row key, ordered bottom-up.
const LEFT_BATTERY_SLOTS: [LedSlot; 5] = [
    LedSlot(39),
    LedSlot(38),
    LedSlot(37),
    LedSlot(36),
    LedSlot(35),
];
const RIGHT_BATTERY_SLOTS: [LedSlot; 5] = [
    LedSlot(79),
    LedSlot(78),
    LedSlot(77),
    LedSlot(76),
    LedSlot(75),
];
// W, A, S, D, and the left-thumb Backspace position whose Games action is Space.
const GAMES_SLOTS: [LedSlot; 5] = [
    LedSlot(24),
    LedSlot(31),
    LedSlot(25),
    LedSlot(19),
    LedSlot(3),
];

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

/// Board-local status composition layered above host overlays.
///
/// Layer state is always visible on F1-F5. Games highlights appear only while
/// layer 3 participates in resolution; the two five-segment battery bars
/// appear only while the momentary Magic layer is held.
#[derive(Clone, Copy, Debug, Default)]
pub struct InformationSource;

impl InformationSource {
    fn magic_active(context: &LightingContext) -> bool {
        context.layers.is_active(MAGIC_LAYER)
    }

    fn games_active(context: &LightingContext) -> bool {
        context.layers.is_active(GAMES_LAYER)
    }

    fn battery_color(status: BatteryStatus, segment: usize) -> Rgb8 {
        const UNLIT: Rgb8 = Rgb8::new(2, 2, 2);
        match status {
            BatteryStatus::Unavailable => Rgb8::new(6, 3, 0),
            BatteryStatus::Available {
                charge_state,
                level: None,
            } => match charge_state {
                ChargeState::Charging => Rgb8::new(0, 48, 128),
                ChargeState::Discharging | ChargeState::Unknown => Rgb8::new(24, 12, 0),
            },
            BatteryStatus::Available {
                charge_state,
                level: Some(level),
            } => {
                // Segments represent 1-20, 21-40, 41-60, 61-80, and 81-100%.
                if level <= (segment as u8) * 20 {
                    return UNLIT;
                }
                match charge_state {
                    ChargeState::Charging => Rgb8::new(0, 64, 160),
                    ChargeState::Discharging | ChargeState::Unknown if level <= 20 => {
                        Rgb8::new(160, 0, 0)
                    }
                    ChargeState::Discharging | ChargeState::Unknown if level <= 40 => {
                        Rgb8::new(160, 48, 0)
                    }
                    ChargeState::Discharging | ChargeState::Unknown => Rgb8::new(0, 128, 0),
                }
            }
        }
    }

    fn color(&self, index: usize, context: &LightingContext) -> Rgb8 {
        let mut index = index;
        if Self::magic_active(context) {
            let batteries = battery_statuses();
            if index < LEFT_BATTERY_SLOTS.len() {
                return Self::battery_color(batteries.left, index);
            }
            index -= LEFT_BATTERY_SLOTS.len();
            if index < RIGHT_BATTERY_SLOTS.len() {
                return Self::battery_color(batteries.right, index);
            }
            index -= RIGHT_BATTERY_SLOTS.len();
        }
        if index < LAYER_STATUS_SLOTS.len() {
            return if context.layers.is_active(index as u8) {
                Rgb8::new(0, 128, 0)
            } else {
                Rgb8::new(64, 0, 0)
            };
        }
        index -= LAYER_STATUS_SLOTS.len();
        if Self::games_active(context) && index < GAMES_SLOTS.len() {
            return if index + 1 == GAMES_SLOTS.len() {
                Rgb8::new(160, 48, 0)
            } else {
                Rgb8::new(160, 0, 0)
            };
        }
        unreachable!("information source index must be below len")
    }
}

impl LightingSource<Rgb8, LightingContext> for InformationSource {
    fn len(&self, input: &RenderInput<'_, LightingContext>) -> usize {
        usize::from(Self::magic_active(input.context))
            * (LEFT_BATTERY_SLOTS.len() + RIGHT_BATTERY_SLOTS.len())
            + LAYER_STATUS_SLOTS.len()
            + usize::from(Self::games_active(input.context)) * GAMES_SLOTS.len()
    }

    fn slot(&self, index: usize, input: &RenderInput<'_, LightingContext>) -> LedSlot {
        let mut index = index;
        if Self::magic_active(input.context) {
            if index < LEFT_BATTERY_SLOTS.len() {
                return LEFT_BATTERY_SLOTS[index];
            }
            index -= LEFT_BATTERY_SLOTS.len();
            if index < RIGHT_BATTERY_SLOTS.len() {
                return RIGHT_BATTERY_SLOTS[index];
            }
            index -= RIGHT_BATTERY_SLOTS.len();
        }
        if index < LAYER_STATUS_SLOTS.len() {
            return LAYER_STATUS_SLOTS[index];
        }
        index -= LAYER_STATUS_SLOTS.len();
        if Self::games_active(input.context) && index < GAMES_SLOTS.len() {
            return GAMES_SLOTS[index];
        }
        unreachable!("information source index must be below len")
    }

    fn contribution(
        &mut self,
        index: usize,
        input: &RenderInput<'_, LightingContext>,
    ) -> Contribution<Rgb8> {
        Contribution::Opaque(EffectSample {
            color: self.color(index, input.context),
            next_change_ms: None,
        })
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
    Engine::new(
        crate::LIGHTING_BACKGROUND,
        crate::LIGHTING_LAYER_SCENES,
        EmptySource,
        InformationSource,
    )
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
        PERIPHERAL_CONTEXT.lock(Cell::get)
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
