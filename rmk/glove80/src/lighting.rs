//! Minimum viable lighting engine for the Glove80 RMK port (spike stage 5).
//!
//! This module owns all LED hardware on a half:
//!
//! - the 40-LED WS2812-compatible per-key chain, driven over SPIM3 exactly
//!   like ZMK does (4 MHz, one WS2812 bit encoded as one SPI byte: `0x70` for
//!   a one, `0x40` for a zero; wire order GRB), plus its power-enable GPIO
//!   (active high, needs ~100 ms to settle before the first frame), and
//! - the rear power-button LED, a plain PWM output run at the same settings
//!   ZMK uses (20 us period, ~5% duty).
//!
//! The design is a seed for the future sparse lighting compositor
//! (docs/desired-system.md "Lighting model"): rendering is split into a
//! *frame source* (today: a trivial "layer color on chain index 0" rule fed
//! by RMK `LayerChangeEvent`s) and a *frame sink* ([`Ws2812Chain`], which
//! encodes and DMAs a full 40-cell RGB frame). The compositor replaces the
//! frame source; the driver, frame type, and task wiring stay.
//!
//! Rendering is strictly event-driven: no ticker exists, frames are only
//! encoded and transferred when an event arrives (plus one initial frame at
//! boot). The WS2812 transfer is EasyDMA-timed in hardware, so BLE radio
//! interrupts cannot stretch bit timings; a full 40-LED frame is ~1 KiB of
//! SPI traffic (~2 ms at 4 MHz) with the CPU free during the transfer.
//!
//! SAFETY / WARRANTY NOTE: [`Ws2812Chain`] hard-clamps every color channel to
//! [`MAX_CHANNEL`] (80% of full scale) at encode time, per MoErgo's current
//! limit. Callers cannot bypass the clamp; do not "fix" brightness by raising
//! values above it elsewhere.

use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_nrf::pwm::{DutyCycle, Prescaler, SimpleConfig, SimplePwm};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_time::{Duration, Instant, Timer};
use rmk::event::{EventSubscriber, LayerChangeEvent, SubscribableEvent};
use rmk::processor::Processor;

bind_interrupts!(struct Irqs {
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
});

/// Number of WS2812 LEDs on each half's chain.
///
/// Left-half chain order (from the comment block in
/// `zmk/app/boards/arm/glove80/glove80_lh.dts`; chain index at each physical
/// key position, thumb cluster bottom right):
///
/// ```text
/// 34 28 22 16 10
/// 35 29 23 17 11  6
/// 36 30 24 18 12  7
/// 37 31 25 19 13  8
/// 38 32 26 20 14  9
/// 39 33 27 21 15
///               0  1  2
///               3  4  5
/// ```
///
/// i.e. chain indices 0-5 are the six thumb keys (top row 0 1 2, bottom row
/// 3 4 5), 6-39 cover the main grid column by column, inner column first.
pub const NUM_LEDS: usize = 40;

/// One RGB cell of a lighting frame. Values are pre-clamp; the driver clamps
/// each channel to [`MAX_CHANNEL`] when encoding.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
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
}

/// A full frame for the per-key chain: one RGB cell per chain index.
///
/// This is the interface the future compositor renders into; anything that
/// can produce a `Frame` can drive [`Ws2812Chain::write`].
pub type Frame = [Rgb; NUM_LEDS];

/// Hard per-channel ceiling: 80% of full scale (MoErgo current/warranty
/// limit). Enforced inside the driver at encode time.
pub const MAX_CHANNEL: u8 = 204;

/// SPI byte encoding a WS2812 "1" bit at 4 MHz (matches ZMK's
/// `spi-one-frame`).
const ONE_FRAME: u8 = 0x70;
/// SPI byte encoding a WS2812 "0" bit at 4 MHz (matches ZMK's
/// `spi-zero-frame`).
const ZERO_FRAME: u8 = 0x40;

/// Trailing all-zero SPI bytes appended to every transfer so the line is held
/// low for >= 80 us (WS2812 latch) even if a second frame follows
/// immediately. 48 bytes * 2 us/byte = 96 us.
const RESET_BYTES: usize = 48;

/// Encoded transfer length: 24 SPI bytes per LED (8 per color channel, GRB)
/// plus the latch tail.
const ENCODED_LEN: usize = NUM_LEDS * 24 + RESET_BYTES;

/// How long the chain's 5 V rail needs after the enable GPIO goes high before
/// the first frame is clocked out (ZMK uses `init-delay-ms = <100>`).
const CHAIN_POWER_SETTLE: Duration = Duration::from_millis(120);

/// Rear power-button LED PWM: 20 us period at the 16 MHz PWM clock
/// (matches ZMK's `PWM_USEC(20)`).
const STATUS_PWM_TOP: u16 = 320;
/// ~5% duty, the dim ZMK default.
const STATUS_PWM_DUTY: u16 = 16;

/// Frame sink: owns SPIM3 and the EasyDMA-visible encode buffer, and applies
/// the [`MAX_CHANNEL`] clamp. This is the driver layer the compositor will
/// keep using unchanged.
pub struct Ws2812Chain {
    spim: Spim<'static>,
    /// EasyDMA transmit buffer. Lives inside the (statically allocated) main
    /// task future, so it is always in RAM as EasyDMA requires.
    buf: [u8; ENCODED_LEN],
}

impl Ws2812Chain {
    fn new(spi: Peri<'static, SPI3>, data_pin: Peri<'static, impl Pin>) -> Self {
        let mut config = spim::Config::default();
        config.frequency = spim::Frequency::M4;
        // MODE_0: clock/data idle low, which keeps the WS2812 data line low
        // between transfers.
        config.mode = spim::MODE_0;
        config.orc = 0x00;
        Self {
            spim: Spim::new_txonly_nosck(spi, Irqs, data_pin, config),
            buf: [0; ENCODED_LEN],
        }
    }

