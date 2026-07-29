/// Thermal-compensated sRGB-to-dither LUT.
///
/// Combines standard sRGB linearization (gamma ~2.2) with a thermal bleed
/// compensation curve (gamma correction factor = 5.0). This pushes mid-tones
/// significantly lighter to compensate for dot spread on thermal printers,
/// where anything above ~50% dot density appears solid black.
///
/// Key mappings (W colorspace: 0=black, 255=white):
///   W=  0 -> 100% dots, W= 48 -> 50%, W=128 -> 25%, W=192 -> 12.5%, W=255 -> 0%
static SRGB_TO_LINEAR: [u8; 256] = [
    0, 50, 58, 63, 67, 70, 72, 74, 76, 78, 80, 82, 83, 85, 86, 88, 89, 90, 92, 93, 95, 96, 97, 98,
    100, 101, 102, 103, 105, 106, 107, 108, 109, 110, 112, 113, 114, 115, 116, 117, 118, 119, 120,
    121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 135, 136, 137, 138,
    139, 140, 141, 142, 142, 143, 144, 145, 146, 147, 148, 148, 149, 150, 151, 152, 152, 153, 154,
    155, 156, 156, 157, 158, 159, 159, 160, 161, 162, 162, 163, 164, 165, 165, 166, 167, 167, 168,
    169, 170, 170, 171, 172, 172, 173, 174, 174, 175, 176, 177, 177, 178, 179, 179, 180, 181, 181,
    182, 183, 183, 184, 184, 185, 186, 186, 187, 188, 188, 189, 190, 190, 191, 191, 192, 193, 193,
    194, 195, 195, 196, 196, 197, 198, 198, 199, 199, 200, 201, 201, 202, 202, 203, 203, 204, 205,
    205, 206, 206, 207, 207, 208, 209, 209, 210, 210, 211, 211, 212, 213, 213, 214, 214, 215, 215,
    216, 216, 217, 217, 218, 219, 219, 220, 220, 221, 221, 222, 222, 223, 223, 224, 224, 225, 225,
    226, 226, 227, 227, 228, 228, 229, 230, 230, 231, 231, 232, 232, 233, 233, 234, 234, 235, 235,
    236, 236, 237, 237, 238, 238, 238, 239, 239, 240, 240, 241, 241, 242, 242, 243, 243, 244, 244,
    245, 245, 246, 246, 247, 247, 248, 248, 249, 249, 249, 250, 250, 251, 251, 252, 252, 253, 253,
    254, 254, 255, 255,
];

/// 4x4 Bayer ordered dither threshold matrix, scaled 0-255.
static BAYER4: [[u8; 4]; 4] = [
    [8, 136, 40, 168],
    [200, 72, 232, 104],
    [56, 184, 24, 152],
    [248, 120, 216, 88],
];

/// The printer has no grayscale mode — the raster is strictly 1bpp — so every
/// tone is a halftone produced here on the host. The vendor does the same in
/// `BitmapUtil`; this just offers the choice of kernel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DitherMode {
    /// 4x4 ordered Bayer. Stateless, no error smearing, and the default:
    /// [`SRGB_TO_LINEAR`] is tuned for this hardware's dot spread, which the
    /// error-diffusion kernels inherit but were not designed around.
    #[default]
    Bayer,
    /// Classic Floyd–Steinberg (7/16, 3/16, 5/16, 1/16).
    FloydSteinberg,
    /// Atkinson: spreads only 6/8 of the error over a wider neighbourhood,
    /// giving higher local contrast and cleaner whites at the cost of clipping.
    Atkinson,
    /// The vendor's own pipeline, reproduced for comparison: quantise to 16
    /// grey levels (`i * 17`) with their kernel, then threshold at 125 the way
    /// `ImgConverter` does. Deliberately skips [`SRGB_TO_LINEAR`] — the vendor
    /// applies no thermal compensation, and adding ours would stop this being
    /// their pipeline.
    Vendor16,
}

impl DitherMode {
    pub const NAMES: [&'static str; 4] = ["bayer", "floyd-steinberg", "atkinson", "vendor16"];
}

impl std::str::FromStr for DitherMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bayer" => Ok(Self::Bayer),
            "floyd-steinberg" | "fs" => Ok(Self::FloydSteinberg),
            "atkinson" => Ok(Self::Atkinson),
            "vendor16" => Ok(Self::Vendor16),
            other => Err(format!(
                "unknown dither `{other}` ({})",
                Self::NAMES.join("|")
            )),
        }
    }
}

