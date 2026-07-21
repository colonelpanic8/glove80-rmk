//! `keymap …` subcommands: read/write the live keymap through Rynk and search
//! the legacy QMK/VIA keycode table used by the CLI's text format.
//!
//! Everything transport-independent (argument parsing, request shaping,
//! response rendering) is a pure function so it can be unit-tested against
//! the mock transport.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
#[cfg(test)]
use glove80_host_protocol::Capabilities;
use glove80_host_protocol::KeymapEntry;

#[cfg(test)]
use crate::hostproto::HostClient;
use crate::keycodes;
use crate::transport::Selector;

/// Edit the keymap over Rynk (USB HID, serial fallback, or native BLE GATT).
#[derive(Subcommand)]
pub enum KeymapCommand {
    /// Read a layer (default 0) — or every layer — as a keycode grid.
    Read {
        /// Layer to read (default 0).
        #[arg(long, conflicts_with = "all")]
        layer: Option<u8>,
        /// Read every layer the device advertises.
        #[arg(long)]
        all: bool,
        /// Print raw hex u16 VIA keycodes instead of names.
        #[arg(long)]
        raw: bool,
    },
    /// Write one or more keys: LAYER KEY KEYCODE triples.
    ///
    /// KEY is a flat grid index (row*cols + col) or "row,col". KEYCODE is a
    /// QMK-style name (KC_A, MO(2), LT(1,KC_A), LSFT_T(KC_ESC), ...) or a
    /// hex u16 (0x0004). The firmware echoes what it actually stored; any
    /// entry whose read-back differs from the request is flagged as LOSSY.
    Set {
        /// LAYER KEY KEYCODE, repeated: e.g. `0 28 KC_A 1 2,3 MO(2)`.
        #[arg(required = true, value_name = "LAYER KEY KEYCODE")]
        entries: Vec<String>,
    },
    /// Read or set the persistent default layer.
    Default {
        /// New default layer; omit to query it.
        layer: Option<u8>,
    },
    /// Search the keycode name table.
    Find {
        /// Case-insensitive fragment of a keycode name or alias.
        fragment: String,
    },
}

/// Grid positions with no physical key on the Glove80's 6x14 matrix.
#[cfg(test)]
const GLOVE80_HOLES: [u8; 4] = [5, 8, 75, 78];
/// Parse a key position: a flat grid index or "row,col".
pub fn parse_key_position(text: &str, rows: u8, cols: u8) -> Result<u8> {
    let total = u16::from(rows) * u16::from(cols);
    let key = if let Some((row, col)) = text.split_once(',') {
        let row: u16 = row
            .trim()
            .parse()
            .with_context(|| format!("bad row in '{text}'"))?;
        let col: u16 = col
            .trim()
            .parse()
            .with_context(|| format!("bad column in '{text}'"))?;
        if row >= u16::from(rows) || col >= u16::from(cols) {
            bail!("position '{text}' is outside the {rows}x{cols} grid");
        }
        row * u16::from(cols) + col
    } else {
        text.trim()
            .parse()
            .with_context(|| format!("key '{text}' must be a flat index or row,col"))?
    };
    if key >= total {
        bail!(
            "key {key} is out of range (grid has positions 0..{})",
            total - 1
        );
    }
    Ok(key as u8)
}

/// Parse the flat `LAYER KEY KEYCODE …` argument list into entries.
pub fn parse_set_entries(arguments: &[String], rows: u8, cols: u8) -> Result<Vec<KeymapEntry>> {
    if !arguments.len().is_multiple_of(3) {
        bail!(
            "expected LAYER KEY KEYCODE triples, got {} argument(s); e.g. \
             `keymap set 0 28 KC_A 0 2,3 MO(2)`",
            arguments.len()
        );
    }
    let mut entries = Vec::with_capacity(arguments.len() / 3);
    for triple in arguments.chunks(3) {
        let layer: u8 = triple[0]
            .parse()
            .with_context(|| format!("bad layer '{}'", triple[0]))?;
        let key = parse_key_position(&triple[1], rows, cols)?;
        let keycode = keycodes::parse_keycode(&triple[2])?;
        entries.push(KeymapEntry {
            layer,
            key,
            keycode,
        });
    }
    Ok(entries)
}

