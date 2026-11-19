//! The public value types: options, header, output formats and the metadata of a file.

/// How strict the decoder is. See `docs/profiles.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Go 1.27 `image/png` and byblos-rs today (default): every chunk CRC checked, unknown
    /// chunks (critical too) skipped, extra pixel data after the last row is an error, an
    /// error after the last row (bad Adler-32, truncated stream) is an error.
    #[default]
    Go,
    /// libpng 1.6 defaults (the lenient option): a bad CRC in an ancillary chunk drops the
    /// chunk, a bad CRC in a critical chunk is an error; an unknown critical chunk is an error;
    /// extra compressed data, a bad Adler-32 or a short zlib tail after the last row are
    /// ignored; invalid ancillary chunks are dropped as libpng does.
    Libpng,
}

/// Decoding options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// The strictness profile (default [`Profile::Go`]).
    pub profile: Profile,
}

/// The PNG colour type of IHDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorType {
    /// Greyscale, 1 sample.
    Gray = 0,
    /// Truecolour, 3 samples.
    Rgb = 2,
    /// Palette indices, 1 sample.
    Indexed = 3,
    /// Greyscale with alpha, 2 samples.
    GrayAlpha = 4,
    /// Truecolour with alpha, 4 samples.
    Rgba = 6,
}

impl ColorType {
    /// The IHDR byte, if it is one of the five colour types.
    pub(crate) fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Gray),
            2 => Some(Self::Rgb),
            3 => Some(Self::Indexed),
            4 => Some(Self::GrayAlpha),
            6 => Some(Self::Rgba),
            _ => None,
        }
    }

    /// Samples per pixel as stored in the file.
    pub(crate) fn samples(self) -> usize {
        match self {
            Self::Gray | Self::Indexed => 1,
            Self::GrayAlpha => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }

    /// Whether the PNG rules allow this bit depth with this colour type.
    pub(crate) fn allows_depth(self, depth: u8) -> bool {
        match self {
            Self::Gray => matches!(depth, 1 | 2 | 4 | 8 | 16),
            Self::Indexed => matches!(depth, 1 | 2 | 4 | 8),
            Self::Rgb | Self::GrayAlpha | Self::Rgba => matches!(depth, 8 | 16),
        }
    }
}

/// The image header (IHDR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Pixels per row.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// Bits per sample: 1, 2, 4, 8 or 16.
    pub bit_depth: u8,
    /// The colour type.
    pub color_type: ColorType,
    /// Whether the image is Adam7 interlaced.
    pub interlaced: bool,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            bit_depth: 0,
            color_type: ColorType::Gray,
            interlaced: false,
        }
    }
}

/// The layout of the rows a decode writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Rows as stored after unfiltering and Adam7 merge: samples at bit_depth, sub-byte
    /// samples packed MSB first with each row padded to a byte, 16-bit big-endian, palette
    /// indices as stored. libpng with no transforms.
    Native,
    /// One byte per sample: 1/2/4-bit gray scaled to 0..255 (v * 255 / (2^d - 1)), 16-bit
    /// reduced to the high byte, palette expanded to RGB (RGBA if tRNS is present; entries
    /// past tRNS are alpha 255; an index past the palette is 0,0,0,255), a tRNS colour key on
    /// Gray / Rgb adds an alpha channel (0 where the sample equals the key at full depth,
    /// else 255). libpng with png_set_expand + png_set_strip_16.
    Eight,
}

impl Header {
    /// Channels of `fmt` output (Native: 1 for Indexed; Eight: 3/4 for Indexed, +1 for tRNS).
    #[must_use]
    pub fn channels(&self, fmt: Format, has_trns: bool) -> usize {
        match fmt {
            Format::Native => self.color_type.samples(),
            Format::Eight => match self.color_type {
                ColorType::Gray => 1 + usize::from(has_trns),
                ColorType::Rgb | ColorType::Indexed => 3 + usize::from(has_trns),
                ColorType::GrayAlpha => 2,
                ColorType::Rgba => 4,
            },
        }
    }

    /// Bytes per output row, `None` on overflow.
    #[must_use]
    pub fn row_bytes(&self, fmt: Format, has_trns: bool) -> Option<usize> {
        let w = usize::try_from(self.width).ok()?;
        match fmt {
            Format::Native => self.native_row_bytes(w),
            Format::Eight => w.checked_mul(self.channels(fmt, has_trns)),
        }
    }

    /// Bits per pixel as stored in the file.
    pub(crate) fn bits_per_pixel(&self) -> usize {
        self.color_type.samples() * usize::from(self.bit_depth)
    }

    /// Bytes of a stored row of `width` pixels, without the filter byte.
    pub(crate) fn native_row_bytes(&self, width: usize) -> Option<usize> {
        Some(width.checked_mul(self.bits_per_pixel())?.div_ceil(8))
    }
}

