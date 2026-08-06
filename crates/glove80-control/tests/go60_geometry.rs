//! Pin the Go60 firmware's physical key geometry to the editor drawing of the
//! board.
//!
//! `[layout].map` in `crates/go60-rmk/keyboard.toml` is the only source of key
//! centres in the whole stack: the macro bakes them into `PHYSICAL_KEYS`, the
//! lighting topology hands them to `TopologyLayout`, and every spatial effect
//! asks that for "where is this LED". A map that merely enumerates the scan
//! matrix therefore silently teaches the effects a keyboard shape that doesn't
//! exist — the Go60's finger columns are staggered and its thumb keys are an
//! arc, not a straight run.
//!
//! The expected centres below are independently derived from `GO60_GEO_RAW` in
//! moosylog's Glide editor (`basic_edit/glide_v20.html`), which draws the board
//! 17u wide from per-column cells plus a pivot sweep for each thumb arc. This
//! file re-does that derivation rather than restating the map's shape offsets,
//! so the two only agree if the map really places the keys where the board
//! draws them.
//!
//! This lives in the host workspace because `go60-rmk` cross-compiles and
//! cannot run host tests; `rynk-kle` decodes the layout through the same blob
//! path the firmware serves over `GetLayout`.

/// Glide's `GO60_TOTAL_WIDTH`: the right half is the left half reflected in it.
const TOTAL_WIDTH: f32 = 17.0;

/// How far below the row baseline each of a half's six finger columns sits,
/// outer to inner. The middle-finger column is the high point of the stagger.
const COLUMN_DROP: [f32; 6] = [0.9, 0.9, 0.25, 0.0, 0.15, 0.25];

/// Left-half finger columns present in each physical row, outer to inner. The
/// fifth row is just the three middle columns.
const ROW_COLUMNS: [&[u8]; 5] = [
    &[0, 1, 2, 3, 4, 5],
    &[0, 1, 2, 3, 4, 5],
    &[0, 1, 2, 3, 4, 5],
    &[0, 1, 2, 3, 4, 5],
    &[2, 3, 4],
];

/// Every thumb cap starts at this centre, flat, and is swung about the pivot.
const THUMB_FLAT: (f32, f32) = (4.5, 4.75);
const THUMB_PIVOT: (f32, f32) = (4.5, 9.0);
/// Clockwise degrees for the left arc's keys, in matrix-row order.
const THUMB_ANGLES: [f32; 3] = [15.0, 30.0, 45.0];

/// Rotate `point` clockwise about `pivot` (screen axes, y growing downward).
fn swing(point: (f32, f32), pivot: (f32, f32), deg: f32) -> (f32, f32) {
    let (sin, cos) = deg.to_radians().sin_cos();
    let (dx, dy) = (point.0 - pivot.0, point.1 - pivot.1);
    (pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos)
}

/// One key as `((row, col), centre, cap angle)`, in key units and degrees.
type Placed = ((u8, u8), (f32, f32), f32);

/// Where the editor drawing puts every key. See the module docs.
fn editor_keys() -> Vec<Placed> {
    let mut keys = Vec::new();
    for (row, columns) in ROW_COLUMNS.iter().enumerate() {
        for &column in *columns {
            let x = f32::from(column) + 0.5;
            let y = row as f32 + COLUMN_DROP[usize::from(column)] + 0.5;
            keys.push(((row as u8, column), (x, y), 0.0));
            // The right half mirrors the left: matrix column 13 - c, drawn at
            // the reflection of the left cell in the board's width.
            keys.push(((row as u8, 13 - column), (TOTAL_WIDTH - x, y), 0.0));
        }
    }
    for (row, &angle) in THUMB_ANGLES.iter().enumerate() {
        let (x, y) = swing(THUMB_FLAT, THUMB_PIVOT, angle);
        keys.push(((row as u8, 6), (x, y), angle));
        keys.push(((row as u8, 7), (TOTAL_WIDTH - x, y), -angle));
    }
    keys
}

fn decoded_keys() -> Vec<Placed> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../go60-rmk/keyboard.toml");
    let text = std::fs::read_to_string(path).expect("read the firmware keyboard.toml");
    let info = rynk_kle::decode_layout(&text).expect("decode [layout]");
    let variant = &info.variants[info.default_variant as usize];
    variant
        .keys
        .iter()
        .map(|key| ((key.row, key.col), (key.rect.x, key.rect.y), key.r))
        .collect()
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Every key sits where the editor drawing puts it. The tolerance is one step
/// of the 8.8 fixed-point centre the firmware stores, plus rounding.
#[test]
fn key_centers_match_the_editor_board_drawing() {
    let decoded = decoded_keys();
    let expected = editor_keys();
    assert_eq!(decoded.len(), expected.len(), "key count");

    for (at, want, want_angle) in expected {
        let (_, got, got_angle) = *decoded
            .iter()
            .find(|&&(pos, _, _)| pos == at)
            .unwrap_or_else(|| panic!("{at:?} is missing from [layout].map"));
        let off = distance(got, want);
        assert!(
            off < 0.01,
            "{at:?} is at {got:?}, the editor drawing says {want:?}"
        );
        assert!(
            (got_angle - want_angle).abs() < 0.01,
            "{at:?} is tilted {got_angle}°, the editor drawing says {want_angle}°"
        );
    }
}

/// The stagger is what makes a column a column: the middle-finger column is the
/// high point and each half's outer pair sits nearly a full unit below it. A
/// map that lost the stagger would still pass a bounding-box check.
#[test]
fn the_finger_columns_carry_the_boards_stagger() {
    let decoded = decoded_keys();
    let center = |at: (u8, u8)| {
        decoded
            .iter()
            .find(|&&(pos, _, _)| pos == at)
            .unwrap_or_else(|| panic!("{at:?} is missing from [layout].map"))
            .1
    };

    for row in 0..4u8 {
        let middle = center((row, 3)).1;
        for (column, drop) in COLUMN_DROP.iter().enumerate() {
            let column = column as u8;
            for at in [(row, column), (row, 13 - column)] {
                let got = center(at).1 - middle;
                assert!(
                    (got - drop).abs() < 0.01,
                    "{at:?} sits {got}u below its row's middle column, expected {drop}u"
                );
            }
        }
    }
}