    /// Encode `frame` (clamping every channel to [`MAX_CHANNEL`]) and DMA it
    /// out. Returns after the transfer completes; the latch tail is part of
    /// the transfer.
    pub async fn write(&mut self, frame: &Frame) {
        let mut i = 0;
        for cell in frame {
            // Wire order is GRB.
            for channel in [cell.g, cell.r, cell.b] {
                // Hard current clamp -- keep this in the driver.
                let clamped = channel.min(MAX_CHANNEL);
                for bit in (0..8).rev() {
                    self.buf[i] = if clamped & (1 << bit) != 0 { ONE_FRAME } else { ZERO_FRAME };
                    i += 1;
                }
            }
        }
        // buf[i..] stays 0x00: the >= 80 us latch tail.
        if let Err(e) = self.spim.write_from_ram(&self.buf).await {
            defmt::warn!("lighting: WS2812 SPI transfer failed: {:?}", e);
        }
    }
}

/// Dim per-layer colors for chain index 0 (kept low; well under the clamp).
/// Layer order matches keyboard.toml: Base, Lower, Magic, Games, Mac Hyper,
/// then the three unassigned layers.
const LAYER_COLORS: [Rgb; 8] = [
    Rgb::new(0, 0, 24),   // 0 Base: blue
    Rgb::new(0, 24, 0),   // 1 Lower: green
    Rgb::new(24, 0, 24),  // 2 Magic: magenta
    Rgb::new(24, 0, 0),   // 3 Games: red
    Rgb::new(0, 24, 24),  // 4 Mac Hyper: cyan
    Rgb::new(24, 12, 0),  // 5: amber
    Rgb::new(12, 24, 0),  // 6: chartreuse
    Rgb::new(16, 16, 16), // 7: white
];

/// Event-driven lighting task. Registered with RMK via
/// `#[register_processor(event)]`; RMK joins its `process_loop` with the
/// other firmware tasks. Today's frame source is "layer color on chain
/// index 0"; the compositor later replaces exactly that part.
pub struct LightingProcessor {
    chain: Ws2812Chain,
    frame: Frame,
    /// First moment the chain's 5 V rail is considered stable.
    power_ready_at: Instant,
    /// Chain power enable (active high). Held for the lifetime of the
    /// firmware; dropping it would re-float the enable line.
    _chain_power: Output<'static>,
    /// Rear power-button LED. The PWM peripheral free-runs in hardware; kept
    /// alive here so the pin is not released.
    _status_pwm: SimplePwm<'static>,
}

/// Bring up all lighting hardware for one half and return the processor that
/// drives it.
///
/// Left half: `init(p.SPI3, p.P0_27, p.P0_31, p.PWM0, p.P1_15)`.
/// Right half: `init(p.SPI3, p.P0_13, p.P0_19, p.PWM0, p.P0_16)`.
pub fn init(
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> LightingProcessor {
    // Power up the LED chain rail immediately so it settles while the rest
    // of the firmware initializes; the first frame waits for
    // `power_ready_at` in `process_loop`.
    let chain_power = Output::new(chain_power_pin, Level::High, OutputDrive::Standard);
    let power_ready_at = Instant::now() + CHAIN_POWER_SETTLE;

    // Rear LED: fixed dim PWM, set once, free-runs with no task behind it.
    let mut pwm_config = SimpleConfig::default();
    pwm_config.prescaler = Prescaler::Div1; // 16 MHz PWM clock
    pwm_config.max_duty = STATUS_PWM_TOP;
    let mut status_pwm = SimplePwm::new_1ch(pwm, status_led_pin, &pwm_config);
    // `inverted` = pin high while the counter is below the duty value, i.e.
    // a high pulse of STATUS_PWM_DUTY counts -- same as ZMK's
    // PWM_POLARITY_NORMAL wiring of this LED.
    status_pwm.set_duty(0, DutyCycle::inverted(STATUS_PWM_DUTY));

    LightingProcessor {
        chain: Ws2812Chain::new(spi, data_pin),
        frame: [Rgb::OFF; NUM_LEDS],
        power_ready_at,
        _chain_power: chain_power,
        _status_pwm: status_pwm,
    }
}

impl LightingProcessor {
    fn apply_layer(&mut self, layer: u8) {
        self.frame[0] = LAYER_COLORS[layer as usize % LAYER_COLORS.len()];
    }

    async fn render(&mut self) {
        self.chain.write(&self.frame).await;
    }
}

impl rmk::core_traits::Runnable for LightingProcessor {
    async fn run(&mut self) -> ! {
        self.process_loop().await
    }
}

impl Processor for LightingProcessor {
    type Event = LayerChangeEvent;

    fn subscriber() -> impl EventSubscriber<Event = LayerChangeEvent> {
        LayerChangeEvent::subscriber()
    }

    async fn process(&mut self, event: LayerChangeEvent) {
        self.apply_layer(event.0);
        self.render().await;
    }

    /// Overrides the default loop to draw one initial frame (after the chain
    /// rail settles) before going fully event-driven. The subscriber is
    /// created first so no early layer change is lost.
    async fn process_loop(&mut self) -> ! {
        let mut sub = Self::subscriber();
        Timer::at(self.power_ready_at).await;
        self.apply_layer(0);
        self.render().await;
        loop {
            let event = sub.next_event().await;
            self.process(event).await;
        }
    }
}