/// Error-diffusion neighbours as `(dx, dy, weight)`.
type Kernel = &'static [(i32, usize, f32)];

const FLOYD_STEINBERG: Kernel = &[
    (1, 0, 7.0 / 16.0),
    (-1, 1, 3.0 / 16.0),
    (0, 1, 5.0 / 16.0),
    (1, 1, 1.0 / 16.0),
];

const ATKINSON: Kernel = &[
    (1, 0, 0.125),
    (2, 0, 0.125),
    (-1, 1, 0.125),
    (0, 1, 0.125),
    (1, 1, 0.125),
    (0, 2, 0.125),
];

/// The vendor's `diffuseError` weights. They sum to 15/16, not 1 — a little
/// error is dropped on every pixel, which is theirs to own, not a transcription
/// slip.
const VENDOR: Kernel = &[
    (1, 0, 0.3125),
    (-1, 1, 0.1875),
    (0, 1, 0.375),
    (1, 1, 0.0625),
];

/// Vendor grey levels: `GRAY_16_LUT` in `BitmapUtil`, i.e. `i * 17`.
const VENDOR_LEVELS: u32 = 16;
/// `ImgConverter`'s ink threshold — what turns their 16-level grey into 1bpp.
const VENDOR_INK_THRESHOLD: f32 = 125.0;

/// Rows of carried error the widest kernel needs (Atkinson reaches `y + 2`).
const ERROR_ROWS: usize = 3;

/// Halftones 8bpp grayscale scanlines to 1bpp, carrying error between rows for
/// the diffusion kernels.
///
/// Feed lines in increasing `y` order; the carried error is meaningless
/// otherwise. [`DitherMode::Bayer`] is stateless and immune to ordering.
pub struct Ditherer {
    mode: DitherMode,
    width: usize,
    /// Ring of error rows; `err[row]` is the one being consumed.
    err: [Vec<f32>; ERROR_ROWS],
    row: usize,
}

impl Ditherer {
    pub fn new(mode: DitherMode, width: u32) -> Self {
        let width = width as usize;
        Self {
            mode,
            width,
            err: std::array::from_fn(|_| vec![0.0; width]),
            row: 0,
        }
    }

    pub fn mode(&self) -> DitherMode {
        self.mode
    }

