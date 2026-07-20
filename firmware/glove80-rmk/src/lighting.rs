//! Lighting engine for the Glove80 RMK port: WS2812 driver (frame sink) plus
//! the sparse lighting compositor (frame source, Phase 1 of
//! docs/implementation-plan.md).
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
//! The frame source is the [`glove80_compositor`] crate: pure `no_std` logic
//! (host-tested in its own workspace) that composes sparse records — base,
//! layer, toggle, host overlay, status — into a [`Frame`], and reports when
//! that frame can next change. [`LightingProcessor`] is the glue: it feeds
//! the compositor RMK `LayerChangeEvent`s and the embassy clock, arms a
//! deadline timer only when the compositor asks for one (static state keeps
//! the no-ticker guarantee), skips the SPI write when the frame is
//! unchanged, and pushes changed frames through [`Ws2812Chain`].
//!
//! The WS2812 transfer is EasyDMA-timed in hardware, so BLE radio interrupts
//! cannot stretch bit timings; a full 40-LED frame is ~1 KiB of SPI traffic
//! (~2 ms at 4 MHz) with the CPU free during the transfer.
//!
//! SAFETY / WARRANTY NOTE: [`Ws2812Chain`] hard-clamps every color channel to
//! [`CHANNEL_CEILING`] (80% of full scale, MoErgo's current limit — see the
//! constant's definition in `glove80-compositor` for the full warning) at
//! encode time. Callers cannot bypass the clamp; the compositor's runtime
//! ceiling can only lower the limit further, never raise it.

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_nrf::pwm::{DutyCycle, Prescaler, SimpleConfig, SimplePwm};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_time::{Duration, Instant, Timer};
pub use glove80_compositor::{Activation, CHANNEL_CEILING, Cell, Compositor, Record, Rgb};
use rmk::event::{
    ConnectionStatusChangeEvent, EventSubscriber, LayerChangeEvent, SubscribableEvent,
};
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
/// The right half is the mirror image; the same chain indices land on the
/// mirrored physical positions.
pub const NUM_LEDS: usize = 40;

/// A full frame for the per-key chain: one RGB cell per chain index. This is
/// what the compositor renders into and what [`Ws2812Chain::write`] encodes.
pub type Frame = [Rgb; NUM_LEDS];

