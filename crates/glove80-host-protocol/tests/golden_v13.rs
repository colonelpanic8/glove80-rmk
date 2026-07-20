//! Golden vector suite for the protocol v1.3 additions (build-identity
//! reporting, GET_VERSION).
//!
//! Generates/checks `crates/glove80-host-protocol/vectors/host-protocol-v1.3.json`, consumed by
//! this suite and the TypeScript suite. Regenerate with
//! `GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v13`. The v1.0/v1.1/v1.2
//! vectors are frozen and never change.
//!
//! These vectors are **frozen at v1.3**: the version numbers and feature bits
//! below are literals (not the crate constants) so a later minor bump can
//! never change a v1.3 byte. v1.4 vectors live in `golden_v14.rs` /
//! `host-protocol-v1.4.json`.

mod common;

use common::{message_vectors, Message};
use glove80_host_protocol::{
    Capabilities, Command, HalfVersion, Request, Response, ResponsePayload, Status, VersionInfo,
    MAX_CONFIG_BLOB_LEN, MAX_MESSAGE_LEN,
};
use serde_json::{json, Value};

const FILE_NAME: &str = "host-protocol-v1.3.json";

/// The capability shape the Glove80 firmware advertises at v1.3: all nine
/// feature bits. Bit 8 (version reporting) adds no capability extension, so
/// the payload layout is byte-identical to v1.2 apart from the minor version
/// and the feature bits.
fn glove80_caps() -> Capabilities {
    Capabilities {
        // Frozen v1.3 stamp; see the module comment.
        protocol_major: 1,
        protocol_minor: 3,
        led_count_left: 40,
        led_count_right: 40,
        layer_capacity: 8,
        max_cells_per_op: 80,
        effect_mask: 0b0000_0111,
        overlay_cell_capacity: 80,
        max_message_len: MAX_MESSAGE_LEN as u16,
        feature_bits: 0x1FF,
        max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
        keymap_rows: 6,
        keymap_cols: 14,
        max_keymap_entries_per_op: 84,
    }
}

fn half(present: bool, fw: (u8, u8, u8), git_hash: &[u8; 8], dirty: bool) -> HalfVersion {
    HalfVersion {
        present,
        fw_major: fw.0,
        fw_minor: fw.1,
        fw_patch: fw.2,
        git_hash: *git_hash,
        dirty,
    }
}

fn messages() -> Vec<(&'static str, Message)> {
    use Message::{Req, Resp};

    let version_ok = |id: u8, info: VersionInfo| {
        Resp(Response {
            request_id: id,
            command: Command::GetVersion,
            status: Status::Ok,
            payload: ResponsePayload::Version(info),
        })
    };

    vec![
        (
            "get_capabilities_response_version",
            Resp(Response {
                request_id: 70,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(glove80_caps()),
            }),
        ),
        ("get_version_request", Req(71, Request::GetVersion)),
        (
            // Both halves connected, identical builds, clean trees.
            "get_version_response_both_present",
            version_ok(
                71,
                VersionInfo {
                    central: half(true, (0, 1, 0), b"1a2b3c4d", false),
                    peripheral: half(true, (0, 1, 0), b"1a2b3c4d", false),
                    halves_mismatch: false,
                },
            ),
        ),
        (
            // Both connected but built from different commits (central
            // dirty): the firmware sets halves_mismatch.
            "get_version_response_mismatch",
            version_ok(
                72,
                VersionInfo {
                    central: half(true, (0, 1, 0), b"1a2b3c4d", true),
                    peripheral: half(true, (0, 1, 0), b"9f8e7d6c", false),
                    halves_mismatch: true,
                },
            ),
        ),
        (
            // Split link down: the peripheral entry keeps its last-known
            // fields with present = false; no mismatch is reported.
            "get_version_response_peripheral_disconnected",
            version_ok(
                73,
                VersionInfo {
                    central: half(true, (0, 1, 0), b"1a2b3c4d", false),
                    peripheral: half(false, (0, 1, 0), b"9f8e7d6c", false),
                    halves_mismatch: false,
                },
            ),
        ),
        (
            // Peripheral never seen since boot: all-zero fields. A build
            // without git available reports the literal hash "unknown0".
            "get_version_response_peripheral_never_seen",
            version_ok(
                74,
                VersionInfo {
                    central: half(true, (0, 1, 0), b"unknown0", false),
                    peripheral: HalfVersion::default(),
                    halves_mismatch: false,
                },
            ),
        ),
        (
            // A pre-v1.3 device answers UNKNOWN_COMMAND with an empty
            // payload, per the base protocol rules.
            "get_version_response_unknown_command",
            Resp(Response {
                request_id: 75,
                command: Command::GetVersion,
                status: Status::UnknownCommand,
                payload: ResponsePayload::Empty,
            }),
        ),
    ]
}

fn golden_doc() -> Value {
    json!({
        // Frozen v1.3 stamp; see the module comment.
        "protocol": { "major": 1, "minor": 3 },
        "generatedBy": "crates/glove80-host-protocol tests/golden_v13.rs (GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v13)",
        "messages": message_vectors(&messages()),
    })
}

#[test]
fn golden_file_matches_generator() {
    let doc = golden_doc();
    if common::maybe_write(FILE_NAME, &doc) {
        return;
    }
    assert_eq!(
        doc,
        common::load_file(FILE_NAME),
        "vector file is stale; regenerate with GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v13"
    );
}

#[test]
fn golden_messages_decode() {
    common::assert_messages_decode(&common::load_file(FILE_NAME), &messages());
}