    /// Dither one line to 1bpp MSB-first, mirrored horizontally.
    ///
    /// `line` is W colorspace (0x00 black, 0xFF white). `mono` must be at least
    /// `(width + 7) / 8` bytes and zeroed by the caller.
    ///
    /// The mirror is applied only when placing the output bit — diffusion runs
    /// in natural left-to-right order, so the kernel isn't reflected with it.
    pub fn line(&mut self, line: &[u8], y: u32, mono: &mut [u8]) {
        let width = self.width.min(line.len());
        if self.mode == DitherMode::Bayer {
            let bayer_row = &BAYER4[(y & 3) as usize];
            for x in 0..width {
                let mx = self.width - 1 - x;
                if SRGB_TO_LINEAR[line[x] as usize] < bayer_row[mx & 3] {
                    mono[mx / 8] |= 0x80 >> (mx & 7);
                }
            }
            return;
        }

        let (kernel, vendor) = match self.mode {
            DitherMode::FloydSteinberg => (FLOYD_STEINBERG, false),
            DitherMode::Atkinson => (ATKINSON, false),
            DitherMode::Vendor16 => (VENDOR, true),
            DitherMode::Bayer => unreachable!("handled above"),
        };

        for x in 0..width {
            let source = if vendor {
                line[x] as f32
            } else {
                SRGB_TO_LINEAR[line[x] as usize] as f32
            };
            let value = source + self.err[self.row][x];

            // Quantise, then decide ink. The vendor lands on one of 16 grey
            // levels and only later thresholds; we go straight to black/white.
            let (quantised, inked) = if vendor {
                let step = 255.0 / (VENDOR_LEVELS - 1) as f32;
                let level = (value / step)
                    .round()
                    .clamp(0.0, (VENDOR_LEVELS - 1) as f32);
                let grey = level * step;
                (grey, grey < VENDOR_INK_THRESHOLD)
            } else if value < 128.0 {
                (0.0, true)
            } else {
                (255.0, false)
            };

            if inked {
                let mx = self.width - 1 - x;
                mono[mx / 8] |= 0x80 >> (mx & 7);
            }

            let error = value - quantised;
            for &(dx, dy, weight) in kernel {
                let Ok(nx) = usize::try_from(x as i32 + dx) else {
                    continue;
                };
                if nx >= self.width {
                    continue;
                }
                self.err[(self.row + dy) % ERROR_ROWS][nx] += error * weight;
            }
        }

        // This row is spent; clear it so it can serve as the far future row.
        self.err[self.row].fill(0.0);
        self.row = (self.row + 1) % ERROR_ROWS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dither_once(mode: DitherMode, line: &[u8], width: u32) -> Vec<u8> {
        let mut d = Ditherer::new(mode, width);
        let mut mono = vec![0u8; width.div_ceil(8) as usize];
        d.line(line, 0, &mut mono);
        mono
    }

    fn bits(mono: &[u8]) -> u32 {
        mono.iter().map(|b| b.count_ones()).sum()
    }

    /// Solid black and solid white must be exact in every mode — a kernel that
    /// smears error into the extremes would speckle large flat areas.
    #[test]
    fn extremes_are_exact_in_every_mode() {
        for mode in [
            DitherMode::Bayer,
            DitherMode::FloydSteinberg,
            DitherMode::Atkinson,
            DitherMode::Vendor16,
        ] {
            assert_eq!(dither_once(mode, &[0x00; 8], 8)[0], 0xFF, "{mode:?} black");
            assert_eq!(dither_once(mode, &[0xFF; 8], 8)[0], 0x00, "{mode:?} white");
        }
    }

    /// The thermal LUT pushes mid-tones well under 50% coverage to compensate
    /// for dot spread. Vendor16 is excluded: it deliberately skips the LUT.
    #[test]
    fn midtone_is_lighter_than_half_with_thermal_compensation() {
        for mode in [
            DitherMode::Bayer,
            DitherMode::FloydSteinberg,
            DitherMode::Atkinson,
        ] {
            let mut d = Ditherer::new(mode, 32);
            let mut total = 0;
            for y in 0..4 {
                let mut mono = vec![0u8; 4];
                d.line(&[0x80; 32], y, &mut mono);
                total += bits(&mono);
            }
            assert!(
                total > 8 && total < 64,
                "{mode:?}: expected well under 50% of 128, got {total}"
            );
        }
    }

    /// Error diffusion is meaningless without carry between rows; a uniform
    /// mid-tone must not repeat the identical pattern every line the way an
    /// ordered kernel does.
    #[test]
    fn diffusion_carries_error_between_rows() {
        let mut d = Ditherer::new(DitherMode::FloydSteinberg, 32);
        let mut rows = Vec::new();
        for y in 0..4 {
            let mut mono = vec![0u8; 4];
            d.line(&[0x60; 32], y, &mut mono);
            rows.push(mono);
        }
        assert!(
            rows.iter().any(|r| *r != rows[0]),
            "all rows identical — error is not carrying"
        );
    }

    /// Bayer is stateless, so the same input and y must always give the same
    /// output regardless of what came before.
    #[test]
    fn bayer_is_stateless() {
        let mut d = Ditherer::new(DitherMode::Bayer, 32);
        let mut first = vec![0u8; 4];
        d.line(&[0x80; 32], 0, &mut first);
        for y in 1..5 {
            d.line(&[0x40; 32], y, &mut [0u8; 4]);
        }
        let mut again = vec![0u8; 4];
        d.line(&[0x80; 32], 0, &mut again);
        assert_eq!(first, again);
    }

    /// Output is mirrored horizontally for the printhead: ink on the left of
    /// the input must land on the right of the packed byte.
    #[test]
    fn output_is_mirrored() {
        let mut line = [0xFFu8; 8];
        line[0] = 0x00; // black at the far left
        let mono = dither_once(DitherMode::Bayer, &line, 8);
        assert_eq!(mono[0], 0x01, "leftmost input pixel should be the LSB");
    }

    #[test]
    fn mode_names_round_trip() {
        for name in DitherMode::NAMES {
            let mode: DitherMode = name.parse().unwrap();
            assert_eq!(
                format!("{mode:?}").to_lowercase().replace('-', ""),
                name.replace('-', "")
            );
        }
        assert_eq!(
            "fs".parse::<DitherMode>().unwrap(),
            DitherMode::FloydSteinberg
        );
        assert!("nope".parse::<DitherMode>().is_err());
    }

    #[test]
    fn non_multiple_of_eight_width_does_not_panic() {
        let mono = dither_once(DitherMode::Atkinson, &[0x80; 13], 13);
        assert_eq!(mono.len(), 2);
    }
}
