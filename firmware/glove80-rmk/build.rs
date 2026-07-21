//! Build script for the Glove80 firmware binaries.
//!
//! The `println!("cargo:...")` lines throughout are not debug output: writing
//! `cargo:` directives to stdout is Cargo's standard build-script interface
//! (link search paths, linker args, rerun conditions, `rustc-env` values all
//! travel this way). Each section below is a self-contained step with its own
//! provenance:
//!
//! - [`link_script_setup`] — the cortex-m-quickstart `memory.x` pattern plus
//!   the linker args this target needs.
//! - [`vial_config_generation`] — RMK's Vial contract: the compressed
//!   keyboard definition and keyboard UID constants Vial expects.
//! - [`version_embedding`] — ours: git/semver build identity served by Rynk's
//!   application-defined `GetBuildInfo` label.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use const_gen::*;
use xz2::read::XzEncoder;

fn main() {
    println!("cargo:rerun-if-changed=keyboard.toml");
    vial_config_generation();
    link_script_setup();
    version_embedding();
}

/// Put `memory.x` where the linker finds it and set the linker arguments.
///
/// This is the standard cortex-m-quickstart pattern: copy `memory.x` into
/// `OUT_DIR` (needed once a workspace is involved, since the linker only
/// searches the search path, not the crate root) and re-run only when it
/// changes.
fn link_script_setup() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // By default, Cargo re-runs a build script whenever any file in the
    // project changes; naming `memory.x` limits that to what matters here.
    println!("cargo:rerun-if-changed=memory.x");

    // `--nmagic` is required if memory section addresses are not aligned to
    // 0x10000, for example the FLASH and RAM sections in `memory.x`.
    // See https://github.com/rust-embedded/cortex-m-quickstart/pull/95
    println!("cargo:rustc-link-arg=--nmagic");
    // The cortex-m-rt link script, then defmt's extra section.
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=-Tdefmt.x");
}

/// Generate the Vial constants RMK's Vial service expects.
///
/// This is RMK's contract (see the `use_config` examples upstream): Vial
/// wants the keyboard definition JSON XZ-compressed plus an 8-byte keyboard
/// UID, exposed as `VIAL_KEYBOARD_DEF` / `VIAL_KEYBOARD_ID` constants
/// included from `OUT_DIR/config_generated.rs`.
fn vial_config_generation() {
    println!("cargo:rerun-if-changed=vial.json");
    let out_file = Path::new(&env::var_os("OUT_DIR").unwrap()).join("config_generated.rs");

    let p = Path::new("vial.json");
    let mut content = String::new();
    match File::open(p) {
        Ok(mut file) => {
            file.read_to_string(&mut content)
                .expect("Cannot read vial.json");
        }
        Err(e) => println!("Cannot find vial.json {:?}: {}", p, e),
    };

    let vial_cfg = json::stringify(json::parse(&content).unwrap());
    let mut keyboard_def_compressed: Vec<u8> = Vec::new();
    XzEncoder::new(vial_cfg.as_bytes(), 6)
        .read_to_end(&mut keyboard_def_compressed)
        .unwrap();

    // Unique Vial keyboard UID for the Glove80 RMK port ("GLV80RMK").
    let keyboard_id: Vec<u8> = vec![0x47, 0x4C, 0x56, 0x38, 0x30, 0x52, 0x4D, 0x4B];
    let const_declarations = [
        const_declaration!(pub VIAL_KEYBOARD_DEF = keyboard_def_compressed),
        const_declaration!(pub VIAL_KEYBOARD_ID = keyboard_id),
    ]
    .map(|s| "#[allow(clippy::redundant_static_lifetimes)]\n".to_owned() + s.as_str())
    .join("\n");
    fs::write(out_file, const_declarations).unwrap();
}

/// Embed this application build's identity for Rynk `GetBuildInfo`.
///
/// The firmware reports its crate semver plus the git state of the build tree.
/// A downstream configuration repository may also provide its full commit and
/// dirty state through `GLOVE80_CONFIG_GIT_COMMIT` and
/// `GLOVE80_CONFIG_GIT_DIRTY`. These `rustc-env` values are composed with RMK's
/// version in `central.rs`:
///
/// - `GLOVE80_GIT_HASH`: `git rev-parse --short=8 HEAD`, exactly 8 ASCII
///   chars (padded with '0' on the right if git ever yields fewer). The
///   literal `unknown0` when git is unavailable.
/// - `GLOVE80_GIT_DIRTY`: `1` if `git status --porcelain` reports any
///   uncommitted change, else `0` (also `0` on the no-git fallback).
/// - `GLOVE80_CONFIG_GIT_HASH`: the first eight hexadecimal characters of the
///   downstream configuration commit, or `standalone` when this repository is
///   built directly.
/// - `GLOVE80_CONFIG_GIT_DIRTY`: normalized to `1` or `0`.
/// - `GLOVE80_RMK_GIT_VERSION`: the pinned RMK submodule's full `git describe`
///   identity (for example `rmk-v0.8.2-837-g566cbcf9`). Release builds pass
///   this explicitly; direct Cargo builds derive it from the submodule.
///
/// The semver travels via Cargo's own `CARGO_PKG_VERSION_*` envs; nothing to
/// emit here. Re-run when the repo's HEAD moves (commit/checkout); a
/// dirty-flag change without a HEAD move is only picked up by the next
/// rebuild that runs this script anyway.
fn version_embedding() {
    println!("cargo:rerun-if-env-changed=GLOVE80_CONFIG_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=GLOVE80_CONFIG_GIT_DIRTY");
    println!("cargo:rerun-if-env-changed=GLOVE80_RMK_GIT_VERSION");

    // Two levels up: <repo root>/.git/HEAD (this crate is firmware/glove80-rmk).
    // HEAD only changes on checkout/branch switch; ordinary commits move the
    // branch ref file instead, so watch that too or the embedded hash goes
    // stale until an unrelated rebuild.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = fs::read_to_string("../../.git/HEAD")
        && let Some(refpath) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=../../.git/{refpath}");
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

    let config_commit = env::var("GLOVE80_CONFIG_GIT_COMMIT").unwrap_or_default();
    let config_hash = if config_commit.is_empty() {
        "standalone".to_owned()
    } else {
        assert!(
            config_commit.len() >= 8 && config_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "GLOVE80_CONFIG_GIT_COMMIT must contain at least eight hexadecimal characters"
        );
        config_commit[..8].to_ascii_lowercase()
    };
    let config_dirty = match env::var("GLOVE80_CONFIG_GIT_DIRTY").as_deref() {
        Ok("1" | "true") => true,
        Ok("0" | "false") | Err(_) => false,
        Ok(value) => panic!("GLOVE80_CONFIG_GIT_DIRTY must be true/false or 1/0, got {value}"),
    };
    println!("cargo:rustc-env=GLOVE80_CONFIG_GIT_HASH={config_hash}");
    println!(
        "cargo:rustc-env=GLOVE80_CONFIG_GIT_DIRTY={}",
        config_dirty as u8
    );

    let rmk_git_version = env::var("GLOVE80_RMK_GIT_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            Command::new("git")
                .args([
                    "-C",
                    "../../dependencies/rmk",
                    "describe",
                    "--tags",
                    "--always",
                    "--dirty",
                ])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|value| value.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    assert!(
        rmk_git_version.len() <= 48 && rmk_git_version.bytes().all(|byte| byte.is_ascii_graphic()),
        "GLOVE80_RMK_GIT_VERSION must be 1-48 printable ASCII characters"
    );
    println!("cargo:rustc-env=GLOVE80_RMK_GIT_VERSION={rmk_git_version}");
}
