//! Pin the firmware's physical key geometry to the vendor board definition.
//!
//! `[layout].map` in `crates/glove80-rmk/keyboard.toml` is the only source of
//! key centres in the whole stack: the macro bakes them into `PHYSICAL_KEYS`,
//! the lighting topology hands them to `TopologyLayout`, and every spatial
//! effect asks that for "where is this LED". A map that merely enumerates the
//! scan matrix therefore silently teaches the effects a keyboard shape that
//! doesn't exist — matrix columns 6 and 7 are the thumb clusters, which are
//! fans rather than columns.
//!
//! The expected centres below are independently derived from MoErgo's ZMK board
//! definition (`glove80-layouts.dtsi`, whose `key_physical_attrs` rows are
//! ordered by the `glove80.dtsi` matrix transform), converted from centi-units
//! to key units and with each thumb key's pivot sweep resolved into its final
//! centre. That makes this a cross-check against the vendor geometry rather
//! than a restatement of the map.
//!
//! This lives in the host workspace because `glove80-rmk` cross-compiles and
//! cannot run host tests; `rynk-kle` decodes the layout through the same blob
//! path the firmware serves over `GetLayout`.

/// `(row, col, x, y)` in key units. See the module docs for provenance.
const VENDOR_CENTERS: &[(u8, u8, f32, f32)] = &[
    (0, 0, 0.5, 1.0),
    (0, 1, 1.5, 1.0),
    (0, 2, 2.5, 0.5),
    (0, 3, 3.5, 0.5),
    (0, 4, 4.5, 0.5),
    (0, 9, 13.5, 0.5),
    (0, 10, 14.5, 0.5),
    (0, 11, 15.5, 0.5),
    (0, 12, 16.5, 1.0),
    (0, 13, 17.5, 1.0),
    (1, 0, 0.5, 2.0),
    (1, 1, 1.5, 2.0),
    (1, 2, 2.5, 1.5),
    (1, 3, 3.5, 1.5),
    (1, 4, 4.5, 1.5),
    (1, 5, 5.5, 1.5),
    (1, 8, 12.5, 1.5),
    (1, 9, 13.5, 1.5),
    (1, 10, 14.5, 1.5),
    (1, 11, 15.5, 1.5),
    (1, 12, 16.5, 2.0),
    (1, 13, 17.5, 2.0),
    (2, 0, 0.5, 3.0),
    (2, 1, 1.5, 3.0),
    (2, 2, 2.5, 2.5),
    (2, 3, 3.5, 2.5),
    (2, 4, 4.5, 2.5),
    (2, 5, 5.5, 2.5),
    (2, 8, 12.5, 2.5),
    (2, 9, 13.5, 2.5),
    (2, 10, 14.5, 2.5),
    (2, 11, 15.5, 2.5),
    (2, 12, 16.5, 3.0),
    (2, 13, 17.5, 3.0),
    (3, 0, 0.5, 4.0),
    (3, 1, 1.5, 4.0),
    (3, 2, 2.5, 3.5),
    (3, 3, 3.5, 3.5),
    (3, 4, 4.5, 3.5),
    (3, 5, 5.5, 3.5),
    (3, 8, 12.5, 3.5),
    (3, 9, 13.5, 3.5),
    (3, 10, 14.5, 3.5),
    (3, 11, 15.5, 3.5),
    (3, 12, 16.5, 4.0),
    (3, 13, 17.5, 4.0),
    (4, 0, 0.5, 5.0),
    (4, 1, 1.5, 5.0),
    (4, 2, 2.5, 4.5),
    (4, 3, 3.5, 4.5),
    (4, 4, 4.5, 4.5),
    (4, 5, 5.5, 4.5),
    (4, 8, 12.5, 4.5),
    (4, 9, 13.5, 4.5),
    (4, 10, 14.5, 4.5),
    (4, 11, 15.5, 4.5),
    (4, 12, 16.5, 5.0),
    (4, 13, 17.5, 5.0),
    (5, 0, 0.5, 6.0),
    (5, 1, 1.5, 6.0),
    (5, 2, 2.5, 5.5),
    (5, 3, 3.5, 5.5),
    (5, 4, 4.5, 5.5),
    (5, 9, 13.5, 5.5),
    (5, 10, 14.5, 5.5),
    (5, 11, 15.5, 5.5),
    (5, 12, 16.5, 6.0),
    (5, 13, 17.5, 6.0),
    // Left thumb cluster: upper fan (30/45/60°) then lower fan (20/40/60°).
    (0, 6, 6.625, 5.5694),
    (1, 6, 7.5052, 6.2448),
    (2, 6, 8.1806, 7.125),
    (3, 6, 5.6116, 6.196),
    (4, 6, 6.5891, 6.7604),
    (5, 6, 7.3146, 7.625),
    // Right thumb cluster: the mirror image about x = 9.
    (0, 7, 11.375, 5.5694),
    (1, 7, 10.4948, 6.2448),
    (2, 7, 9.8194, 7.125),
    (3, 7, 12.3884, 6.196),
    (4, 7, 11.4109, 6.7604),
    (5, 7, 10.6854, 7.625),
];

