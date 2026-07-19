use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

mod config;
mod hostproto;
mod lightcfg;
mod lighting;
pub mod runtime_manifest;
mod transport;

const SOF: u8 = 0xab;
const ESC: u8 = 0xac;
const EOF: u8 = 0xad;
const DEFAULT_DEVICE: &str = "/dev/ttyACM0";

#[derive(Parser)]
#[command(about = "Control Glove80 host extensions (ZMK Studio serial, or the RMK host \
                   protocol over USB raw HID / BLE for `lighting` and `bootloader`)")]
struct Cli {
    /// Device to talk to. Legacy commands: a serial port (default
    /// /dev/ttyACM0). `lighting`/host-protocol `bootloader`: a
    /// /dev/hidraw* path or a BLE address (AA:BB:CC:DD:EE:FF).
    #[arg(long, global = true)]
    device: Option<PathBuf>,

    /// Use the USB raw-HID transport (host-protocol commands only).
    #[arg(long, global = true, conflicts_with = "ble")]
    usb: bool,

    /// Use the BLE transport (host-protocol commands only). Default is
    /// auto: USB when present, BLE otherwise.
    #[arg(long, global = true)]
    ble: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage canonical configuration: keymap schema validation, and the
    /// persistent lighting config (apply/export/show/validate over the
    /// host protocol v1.1).
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Show firmware lighting capabilities.
    Capabilities,
    /// Set every key to one color.
    All {
        color: Color,
        #[arg(long)]
        timeout_ms: Option<u32>,
        #[arg(long)]
        batch_size: Option<usize>,
    },
    /// Set one or more indexed keys.
    Set {
        #[arg(required = true, value_name = "INDEX=RRGGBB")]
        pixels: Vec<Pixel>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        timeout_ms: Option<u32>,
        #[arg(long)]
        batch_size: Option<usize>,
    },
    /// Animate one indexed key.
    Effect {
        index: u32,
        mode: EffectMode,
        color: Color,
        #[arg(long, default_value_t = 1500)]
        period_ms: u32,
        #[arg(long, default_value_t = 0)]
        phase_ms: u32,
        #[arg(long, default_value_t = 50)]
        duty_percent: u32,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        timeout_ms: Option<u32>,
    },
    /// Release host control and restore firmware lighting.
    Clear,
    /// Reboot a half into its UF2 bootloader.
    ///
    /// With a positional TARGET (left/right) — or bare — this uses the
    /// legacy ZMK Studio serial path. With --peripheral, --yes, --usb,
    /// --ble, or a host-protocol --device it sends ENTER_BOOTLOADER over
    /// the RMK host protocol instead (central half unless --peripheral).
    Bootloader {
        /// Legacy ZMK Studio serial target (defaults to left when the
        /// legacy path is used).
        #[arg(value_enum)]
        target: Option<Half>,
        #[command(flatten)]
        host: lighting::BootloaderArgs,
    },
    /// Control the RMK lighting host overlay over USB raw HID or BLE.
    Lighting {
        #[command(subcommand)]
        command: lighting::LightingCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Parse and semantically validate a configuration file, offline.
    ///
    /// A `.json` file is checked against the canonical runtime keymap
    /// schema. Anything else is treated as a persistent *lighting* config:
    /// canonical TOML (see `examples/lighting-default.toml`) or a raw
    /// config blob, validated with the exact checks the firmware runs.
    Validate {
        path: PathBuf,
        /// Validate against a target firmware's total layer capacity
        /// (canonical keymap JSON only).
        #[arg(long, value_name = "COUNT")]
        layer_capacity: Option<usize>,
    },
    /// Transactionally apply a persistent lighting config to the keyboard
    /// (host protocol v1.1: CONFIG_BEGIN → CONFIG_DATA… → CONFIG_COMMIT).
    ///
    /// FILE is canonical TOML (start from `examples/lighting-default.toml`)
    /// or a raw config blob (detected by content or a `.bin` extension).
    /// The device activates and persists either the complete new config or
    /// keeps the old one — never a hybrid. Comments in the TOML are lost
    /// if you later re-export from the device.
    Apply {
        file: PathBuf,
        /// Validate and print the summary without touching the device.
        #[arg(long)]
        dry_run: bool,
    },
    /// Export the keyboard's active lighting config to a file.
    ///
    /// Writes canonical TOML by default (comments and toggle names from
    /// your original file are not stored on the device and will be
    /// absent), or the raw byte-stable blob with --raw.
    Export {
        file: PathBuf,
        /// Write the raw config blob instead of TOML.
        #[arg(long)]
        raw: bool,
    },
    /// Read the keyboard's active lighting config and print a summary.
    Show,
}

#[derive(Clone, Copy, ValueEnum)]
enum Half {
    Left,
    Right,
}

impl Half {
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum EffectMode {
    Static,
    Blink,
    Breathe,
}

impl EffectMode {
    fn protocol_value(self) -> u32 {
        match self {
            Self::Static => 0,
            Self::Blink => 1,
            Self::Breathe => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Blink => "blink",
            Self::Breathe => "breathe",
        }
    }
}

#[derive(Clone, Copy)]
struct Color(u32);

impl FromStr for Color {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let text = value
            .strip_prefix('#')
            .or_else(|| value.strip_prefix("0x"))
            .unwrap_or(value);
        if text.len() != 6 {
            return Err("color must be a six-digit RGB value such as ff0066".into());
        }
        u32::from_str_radix(text, 16)
            .map(Self)
            .map_err(|_| "color must contain only hexadecimal digits".into())
    }
}

#[derive(Clone, Copy)]
struct Pixel {
    index: u32,
    color: Color,
}

impl FromStr for Pixel {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let (index, color) = value
            .split_once('=')
            .ok_or_else(|| "pixel must use INDEX=RRGGBB, such as 12=ff0066".to_string())?;
        Ok(Self {
            index: index
                .parse()
                .map_err(|_| "pixel index must be a non-negative integer".to_string())?,
            color: color.parse()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Capabilities {
    protocol_version: u32,
    pixel_count: u32,
    pixels_per_half: u32,
    max_updates_per_request: u32,
    max_update_hz: u32,
    default_timeout_ms: u32,
    max_timeout_ms: u32,
    max_channel_value: u32,
    supports_replace: bool,
    supports_split: bool,
    supports_effects: bool,
    min_effect_period_ms: u32,
    max_effect_period_ms: u32,
    effect_time_quantum_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseKind {
    Capabilities,
    Set,
    Effect,
    Clear,
    Bootloader,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResponseValue {
    Capabilities(Capabilities),
    Integer(u32),
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_uint_field(output: &mut Vec<u8>, field: u32, value: u32) {
    push_varint(output, (field << 3) as u64);
    push_varint(output, value as u64);
}

fn push_message_field(output: &mut Vec<u8>, field: u32, value: &[u8]) {
    push_varint(output, ((field << 3) | 2) as u64);
    push_varint(output, value.len() as u64);
    output.extend_from_slice(value);
}

fn studio_request(request_id: u32, subsystem: u32, request: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    push_uint_field(&mut output, 1, request_id);
    push_message_field(&mut output, subsystem, request);
    output
}

fn capabilities_request(request_id: u32) -> Vec<u8> {
    let mut request = Vec::new();
    push_uint_field(&mut request, 1, 1);
    studio_request(request_id, 6, &request)
}

fn clear_request(request_id: u32) -> Vec<u8> {
    let mut request = Vec::new();
    push_uint_field(&mut request, 3, 1);
    studio_request(request_id, 6, &request)
}

fn bootloader_request(request_id: u32, target: Half) -> Vec<u8> {
    let mut request = Vec::new();
    push_uint_field(&mut request, 1, matches!(target, Half::Right) as u32);
    studio_request(request_id, 7, &request)
}

fn set_pixels_request(
    request_id: u32,
    pixels: &[Pixel],
    replace: bool,
    timeout_ms: u32,
) -> Vec<u8> {
    let mut body = Vec::new();
    for pixel in pixels {
        let mut update = Vec::new();
        push_uint_field(&mut update, 1, pixel.index);
        push_uint_field(&mut update, 2, pixel.color.0);
        push_message_field(&mut body, 1, &update);
    }
    if replace {
        push_uint_field(&mut body, 2, 1);
    }
    if timeout_ms > 0 {
        push_uint_field(&mut body, 3, timeout_ms);
    }
    let mut request = Vec::new();
    push_message_field(&mut request, 2, &body);
    studio_request(request_id, 6, &request)
}

#[derive(Clone, Copy)]
struct Effect {
    index: u32,
    color: Color,
    mode: EffectMode,
    period_ms: u32,
    phase_ms: u32,
    duty_percent: u32,
}

fn set_effects_request(
    request_id: u32,
    effects: &[Effect],
    replace: bool,
    timeout_ms: u32,
) -> Vec<u8> {
    let mut body = Vec::new();
    for effect in effects {
        let mut update = Vec::new();
        push_uint_field(&mut update, 1, effect.index);
        push_uint_field(&mut update, 2, effect.color.0);
        if effect.mode.protocol_value() > 0 {
            push_uint_field(&mut update, 3, effect.mode.protocol_value());
        }
        if effect.period_ms > 0 {
            push_uint_field(&mut update, 4, effect.period_ms);
        }
        if effect.phase_ms > 0 {
            push_uint_field(&mut update, 5, effect.phase_ms);
        }
        if effect.duty_percent > 0 {
            push_uint_field(&mut update, 6, effect.duty_percent);
        }
        push_message_field(&mut body, 1, &update);
    }
    if replace {
        push_uint_field(&mut body, 2, 1);
    }
    if timeout_ms > 0 {
        push_uint_field(&mut body, 3, timeout_ms);
    }
    let mut request = Vec::new();
    push_message_field(&mut request, 4, &body);
    studio_request(request_id, 6, &request)
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(payload.len() + 2);
    output.push(SOF);
    for byte in payload {
        if matches!(*byte, SOF | ESC | EOF) {
            output.push(ESC);
        }
        output.push(*byte);
    }
    output.push(EOF);
    output
}

#[derive(Default)]
struct FrameDecoder {
    active: bool,
    escaped: bool,
    data: Vec<u8>,
}

impl FrameDecoder {
    fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        for byte in chunk {
            if !self.active {
                if *byte == SOF {
                    self.active = true;
                    self.escaped = false;
                    self.data.clear();
                }
                continue;
            }
            if self.escaped {
                self.data.push(*byte);
                self.escaped = false;
            } else if *byte == ESC {
                self.escaped = true;
            } else if *byte == EOF {
                frames.push(std::mem::take(&mut self.data));
                self.active = false;
            } else if *byte == SOF {
                self.escaped = false;
                self.data.clear();
            } else {
                self.data.push(*byte);
            }
        }
        frames
    }
}

#[derive(Debug)]
enum FieldValue {
    Integer(u64),
    Bytes(Vec<u8>),
}

fn read_varint(data: &[u8], position: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    while shift < 70 && *position < data.len() {
        let byte = data[*position];
        *position += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
    bail!("invalid protobuf varint")
}

fn fields(data: &[u8]) -> Result<Vec<(u32, FieldValue)>> {
    let mut position = 0;
    let mut result = Vec::new();
    while position < data.len() {
        let tag = read_varint(data, &mut position)?;
        let field = (tag >> 3) as u32;
        match tag & 7 {
            0 => result.push((
                field,
                FieldValue::Integer(read_varint(data, &mut position)?),
            )),
            1 => {
                position = position
                    .checked_add(8)
                    .context("protobuf position overflow")?;
            }
            2 => {
                let length = read_varint(data, &mut position)? as usize;
                let end = position
                    .checked_add(length)
                    .context("protobuf length overflow")?;
                if end > data.len() {
                    bail!("truncated protobuf field");
                }
                result.push((field, FieldValue::Bytes(data[position..end].to_vec())));
                position = end;
            }
            5 => {
                position = position
                    .checked_add(4)
                    .context("protobuf position overflow")?;
            }
            wire => bail!("unsupported protobuf wire type {wire}"),
        }
        if position > data.len() {
            bail!("truncated protobuf field");
        }
    }
    Ok(result)
}

fn integer_fields(data: &[u8]) -> Result<std::collections::HashMap<u32, u32>> {
    Ok(fields(data)?
        .into_iter()
        .filter_map(|(field, value)| match value {
            FieldValue::Integer(value) => Some((field, value as u32)),
            FieldValue::Bytes(_) => None,
        })
        .collect())
}

fn decode_capabilities(payload: &[u8]) -> Result<Capabilities> {
    let value = integer_fields(payload)?;
    Ok(Capabilities {
        protocol_version: value.get(&1).copied().unwrap_or_default(),
        pixel_count: value.get(&2).copied().unwrap_or_default(),
        pixels_per_half: value.get(&3).copied().unwrap_or_default(),
        max_updates_per_request: value.get(&4).copied().unwrap_or_default(),
        max_update_hz: value.get(&5).copied().unwrap_or_default(),
        default_timeout_ms: value.get(&6).copied().unwrap_or_default(),
        max_timeout_ms: value.get(&7).copied().unwrap_or_default(),
        max_channel_value: value.get(&8).copied().unwrap_or_default(),
        supports_replace: value.get(&9) == Some(&1),
        supports_split: value.get(&10) == Some(&1),
        supports_effects: value.get(&11) == Some(&1),
        min_effect_period_ms: value.get(&12).copied().unwrap_or_default(),
        max_effect_period_ms: value.get(&13).copied().unwrap_or_default(),
        effect_time_quantum_ms: value.get(&14).copied().unwrap_or_default(),
    })
}

fn decode_response(payload: &[u8]) -> Result<Option<(u32, ResponseKind, ResponseValue)>> {
    let request_response = fields(payload)?.into_iter().find_map(|(field, value)| {
        if field == 1 {
            if let FieldValue::Bytes(bytes) = value {
                return Some(bytes);
            }
        }
        None
    });
    let Some(request_response) = request_response else {
        return Ok(None);
    };

    let mut request_id = 0;
    let mut subsystem_response = None;
    for (field, value) in fields(&request_response)? {
        match value {
            FieldValue::Integer(value) if field == 1 => request_id = value as u32,
            FieldValue::Bytes(bytes) if matches!(field, 2 | 6 | 7) => {
                subsystem_response = Some((field, bytes));
            }
            _ => {}
        }
    }
    let Some((subsystem, response)) = subsystem_response else {
        return Ok(Some((
            request_id,
            ResponseKind::Unknown,
            ResponseValue::Integer(u32::MAX),
        )));
    };

    if subsystem == 2 {
        let value = integer_fields(&response)?
            .get(&2)
            .copied()
            .unwrap_or(u32::MAX);
        return Ok(Some((
            request_id,
            ResponseKind::Error,
            ResponseValue::Integer(value),
        )));
    }
    if subsystem == 7 {
        let value = integer_fields(&response)?
            .get(&1)
            .copied()
            .unwrap_or(u32::MAX);
        return Ok(Some((
            request_id,
            ResponseKind::Bootloader,
            ResponseValue::Integer(value),
        )));
    }
    for (field, value) in fields(&response)? {
        match (field, value) {
            (1, FieldValue::Bytes(bytes)) => {
                return Ok(Some((
                    request_id,
                    ResponseKind::Capabilities,
                    ResponseValue::Capabilities(decode_capabilities(&bytes)?),
                )));
            }
            (2, FieldValue::Integer(value)) => {
                return Ok(Some((
                    request_id,
                    ResponseKind::Set,
                    ResponseValue::Integer(value as u32),
                )));
            }
            (3, FieldValue::Integer(value)) => {
                return Ok(Some((
                    request_id,
                    ResponseKind::Clear,
                    ResponseValue::Integer(value as u32),
                )));
            }
            (4, FieldValue::Integer(value)) => {
                return Ok(Some((
                    request_id,
                    ResponseKind::Effect,
                    ResponseValue::Integer(value as u32),
                )));
            }
            _ => {}
        }
    }
    Ok(Some((
        request_id,
        ResponseKind::Unknown,
        ResponseValue::Integer(u32::MAX),
    )))
}

struct SerialClient {
    port: Box<dyn serialport::SerialPort>,
    request_id: u32,
    response_timeout: Duration,
    decoder: FrameDecoder,
}

impl SerialClient {
    fn open(device: &Path) -> Result<Self> {
        let port = serialport::new(device.to_string_lossy(), 115_200)
            .timeout(Duration::from_millis(25))
            .open()
            .with_context(|| {
                format!(
                    "could not open {}; ensure it exists and this login has serial access (normally through dialout)",
                    device.display()
                )
            })?;
        port.clear(serialport::ClearBuffer::All)
            .context("could not flush the Studio serial device")?;
        Ok(Self {
            port,
            request_id: 1,
            response_timeout: Duration::from_secs(2),
            decoder: FrameDecoder::default(),
        })
    }

    fn call(
        &mut self,
        build_request: impl FnOnce(u32) -> Vec<u8>,
    ) -> Result<(ResponseKind, ResponseValue)> {
        let expected_id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        self.port
            .write_all(&encode_frame(&build_request(expected_id)))
            .context("could not write to the keyboard")?;
        self.port
            .flush()
            .context("could not flush the keyboard request")?;

        let deadline = Instant::now() + self.response_timeout;
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            match self.port.read(&mut buffer) {
                Ok(0) => continue,
                Ok(length) => {
                    for incoming in self.decoder.feed(&buffer[..length]) {
                        if let Some((request_id, kind, value)) = decode_response(&incoming)? {
                            if request_id == expected_id {
                                return Ok((kind, value));
                            }
                        }
                    }
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(error) => return Err(error).context("could not read from the keyboard"),
            }
        }
        bail!("keyboard did not respond to Studio request {expected_id}")
    }

    fn capabilities(&mut self) -> Result<Capabilities> {
        match self.call(capabilities_request)? {
            (ResponseKind::Capabilities, ResponseValue::Capabilities(capabilities))
                if matches!(capabilities.protocol_version, 1 | 2) =>
            {
                Ok(capabilities)
            }
            (ResponseKind::Capabilities, ResponseValue::Capabilities(capabilities)) => {
                bail!(
                    "unsupported host-lighting protocol version {}",
                    capabilities.protocol_version
                )
            }
            _ => bail!("keyboard does not expose the host-lighting protocol"),
        }
    }

    fn set_pixels(&mut self, pixels: &[Pixel], replace: bool, timeout_ms: u32) -> Result<()> {
        let (kind, value) = self.call(|id| set_pixels_request(id, pixels, replace, timeout_ms))?;
        expect_apply_result(kind, ResponseKind::Set, value, "lighting")
    }

    fn set_effects(&mut self, effects: &[Effect], replace: bool, timeout_ms: u32) -> Result<()> {
        let (kind, value) =
            self.call(|id| set_effects_request(id, effects, replace, timeout_ms))?;
        expect_apply_result(kind, ResponseKind::Effect, value, "effect")
    }

    fn clear(&mut self) -> Result<()> {
        let (kind, value) = self.call(clear_request)?;
        expect_apply_result(kind, ResponseKind::Clear, value, "clear")
    }

    fn enter_bootloader(&mut self, target: Half) -> Result<()> {
        match self.call(|id| bootloader_request(id, target))? {
            (ResponseKind::Bootloader, ResponseValue::Integer(1)) => Ok(()),
            (_, value) => bail!(
                "keyboard rejected {} bootloader request: {value:?}",
                target.name()
            ),
        }
    }
}

fn expect_apply_result(
    actual: ResponseKind,
    expected: ResponseKind,
    value: ResponseValue,
    operation: &str,
) -> Result<()> {
    let result = match value {
        ResponseValue::Integer(value) if actual == expected => value,
        value => bail!("unexpected {operation} response: {actual:?} {value:?}"),
    };
    if result == 0 {
        return Ok(());
    }
    let description = match result {
        1 => "invalid pixel",
        2 => "partial update",
        3 => "right half unavailable",
        4 => "internal error",
        5 => "invalid effect",
        _ => "unknown error",
    };
    bail!("keyboard rejected {operation} update: {description} ({result})")
}

fn scale_color(color: Color, maximum: u32) -> Color {
    let channels = [
        (color.0 >> 16) & 0xff,
        (color.0 >> 8) & 0xff,
        color.0 & 0xff,
    ];
    let peak = channels.into_iter().max().unwrap_or_default();
    if peak == 0 || peak <= maximum {
        return color;
    }
    let scale = |channel: u32| (channel * maximum + peak / 2) / peak;
    Color((scale(channels[0]) << 16) | (scale(channels[1]) << 8) | scale(channels[2]))
}

fn validated_timeout(argument: Option<u32>, capabilities: &Capabilities) -> Result<u32> {
    let value = argument.unwrap_or(capabilities.default_timeout_ms);
    if value > capabilities.max_timeout_ms {
        bail!(
            "timeout must be between 0 and {} ms",
            capabilities.max_timeout_ms
        );
    }
    Ok(value)
}

fn quantized_time(value: u32, quantum: u32) -> u32 {
    ((value + quantum / 2) / quantum) * quantum
}

fn send_pixels(
    client: &mut SerialClient,
    capabilities: &Capabilities,
    mut pixels: Vec<Pixel>,
    replace: bool,
    timeout_ms: u32,
    requested_batch_size: Option<usize>,
) -> Result<()> {
    if pixels
        .iter()
        .any(|pixel| pixel.index >= capabilities.pixel_count)
    {
        bail!(
            "pixel indices must be between 0 and {}",
            capabilities.pixel_count - 1
        );
    }
    if capabilities.max_updates_per_request == 0 {
        bail!("firmware reported an invalid update limit");
    }
    for pixel in &mut pixels {
        pixel.color = scale_color(pixel.color, capabilities.max_channel_value);
    }
    let maximum = capabilities.max_updates_per_request as usize;
    let batch_size = requested_batch_size.unwrap_or(maximum);
    if batch_size == 0 || batch_size > maximum {
        bail!("batch size must be between 1 and {maximum}");
    }
    let delay = (capabilities.max_update_hz > 0)
        .then(|| Duration::from_secs_f64(1.0 / capabilities.max_update_hz as f64));
    let batch_count = pixels.chunks(batch_size).len();
    for (batch_index, batch) in pixels.chunks(batch_size).enumerate() {
        client.set_pixels(batch, replace && batch_index == 0, timeout_ms)?;
        if batch_index + 1 < batch_count {
            if let Some(delay) = delay {
                thread::sleep(delay);
            }
        }
    }
    println!("Updated {} key LEDs", pixels.len());
    Ok(())
}

fn print_capabilities(value: &Capabilities) {
    println!("protocol_version: {}", value.protocol_version);
    println!("pixel_count: {}", value.pixel_count);
    println!("pixels_per_half: {}", value.pixels_per_half);
    println!("max_updates_per_request: {}", value.max_updates_per_request);
    println!("max_update_hz: {}", value.max_update_hz);
    println!("default_timeout_ms: {}", value.default_timeout_ms);
    println!("max_timeout_ms: {}", value.max_timeout_ms);
    println!("max_channel_value: {}", value.max_channel_value);
    println!("supports_replace: {}", value.supports_replace);
    println!("supports_split: {}", value.supports_split);
    println!("supports_effects: {}", value.supports_effects);
    println!("min_effect_period_ms: {}", value.min_effect_period_ms);
    println!("max_effect_period_ms: {}", value.max_effect_period_ms);
    println!("effect_time_quantum_ms: {}", value.effect_time_quantum_ms);
}

fn hostproto_selector(cli: &Cli) -> transport::Selector {
    let preference = if cli.usb {
        transport::Preference::Usb
    } else if cli.ble {
        transport::Preference::Ble
    } else {
        transport::Preference::Auto
    };
    transport::Selector {
        preference,
        device: cli
            .device
            .as_ref()
            .map(|device| device.to_string_lossy().into_owned()),
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Command::Config { command } = &cli.command {
        match command {
            ConfigCommand::Validate {
                path,
                layer_capacity,
            } => {
                // Canonical keymap JSON keeps the legacy path; everything
                // else is a persistent lighting config (TOML or raw blob).
                let is_json = path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
                if is_json {
                    let configuration = config::read_and_validate(path, *layer_capacity)?;
                    println!(
                        "Valid schema v{} configuration: {} layers, {} lighting layers, {} toggles",
                        configuration.schema_version,
                        configuration.layers.len(),
                        configuration.lighting_layers.len(),
                        configuration.toggles.len()
                    );
                    return Ok(());
                }
                if layer_capacity.is_some() {
                    bail!("--layer-capacity applies only to canonical keymap JSON files");
                }
                return lightcfg::run_validate(path);
            }
            ConfigCommand::Apply { file, dry_run } => {
                return lightcfg::run_apply(&hostproto_selector(&cli), file, *dry_run);
            }
            ConfigCommand::Export { file, raw } => {
                return lightcfg::run_export(&hostproto_selector(&cli), file, *raw);
            }
            ConfigCommand::Show => {
                return lightcfg::run_show(&hostproto_selector(&cli));
            }
        }
    }

    if let Command::Lighting { command } = &cli.command {
        return lighting::run(&hostproto_selector(&cli), command);
    }

    let serial_device = cli
        .device
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DEVICE));

    if let Command::Bootloader { target, host } = &cli.command {
        // Host-protocol path when any of its flags/transports are selected;
        // otherwise the legacy ZMK Studio serial path, unchanged.
        let hostproto = host.peripheral
            || host.yes
            || cli.usb
            || cli.ble
            || cli
                .device
                .as_ref()
                .is_some_and(|device| transport::is_hostproto_device(&device.to_string_lossy()));
        if hostproto {
            if target.is_some() {
                bail!("positional left/right targets belong to the legacy serial path; \
                       use --peripheral to target the peripheral half");
            }
            return lighting::run_bootloader(&hostproto_selector(&cli), host.peripheral, host.yes);
        }
        let target = target.unwrap_or(Half::Left);
        let mut client = SerialClient::open(&serial_device)?;
        client.enter_bootloader(target)?;
        println!("{} bootloader request accepted", target.name());
        return Ok(());
    }

    let mut client = SerialClient::open(&serial_device)?;

    let capabilities = client.capabilities()?;
    match cli.command {
        Command::Capabilities => print_capabilities(&capabilities),
        Command::Clear => {
            client.clear()?;
            println!("Firmware lighting restored");
        }
        Command::All {
            color,
            timeout_ms,
            batch_size,
        } => {
            let pixels = (0..capabilities.pixel_count)
                .map(|index| Pixel { index, color })
                .collect();
            let timeout = validated_timeout(timeout_ms, &capabilities)?;
            send_pixels(
                &mut client,
                &capabilities,
                pixels,
                true,
                timeout,
                batch_size,
            )?;
        }
        Command::Set {
            pixels,
            replace,
            timeout_ms,
            batch_size,
        } => {
            let timeout = validated_timeout(timeout_ms, &capabilities)?;
            send_pixels(
                &mut client,
                &capabilities,
                pixels,
                replace,
                timeout,
                batch_size,
            )?;
        }
        Command::Effect {
            index,
            mode,
            color,
            period_ms,
            phase_ms,
            duty_percent,
            replace,
            timeout_ms,
        } => {
            if !capabilities.supports_effects || capabilities.effect_time_quantum_ms == 0 {
                bail!("keyboard firmware does not support per-key effects");
            }
            if index >= capabilities.pixel_count {
                bail!(
                    "pixel index must be between 0 and {}",
                    capabilities.pixel_count - 1
                );
            }
            let period_ms = quantized_time(period_ms, capabilities.effect_time_quantum_ms);
            if mode.protocol_value() > 0
                && !(capabilities.min_effect_period_ms..=capabilities.max_effect_period_ms)
                    .contains(&period_ms)
            {
                bail!(
                    "period must be between {} and {} ms",
                    capabilities.min_effect_period_ms,
                    capabilities.max_effect_period_ms
                );
            }
            if !(1..100).contains(&duty_percent) {
                bail!("duty percent must be between 1 and 99");
            }
            let phase_ms = if mode.protocol_value() == 0 {
                0
            } else {
                quantized_time(phase_ms, capabilities.effect_time_quantum_ms) % period_ms
            };
            let timeout = validated_timeout(timeout_ms, &capabilities)?;
            client.set_effects(
                &[Effect {
                    index,
                    color: scale_color(color, capabilities.max_channel_value),
                    mode,
                    period_ms,
                    phase_ms,
                    duty_percent,
                }],
                replace,
                timeout,
            )?;
            println!(
                "Applied {} effect to LED {} (period {} ms)",
                mode.name(),
                index,
                period_ms
            );
        }
        Command::Bootloader { .. } => unreachable!(),
        Command::Config { .. } => unreachable!(),
        Command::Lighting { .. } => unreachable!(),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_escapes_reserved_bytes() {
        let payload = [1, SOF, 2, ESC, 3, EOF, 4];
        let encoded = encode_frame(&payload);
        let mut decoder = FrameDecoder::default();
        assert_eq!(decoder.feed(&encoded), vec![payload]);
    }

    #[test]
    fn protocol_requests_match_browser_codec() {
        assert_eq!(
            capabilities_request(7),
            [0x08, 0x07, 0x32, 0x02, 0x08, 0x01]
        );
        assert_eq!(clear_request(9), [0x08, 0x09, 0x32, 0x02, 0x18, 0x01]);
        assert_eq!(
            set_pixels_request(
                3,
                &[Pixel {
                    index: 40,
                    color: Color(0x12ab34),
                }],
                true,
                5000,
            ),
            [
                0x08, 0x03, 0x32, 0x0f, 0x12, 0x0d, 0x0a, 0x06, 0x08, 0x28, 0x10, 0xb4, 0xd6, 0x4a,
                0x10, 0x01, 0x18, 0x88, 0x27,
            ]
        );
    }

    #[test]
    fn effect_request_matches_browser_codec() {
        assert_eq!(
            set_effects_request(
                4,
                &[Effect {
                    index: 40,
                    color: Color(0x12ab34),
                    mode: EffectMode::Breathe,
                    period_ms: 1500,
                    phase_ms: 100,
                    duty_percent: 50,
                }],
                true,
                5000,
            ),
            [
                0x08, 0x04, 0x32, 0x18, 0x22, 0x16, 0x0a, 0x0f, 0x08, 0x28, 0x10, 0xb4, 0xd6, 0x4a,
                0x18, 0x02, 0x20, 0xdc, 0x0b, 0x28, 0x64, 0x30, 0x32, 0x10, 0x01, 0x18, 0x88, 0x27,
            ]
        );
    }

    #[test]
    fn scales_bright_colors_without_changing_hue() {
        assert_eq!(scale_color(Color(0xff_80_00), 96).0, 0x60_30_00);
        assert_eq!(scale_color(Color(0x20_10_00), 96).0, 0x20_10_00);
    }
}
