//! `version` verb: report this CLI's own build identity plus both keyboard
//! halves' firmware identity over the host protocol (v1.3, GET_VERSION).
//!
//! Rendering is a pure function over the decoded response so it can be
//! unit-tested against the mock transport.

use anyhow::Result;
use glove80_host_protocol::{
    Capabilities, HalfVersion, VersionInfo, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR,
};

use crate::hostproto::HostClient;
use crate::transport::{self, Selector};

/// This CLI's git short hash, embedded by `build.rs` the same way the
/// firmware embeds its own (`unknown0` when built without git).
const CLI_GIT_HASH: &str = env!("GLOVE80_GIT_HASH");
const CLI_GIT_DIRTY: bool = {
    let s = env!("GLOVE80_GIT_DIRTY").as_bytes();
    s.len() == 1 && s[0] == b'1'
};

/// One half's git hash for display: the 8 wire bytes minus right zero
/// padding, with a `-dirty` suffix.
fn hash_display(half: &HalfVersion) -> String {
    let bytes: Vec<u8> = half
        .git_hash
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .collect();
    let hash = match std::str::from_utf8(&bytes) {
        Ok(text) if !text.is_empty() => text.to_string(),
        _ => format!("{:02x?}", half.git_hash),
    };
    if half.dirty { format!("{hash}-dirty") } else { hash }
}

fn half_line(name: &str, half: &HalfVersion) -> String {
    // All-zero fields = this half was never seen since the central booted.
    if *half == HalfVersion::default() {
        return format!("  {name:<12} (never seen since the central booted)");
    }
    let state = if half.present { "connected" } else { "disconnected (last known)" };
    format!(
        "  {name:<12} v{}.{}.{}  {:<17} {state}",
        half.fw_major,
        half.fw_minor,
        half.fw_patch,
        hash_display(half),
    )
}

/// Render the whole `version` report.
fn render(capabilities: &Capabilities, info: &VersionInfo) -> String {
    let mut out = String::new();
    let cli_dirty = if CLI_GIT_DIRTY { "-dirty" } else { "" };
    out.push_str(&format!(
        "glove80-control v{} ({CLI_GIT_HASH}{cli_dirty}), host protocol v{}.{}\n",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_VERSION_MAJOR,
        PROTOCOL_VERSION_MINOR
    ));
    out.push_str("firmware:\n");
    out.push_str(&half_line("central", &info.central));
    out.push('\n');
    out.push_str(&half_line("peripheral", &info.peripheral));
    out.push('\n');
    if info.halves_mismatch {
        out.push_str(
            "\nWARNING: HALVES MISMATCH — the two halves run different firmware builds.\n\
             Flash both halves from the same build (this usually means one half was\n\
             updated and the other was not).\n",
        );
    }
    if (capabilities.protocol_major, capabilities.protocol_minor)
        != (PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR)
    {
        out.push_str(&format!(
            "\nnote: the keyboard speaks host protocol v{}.{} while this CLI speaks \
             v{}.{}.\n",
            capabilities.protocol_major,
            capabilities.protocol_minor,
            PROTOCOL_VERSION_MAJOR,
            PROTOCOL_VERSION_MINOR
        ));
    }
    out
}

/// Query the device and produce the report (transport-independent; used by
/// [`run`] and the mock-transport tests).
fn report(client: &mut HostClient) -> Result<String> {
    let capabilities = client.capabilities()?;
    let info = client.version()?;
    Ok(render(&capabilities, &info))
}

