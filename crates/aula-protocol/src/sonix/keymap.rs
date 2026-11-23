//! AULA Nova 75 Pro LED map and physical geometry.
//!
//! The firmware addresses LEDs by a matrix index, `row * 18 + col`
//! (0x00–0x8f). The vendor driver's per-key frame lists exactly the 80 lit
//! keys, always in the same order — physical reading order, row by row, with
//! each row's right-hand keys (Backspace, Del, PgUp, …) taken from further down
//! the matrix. That list is [`SLOTS`]; a frame slot here is a position in it.
//!
//! VERIFIED on hardware: slot 0 (matrix 0x01) is Esc.
//! The rest of the names follow from the order. Two are inferred rather than
//! seen lit and are marked ❓ — confirm them with
//! `cargo run --example smoke -- nova-led <slot>`.

use crate::device::KeyPos;

/// Frame slot -> (matrix index, key name), in the vendor driver's order.
#[rustfmt::skip]
pub const SLOTS: [(u8, &str); 80] = [
    // Row 0
    (0x01, "Esc"), (0x02, "F1"), (0x03, "F2"), (0x04, "F3"), (0x05, "F4"), (0x06, "F5"),
    (0x07, "F6"), (0x08, "F7"), (0x09, "F8"), (0x0a, "F9"), (0x0b, "F10"), (0x0c, "F11"),
    (0x0d, "F12"),
    // Row 1
    (0x13, "`"), (0x14, "1"), (0x15, "2"), (0x16, "3"), (0x17, "4"), (0x18, "5"), (0x19, "6"),
    (0x1a, "7"), (0x1b, "8"), (0x1c, "9"), (0x1d, "0"), (0x1e, "-"), (0x1f, "="),
    (0x67, "Backspace"), (0x77, "Del"),
    // Row 2
    (0x25, "Tab"), (0x26, "Q"), (0x27, "W"), (0x28, "E"), (0x29, "R"), (0x2a, "T"), (0x2b, "Y"),
    (0x2c, "U"), (0x2d, "I"), (0x2e, "O"), (0x2f, "P"), (0x30, "["), (0x31, "]"),
    (0x43, "\\"), (0x76, "PgUp"),
    // Row 3
    (0x37, "Caps"), (0x38, "A"), (0x39, "S"), (0x3a, "D"), (0x3b, "F"), (0x3c, "G"), (0x3d, "H"),
    (0x3e, "J"), (0x3f, "K"), (0x40, "L"), (0x41, ";"), (0x42, "'"),
    (0x55, "Enter"), (0x79, "PgDn"),
    // Row 4
    (0x49, "LShift"), (0x4a, "Z"), (0x4b, "X"), (0x4c, "C"), (0x4d, "V"), (0x4e, "B"),
    (0x4f, "N"), (0x50, "M"), (0x51, ","), (0x52, "."), (0x53, "/"), (0x54, "RShift"),
    (0x65, "Up"), (0x78, "End"),
    // Row 5
    (0x5b, "LCtrl"), (0x5c, "LWin"), (0x5d, "LAlt"), (0x5e, "Space"),
    (0x60, "RAlt"), // ❓
    (0x62, "Fn"),   // ❓
    (0x63, "Left"), (0x64, "Down"), (0x66, "Right"),
];

/// Number of addressable slots in a frame.
pub const LED_COUNT: usize = SLOTS.len();

