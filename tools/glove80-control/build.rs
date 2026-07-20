//! Embed this build's git identity for the `version` verb, exactly the way
//! the firmware embeds its own (see `firmware/glove80-rmk/build.rs`,
//! `version_embedding()`): `GLOVE80_GIT_HASH` is `git rev-parse --short=8
//! HEAD` padded with '0' to exactly 8 ASCII chars (`unknown0` without git),
//! `GLOVE80_GIT_DIRTY` is `1` iff `git status --porcelain` is non-empty.

use std::process::Command;

fn main() {
    // Two levels up: <repo root>/.git/HEAD (this crate is tools/glove80-control).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    // HEAD only moves on checkout; ordinary commits move the branch ref, so
    // watch that too or the embedded hash goes stale (same fix as the
    // firmware's build.rs).
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD") {
        if let Some(refpath) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=../../.git/{refpath}");
        }
    }

    let hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| {
            let mut h: String = s.trim().chars().take(8).collect();
            while h.len() < 8 {
                h.push('0');
            }
            h
        })
        .unwrap_or_else(|| "unknown0".to_string());
    let dirty = if hash == "unknown0" {
        false
    } else {
        Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .is_some_and(|out| !out.stdout.is_empty())
    };
    println!("cargo:rustc-env=GLOVE80_GIT_HASH={hash}");
    println!("cargo:rustc-env=GLOVE80_GIT_DIRTY={}", dirty as u8);
}