/// Render one layer as a grid. Holes render as `--` (unless they hold a
/// non-zero code, which is surfaced rather than hidden).
pub fn render_layer(
    layer: u8,
    keycodes_flat: &[u16],
    rows: u8,
    cols: u8,
    holes: &[u8],
    raw: bool,
) -> String {
    let cols = usize::from(cols);
    let cell = |index: usize, code: u16| -> String {
        if holes.contains(&(index as u8)) && code == 0 {
            "--".into()
        } else if raw {
            format!("0x{code:04X}")
        } else {
            keycodes::format_keycode(code)
        }
    };
    let cells: Vec<String> = keycodes_flat
        .iter()
        .enumerate()
        .map(|(index, &code)| cell(index, code))
        .collect();
    let mut widths = vec![0usize; cols];
    for (index, text) in cells.iter().enumerate() {
        let column = index % cols;
        widths[column] = widths[column].max(text.len());
    }
    let mut out = format!("layer {layer} ({rows}x{cols} grid, key = row*{cols} + col):\n");
    for (row_index, row) in cells.chunks(cols).enumerate() {
        let line = row
            .iter()
            .zip(&widths)
            .map(|(text, width)| format!("{text:<width$}"))
            .collect::<Vec<_>>()
            .join("  ");
        out.push_str(&format!("r{row_index}  {}\n", line.trim_end()));
    }
    out.pop();
    out
}

/// Render the outcome of a write: requested vs stored, flagging any entry
/// the firmware could not represent exactly.
pub fn render_write_outcome(entries: &[KeymapEntry], readback: &[u16], cols: u8) -> String {
    let mut out = String::new();
    let mut lossy = 0usize;
    for (entry, &stored) in entries.iter().zip(readback) {
        let row = entry.key / cols;
        let col = entry.key % cols;
        let requested_name = keycodes::format_keycode(entry.keycode);
        let line = if stored == entry.keycode {
            format!(
                "layer {} key {} (r{row},c{col}): {requested_name} (0x{:04X})",
                entry.layer, entry.key, entry.keycode
            )
        } else {
            lossy += 1;
            format!(
                "layer {} key {} (r{row},c{col}): LOSSY — wrote {requested_name} \
                 (0x{:04X}) but the firmware stored {} (0x{stored:04X})",
                entry.layer,
                entry.key,
                entry.keycode,
                keycodes::format_keycode(stored),
            )
        };
        out.push_str(&line);
        out.push('\n');
    }
    if lossy > 0 {
        out.push_str(&format!(
            "{lossy} of {} entr{} stored differently than requested (no exact VIA \
             representation); the stored value is what the keyboard will do",
            entries.len(),
            if entries.len() == 1 {
                "y was"
            } else {
                "ies were"
            },
        ));
    } else {
        out.push_str(&format!(
            "wrote {} entr{} (read-back matches; changes are live and persisted)",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" },
        ));
    }
    out
}

pub fn run(selector: &Selector, command: &KeymapCommand) -> Result<()> {
    // `find` is offline; don't touch the device for it.
    if let KeymapCommand::Find { fragment } = command {
        println!("{}", render_find(fragment));
        return Ok(());
    }
    crate::rynk_client::run_keymap(selector, command)
}