const LEFT_THUMBS: &[(u8, u8)] = &[(0, 6), (1, 6), (2, 6), (3, 6), (4, 6), (5, 6)];
const RIGHT_THUMBS: &[(u8, u8)] = &[(0, 7), (1, 7), (2, 7), (3, 7), (4, 7), (5, 7)];

/// The reach of PaletteFx's Reactive bump, on the same 0..=255 grid
/// `TopologyLayout` normalises key centres onto. Reactive halves each axis
/// delta (`abs_half_diff`) before testing against a radius of 21, so a hit
/// lights everything within 42 grid units of it and nothing beyond.
const REACTIVE_REACH: f32 = 42.0;

/// Key centres on the effect grid, reproducing `TopologyLayout::new`: shift to
/// the minimum corner and scale by one shared factor so the longer axis spans
/// 0..=255 and polar effects stay isotropic.
fn effect_grid(centers: &[(u8, u8, f32, f32)]) -> Vec<((u8, u8), (f32, f32))> {
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for &(_, _, x, y) in centers {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let span = (max_x - min_x).max(max_y - min_y);
    centers
        .iter()
        .map(|&(row, col, x, y)| {
            (
                (row, col),
                ((x - min_x) * 255.0 / span, (y - min_y) * 255.0 / span),
            )
        })
        .collect()
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn decoded_centers() -> Vec<(u8, u8, f32, f32)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../glove80-rmk/keyboard.toml");
    let text = std::fs::read_to_string(path).expect("read the firmware keyboard.toml");
    let info = rynk_kle::decode_layout(&text).expect("decode [layout]");
    let variant = &info.variants[info.default_variant as usize];
    variant
        .keys
        .iter()
        .map(|key| (key.row, key.col, key.rect.x, key.rect.y))
        .collect()
}

/// Every key sits where MoErgo's board definition puts it. The tolerance is one
/// step of the 8.8 fixed-point centre the firmware stores, plus rounding.
#[test]
fn key_centers_match_the_vendor_board_definition() {
    let decoded = decoded_centers();
    assert_eq!(decoded.len(), VENDOR_CENTERS.len(), "key count");

    for &(row, col, want_x, want_y) in VENDOR_CENTERS {
        let (_, _, got_x, got_y) = *decoded
            .iter()
            .find(|&&(r, c, _, _)| (r, c) == (row, col))
            .unwrap_or_else(|| panic!("({row},{col}) is missing from [layout].map"));
        let off = distance((got_x, got_y), (want_x, want_y));
        assert!(
            off < 0.01,
            "({row},{col}) is at ({got_x}, {got_y}), vendor geometry says ({want_x}, {want_y})"
        );
    }
}

/// Each thumb cluster is a compact fan, so a hit anywhere in it reaches all six
/// keys. When the map enumerated the scan matrix instead, a cluster was rendered
/// as one full-height column and only matrix-adjacent pairs lit together.
#[test]
fn a_thumb_cluster_lights_as_one_unit() {
    let grid = effect_grid(&decoded_centers());
    let at = |pos| grid.iter().find(|&&(p, _)| p == pos).expect("key").1;

    for cluster in [LEFT_THUMBS, RIGHT_THUMBS] {
        for &a in cluster {
            for &b in cluster {
                let d = distance(at(a), at(b));
                assert!(
                    d <= REACTIVE_REACH,
                    "thumb keys {a:?} and {b:?} are {d} apart on the effect grid, \
                     beyond Reactive's {REACTIVE_REACH}-unit reach"
                );
            }
        }
    }
}

/// The outer key of the left thumb cluster is nowhere near the top-left of the
/// board. The matrix-shaped map put it one key from F5, because collapsing the
/// F-row's two empty positions slid it under the function keys.
#[test]
fn the_thumb_cluster_is_clear_of_the_function_row() {
    let grid = effect_grid(&decoded_centers());
    let at = |pos| grid.iter().find(|&&(p, _)| p == pos).expect("key").1;

    // F5, 4, and 5 — the keys that used to light up on an Escape press.
    for neighbour in [(0, 4), (1, 4), (1, 5)] {
        let d = distance(at((0, 6)), at(neighbour));
        assert!(
            d > REACTIVE_REACH,
            "left thumb (0,6) is {d} from {neighbour:?}, inside Reactive's \
             {REACTIVE_REACH}-unit reach"
        );
    }
}
