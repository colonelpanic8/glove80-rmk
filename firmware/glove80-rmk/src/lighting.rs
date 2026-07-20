//! Shared Glove80 LED hardware and the right-half frame receiver.

use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_nrf::pwm::{DutyCycle, Prescaler, SimpleConfig, SimplePwm};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_time::{Duration, Timer};
use rmk::core_traits::Runnable;
use rmk::lighting::Rgb8;
use rmk::split_app::SplitAppData;

bind_interrupts!(struct Irqs {
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
});

pub const LEDS_PER_HALF: usize = 40;

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

pub(crate) const FRAME_TAG: u8 = 0x4c;
pub(crate) const BOOTLOADER_TAG: u8 = 0xb0;
pub(crate) const FRAME_HEADER: usize = 4;

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

pub struct PeripheralLighting {
    hardware: LightingHardware,
    staged: [Rgb8; LEDS_PER_HALF],
    sequence: u8,
    received: usize,
}

pub fn init_peripheral(
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> PeripheralLighting {
    PeripheralLighting {
        hardware: LightingHardware::new(spi, data_pin, chain_power_pin, pwm, status_led_pin),
        staged: [Rgb8::BLACK; LEDS_PER_HALF],
        sequence: 0,
        received: 0,
    }
}

impl PeripheralLighting {
    async fn process_message(&mut self, message: SplitAppData) {
        let payload = message.payload();
        if payload == [BOOTLOADER_TAG] {
            rmk::boot::jump_to_bootloader();
            return;
        }
        if payload.len() < FRAME_HEADER || payload[0] != FRAME_TAG {
            return;
        }
        let sequence = payload[1];
        let start = payload[2] as usize;
        let count = payload[3] as usize;
        if count == 0 || payload.len() != FRAME_HEADER + count * 3 {
            return;
        }
        if start == 0 {
            self.sequence = sequence;
            self.received = 0;
        }
        if sequence != self.sequence || start != self.received || start + count > LEDS_PER_HALF {
            self.received = 0;
            return;
        }
        for index in 0..count {
            let offset = FRAME_HEADER + index * 3;
            self.staged[start + index] =
                Rgb8::new(payload[offset], payload[offset + 1], payload[offset + 2]);
        }
        self.received += count;
        if self.received == LEDS_PER_HALF {
            if let Err(error) = self.hardware.write(&self.staged).await {
                defmt::warn!("lighting: peripheral SPI write failed: {:?}", error);
            }
            self.received = 0;
        }
    }
}

impl Runnable for PeripheralLighting {
    async fn run(&mut self) -> ! {
        self.hardware.initialize().await;
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("lighting owns one split-link receiver");
        loop {
            match embassy_futures::select::select(
                link.changed(),
                rmk::split_app::SPLIT_APP_RX.receive(),
            )
            .await
            {
                embassy_futures::select::Either::First(_) => {
                    self.received = 0;
                    while rmk::split_app::SPLIT_APP_RX.try_receive().is_ok() {}
                }
                embassy_futures::select::Either::Second(message) => {
                    self.process_message(message).await;
                }
            }
        }
    }
}
