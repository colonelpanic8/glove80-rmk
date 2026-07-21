//! `lighting …` subcommands and the host-protocol `bootloader` path.
//!
//! Everything transport-independent (argument parsing, request shaping,
//! response rendering) is a pure function so it can be unit-tested against
//! the mock transport.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use glove80_host_protocol::{feature, Capabilities, CellState, CellWrite, Effect, EffectKind};

use crate::hostproto::{effect_name, ApplyOutcome, HostClient};
use crate::transport::Selector;

/// Control the RMK lighting host overlay over USB raw HID or BLE.
#[derive(Subcommand)]
pub enum LightingCommand {
    /// Round-trip a Rynk version query and report the latency.
    Ping {
        /// Optional payload text to echo (up to 64 bytes).
        #[arg(long)]
        data: Option<String>,
    },
    /// Show the device's Rynk lighting capabilities and topology summary.
    Caps,
    /// Set one or more overlay cells to a color, optionally animated.
    ///
    /// KEYS is a comma/range list of key indices, e.g. "3", "0-5,12,40".
    Set {
        /// Key indices: comma-separated, ranges allowed ("0-5,12").
        keys: String,
        /// #RRGGBB, RRGGBB, or a named color (red, green, blue, ...).
        color: String,
        /// Animation applied to every listed key.
        #[arg(long, value_enum, default_value_t = EffectArg::Solid)]
        effect: EffectArg,
        /// Effect period in milliseconds (blink/breathe; default 1000).
        #[arg(long, value_name = "MS")]
        period: Option<u16>,
        /// Effect phase offset in milliseconds (blink/breathe; default 0).
        #[arg(long, value_name = "MS")]
        phase: Option<u16>,
        /// Blink duty cycle percent (default 50).
        #[arg(long, value_name = "PCT")]
        duty: Option<u8>,
        /// TTL in milliseconds for this write; cells revert to transparent
        /// on expiry (default: no TTL).
        #[arg(long, value_name = "MS")]
        ttl: Option<u32>,
    },
    /// Unset (make transparent) one or more overlay cells.
    Unset {
        /// Key indices: comma-separated, ranges allowed ("0-5,12").
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Clear the whole host overlay.
    Clear,
    /// Read authoritative lighting state, including revision and overlay size.
    Read,
    /// Atomically replace the whole overlay from cell-spec lines.
    ///
    /// Reads FILE (or stdin when omitted or "-"). One cell per line:
    /// "KEY COLOR [EFFECT] [period=MS] [phase=MS] [duty=PCT]", e.g.
    /// "12 #ff0000" or "40 00ff00 blink period=750 duty=30". Blank lines
    /// and lines starting with '#' are ignored. An empty spec clears the
    /// overlay.
    Replace {
        /// Cell-spec file; "-" or omitted reads stdin.
        file: Option<PathBuf>,
        /// TTL in milliseconds applied to every cell in the new overlay.
        #[arg(long, value_name = "MS")]
        ttl: Option<u32>,
    },
    /// Get (no argument) or set (0-255) the global brightness scalar.
    Brightness {
        /// New level 0-255; omit to read the current level.
        value: Option<u8>,
    },
    /// Legacy named toggle overlay command (unsupported by RMK lighting).
    Toggle {
        /// Toggle id as configured on the device.
        id: u8,
        /// New state; omit to read the current state.
        #[arg(value_enum)]
        state: Option<ToggleState>,
    },
}

/// Flags for the bootloader verb.
#[derive(Args, Clone, Copy)]
pub struct BootloaderArgs {
    /// Reboot the peripheral half instead of the central.
    #[arg(long)]
    pub peripheral: bool,
    /// Skip the confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EffectArg {
    Solid,
    Blink,
    Breathe,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ToggleState {
    On,
    Off,
}

const NAMED_COLORS: &[(&str, (u8, u8, u8))] = &[
    ("red", (0xFF, 0x00, 0x00)),
    ("green", (0x00, 0xFF, 0x00)),
    ("blue", (0x00, 0x00, 0xFF)),
    ("white", (0xFF, 0xFF, 0xFF)),
    ("black", (0x00, 0x00, 0x00)),
    ("off", (0x00, 0x00, 0x00)),
    ("yellow", (0xFF, 0xFF, 0x00)),
    ("cyan", (0x00, 0xFF, 0xFF)),
    ("magenta", (0xFF, 0x00, 0xFF)),
    ("orange", (0xFF, 0x80, 0x00)),
    ("purple", (0x80, 0x00, 0xFF)),
    ("pink", (0xFF, 0x69, 0xB4)),
];

/// Parse `#RRGGBB`, `0xRRGGBB`, bare `RRGGBB`, or a named color.
pub fn parse_color(text: &str) -> Result<(u8, u8, u8)> {
    let lowered = text.to_ascii_lowercase();
    if let Some((_, rgb)) = NAMED_COLORS.iter().find(|(name, _)| *name == lowered) {
        return Ok(*rgb);
    }
    let hex = lowered
        .strip_prefix('#')
        .or_else(|| lowered.strip_prefix("0x"))
        .unwrap_or(&lowered);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let value = u32::from_str_radix(hex, 16).expect("validated hex");
        return Ok(((value >> 16) as u8, (value >> 8) as u8, value as u8));
    }
    bail!(
        "'{text}' is not a color; use #RRGGBB or one of: {}",
        NAMED_COLORS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Parse a comma/range key list like `0-5,12,40` into indices.
pub fn parse_key_list(text: &str) -> Result<Vec<u8>> {
    let mut keys = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("empty entry in key list '{text}'");
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: u8 = start
                .trim()
                .parse()
                .with_context(|| format!("bad key '{part}'"))?;
            let end: u8 = end
                .trim()
                .parse()
                .with_context(|| format!("bad key '{part}'"))?;
            if end < start {
                bail!("descending range '{part}' in key list");
            }
            keys.extend(start..=end);
        } else {
            keys.push(part.parse().with_context(|| format!("bad key '{part}'"))?);
        }
    }
    Ok(keys)
}

/// Build one effect record from CLI-ish parameters, with defaults and
/// solid-effect parameter rejection.
pub fn build_effect(
    kind: EffectKind,
    rgb: (u8, u8, u8),
    period: Option<u16>,
    phase: Option<u16>,
    duty: Option<u8>,
) -> Result<Effect> {
    let (r, g, b) = rgb;
    if kind == EffectKind::Solid {
        if period.is_some() || phase.is_some() || duty.is_some() {
            bail!("--period/--phase/--duty are only valid with --effect blink|breathe");
        }
        return Ok(Effect::solid(r, g, b));
    }
    let period = period.unwrap_or(1000);
    if period == 0 {
        bail!("period must be at least 1 ms");
    }
    let phase = phase.unwrap_or(0);
    match kind {
        EffectKind::Blink => {
            let duty = duty.unwrap_or(50);
            if duty > 100 {
                bail!("duty must be between 0 and 100 percent");
            }
            Ok(Effect::blink(r, g, b, period, phase, duty))
        }
        EffectKind::Breathe => {
            if duty.is_some() {
                bail!("--duty is only valid with --effect blink");
            }
            Ok(Effect::breathe(r, g, b, period, phase))
        }
        EffectKind::Solid => unreachable!(),
    }
}

impl EffectArg {
    pub(crate) fn kind(self) -> EffectKind {
        match self {
            EffectArg::Solid => EffectKind::Solid,
            EffectArg::Blink => EffectKind::Blink,
            EffectArg::Breathe => EffectKind::Breathe,
        }
    }
}

/// Parse a full replace spec (see `lighting replace --help` for the format).
pub fn parse_replace_spec(text: &str) -> Result<Vec<CellWrite>> {
    let mut cells = Vec::new();
    for (line_number, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        cells.push(
            parse_replace_line(line)
                .with_context(|| format!("line {}: '{line}'", line_number + 1))?,
        );
    }
    Ok(cells)
}

fn parse_replace_line(line: &str) -> Result<CellWrite> {
    let mut tokens = line.split_whitespace();
    let key: u8 = tokens
        .next()
        .context("missing key index")?
        .parse()
        .context("key must be an integer 0-255")?;
    let color = parse_color(tokens.next().context("missing color")?)?;
    let mut kind = EffectKind::Solid;
    let mut period = None;
    let mut phase = None;
    let mut duty = None;
    let mut first = true;
    for token in tokens {
        if first && !token.contains('=') {
            kind = match token {
                "solid" => EffectKind::Solid,
                "blink" => EffectKind::Blink,
                "breathe" => EffectKind::Breathe,
                other => bail!("unknown effect '{other}' (solid, blink, breathe)"),
            };
            first = false;
            continue;
        }
        first = false;
        let (name, value) = token
            .split_once('=')
            .with_context(|| format!("expected name=value, got '{token}'"))?;
        match name {
            "period" => period = Some(value.parse().context("bad period")?),
            "phase" => phase = Some(value.parse().context("bad phase")?),
            "duty" => duty = Some(value.parse().context("bad duty")?),
            other => bail!("unknown parameter '{other}' (period, phase, duty)"),
        }
    }
    Ok(CellWrite {
        key,
        effect: build_effect(kind, color, period, phase, duty)?,
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[allow(dead_code)] // legacy product-protocol renderer
fn format_color(effect: &Effect) -> String {
    format!("#{:02x}{:02x}{:02x}", effect.r, effect.g, effect.b)
}

#[allow(dead_code)] // legacy product-protocol renderer
fn format_ttl(remaining_ms: u32) -> String {
    match remaining_ms {
        0 => "none".into(),
        ms if ms < 1000 => format!("{ms}ms"),
        ms => format!("{:.1}s", f64::from(ms) / 1000.0),
    }
}

/// Render an overlay-write outcome. Pending keys are always surfaced —
/// a partial apply must never look like a full one.
#[allow(dead_code)] // legacy product-protocol renderer
pub fn render_apply(operation: &str, outcome: &ApplyOutcome) -> String {
    if !outcome.partial && outcome.pending_keys.is_empty() {
        return format!("{operation}: applied to both halves");
    }
    let mut text = format!("{operation}: PARTIAL APPLY — peripheral half offline\n");
    text.push_str("  applied on the central half now\n");
    if outcome.pending_keys.is_empty() {
        text.push_str(
            "  pending on the peripheral: the full operation (will sync when it reconnects)",
        );
    } else {
        let keys = outcome
            .pending_keys
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(
            "  pending on the peripheral: keys {keys} (will sync when it reconnects)"
        ));
    }
    text
}

/// Render READ_OVERLAY as a table with remaining TTLs.
#[allow(dead_code)] // legacy product-protocol renderer
pub fn render_overlay(cells: &[CellState]) -> String {
    if cells.is_empty() {
        return "host overlay is empty".into();
    }
    let mut rows = vec![[
        "KEY".to_string(),
        "EFFECT".into(),
        "COLOR".into(),
        "PERIOD".into(),
        "PHASE".into(),
        "DUTY".into(),
        "TTL".into(),
    ]];
    for cell in cells {
        let animated = cell.effect.kind != EffectKind::Solid;
        rows.push([
            cell.key.to_string(),
            effect_name(cell.effect.kind).into(),
            format_color(&cell.effect),
            if animated {
                format!("{}ms", cell.effect.period_ms)
            } else {
                "-".into()
            },
            if animated {
                format!("{}ms", cell.effect.phase_ms)
            } else {
                "-".into()
            },
            if cell.effect.kind == EffectKind::Blink {
                format!("{}%", cell.effect.duty_percent)
            } else {
                "-".into()
            },
            format_ttl(cell.remaining_ttl_ms),
        ]);
    }
    let mut widths = [0usize; 7];
    for row in &rows {
        for (width, column) in widths.iter_mut().zip(row) {
            *width = (*width).max(column.len());
        }
    }
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(widths)
                .map(|(column, width)| format!("{column:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)] // legacy product-protocol renderer
pub fn render_capabilities(capabilities: &Capabilities) -> String {
    let effects = [EffectKind::Solid, EffectKind::Blink, EffectKind::Breathe]
        .iter()
        .filter(|kind| capabilities.effect_mask & (1 << (**kind as u16)) != 0)
        .map(|kind| effect_name(*kind))
        .collect::<Vec<_>>()
        .join(", ");
    let features = [
        (feature::TTL, "per-write TTL"),
        (feature::TOGGLES, "toggles"),
        (feature::BOOTLOADER_ENTRY, "bootloader entry"),
        (feature::ATOMIC_REPLACE, "atomic replace"),
        (feature::OVERLAY_READBACK, "overlay read-back"),
        (feature::PARTIAL_APPLY, "partial-apply reporting"),
    ]
    .iter()
    .filter(|(bit, _)| capabilities.feature_bits & bit != 0)
    .map(|(_, name)| *name)
    .collect::<Vec<_>>()
    .join(", ");
    format!(
        "protocol: v{}.{}\n\
         keys: {} left + {} right\n\
         layer capacity: {}\n\
         max cells per operation: {}\n\
         overlay cell capacity: {}\n\
         max message length: {}\n\
         effects: {effects}\n\
         features: {features}",
        capabilities.protocol_major,
        capabilities.protocol_minor,
        capabilities.led_count_left,
        capabilities.led_count_right,
        capabilities.layer_capacity,
        capabilities.max_cells_per_op,
        capabilities.overlay_cell_capacity,
        capabilities.max_message_len,
    )
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn run(selector: &Selector, command: &LightingCommand) -> Result<()> {
    crate::rynk_client::run_lighting(selector, command)
}

/// Transport-independent dispatch (unit-tested with the mock transport).
#[allow(dead_code)] // legacy product-protocol runner
pub fn run_with_client(client: &mut HostClient, command: &LightingCommand) -> Result<()> {
    match command {
        LightingCommand::Ping { data } => {
            let payload = data.as_deref().unwrap_or("glove80").as_bytes().to_vec();
            let elapsed = client.ping(&payload)?;
            println!(
                "PING {} bytes over {}: {:.1} ms",
                payload.len(),
                client.transport_description(),
                elapsed.as_secs_f64() * 1000.0
            );
        }
        LightingCommand::Caps => {
            let capabilities = client.capabilities()?;
            println!("{}", render_capabilities(&capabilities));
        }
        LightingCommand::Set {
            keys,
            color,
            effect,
            period,
            phase,
            duty,
            ttl,
        } => {
            let keys = parse_key_list(keys)?;
            let effect = build_effect(effect.kind(), parse_color(color)?, *period, *phase, *duty)?;
            let cells: Vec<CellWrite> = keys.iter().map(|&key| CellWrite { key, effect }).collect();
            let outcome = client.set_cells(ttl.unwrap_or(0), &cells)?;
            println!(
                "{}",
                render_apply(&format!("set {} cell(s)", cells.len()), &outcome)
            );
        }
        LightingCommand::Unset { keys } => {
            let mut parsed = Vec::new();
            for list in keys {
                parsed.extend(parse_key_list(list)?);
            }
            let outcome = client.unset_cells(&parsed)?;
            println!(
                "{}",
                render_apply(&format!("unset {} cell(s)", parsed.len()), &outcome)
            );
        }
        LightingCommand::Clear => {
            let outcome = client.clear_overlay()?;
            println!("{}", render_apply("clear overlay", &outcome));
        }
        LightingCommand::Read => {
            let cells = client.read_overlay()?;
            println!("{}", render_overlay(&cells));
        }
        LightingCommand::Replace { file, ttl } => {
            let spec = match file.as_deref() {
                None => read_stdin()?,
                Some(path) if path.as_os_str() == "-" => read_stdin()?,
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("could not read {}", path.display()))?,
            };
            let cells = parse_replace_spec(&spec)?;
            let outcome = client.replace_overlay(ttl.unwrap_or(0), &cells)?;
            println!(
                "{}",
                render_apply(
                    &format!("replace overlay with {} cell(s)", cells.len()),
                    &outcome
                )
            );
        }
        LightingCommand::Brightness { value } => {
            let level = client.brightness(*value)?;
            match value {
                Some(_) => println!("brightness set to {level}"),
                None => println!("brightness: {level}"),
            }
        }
        LightingCommand::Toggle { id, state } => {
            let requested = state.map(|state| state == ToggleState::On);
            let (id, state) = client.toggle(*id, requested)?;
            println!("toggle {id}: {}", if state { "on" } else { "off" });
        }
    }
    Ok(())
}

#[allow(dead_code)] // legacy product-protocol runner
fn read_stdin() -> Result<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut text)
        .context("could not read the cell spec from stdin")?;
    Ok(text)
}

/// Host-protocol bootloader entry with a confirmation prompt.
pub fn run_bootloader(selector: &Selector, peripheral: bool, yes: bool) -> Result<()> {
    let half = if peripheral { "peripheral" } else { "central" };
    if !yes {
        print!("Reboot the {half} half into its UF2 bootloader? [y/N] ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .context("could not read confirmation")?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }
    crate::rynk_client::run_bootloader(selector, peripheral)?;
    println!("{half} half accepted the Rynk bootloader request");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glove80_host_protocol::{
        Command, Request, Response, ResponsePayload, Status, MAX_MESSAGE_LEN,
        PROTOCOL_VERSION_MAJOR,
    };

    use crate::transport::mock::MockTransport;

    fn test_capabilities() -> Capabilities {
        Capabilities {
            protocol_major: PROTOCOL_VERSION_MAJOR,
            protocol_minor: 0,
            led_count_left: 40,
            led_count_right: 40,
            layer_capacity: 8,
            max_cells_per_op: 8,
            effect_mask: 0b111,
            overlay_cell_capacity: 80,
            max_message_len: MAX_MESSAGE_LEN as u16,
            feature_bits: 0x3F,
            max_config_blob_len: 0,
            keymap_rows: 0,
            keymap_cols: 0,
            max_keymap_entries_per_op: 0,
        }
    }

    fn caps_handler(
        capabilities: Capabilities,
    ) -> impl FnMut(u8, &Request) -> Vec<Response> + Send + 'static {
        move |request_id, request| {
            assert!(matches!(request, Request::GetCapabilities { .. }));
            vec![Response {
                request_id,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(capabilities),
            }]
        }
    }

    fn ack(request_id: u8, command: Command, status: Status, pending: &[u8]) -> Response {
        Response {
            request_id,
            command,
            status,
            payload: ResponsePayload::OverlayAck {
                pending_keys: heapless::Vec::from_slice(pending).unwrap(),
            },
        }
    }

    #[test]
    fn ping_round_trips_and_caches_capabilities() {
        // Only ONE capability handler queued: a second GET_CAPABILITIES
        // would panic the mock, proving per-connection caching.
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, request| {
                let Request::Ping { data } = request else {
                    panic!("expected ping")
                };
                vec![Response {
                    request_id,
                    command: Command::Ping,
                    status: Status::Ok,
                    payload: ResponsePayload::Echo { data: data.clone() },
                }]
            })
            .expect(|request_id, request| {
                let Request::Ping { data } = request else {
                    panic!("expected ping")
                };
                vec![Response {
                    request_id,
                    command: Command::Ping,
                    status: Status::Ok,
                    payload: ResponsePayload::Echo { data: data.clone() },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        client.ping(b"hello").unwrap();
        client.ping(b"again").unwrap();
    }

    #[test]
    fn set_builds_correct_request_and_batches() {
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let mock = mock
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| vec![ack(request_id, Command::SetCells, Status::Ok, &[])])
            .expect(|request_id, _| vec![ack(request_id, Command::SetCells, Status::Ok, &[])]);
        let mut client = HostClient::new(Box::new(mock));

        // 10 cells with max_cells_per_op = 8 must split into 8 + 2.
        let effect = Effect::blink(0xFF, 0x00, 0x66, 500, 100, 30);
        let cells: Vec<CellWrite> = (0..10).map(|key| CellWrite { key, effect }).collect();
        let outcome = client.set_cells(2500, &cells).unwrap();
        assert_eq!(outcome, ApplyOutcome::default());

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3); // caps + two batches
        let Request::SetCells {
            ttl_ms,
            cells: first,
        } = &requests[1]
        else {
            panic!("expected SetCells, got {:?}", requests[1]);
        };
        assert_eq!(*ttl_ms, 2500);
        assert_eq!(first.len(), 8);
        assert_eq!(first[0], CellWrite { key: 0, effect });
        let Request::SetCells { cells: second, .. } = &requests[2] else {
            panic!("expected SetCells, got {:?}", requests[2]);
        };
        assert_eq!(second.len(), 2);
        assert_eq!(second[1].key, 9);
    }

    #[test]
    fn overlay_write_can_be_queried_back() {
        let effect = Effect::solid(1, 2, 3);
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(move |request_id, request| {
                let Request::SetCells { ttl_ms, cells } = request else {
                    panic!("expected SetCells, got {request:?}");
                };
                assert_eq!(*ttl_ms, 5_000);
                assert_eq!(cells.as_slice(), &[CellWrite { key: 7, effect }]);
                vec![ack(request_id, Command::SetCells, Status::Ok, &[])]
            })
            .expect(move |request_id, request| {
                assert_eq!(*request, Request::ReadOverlay);
                vec![Response {
                    request_id,
                    command: Command::ReadOverlay,
                    status: Status::Ok,
                    payload: ResponsePayload::OverlayState {
                        cells: heapless::Vec::from_slice(&[CellState {
                            key: 7,
                            effect,
                            remaining_ttl_ms: 4_900,
                        }])
                        .unwrap(),
                    },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));

        assert_eq!(
            client
                .set_cells(5_000, &[CellWrite { key: 7, effect }])
                .unwrap(),
            ApplyOutcome::default()
        );
        assert_eq!(
            client.read_overlay().unwrap(),
            vec![CellState {
                key: 7,
                effect,
                remaining_ttl_ms: 4_900,
            }]
        );
    }

    #[test]
    fn partial_apply_is_surfaced_with_pending_keys() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| {
                vec![ack(
                    request_id,
                    Command::SetCells,
                    Status::PartialApply,
                    &[41, 42],
                )]
            });
        let mut client = HostClient::new(Box::new(mock));
        let cells = [
            CellWrite {
                key: 41,
                effect: Effect::solid(1, 2, 3),
            },
            CellWrite {
                key: 42,
                effect: Effect::solid(1, 2, 3),
            },
        ];
        let outcome = client.set_cells(0, &cells).unwrap();
        assert!(outcome.partial);
        assert_eq!(outcome.pending_keys, vec![41, 42]);

        // The rendered message must show the pending keys, not hide them.
        let rendered = render_apply("set 2 cell(s)", &outcome);
        assert!(rendered.contains("PARTIAL APPLY"), "{rendered}");
        assert!(rendered.contains("41, 42"), "{rendered}");
        assert!(rendered.contains("peripheral"), "{rendered}");
    }

    #[test]
    fn partial_clear_with_no_keys_is_still_surfaced() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, request| {
                assert!(matches!(request, Request::ClearOverlay));
                vec![ack(
                    request_id,
                    Command::ClearOverlay,
                    Status::PartialApply,
                    &[],
                )]
            });
        let mut client = HostClient::new(Box::new(mock));
        let outcome = client.clear_overlay().unwrap();
        assert!(outcome.partial);
        let rendered = render_apply("clear overlay", &outcome);
        assert!(rendered.contains("PARTIAL APPLY"), "{rendered}");
        assert!(rendered.contains("full operation"), "{rendered}");
    }

    #[test]
    fn unadvertised_effect_is_rejected_client_side() {
        // Device advertises solid + blink only; no SET_CELLS handler is
        // queued, so reaching the wire would panic.
        let capabilities = Capabilities {
            effect_mask: 0b011,
            ..test_capabilities()
        };
        let mock = MockTransport::new().expect(caps_handler(capabilities));
        let mut client = HostClient::new(Box::new(mock));
        let cells = [CellWrite {
            key: 0,
            effect: Effect::breathe(0, 0, 255, 3000, 0),
        }];
        let error = client.set_cells(0, &cells).unwrap_err();
        assert!(error.to_string().contains("breathe"), "{error}");
    }

    #[test]
    fn out_of_range_key_and_ttl_feature_are_validated() {
        let capabilities = Capabilities {
            feature_bits: 0x3F & !feature::TTL,
            ..test_capabilities()
        };
        let mock = MockTransport::new().expect(caps_handler(capabilities));
        let mut client = HostClient::new(Box::new(mock));
        let cells = [CellWrite {
            key: 80,
            effect: Effect::solid(1, 1, 1),
        }];
        let error = client.set_cells(0, &cells).unwrap_err();
        assert!(error.to_string().contains("out of range"), "{error}");
        let cells = [CellWrite {
            key: 0,
            effect: Effect::solid(1, 1, 1),
        }];
        let error = client.set_cells(1000, &cells).unwrap_err();
        assert!(error.to_string().contains("TTL"), "{error}");
    }

    #[test]
    fn replace_refuses_to_split_batches() {
        let mock = MockTransport::new().expect(caps_handler(test_capabilities()));
        let mut client = HostClient::new(Box::new(mock));
        let cells: Vec<CellWrite> = (0..9)
            .map(|key| CellWrite {
                key,
                effect: Effect::solid(9, 9, 9),
            })
            .collect();
        let error = client.replace_overlay(0, &cells).unwrap_err();
        assert!(error.to_string().contains("atomic"), "{error}");
    }

    #[test]
    fn stray_responses_are_ignored_until_the_correlated_one() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| {
                vec![
                    // Wrong request id, then wrong command, then the real ack.
                    ack(
                        request_id.wrapping_add(7),
                        Command::SetCells,
                        Status::Ok,
                        &[],
                    ),
                    Response {
                        request_id,
                        command: Command::GetBrightness,
                        status: Status::Ok,
                        payload: ResponsePayload::Brightness { level: 1 },
                    },
                    ack(request_id, Command::SetCells, Status::Ok, &[]),
                ]
            });
        let mut client = HostClient::new(Box::new(mock));
        let cells = [CellWrite {
            key: 3,
            effect: Effect::solid(0, 255, 0),
        }];
        let outcome = client.set_cells(0, &cells).unwrap();
        assert_eq!(outcome, ApplyOutcome::default());
    }

    #[test]
    fn error_statuses_render_readably() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, _| {
                vec![Response {
                    request_id,
                    command: Command::SetCells,
                    status: Status::CapacityExceeded,
                    payload: ResponsePayload::Empty,
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        let cells = [CellWrite {
            key: 0,
            effect: Effect::solid(1, 1, 1),
        }];
        let error = client.set_cells(0, &cells).unwrap_err();
        assert!(error.to_string().contains("CAPACITY_EXCEEDED"), "{error}");
    }

    #[test]
    fn brightness_and_toggle_changes_can_be_queried_back() {
        let mock = MockTransport::new()
            .expect(caps_handler(test_capabilities()))
            .expect(|request_id, request| {
                assert_eq!(*request, Request::SetBrightness { level: 128 });
                vec![Response {
                    request_id,
                    command: Command::SetBrightness,
                    status: Status::Ok,
                    payload: ResponsePayload::Brightness { level: 128 },
                }]
            })
            .expect(|request_id, request| {
                assert_eq!(*request, Request::GetBrightness);
                vec![Response {
                    request_id,
                    command: Command::GetBrightness,
                    status: Status::Ok,
                    payload: ResponsePayload::Brightness { level: 128 },
                }]
            })
            .expect(|request_id, request| {
                assert_eq!(*request, Request::SetToggle { id: 2, state: true });
                vec![Response {
                    request_id,
                    command: Command::SetToggle,
                    status: Status::Ok,
                    payload: ResponsePayload::Toggle { id: 2, state: true },
                }]
            })
            .expect(|request_id, request| {
                assert_eq!(*request, Request::GetToggle { id: 2 });
                vec![Response {
                    request_id,
                    command: Command::GetToggle,
                    status: Status::Ok,
                    payload: ResponsePayload::Toggle { id: 2, state: true },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        assert_eq!(client.brightness(Some(128)).unwrap(), 128);
        assert_eq!(client.brightness(None).unwrap(), 128);
        assert_eq!(client.toggle(2, Some(true)).unwrap(), (2, true));
        assert_eq!(client.toggle(2, None).unwrap(), (2, true));
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#ff0066").unwrap(), (0xFF, 0x00, 0x66));
        assert_eq!(parse_color("FF0066").unwrap(), (0xFF, 0x00, 0x66));
        assert_eq!(parse_color("0x00ff00").unwrap(), (0x00, 0xFF, 0x00));
        assert_eq!(parse_color("red").unwrap(), (0xFF, 0x00, 0x00));
        assert_eq!(parse_color("Orange").unwrap(), (0xFF, 0x80, 0x00));
        assert!(parse_color("nope").is_err());
        assert!(parse_color("#ff00").is_err());
    }

    #[test]
    fn parses_key_lists() {
        assert_eq!(parse_key_list("3").unwrap(), vec![3]);
        assert_eq!(
            parse_key_list("0-3,12,40-41").unwrap(),
            vec![0, 1, 2, 3, 12, 40, 41]
        );
        assert!(parse_key_list("5-2").is_err());
        assert!(parse_key_list("a").is_err());
        assert!(parse_key_list("1,,2").is_err());
    }

    #[test]
    fn parses_replace_specs() {
        let spec = "\
# comment line
12 #ff0000

40 00ff00 blink period=750 duty=30
41 blue breathe period=3000 phase=1500
";
        let cells = parse_replace_spec(spec).unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(
            cells[0],
            CellWrite {
                key: 12,
                effect: Effect::solid(0xFF, 0, 0)
            }
        );
        assert_eq!(
            cells[1],
            CellWrite {
                key: 40,
                effect: Effect::blink(0, 0xFF, 0, 750, 0, 30)
            }
        );
        assert_eq!(
            cells[2],
            CellWrite {
                key: 41,
                effect: Effect::breathe(0, 0, 0xFF, 3000, 1500)
            }
        );
        assert!(parse_replace_spec("12").is_err());
        assert!(parse_replace_spec("12 red wobble").is_err());
        assert!(parse_replace_spec("12 red duty=200").is_err());
        assert!(parse_replace_spec("12 red breathe duty=10").is_err());
    }

    #[test]
    fn renders_overlay_table_with_ttls() {
        let cells = [
            CellState {
                key: 12,
                effect: Effect::solid(0xFF, 0, 0),
                remaining_ttl_ms: 0,
            },
            CellState {
                key: 60,
                effect: Effect::breathe(0, 0, 0xFF, 3000, 1500),
                remaining_ttl_ms: 4200,
            },
            CellState {
                key: 61,
                effect: Effect::blink(0, 0xFF, 0, 500, 0, 30),
                remaining_ttl_ms: 750,
            },
        ];
        let table = render_overlay(&cells);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(
            lines[0].contains("KEY") && lines[0].contains("TTL"),
            "{table}"
        );
        assert!(
            lines[1].contains("#ff0000") && lines[1].contains("none"),
            "{table}"
        );
        assert!(
            lines[2].contains("breathe") && lines[2].contains("4.2s"),
            "{table}"
        );
        assert!(
            lines[3].contains("30%") && lines[3].contains("750ms"),
            "{table}"
        );
        assert_eq!(render_overlay(&[]), "host overlay is empty");
    }

    #[test]
    fn renders_capabilities() {
        let text = render_capabilities(&test_capabilities());
        assert!(text.contains("v1.0"), "{text}");
        assert!(text.contains("40 left + 40 right"), "{text}");
        assert!(text.contains("solid, blink, breathe"), "{text}");
        assert!(text.contains("atomic replace"), "{text}");
    }

    #[test]
    fn builds_effects_with_defaults_and_rejections() {
        assert_eq!(
            build_effect(EffectKind::Blink, (1, 2, 3), None, None, None).unwrap(),
            Effect::blink(1, 2, 3, 1000, 0, 50)
        );
        assert!(build_effect(EffectKind::Solid, (1, 2, 3), Some(100), None, None).is_err());
        assert!(build_effect(EffectKind::Breathe, (1, 2, 3), None, None, Some(10)).is_err());
        assert!(build_effect(EffectKind::Blink, (1, 2, 3), Some(0), None, None).is_err());
        assert!(build_effect(EffectKind::Blink, (1, 2, 3), None, None, Some(101)).is_err());
    }
}