/// tRNS: the colour key of Gray and Rgb, or the alpha of the first palette entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trns {
    /// The grey sample that is fully transparent (raw 16-bit value of the chunk).
    Gray(u16),
    /// The red, green and blue samples that are fully transparent (raw values of the chunk).
    Rgb([u16; 3]),
    /// Alpha of palette entries 0.. (the chunk data).
    Alpha(Vec<u8>),
}

/// iCCP: the profile name and the decompressed profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iccp {
    /// The profile name (Latin-1, 1 to 79 bytes).
    pub name: Vec<u8>,
    /// The decompressed profile.
    pub profile: Vec<u8>,
}

/// cICP: coding-independent code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cicp {
    /// Colour primaries.
    pub primaries: u8,
    /// Transfer function.
    pub transfer: u8,
    /// Matrix coefficients.
    pub matrix: u8,
    /// Video full range flag.
    pub full_range: u8,
}

/// Which text chunk a [`Text`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// tEXt.
    Text,
    /// zTXt.
    Ztxt,
    /// iTXt.
    Itxt,
}

/// A text chunk, kept raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    /// The chunk type.
    pub kind: TextKind,
    /// The chunk data as stored in the file.
    pub data: Vec<u8>,
}

/// acTL: the animation control of an APNG. Decoding returns the default image (the IDAT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Apng {
    /// Number of frames.
    pub num_frames: u32,
    /// Number of plays, 0 for infinite.
    pub num_plays: u32,
}

/// The header and the metadata chunks of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// IHDR.
    pub header: Header,
    /// PLTE entries.
    pub palette: Vec<[u8; 3]>,
    /// tRNS.
    pub trns: Option<Trns>,
    /// gAMA, raw value (gamma * 100000).
    pub gamma: Option<u32>,
    /// cHRM raw: wx wy rx ry gx gy bx by.
    pub chrm: Option<[u32; 8]>,
    /// sRGB rendering intent.
    pub srgb: Option<u8>,
    /// iCCP.
    pub iccp: Option<Iccp>,
    /// cICP.
    pub cicp: Option<Cicp>,
    /// eXIf data, raw.
    pub exif: Option<Vec<u8>>,
    /// tEXt / zTXt / iTXt in file order, raw chunk data.
    pub text: Vec<Text>,
    /// acTL seen.
    pub apng: Option<Apng>,
}

impl Info {
    /// Whether a tRNS chunk was kept.
    #[must_use]
    pub fn has_trns(&self) -> bool {
        self.trns.is_some()
    }

    /// Bytes per output row of `fmt`, `None` on overflow.
    #[must_use]
    pub fn row_bytes(&self, fmt: Format) -> Option<usize> {
        self.header.row_bytes(fmt, self.has_trns())
    }

    /// Bytes of the whole output of `fmt`, `None` on overflow.
    #[must_use]
    pub fn output_size(&self, fmt: Format) -> Option<usize> {
        self.row_bytes(fmt)?
            .checked_mul(usize::try_from(self.header.height).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(ct: ColorType, depth: u8) -> Header {
        Header {
            width: 5,
            height: 3,
            bit_depth: depth,
            color_type: ct,
            interlaced: false,
        }
    }

    #[test]
    fn channels_and_row_bytes() {
        let g = h(ColorType::Gray, 1);
        assert_eq!(g.channels(Format::Native, false), 1);
        assert_eq!(g.channels(Format::Eight, true), 2);
        assert_eq!(g.row_bytes(Format::Native, false), Some(1));
        assert_eq!(g.row_bytes(Format::Eight, false), Some(5));
        let p = h(ColorType::Indexed, 4);
        assert_eq!(p.channels(Format::Native, true), 1);
        assert_eq!(p.channels(Format::Eight, false), 3);
        assert_eq!(p.channels(Format::Eight, true), 4);
        assert_eq!(p.row_bytes(Format::Native, false), Some(3));
        let rgb = h(ColorType::Rgb, 16);
        assert_eq!(rgb.row_bytes(Format::Native, false), Some(30));
        assert_eq!(rgb.row_bytes(Format::Eight, true), Some(20));
        assert_eq!(h(ColorType::GrayAlpha, 16).channels(Format::Eight, true), 2);
        assert_eq!(h(ColorType::Rgba, 8).channels(Format::Eight, true), 4);
    }

    #[test]
    fn output_size_overflow_is_none() {
        let big = Header {
            width: u32::MAX,
            height: u32::MAX,
            bit_depth: 16,
            color_type: ColorType::Rgba,
            interlaced: false,
        };
        let info = Info {
            header: big,
            ..Info::default()
        };
        assert_eq!(info.output_size(Format::Native), None);
    }
}