pub fn run(selector: &Selector) -> Result<()> {
    let mut client = HostClient::new(transport::connect(selector)?);
    print!("{}", report(&mut client)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use glove80_host_protocol::{
        feature, Command, Request, Response, ResponsePayload, Status, MAX_MESSAGE_LEN,
    };

    use super::*;
    use crate::transport::mock::MockTransport;

    fn caps(feature_bits: u32, minor: u8) -> Capabilities {
        Capabilities {
            protocol_major: PROTOCOL_VERSION_MAJOR,
            protocol_minor: minor,
            led_count_left: 40,
            led_count_right: 40,
            layer_capacity: 8,
            max_cells_per_op: 80,
            effect_mask: 0b111,
            overlay_cell_capacity: 80,
            max_message_len: MAX_MESSAGE_LEN as u16,
            feature_bits,
            max_config_blob_len: 4096,
            keymap_rows: 6,
            keymap_cols: 14,
            max_keymap_entries_per_op: 84,
        }
    }

    fn half(present: bool, hash: &[u8; 8], dirty: bool) -> HalfVersion {
        HalfVersion {
            present,
            fw_major: 0,
            fw_minor: 1,
            fw_patch: 0,
            git_hash: *hash,
            dirty,
        }
    }

    fn mock(capabilities: Capabilities, info: VersionInfo) -> MockTransport {
        MockTransport::new()
            .expect(move |request_id, _| {
                vec![Response {
                    request_id,
                    command: Command::GetCapabilities,
                    status: Status::Ok,
                    payload: ResponsePayload::Capabilities(capabilities),
                }]
            })
            .expect(move |request_id, request| {
                assert!(matches!(request, Request::GetVersion));
                vec![Response {
                    request_id,
                    command: Command::GetVersion,
                    status: Status::Ok,
                    payload: ResponsePayload::Version(info),
                }]
            })
    }

    #[test]
    fn reports_both_halves() {
        let info = VersionInfo {
            central: half(true, b"1a2b3c4d", false),
            peripheral: half(true, b"1a2b3c4d", false),
            halves_mismatch: false,
        };
        let transport = mock(caps(0x1FF, PROTOCOL_VERSION_MINOR), info);
        let mut client = HostClient::new(Box::new(transport));
        let report = report(&mut client).unwrap();
        assert!(report.contains("glove80-control v"));
        assert!(report.contains("central      v0.1.0  1a2b3c4d          connected"));
        assert!(report.contains("peripheral   v0.1.0  1a2b3c4d          connected"));
        assert!(!report.contains("MISMATCH"));
        assert!(!report.contains("note:"));
    }

    #[test]
    fn warns_on_mismatch_and_shows_dirty() {
        let info = VersionInfo {
            central: half(true, b"1a2b3c4d", true),
            peripheral: half(true, b"9f8e7d6c", false),
            halves_mismatch: true,
        };
        let transport = mock(caps(0x1FF, PROTOCOL_VERSION_MINOR), info);
        let mut client = HostClient::new(Box::new(transport));
        let report = report(&mut client).unwrap();
        assert!(report.contains("1a2b3c4d-dirty"));
        assert!(report.contains("WARNING: HALVES MISMATCH"));
    }

    #[test]
    fn shows_disconnected_and_never_seen_peripherals() {
        let info = VersionInfo {
            central: half(true, b"1a2b3c4d", false),
            peripheral: half(false, b"9f8e7d6c", false),
            halves_mismatch: false,
        };
        let transport = mock(caps(0x1FF, PROTOCOL_VERSION_MINOR), info);
        let mut client = HostClient::new(Box::new(transport));
        let disconnected = report(&mut client).unwrap();
        assert!(disconnected.contains("disconnected (last known)"));

        let info = VersionInfo {
            central: half(true, b"1a2b3c4d", false),
            peripheral: HalfVersion::default(),
            halves_mismatch: false,
        };
        let transport = mock(caps(0x1FF, PROTOCOL_VERSION_MINOR), info);
        let mut client = HostClient::new(Box::new(transport));
        let never_seen = report(&mut client).unwrap();
        assert!(never_seen.contains("never seen since the central booted"));
    }

    #[test]
    fn notes_protocol_minor_skew() {
        // A hypothetical newer device: same feature bit, higher minor.
        let info = VersionInfo {
            central: half(true, b"1a2b3c4d", false),
            peripheral: half(true, b"1a2b3c4d", false),
            halves_mismatch: false,
        };
        let transport = mock(caps(0x1FF, PROTOCOL_VERSION_MINOR + 1), info);
        let mut client = HostClient::new(Box::new(transport));
        let report = report(&mut client).unwrap();
        assert!(report.contains("note: the keyboard speaks host protocol"));
    }

    #[test]
    fn requires_the_feature_bit() {
        // v1.2-shaped device: no VERSION_REPORT bit; GET_VERSION never sent.
        let transport = MockTransport::new().expect(move |request_id, _| {
            vec![Response {
                request_id,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(caps(0xFF, 2)),
            }]
        });
        let mut client = HostClient::new(Box::new(transport));
        let error = report(&mut client).unwrap_err();
        assert!(error.to_string().contains("does not advertise"), "{error}");
        let _ = feature::VERSION_REPORT;
    }
}