/// Physical rows: (key name, width in 1u units), left to right. An empty name
/// is a gap.
#[rustfmt::skip]
const ROWS: &[&[(&str, f32)]] = &[
    &[("Esc", 1.0), ("", 0.9), ("F1", 1.0), ("F2", 1.0), ("F3", 1.0), ("F4", 1.0), ("", 0.4), ("F5", 1.0),
      ("F6", 1.0), ("F7", 1.0), ("F8", 1.0), ("", 0.4), ("F9", 1.0), ("F10", 1.0), ("F11", 1.0), ("F12", 1.0)],
    &[("`", 1.0), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0), ("6", 1.0), ("7", 1.0),
      ("8", 1.0), ("9", 1.0), ("0", 1.0), ("-", 1.0), ("=", 1.0), ("Backspace", 2.0), ("Del", 1.0)],
    &[("Tab", 1.5), ("Q", 1.0), ("W", 1.0), ("E", 1.0), ("R", 1.0), ("T", 1.0), ("Y", 1.0), ("U", 1.0),
      ("I", 1.0), ("O", 1.0), ("P", 1.0), ("[", 1.0), ("]", 1.0), ("\\", 1.5), ("PgUp", 1.0)],
    &[("Caps", 1.75), ("A", 1.0), ("S", 1.0), ("D", 1.0), ("F", 1.0), ("G", 1.0), ("H", 1.0), ("J", 1.0),
      ("K", 1.0), ("L", 1.0), (";", 1.0), ("'", 1.0), ("Enter", 2.25), ("PgDn", 1.0)],
    &[("LShift", 2.25), ("Z", 1.0), ("X", 1.0), ("C", 1.0), ("V", 1.0), ("B", 1.0), ("N", 1.0), ("M", 1.0),
      (",", 1.0), (".", 1.0), ("/", 1.0), ("RShift", 1.75), ("Up", 1.0), ("End", 1.0)],
    &[("LCtrl", 1.25), ("LWin", 1.25), ("LAlt", 1.25), ("Space", 6.25), ("RAlt", 1.25),
      ("Fn", 1.25), ("", 0.5), ("Left", 1.0), ("Down", 1.0), ("Right", 1.0)],
];

/// Build the layout: every key that has both a physical position and a slot.
pub fn layout() -> Vec<KeyPos> {
    let mut keys = Vec::new();
    for (row_idx, row) in ROWS.iter().enumerate() {
        let mut x = 0.0f32;
        for (name, w) in row.iter() {
            if let Some(led) = slot_for(name) {
                keys.push(KeyPos {
                    name: (*name).to_string(),
                    row: row_idx as u8,
                    x: x + w / 2.0,
                    w: *w,
                    led,
                });
            }
            x += w;
        }
    }
    keys
}

/// Frame slot for a key name.
pub fn slot_for(name: &str) -> Option<usize> {
    SLOTS.iter().position(|(_, n)| *n == name)
}

/// Matrix index the firmware uses for a frame slot.
pub fn matrix_index(slot: usize) -> Option<u8> {
    SLOTS.get(slot).map(|(i, _)| *i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_is_slot_zero_matrix_one() {
        assert_eq!(slot_for("Esc"), Some(0));
        assert_eq!(matrix_index(0), Some(0x01));
    }

    #[test]
    fn slots_are_the_captured_order() {
        // The id sequence of one per-key frame in 03_esc_blue.pcapng.
        const CAPTURED: &str = "01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 13 14 15 16 17 18 19 \
            1a 1b 1c 1d 1e 1f 67 77 25 26 27 28 29 2a 2b 2c 2d 2e 2f 30 31 43 76 37 38 39 3a \
            3b 3c 3d 3e 3f 40 41 42 55 79 49 4a 4b 4c 4d 4e 4f 50 51 52 53 54 65 78 5b 5c 5d \
            5e 60 62 63 64 66";
        let captured: Vec<u8> = CAPTURED
            .split_whitespace()
            .map(|h| u8::from_str_radix(h, 16).unwrap())
            .collect();
        let ours: Vec<u8> = SLOTS.iter().map(|(i, _)| *i).collect();
        assert_eq!(ours, captured);
    }

    #[test]
    fn matrix_indices_are_unique_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for (i, _) in SLOTS {
            assert!(i <= 0x8f, "{i:#x} is outside the 8x18 matrix");
            assert!(seen.insert(i), "duplicate matrix index {i:#x}");
        }
    }

    #[test]
    fn every_slot_is_placed_exactly_once() {
        let keys = layout();
        assert_eq!(keys.len(), LED_COUNT, "a slot has no physical position");
        let mut leds: Vec<usize> = keys.iter().map(|k| k.led).collect();
        leds.sort_unstable();
        leds.dedup();
        assert_eq!(leds.len(), LED_COUNT, "two keys share a slot");
    }
}
