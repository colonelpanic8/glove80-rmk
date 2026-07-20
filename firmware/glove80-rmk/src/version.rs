//! This build's identity, embedded by `build.rs` (`version_embedding()`) and
//! served over the host protocol's GET_VERSION (v1.3). Both halves compile
//! this module: the central reports it directly, the peripheral announces it
//! over the split link at link-up (`split_lighting.rs`).

/// Parse a small decimal env literal at compile time (Cargo's
/// `CARGO_PKG_VERSION_*` are decimal strings).
const fn parse_u8(s: &str) -> u8 {
    let bytes = s.as_bytes();
    let mut value: u8 = 0;
    let mut i = 0;
    while i < bytes.len() {
        assert!(
            bytes[i] >= b'0' && bytes[i] <= b'9',
            "version component is not a number"
        );
        value = value * 10 + (bytes[i] - b'0');
        i += 1;
    }
    value
}

/// The firmware crate's semver, from `CARGO_PKG_VERSION`.
pub const FW_MAJOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MAJOR"));
pub const FW_MINOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MINOR"));
pub const FW_PATCH: u8 = parse_u8(env!("CARGO_PKG_VERSION_PATCH"));

/// Git short hash of the build tree, exactly 8 ASCII bytes (`unknown0` when
/// the build had no git available). See `build.rs::version_embedding`.
pub const GIT_HASH: [u8; 8] = {
    let s = env!("GLOVE80_GIT_HASH").as_bytes();
    assert!(
        s.len() == 8,
        "GLOVE80_GIT_HASH must be exactly 8 ASCII chars"
    );
    [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]
};

/// Whether the build tree had uncommitted changes.
pub const GIT_DIRTY: bool = {
    let s = env!("GLOVE80_GIT_DIRTY").as_bytes();
    assert!(
        s.len() == 1 && (s[0] == b'0' || s[0] == b'1'),
        "GLOVE80_GIT_DIRTY must be 0 or 1"
    );
    s[0] == b'1'
};
