//! Two-colour (red/black) raster encoding.
//!
//! On two-colour thermal stock the printer develops a different colour depending
//! on burn energy, and the protocol carries that as **two interleaved bitplanes**
//! rather than one — each plane burned at its own trim from
//! [`Density`](crate::buffer::Density). Unlike the per-band density in
//! [`buffer::split_into_banded_buffers`](crate::buffer::split_into_banded_buffers),
//! which only varies energy along the feed axis, this selects colour **per dot**.
//!
//! Recovered from `ImgConverter.GetBytes(int, int, int, List<Color>)` in the
//! vendor Android app, which switches on the colour list having 2 entries.
//!
//! Note the vendor gates this off in the shipped build: `PrintPageData.colors`
//! is read but never populated, and `DeviceManager.isTwoColorDevice()` excludes
//! T50 Pro units whose Bluetooth name contains `A` or `B`. The firmware support
//! is real; whether a given unit honours it is an empirical question.

/// Luminance at or above which a pixel is blank — the vendor's `threshold`,
/// default 125.
pub const BLANK_THRESHOLD: u8 = 125;

/// Luminance at or below which an inked pixel burns black rather than red.
///
/// Placed so that pure red — luma `255 * 0.3 = 76` — lands on the red side.
/// It is a luminance cut, not a hue test: any mid-grey in the 48..125 window
/// becomes red, so callers should quantise to two ink colours before encoding
/// rather than relying on this to separate an arbitrary image.
pub const BLACK_THRESHOLD: u8 = 48;

/// Which bitplane an inked pixel belongs to.
///
/// Plane order on the wire is red first, then black, matching the vendor's
/// `luma > 48` branch writing to the even (plane 0) slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    Red = 0,
    Black = 1,
}

/// The vendor's luma: `0.3 R + 0.59 G + 0.11 B`, truncated.
///
/// Deliberately not Rec. 601/709 — these are the coefficients the firmware's
/// threshold pair was chosen against, so changing them would move where pure
/// red falls relative to [`BLACK_THRESHOLD`].
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    let y = (r as f32) * 0.3 + (g as f32) * 0.59 + (b as f32) * 0.11;
    y as u8
}

/// Classify a pixel by luminance. `None` means no ink.
pub fn classify(luma: u8) -> Option<Plane> {
    if luma >= BLANK_THRESHOLD {
        None
    } else if luma > BLACK_THRESHOLD {
        Some(Plane::Red)
    } else {
        Some(Plane::Black)
    }
}

/// Interleave two equal-length column-major planes into the wire layout.
///
/// Both inputs are `cols * per_line_byte` bytes in the same column-major
/// LSB-first packing the mono path uses. The output places each column's red
/// line immediately before its black line, so the printer sees `cols * 2`
/// lines of `per_line_byte` each — which is exactly why the buffer header
/// advertises a doubled column count.
///
/// Returns `None` if the planes disagree in length or aren't a whole number of
/// columns.
pub fn interleave_planes(
    red: &[u8],
    black: &[u8],
    per_line_byte: usize,
    cols: usize,
) -> Option<Vec<u8>> {
    let expected = cols.checked_mul(per_line_byte)?;
    if red.len() != expected || black.len() != expected {
        return None;
    }

    let mut out = Vec::with_capacity(expected * 2);
    for col in 0..cols {
        let range = col * per_line_byte..(col + 1) * per_line_byte;
        out.extend_from_slice(&red[range.clone()]);
        out.extend_from_slice(&black[range]);
    }
    Some(out)
}

/// Split an RGB image into the two column-major bitplanes the printer expects.
///
/// `rgb` is row-major `width * height * 3`. Output planes are column-major
/// LSB-first with `height` columns of `ceil(width / 8)` bytes, matching the
/// rotation [`bitmap::raster_to_column_major`](crate::bitmap::raster_to_column_major)
/// applies — a printed "column" is one printhead line.
///
/// Returns `(red, black, cols, per_line_byte)`.
pub fn rgb_to_planes(rgb: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let per_line_byte = width.div_ceil(8);
    let plane_len = (height * per_line_byte) as usize;
    let mut red = vec![0u8; plane_len];
    let mut black = vec![0u8; plane_len];

    for y in 0..height {
        for x in 0..width {
            let px = ((y * width + x) * 3) as usize;
            let Some([r, g, b]) = rgb.get(px..px + 3).map(|s| [s[0], s[1], s[2]]) else {
                continue;
            };
            let Some(plane) = classify(luma(r, g, b)) else {
                continue;
            };

            // -90° rotation: output column = y, dot position within it = x.
            let idx = (y * per_line_byte + x / 8) as usize;
            let bit = 1 << (x % 8); // LSB-first
            match plane {
                Plane::Red => red[idx] |= bit,
                Plane::Black => black[idx] |= bit,
            }
        }
    }

    (red, black, height, per_line_byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason BLACK_THRESHOLD is 48: pure red must not be classified
    /// as black. If the luma coefficients ever drift, this is what catches it.
    #[test]
    fn pure_red_is_red() {
        assert_eq!(luma(255, 0, 0), 76);
        assert_eq!(classify(luma(255, 0, 0)), Some(Plane::Red));
    }

    #[test]
    fn pure_black_and_white_classify() {
        assert_eq!(classify(luma(0, 0, 0)), Some(Plane::Black));
        assert_eq!(classify(luma(255, 255, 255)), None);
    }

    /// A luminance cut, not a hue test — mid-grey lands on red. Callers must
    /// quantise first; this pins the caveat so it can't be forgotten.
    #[test]
    fn midgrey_lands_on_red() {
        assert_eq!(classify(luma(100, 100, 100)), Some(Plane::Red));
    }

    #[test]
    fn threshold_boundaries() {
        assert_eq!(classify(BLANK_THRESHOLD), None);
        assert_eq!(classify(BLANK_THRESHOLD - 1), Some(Plane::Red));
        assert_eq!(classify(BLACK_THRESHOLD + 1), Some(Plane::Red));
        assert_eq!(classify(BLACK_THRESHOLD), Some(Plane::Black));
    }

    #[test]
    fn interleave_puts_red_before_black_per_column() {
        let red = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let black = vec![0x11, 0x22, 0x33, 0x44];
        let out = interleave_planes(&red, &black, 2, 2).unwrap();
        assert_eq!(out, vec![0xAA, 0xBB, 0x11, 0x22, 0xCC, 0xDD, 0x33, 0x44]);
    }

    #[test]
    fn interleave_rejects_mismatched_planes() {
        assert!(interleave_planes(&[0; 4], &[0; 2], 2, 2).is_none());
        assert!(interleave_planes(&[0; 4], &[0; 4], 2, 3).is_none());
    }

    /// Red and black pixels must land in different planes at the same bit
    /// position — that co-location is what makes them one dot in two colours.
    #[test]
    fn rgb_splits_colours_into_separate_planes() {
        // 8x1: red at x=0, black at x=1, white elsewhere.
        let mut rgb = vec![255u8; 8 * 3];
        rgb[0..3].copy_from_slice(&[255, 0, 0]);
        rgb[3..6].copy_from_slice(&[0, 0, 0]);

        let (red, black, cols, bpl) = rgb_to_planes(&rgb, 8, 1);
        assert_eq!((cols, bpl), (1, 1));
        assert_eq!(red[0], 0b0000_0001);
        assert_eq!(black[0], 0b0000_0010);
    }
}
