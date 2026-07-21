//! Central ownership of the board-wide RMK lighting engine and Rynk state.
//!
//! A successful presentation writes the local 40-LED chain and queues a
//! sequence-checked right-half frame over RMK's split application channel.

use core::num::NonZeroU32;

use embassy_nrf::Peri;
use embassy_nrf::gpio::Pin;
use embassy_nrf::peripherals::{PWM0, SPI3};
use rmk::core_traits::Runnable;
use rmk::host::{
    RynkLightingController, RynkLightingDescriptor, RynkLightingMailbox,
    StandardRynkLightingAdapter,
};
use rmk::keymap::KeyMap;
use rmk::lighting::{
    EmptySource, KeymapLightingState, LightingMailbox, LightingOutput, LightingProcessor,
    LightingService, LogicalFrame, Rgb8, StandardCommand, StandardError, StandardLightingEngine,
    StandardState,
};
use rmk::split_app::{SPLIT_APP_MSG_MAX, SplitAppData};

use crate::lighting::{BOOTLOADER_TAG, FRAME_HEADER, FRAME_TAG, LEDS_PER_HALF, LightingHardware};

pub const TOTAL_LEDS: usize = LEDS_PER_HALF * 2;
pub const OVERLAY_CAPACITY: usize = 64;
const COMMAND_CAPACITY: usize = 4;
const PIXELS_PER_CHUNK: usize = (SPLIT_APP_MSG_MAX - FRAME_HEADER) / 3;
const _: () = assert!(PIXELS_PER_CHUNK > 0);

type Engine =
    StandardLightingEngine<'static, EmptySource, EmptySource, TOTAL_LEDS, OVERLAY_CAPACITY>;
type CoreMailbox = LightingMailbox<
    StandardCommand<OVERLAY_CAPACITY>,
    StandardState,
    StandardError,
    COMMAND_CAPACITY,
>;

static CORE_MAILBOX: CoreMailbox = LightingMailbox::new();
static RYNK_MAILBOX: RynkLightingMailbox = RynkLightingMailbox::new();

/// Coalescing request from the resolved right-half bootloader key.
pub static REMOTE_BOOT_REQUESTS: embassy_sync::channel::Channel<rmk::RawMutex, (), 1> =
    embassy_sync::channel::Channel::new();

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutputError {
    Spi,
    SplitQueueFull,
}

pub(crate) struct CentralOutput {
    hardware: LightingHardware,
    sequence: u8,
}

impl CentralOutput {
    fn queue_right_frame(&mut self, frame: &[Rgb8]) -> Result<(), OutputError> {
        self.sequence = self.sequence.wrapping_add(1);
        for (chunk_index, pixels) in frame.chunks(PIXELS_PER_CHUNK).enumerate() {
            let start = chunk_index * PIXELS_PER_CHUNK;
            let mut payload = [0u8; SPLIT_APP_MSG_MAX];
            payload[0] = FRAME_TAG;
            payload[1] = self.sequence;
            payload[2] = start as u8;
            payload[3] = pixels.len() as u8;
            for (index, pixel) in pixels.iter().enumerate() {
                let offset = FRAME_HEADER + index * 3;
                payload[offset..offset + 3].copy_from_slice(&[pixel.r, pixel.g, pixel.b]);
            }
            let len = FRAME_HEADER + pixels.len() * 3;
            let message = SplitAppData::new(&payload[..len]).expect("bounded lighting chunk");
            rmk::split_app::SPLIT_APP_TX
                .try_send(message)
                .map_err(|_| OutputError::SplitQueueFull)?;
        }
        Ok(())
    }

    async fn present_frame(
        &mut self,
        frame: &LogicalFrame<Rgb8, TOTAL_LEDS>,
    ) -> Result<(), OutputError> {
        let mut left = [Rgb8::BLACK; LEDS_PER_HALF];
        left.copy_from_slice(&frame.as_slice()[..LEDS_PER_HALF]);
        self.hardware
            .write(&left)
            .await
            .map_err(|_| OutputError::Spi)?;
        self.queue_right_frame(&frame.as_slice()[LEDS_PER_HALF..])
    }
}

impl LightingOutput<LogicalFrame<Rgb8, TOTAL_LEDS>> for CentralOutput {
    type Error = OutputError;

    async fn initialize(&mut self) -> Result<(), Self::Error> {
        self.hardware.initialize().await;
        Ok(())
    }

    async fn present(&mut self, frame: &LogicalFrame<Rgb8, TOTAL_LEDS>) -> Result<(), Self::Error> {
        self.present_frame(frame).await
    }

    async fn suspend(&mut self) -> Result<(), Self::Error> {
        let frame = LogicalFrame::new(Rgb8::BLACK);
        self.present_frame(&frame).await
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

pub fn init<'keymap, 'data>(
    keymap: &'keymap KeyMap<'data>,
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> LightingProcessor<
    'static,
    KeymapLightingState<'keymap, 'data>,
    Engine,
    CentralOutput,
    COMMAND_CAPACITY,
> {
    let provider =
        KeymapLightingState::new(keymap).expect("Glove80 layer count fits lighting state");
    let engine = Engine::new(
        crate::LIGHTING_BACKGROUND,
        crate::LIGHTING_LAYER_SCENES,
        EmptySource,
        EmptySource,
    );
    let service = LightingService::new(provider, engine, LogicalFrame::new(Rgb8::BLACK));
    let output = CentralOutput {
        hardware: LightingHardware::new(spi, data_pin, chain_power_pin, pwm, status_led_pin),
        sequence: 0,
    };
    LightingProcessor::new(service, output, &CORE_MAILBOX)
}

pub fn rynk_adapter() -> StandardRynkLightingAdapter<'static, OVERLAY_CAPACITY, COMMAND_CAPACITY> {
    StandardRynkLightingAdapter::new(&RYNK_MAILBOX, &CORE_MAILBOX, crate::LIGHTING_TOPOLOGY)
}

pub const fn rynk_controller() -> RynkLightingController<'static> {
    RynkLightingController::new(
        &RYNK_MAILBOX,
        RynkLightingDescriptor {
            topology_revision: crate::LIGHTING_TOPOLOGY_REVISION,
            topology: crate::LIGHTING_TOPOLOGY,
            routing: crate::LIGHTING_ROUTING,
        },
        OVERLAY_CAPACITY as u16,
    )
}

pub struct RemoteBootDispatcher;

impl Runnable for RemoteBootDispatcher {
    async fn run(&mut self) -> ! {
        loop {
            REMOTE_BOOT_REQUESTS.receive().await;
            let message = SplitAppData::new(&[BOOTLOADER_TAG]).expect("one-byte message");
            // Lighting deliberately drops frames when the one-slot split
            // queue is busy, but a bootloader command must not be dropped:
            // the host has already received an acknowledgement. Wait until
            // this control message owns the next available queue slot.
            rmk::split_app::SPLIT_APP_TX.send(message).await;
        }
    }
}