pub fn render_find(fragment: &str) -> String {
    let hits = keycodes::search(fragment);
    if hits.is_empty() {
        return format!(
            "no keycode matches '{fragment}'; composites like MO(n), TG(n), LT(layer, kc), \
             OSM(MOD_LSFT), and mod-taps like LCTL_T(kc) are built from these base names"
        );
    }
    let mut out = String::new();
    for (code, canonical, aliases) in hits {
        out.push_str(&format!("0x{code:04X}  {canonical}"));
        if !aliases.is_empty() {
            out.push_str(&format!("  ({})", aliases.join(", ")));
        }
        out.push('\n');
    }
    out.push_str(
        "composites are also accepted: MO(n), TG(n), TO(n), DF(n), PDF(n), OSL(n), \
         OSM(MOD_…), LT(layer, kc), LM(layer, MOD_…), MT(MOD_…, kc), LCTL_T(kc)-style \
         mod-taps, LSFT(kc)-style modifiers, TD(n), MACRO(n), USER(n)",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glove80_host_protocol::{
        feature, Command, Request, Response, ResponsePayload, Status, MAX_MESSAGE_LEN,
        PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR,
    };

    use crate::transport::mock::MockTransport;

    fn keymap_capabilities() -> Capabilities {
        Capabilities {
            protocol_major: PROTOCOL_VERSION_MAJOR,
            protocol_minor: PROTOCOL_VERSION_MINOR,
            led_count_left: 40,
            led_count_right: 40,
            layer_capacity: 4,
            max_cells_per_op: 8,
            effect_mask: 0b111,
            overlay_cell_capacity: 80,
            max_message_len: MAX_MESSAGE_LEN as u16,
            feature_bits: 0x3F | feature::KEYMAP,
            max_config_blob_len: 0,
            keymap_rows: 6,
            keymap_cols: 14,
            max_keymap_entries_per_op: 32,
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

    #[test]
    fn missing_keymap_feature_is_refused_client_side() {
        let capabilities = Capabilities {
            feature_bits: 0x3F,
            ..keymap_capabilities()
        };
        let mock = MockTransport::new().expect(caps_handler(capabilities));
        let mut client = HostClient::new(Box::new(mock));
        let error = client.read_keymap_layer(0).unwrap_err();
        assert!(error.to_string().contains("keymap editing"), "{error}");
    }

    #[test]
    fn read_chunks_by_advertised_limit_and_reassembles() {
        let mock = MockTransport::new();
        let requests = mock.requests_handle();
        let read_handler = |request_id: u8, request: &Request| {
            let Request::KeymapRead {
                layer,
                start_key,
                max_count,
            } = request
            else {
                panic!("expected KeymapRead, got {request:?}");
            };
            assert_eq!(*layer, 1);
            let count = u16::from(*max_count).min(84 - u16::from(*start_key));
            let keycodes: heapless::Vec<u16, 128> = (0..count)
                .map(|offset| 0x0100 + u16::from(*start_key) + offset)
                .collect();
            vec![Response {
                request_id,
                command: Command::KeymapRead,
                status: Status::Ok,
                payload: ResponsePayload::KeymapActions {
                    layer: *layer,
                    start_key: *start_key,
                    keycodes,
                },
            }]
        };
        let mock = mock
            .expect(caps_handler(keymap_capabilities()))
            .expect(read_handler)
            .expect(read_handler)
            .expect(read_handler);
        let mut client = HostClient::new(Box::new(mock));
        let keycodes = client.read_keymap_layer(1).unwrap();
        assert_eq!(keycodes.len(), 84);
        assert_eq!(keycodes[0], 0x0100);
        assert_eq!(keycodes[83], 0x0100 + 83);

        // max_keymap_entries_per_op = 32 over an 84-key grid: 32 + 32 + 20.
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 4); // caps + three chunks
        for (request, (expected_start, expected_count)) in
            requests[1..].iter().zip([(0u8, 32u8), (32, 32), (64, 20)])
        {
            let Request::KeymapRead {
                start_key,
                max_count,
                ..
            } = request
            else {
                panic!("expected KeymapRead, got {request:?}");
            };
            assert_eq!((*start_key, *max_count), (expected_start, expected_count));
        }
    }

    #[test]
    fn write_round_trips_and_surfaces_lossy_readback() {
        let mock = MockTransport::new()
            .expect(caps_handler(keymap_capabilities()))
            .expect(|request_id, request| {
                let Request::KeymapWrite { entries } = request else {
                    panic!("expected KeymapWrite, got {request:?}");
                };
                assert_eq!(entries.len(), 2);
                assert_eq!(
                    entries[0],
                    KeymapEntry {
                        layer: 0,
                        key: 28,
                        keycode: 0x0004
                    }
                );
                assert_eq!(
                    entries[1],
                    KeymapEntry {
                        layer: 1,
                        key: 31,
                        keycode: 0x52C3
                    }
                );
                // Second entry is stored lossily: TT(3) has no RMK
                // representation and comes back as KC_NO.
                vec![Response {
                    request_id,
                    command: Command::KeymapWrite,
                    status: Status::Ok,
                    payload: ResponsePayload::KeymapWritten {
                        keycodes: heapless::Vec::from_slice(&[0x0004, 0x0000]).unwrap(),
                    },
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        let entries = parse_set_entries(
            &[
                "0".into(),
                "2,0".into(),
                "KC_A".into(),
                "1".into(),
                "31".into(),
                "TT(3)".into(),
            ],
            6,
            14,
        )
        .unwrap();
        let readback = client.write_keymap(&entries).unwrap();
        assert_eq!(readback, vec![0x0004, 0x0000]);

        let rendered = render_write_outcome(&entries, &readback, 14);
        assert!(
            rendered.contains("layer 0 key 28 (r2,c0): KC_A"),
            "{rendered}"
        );
        assert!(rendered.contains("LOSSY"), "{rendered}");
        assert!(rendered.contains("TT(3)"), "{rendered}");
        assert!(rendered.contains("KC_NO"), "{rendered}");
        assert!(rendered.contains("1 of 2"), "{rendered}");
    }

    #[test]
    fn device_out_of_range_write_fails_whole_batch() {
        let mock = MockTransport::new()
            .expect(caps_handler(keymap_capabilities()))
            .expect(|request_id, request| {
                assert!(matches!(request, Request::KeymapWrite { .. }));
                vec![Response {
                    request_id,
                    command: Command::KeymapWrite,
                    status: Status::OutOfRange,
                    payload: ResponsePayload::Empty,
                }]
            });
        let mut client = HostClient::new(Box::new(mock));
        let entries = [KeymapEntry {
            layer: 0,
            key: 10,
            keycode: 0x0004,
        }];
        let error = client.write_keymap(&entries).unwrap_err();
        assert!(error.to_string().contains("OUT_OF_RANGE"), "{error}");
        assert!(error.to_string().contains("all-or-nothing"), "{error}");
    }

    #[test]
    fn client_side_range_validation_never_reaches_the_wire() {
        // No KEYMAP_WRITE handler queued: hitting the wire would panic.
        let mock = MockTransport::new().expect(caps_handler(keymap_capabilities()));
        let mut client = HostClient::new(Box::new(mock));
        let error = client
            .write_keymap(&[KeymapEntry {
                layer: 9,
                key: 0,
                keycode: 4,
            }])
            .unwrap_err();
        assert!(error.to_string().contains("layer 9"), "{error}");
        let error = client
            .write_keymap(&[KeymapEntry {
                layer: 0,
                key: 84,
                keycode: 4,
            }])
            .unwrap_err();
        assert!(error.to_string().contains("key 84"), "{error}");
        let error = client.read_keymap_layer(4).unwrap_err();
        assert!(error.to_string().contains("layer 4"), "{error}");
    }

    #[test]
    fn parses_key_positions_and_triples() {
        assert_eq!(parse_key_position("0", 6, 14).unwrap(), 0);
        assert_eq!(parse_key_position("83", 6, 14).unwrap(), 83);
        assert_eq!(parse_key_position("2,0", 6, 14).unwrap(), 28);
        assert_eq!(parse_key_position("5,13", 6, 14).unwrap(), 83);
        assert!(parse_key_position("84", 6, 14).is_err());
        assert!(parse_key_position("6,0", 6, 14).is_err());
        assert!(parse_key_position("0,14", 6, 14).is_err());
        assert!(parse_key_position("x", 6, 14).is_err());

        let entries = parse_set_entries(
            &[
                "0".into(),
                "28".into(),
                "KC_A".into(),
                "2".into(),
                "1,3".into(),
                "MO(2)".into(),
            ],
            6,
            14,
        )
        .unwrap();
        assert_eq!(
            entries[0],
            KeymapEntry {
                layer: 0,
                key: 28,
                keycode: 0x0004
            }
        );
        assert_eq!(
            entries[1],
            KeymapEntry {
                layer: 2,
                key: 17,
                keycode: 0x5222
            }
        );
        assert!(parse_set_entries(&["0".into(), "28".into()], 6, 14).is_err());
    }

    #[test]
    fn renders_grid_with_holes() {
        let mut flat = vec![0x0004u16; 84]; // KC_A everywhere
        flat[5] = 0x0000; // hole
        flat[8] = 0x0000; // hole
        flat[75] = 0x0000; // hole
        flat[78] = 0x1234; // hole with an unexpected non-zero code
        flat[14] = 0x5222; // MO(2)
        let text = render_layer(0, &flat, 6, 14, &GLOVE80_HOLES, false);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 7); // header + 6 rows
        assert!(lines[0].contains("layer 0"), "{text}");
        assert!(lines[1].contains("--"), "row 0 must mark holes: {text}");
        assert!(lines[2].starts_with("r1  MO(2)"), "{text}");
        // A non-zero code at a hole position must not be hidden.
        assert!(lines[6].contains("RSFT(KC_QUOT)"), "{text}");

        let raw = render_layer(3, &flat, 6, 14, &GLOVE80_HOLES, true);
        assert!(raw.contains("0x0004"), "{raw}");
        assert!(raw.contains("--"), "raw mode still marks holes: {raw}");
    }

    #[test]
    fn find_renders_matches_and_misses() {
        let text = render_find("mply");
        assert!(text.contains("0x00AE"), "{text}");
        assert!(text.contains("KC_MPLY"), "{text}");
        let text = render_find("zzzznothing");
        assert!(text.contains("no keycode matches"), "{text}");
    }
}