/// Coalescing wake-up from the central's physical remote-boot action
/// processor to this task, which owns split-link state.
pub static REMOTE_BOOT_REQUESTS: embassy_sync::channel::Channel<rmk::RawMutex, (), 1> =
    embassy_sync::channel::Channel::new();

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
/// the [`CHANNEL_CEILING`] clamp. This driver layer is the hard safety
/// backstop below the compositor.
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

    /// Encode `frame` (clamping every channel to [`CHANNEL_CEILING`]) and DMA
    /// it out. Returns after the transfer completes; the latch tail is part
    /// of the transfer.
    pub async fn write(&mut self, frame: &Frame) {
        let mut i = 0;
        for cell in frame {
            // Wire order is GRB.
            for channel in [cell.g, cell.r, cell.b] {
                // Hard current clamp -- keep this in the driver.
                let clamped = channel.min(CHANNEL_CEILING);
                for bit in (0..8).rev() {
                    self.buf[i] = if clamped & (1 << bit) != 0 {
                        ONE_FRAME
                    } else {
                        ZERO_FRAME
                    };
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

/// Per-layer accent colors, at full scale (the driver clamps each channel to
/// [`CHANNEL_CEILING`], so these render at the 80% safety ceiling).
/// Layer order matches keyboard.toml: Base, Lower, Magic, Games, Mac Hyper,
/// then the three unassigned layers.
const LAYER_COLORS: [Rgb; 8] = [
    Rgb::new(0, 0, 255),     // 0 Base: blue
    Rgb::new(0, 255, 0),     // 1 Lower: green
    Rgb::new(255, 0, 255),   // 2 Magic: magenta
    Rgb::new(255, 0, 0),     // 3 Games: red
    Rgb::new(0, 255, 255),   // 4 Mac Hyper: cyan
    Rgb::new(255, 128, 0),   // 5: amber
    Rgb::new(128, 255, 0),   // 6: chartreuse
    Rgb::new(255, 255, 255), // 7: white
];

/// Dim white for the always-on base record on the thumb cluster.
const BASE_THUMB_COLOR: Rgb = Rgb::new(24, 24, 24);

/// Chain indices accented per layer: the top thumb row (0 1 2) and the inner
/// main-grid column (6-9), i.e. the keys nearest the thumbs.
const LAYER_ACCENT_KEYS: [u8; 7] = [0, 1, 2, 6, 7, 8, 9];

/// Default lighting configuration for both halves.
///
/// - Base (always on): all six thumb keys dim white.
/// - Per-layer accents: [`LAYER_ACCENT_KEYS`] in the active layer's color,
///   replacing the base on keys 0-2 while the bottom thumb row (3-5) keeps
///   showing the base through — composition and reveal are both visible on
///   hardware.
///
/// The host overlay starts empty: it is fed live by the host protocol
/// (`src/host_proto.rs` on the central; the Phase 1 hardcoded placeholder
/// cells are gone).
fn default_compositor() -> Compositor<NUM_LEDS> {
    let mut c = Compositor::new();

    let mut base = Record::new(Activation::Always);
    for key in 0..6u8 {
        // Static counts within fixed capacities; unwrap is unreachable.
        base.set(
            key,
            Cell::Solid {
                color: BASE_THUMB_COLOR,
            },
        )
        .unwrap();
    }
    c.add_record(base).unwrap();

    for (layer, &color) in LAYER_COLORS.iter().enumerate() {
        let mut accents = Record::new(Activation::LayerActive(layer as u8));
        for &key in &LAYER_ACCENT_KEYS {
            accents.set(key, Cell::Solid { color }).unwrap();
        }
        c.add_record(accents).unwrap();
    }

    c
}

/// Event- and deadline-driven lighting task. Registered with RMK via
/// `#[register_processor(event)]`; RMK joins its `process_loop` with the
/// other firmware tasks.
///
/// The loop sleeps until either a `LayerChangeEvent` arrives or the
/// compositor's reported `next_wake` elapses; in both cases it re-renders
/// and writes the frame only if it changed. When the compositor reports no
/// upcoming change (`None`), no timer exists at all.
/// Central-only persistent-config runtime state (Phase 4): the transactional
/// store handle plus the RAM blob buffer (boot load now; host config
/// sessions once the protocol's config commands are wired).
pub struct CentralConfig {
    pub store: crate::config_store::ConfigStore,
    /// Assembly buffer: boot load first, then CONFIG_BEGIN/DATA sessions.
    /// (CONFIG_READ serves the active blob straight from flash instead, so
    /// an open session and a concurrent read cannot collide.)
    pub blob_buf: &'static mut [u8; crate::config_store::CONFIG_BLOB_MAX],
    /// The open transfer session, if any (one per keyboard, both transports).
    pub session: Option<crate::host_proto::ConfigSession>,
}

/// Backing storage for [`CentralConfig::blob_buf`]. Only the central takes
/// it; on the peripheral binary it is dead BSS.
static BLOB_BUF: static_cell::StaticCell<[u8; crate::config_store::CONFIG_BLOB_MAX]> =
    static_cell::StaticCell::new();

pub struct LightingProcessor {
    chain: Ws2812Chain,
    compositor: Compositor<NUM_LEDS>,
    /// `Some` on the central once boot load has run; `None` on the
    /// peripheral (which persists nothing — central is authoritative).
    config: Option<CentralConfig>,
    /// Which split half this is, plus that half's lighting-sync state
    /// (Phase 3): the central's authoritative right-half overlay store and
    /// delta queue, or the peripheral's link-loss bookkeeping.
    role: crate::split_lighting::SplitRole,
    /// Compositor-clock (embassy uptime, ms) deadline for the next
    /// self-driven frame change; `None` = fully static, no timer armed.
    next_wake_ms: Option<u64>,
    /// First moment the chain's 5 V rail is considered stable.
    power_ready_at: Instant,
    /// Chain power enable (active high). Held for the lifetime of the
    /// firmware; dropping it would re-float the enable line.
    _chain_power: Output<'static>,
    /// Rear power-button LED. The PWM peripheral free-runs in hardware; kept
    /// alive here so the pin is not released.
    _status_pwm: SimplePwm<'static>,
}

/// The compositor's abstract clock: embassy uptime in milliseconds.
fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Bring up all lighting hardware for one half and return the processor that
/// drives it.
///
/// Left half: `init(p.SPI3, p.P0_27, p.P0_31, p.PWM0, p.P1_15, SplitRole::central())`.
/// Right half: `init(p.SPI3, p.P0_13, p.P0_19, p.PWM0, p.P0_16, SplitRole::peripheral())`.
pub fn init(
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
    role: crate::split_lighting::SplitRole,
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
        compositor: default_compositor(),
        config: None,
        role,
        next_wake_ms: None,
        power_ready_at,
        _chain_power: chain_power,
        _status_pwm: status_pwm,
    }
}

impl LightingProcessor {
    /// Central boot load (Phase 4): open the transactional config store and
    /// apply the newest valid stored config over the compiled defaults.
    /// Recovery order per design-goals.md: newest valid stored config →
    /// compiled defaults (already installed by [`init`], so every failure
    /// path is simply "change nothing"). Runs before the first frame; the
    /// flash traffic is serviced by the shared-flash task and never touches
    /// the key-scan path.
    async fn boot_load_config(&mut self) {
        if self.role.central_mut().is_none() {
            return;
        }
        let store = crate::config_store::ConfigStore::open().await;
        let blob_buf = BLOB_BUF.init([0; crate::config_store::CONFIG_BLOB_MAX]);
        let mut config = CentralConfig {
            store,
            blob_buf,
            session: None,
        };
        if config.store.active_len().is_some() {
            match config.store.read_active(config.blob_buf).await {
                Ok(len) => {
                    let blob = &config.blob_buf[..len];
                    match crate::lighting_config::apply_blob(
                        &mut self.compositor,
                        &mut self.role,
                        blob,
                        now_ms(),
                    ) {
                        Ok(()) => defmt::info!("lighting: stored config applied at boot"),
                        Err(e) => defmt::warn!(
                            "lighting: stored config rejected, keeping compiled defaults: {}",
                            defmt::Debug2Format(&e)
                        ),
                    }
                }
                Err(e) => defmt::warn!(
                    "lighting: stored config unreadable, keeping compiled defaults: {}",
                    defmt::Debug2Format(&e)
                ),
            }
        }
        self.config = Some(config);
    }

    /// Render at `now`, remember the next self-driven deadline, and write
    /// the frame out only if it differs from the last one on the wire.
    async fn render_at(&mut self, now: u64) {
        let out = self.compositor.render(now);
        self.next_wake_ms = out.next_wake_ms;
        if out.changed {
            self.chain.write(&out.frame).await;
        }
    }

    /// Apply one host-protocol request. This task owns the compositor (and
    /// the split-role state), so ALL host mutation funnels through here; the
    /// request semantics live in [`crate::host_proto::apply`]. The response
    /// is routed back to the requesting transport, then the frame re-renders
    /// immediately so a host write is visible without waiting for another
    /// event.
    async fn process_host_request(&mut self, req: crate::host_proto::HostRequest) {
        let response = if crate::host_proto::is_keymap_request(&req.request) {
            // Keymap requests don't touch the compositor: they are serviced
            // by the keymap's owner (RMK's Vial service task) through the
            // RMK keymap-ops channel, one operation at a time.
            crate::host_proto::apply_keymap(req.request_id, &req.request).await
        } else if crate::host_proto::is_config_request(&req.request) {
            match &mut self.config {
                Some(cfg) => {
                    crate::host_proto::apply_config(
                        &mut self.compositor,
                        &mut self.role,
                        cfg,
                        req.request_id,
                        &req.request,
                        now_ms(),
                    )
                    .await
                }
                // Unreachable: the transport pumps only run on the central,
                // where boot load installed the config state before this
                // loop; answered defensively instead of panicking.
                None => crate::host_proto::apply(
                    &mut self.compositor,
                    &mut self.role,
                    req.request_id,
                    &req.request,
                    now_ms(),
                ),
            }
        } else {
            crate::host_proto::apply(
                &mut self.compositor,
                &mut self.role,
                req.request_id,
                &req.request,
                now_ms(),
            )
        };
        crate::host_proto::respond(req.transport, response).await;
        self.render_at(now_ms()).await;
    }

    /// Apply the central's USB data-connection truth and mirror it to the
    /// peripheral in the existing split State message.
    async fn set_usb_connected(&mut self, connected: bool) {
        if self.role.as_central().is_none() {
            return;
        }
        self.compositor.set_usb_connected(connected);
        let now = now_ms();
        if let Some(split) = self.role.central_mut() {
            split.notify_state(&self.compositor, now);
        }
        self.render_at(now).await;
    }

    /// Apply this half's local nRF POWER/VBUS state (the charging gate).
    async fn set_charging(&mut self, charging: bool) {
        self.compositor.set_charging(charging);
        self.render_at(now_ms()).await;
    }

    /// Apply this half's existing split-app link state (the split-link gate).
    async fn set_split_link(&mut self, up: bool) {
        let now = now_ms();
        self.compositor.set_split_link(up);
        self.role.on_link_change(up, now);
        self.render_at(now).await;
    }

    /// The earliest self-driven wake: the compositor's next frame change or
    /// the split role's next deadline (right-half TTL expiry / resync retry
    /// on the central, link-loss overlay clear on the peripheral).
    fn wake_deadline_ms(&self) -> Option<u64> {
        match (self.next_wake_ms, self.role.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
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
        self.compositor.set_active_layer(event.0);
        self.render_at(now_ms()).await;
    }

    /// Overrides the default loop: one initial frame after the chain rail
    /// settles, then sleep on `select(deadline, next event, next host
    /// request, split-lighting traffic)`. The subscribers are created first
    /// so no early layer change or link edge is lost; when neither the
    /// compositor nor the split role reports anything upcoming, no timer is
    /// armed at all (the no-ticker-when-static guarantee).
    ///
    /// Host requests arrive from the transport pumps (`src/host_pump.rs`)
    /// and split messages from RMK's split app channel; both are applied
    /// HERE, so this task stays the single owner of the compositor and the
    /// split-role state. Per half:
    ///
    /// - Central: the host-request arm fires (pumps run), the app-message
    ///   arm never does; link edges drive the reconnect resync.
    /// - Peripheral: the host-request arm never fires (no pumps), the
    ///   app-message arm applies forwarded overlay/state; link edges drive
    ///   the link-loss grace clear.
    async fn process_loop(&mut self) -> ! {
        let mut sub = Self::subscriber();
        // Watch capacity is 2; each binary takes exactly one receiver.
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("split link watch has a free receiver slot");
        let requests = crate::host_proto::HOST_REQUESTS.receiver();
        let app_rx = rmk::split_app::SPLIT_APP_RX.receiver();
        let remote_boot = REMOTE_BOOT_REQUESTS.receiver();
        let mut connection = ConnectionStatusChangeEvent::subscriber();
        let mut vbus = rmk::usb::USB_VBUS_DETECTED
            .receiver()
            .expect("nRF VBUS watch has its application receiver slot");
        // Central: load the persisted lighting config before the first
        // frame (no-op on the peripheral). Interleaves with the chain
        // power-settle wait below, which usually dominates.
        self.boot_load_config().await;
        Timer::at(self.power_ready_at).await;
        self.render_at(now_ms()).await;
        loop {
            let wake = self.wake_deadline_ms();
            let deadline = async {
                match wake {
                    Some(at) => Timer::at(Instant::from_millis(at)).await,
                    None => core::future::pending::<()>().await,
                }
            };
            match select4(
                deadline,
                sub.next_event(),
                requests.receive(),
                select(
                    select(link.changed(), app_rx.receive()),
                    select(
                        connection.next_event(),
                        select(vbus.changed(), remote_boot.receive()),
                    ),
                ),
            )
            .await
            {
                Either4::First(()) => {
                    // Tick-to-ms floor rounding can fire a hair early; clamp
                    // so both the compositor and the role bookkeeping see the
                    // requested boundary as reached and always make progress.
                    let now = now_ms().max(wake.unwrap_or(0));
                    self.role.service(&mut self.compositor, now);
                    self.render_at(now).await;
                }
                Either4::Second(event) => self.process(event).await,
                Either4::Third(req) => self.process_host_request(req).await,
                Either4::Fourth(Either::First(Either::First(up))) => {
                    // Split-link edge; the next loop iteration re-arms the
                    // deadline this may have created (resync / grace clear).
                    self.set_split_link(up).await;
                }
                Either4::Fourth(Either::First(Either::Second(msg))) => {
                    let now = now_ms();
                    self.role
                        .apply_message(&mut self.compositor, msg.payload(), now);
                    // Drain any burst (e.g. a resync) before rendering so a
                    // full overlay push costs one frame, not one per message.
                    while let Ok(more) = app_rx.try_receive() {
                        self.role
                            .apply_message(&mut self.compositor, more.payload(), now);
                    }
                    self.render_at(now).await;
                }
                Either4::Fourth(Either::Second(Either::First(event))) => {
                    use rmk::types::connection::UsbState;

                    let connected =
                        matches!(event.0.usb, UsbState::Configured | UsbState::Suspended);
                    self.set_usb_connected(connected).await;
                }
                Either4::Fourth(Either::Second(Either::Second(Either::First(charging)))) => {
                    self.set_charging(charging).await;
                }
                Either4::Fourth(Either::Second(Either::Second(Either::Second(())))) => {
                    let dispatched = self
                        .role
                        .central_mut()
                        .is_some_and(|split| split.request_peripheral_bootloader());
                    if !dispatched {
                        defmt::warn!("remote-boot: peripheral is offline or split queue is full");
                    }
                }
            }
        }
    }
}
