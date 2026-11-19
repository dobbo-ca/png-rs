//! Row geometry (Adam7 passes) and the conversion of stored rows to the output formats.

use crate::info::{ColorType, Format, Header, Info, Profile, Trns};
use crate::source::at;

/// An Adam7 pass (Go's `interlaceScan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scan {
    pub(crate) xf: usize,
    pub(crate) yf: usize,
    pub(crate) xo: usize,
    pub(crate) yo: usize,
}

const fn scan(xf: usize, yf: usize, xo: usize, yo: usize) -> Scan {
    Scan { xf, yf, xo, yo }
}

static ADAM7: [Scan; 7] = [
    scan(8, 8, 0, 0),
    scan(8, 8, 4, 0),
    scan(4, 8, 0, 4),
    scan(4, 4, 2, 0),
    scan(2, 4, 0, 2),
    scan(2, 2, 1, 0),
    scan(1, 2, 0, 1),
];

/// The one pass of a non-interlaced image.
static FLAT: [Scan; 1] = [scan(1, 1, 0, 0)];

/// A pass that has pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pass {
    /// Pixels per row of the pass.
    pub(crate) w: usize,
    /// Rows of the pass.
    pub(crate) h: usize,
    /// Stored bytes per row, without the filter byte.
    pub(crate) rb: usize,
    pub(crate) scan: &'static Scan,
}

/// The number of passes of an image (1 when not interlaced).
pub(crate) fn pass_count(hdr: &Header) -> usize {
    if hdr.interlaced { ADAM7.len() } else { 1 }
}

/// Pass `idx` of the image, `None` when it has no pixels (an Adam7 pass of a small image has
/// no rows, and no filter bytes either).
pub(crate) fn pass(hdr: &Header, idx: usize) -> Option<Pass> {
    let scan = if hdr.interlaced {
        ADAM7.get(idx)?
    } else {
        FLAT.get(idx)?
    };
    let width = usize::try_from(hdr.width).ok()?;
    let height = usize::try_from(hdr.height).ok()?;
    // Add the factor and subtract one, rounding up.
    let w = width.checked_add(scan.xf - 1 - scan.xo)? / scan.xf;
    let h = height.checked_add(scan.yf - 1 - scan.yo)? / scan.yf;
    if w == 0 || h == 0 {
        return None;
    }
    Some(Pass {
        w,
        h,
        rb: hdr.native_row_bytes(w)?,
        scan,
    })
}

/// The length of the unfiltered data the image needs: every row with its filter byte.
pub(crate) fn filtered_len(hdr: &Header) -> Option<usize> {
    let mut total = 0usize;
    for idx in 0..pass_count(hdr) {
        if let Some(p) = pass(hdr, idx) {
            total = total.checked_add(p.rb.checked_add(1)?.checked_mul(p.h)?)?;
        }
    }
    Some(total)
}

/// Bytes per pixel for the filters: the bytes of a whole pixel, at least 1.
pub(crate) fn filter_bpp(hdr: &Header) -> usize {
    hdr.bits_per_pixel().div_ceil(8).max(1)
}

/// The sample `x` of a row of `depth` (1, 2 or 4) bits per sample.
fn sample(row: &[u8], depth: usize, x: usize) -> u8 {
    let bit = x * depth;
    let shift = (8 - depth).saturating_sub(bit % 8);
    (at(row, bit / 8) >> shift) & (0xffu8 >> (8 - depth))
}

fn samples(row: &[u8], depth: usize, n: usize) -> impl Iterator<Item = u8> + '_ {
    (0..n).map(move |x| sample(row, depth, x))
}

/// `v * 255 / (2^depth - 1)` for depth 1, 2 and 4.
fn scale(depth: usize) -> u8 {
    match depth {
        1 => 0xff,
        2 => 0x55,
        _ => 0x11,
    }
}

/// How the output rows of an image are built: the format, the colour model, the tRNS colour key
/// and the palette.
#[derive(Debug, Clone)]
pub(crate) struct Conv {
    pub(crate) fmt: Format,
    pub(crate) ct: ColorType,
    pub(crate) depth: usize,
    /// Channels of an Eight pixel.
    pub(crate) chan: usize,
    /// Colour key in the compare domain of the sample (see [`key_for`]).
    pub(crate) key: Option<[u16; 3]>,
    /// RGBA of every palette index (Indexed, Eight).
    pub(crate) lut: [[u8; 4]; 256],
}

impl Conv {
    pub(crate) fn new(info: &Info, fmt: Format, profile: Profile) -> Conv {
        let h = &info.header;
        let mut lut = [[0, 0, 0, 255]; 256];
        for (e, &[r, g, b]) in lut.iter_mut().zip(&info.palette) {
            *e = [r, g, b, 255];
        }
        if let Some(Trns::Alpha(a)) = &info.trns {
            for (e, &alpha) in lut.iter_mut().zip(a) {
                e[3] = alpha;
            }
        }
        Conv {
            fmt,
            ct: h.color_type,
            depth: usize::from(h.bit_depth),
            chan: h.channels(Format::Eight, info.has_trns()),
            key: key_for(info, profile),
            lut,
        }
    }

    /// Bytes of one output pixel when it is moved as a unit (not for sub-byte Native).
    pub(crate) fn pixel_bytes(&self, hdr: &Header) -> usize {
        match self.fmt {
            Format::Eight => self.chan,
            Format::Native => filter_bpp(hdr),
        }
    }
}

/// The tRNS colour key of Gray and Rgb images in the domain the converters compare in: the
/// sample scaled to 8 bits for Gray under 8 bits, the low byte for 8 bits, the whole value for
/// 16 bits. For Gray under 8 bits Go scales the low byte of the chunk (and wraps), libpng masks
/// the value to the depth first.
fn key_for(info: &Info, profile: Profile) -> Option<[u16; 3]> {
    let depth = info.header.bit_depth;
    match (&info.trns, info.header.color_type) {
        (Some(Trns::Gray(v)), ColorType::Gray) => {
            let k = match depth {
                16 => *v,
                8 => *v & 0xff,
                d => {
                    let mult = scale(usize::from(d));
                    let low = match profile {
                        Profile::Go => (*v & 0xff) as u8,
                        Profile::Libpng => (*v & ((1u16 << d) - 1)) as u8,
                    };
                    u16::from(low.wrapping_mul(mult))
                }
            };
            Some([k, 0, 0])
        }
        (Some(Trns::Rgb([r, g, b])), ColorType::Rgb) => {
            if depth == 16 {
                Some([*r, *g, *b])
            } else {
                Some([*r & 0xff, *g & 0xff, *b & 0xff])
            }
        }
        _ => None,
    }
}

/// Copies the first `n` bytes of `src` to `dst` (less if either is shorter).
fn copy_n(dst: &mut [u8], src: &[u8], n: usize) {
    let n = n.min(dst.len()).min(src.len());
    if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
        d.copy_from_slice(s);
    }
}

fn alpha16(v: u16, key: u16) -> u8 {
    if v == key { 0 } else { 0xff }
}

/// Converts the `n` pixels of the stored row `raw` to Eight pixels in `dst`.
pub(crate) fn to_eight(c: &Conv, raw: &[u8], n: usize, dst: &mut [u8]) {
    match (c.ct, c.depth) {
        (ColorType::Gray, 8) => match c.key {
            None => copy_n(dst, raw, n),
            Some([k, _, _]) => {
                for (&v, d) in raw.iter().take(n).zip(dst.as_chunks_mut::<2>().0) {
                    *d = [v, alpha16(u16::from(v), k)];
                }
            }
        },
        (ColorType::Gray, 16) => match c.key {
            None => {
                for (&[hi, _], d) in raw.as_chunks::<2>().0.iter().take(n).zip(dst.iter_mut()) {
                    *d = hi;
                }
            }
            Some([k, _, _]) => {
                for (&[hi, lo], d) in raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .take(n)
                    .zip(dst.as_chunks_mut::<2>().0)
                {
                    *d = [hi, alpha16(u16::from_be_bytes([hi, lo]), k)];
                }
            }
        },
        (ColorType::Gray, depth) => {
            let mult = scale(depth);
            match c.key {
                None => {
                    for (v, d) in samples(raw, depth, n).zip(dst.iter_mut()) {
                        *d = v * mult;
                    }
                }
                Some([k, _, _]) => {
                    for (v, d) in samples(raw, depth, n).zip(dst.as_chunks_mut::<2>().0) {
                        let y = v * mult;
                        *d = [y, alpha16(u16::from(y), k)];
                    }
                }
            }
        }
        (ColorType::GrayAlpha, 8) => copy_n(dst, raw, n * 2),
        (ColorType::GrayAlpha, _) => {
            for (&[y, _, a, _], d) in raw
                .as_chunks::<4>()
                .0
                .iter()
                .take(n)
                .zip(dst.as_chunks_mut::<2>().0)
            {
                *d = [y, a];
            }
        }
        (ColorType::Rgb, 8) => match c.key {
            None => copy_n(dst, raw, n * 3),
            Some([kr, kg, kb]) => {
                for (&[r, g, b], d) in raw
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .take(n)
                    .zip(dst.as_chunks_mut::<4>().0)
                {
                    let hit = u16::from(r) == kr && u16::from(g) == kg && u16::from(b) == kb;
                    *d = [r, g, b, if hit { 0 } else { 0xff }];
                }
            }
        },
        (ColorType::Rgb, _) => match c.key {
            None => {
                for (&[r, _, g, _, b, _], d) in raw
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .take(n)
                    .zip(dst.as_chunks_mut::<3>().0)
                {
                    *d = [r, g, b];
                }
            }
            Some([kr, kg, kb]) => {
                for (&[r0, r1, g0, g1, b0, b1], d) in raw
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .take(n)
                    .zip(dst.as_chunks_mut::<4>().0)
                {
                    let hit = u16::from_be_bytes([r0, r1]) == kr
                        && u16::from_be_bytes([g0, g1]) == kg
                        && u16::from_be_bytes([b0, b1]) == kb;
                    *d = [r0, g0, b0, if hit { 0 } else { 0xff }];
                }
            }
        },
        (ColorType::Rgba, 8) => copy_n(dst, raw, n * 4),
        (ColorType::Rgba, _) => {
            for (&[r, _, g, _, b, _, a, _], d) in raw
                .as_chunks::<8>()
                .0
                .iter()
                .take(n)
                .zip(dst.as_chunks_mut::<4>().0)
            {
                *d = [r, g, b, a];
            }
        }
        (ColorType::Indexed, depth) => {
            let entry = |i: u8| c.lut.get(usize::from(i)).copied().unwrap_or([0, 0, 0, 255]);
            if c.chan == 4 {
                if depth == 8 {
                    for (&i, d) in raw.iter().take(n).zip(dst.as_chunks_mut::<4>().0) {
                        *d = entry(i);
                    }
                } else {
                    for (i, d) in samples(raw, depth, n).zip(dst.as_chunks_mut::<4>().0) {
                        *d = entry(i);
                    }
                }
            } else if depth == 8 {
                for (&i, d) in raw.iter().take(n).zip(dst.as_chunks_mut::<3>().0) {
                    let [r, g, b, _] = entry(i);
                    *d = [r, g, b];
                }
            } else {
                for (i, d) in samples(raw, depth, n).zip(dst.as_chunks_mut::<3>().0) {
                    let [r, g, b, _] = entry(i);
                    *d = [r, g, b];
                }
            }
        }
    }
}

/// Writes the `n` pixels of the stored row `raw` to the output row `dst`.
pub(crate) fn convert_flat(c: &Conv, raw: &[u8], n: usize, rb: usize, dst: &mut [u8]) {
    match c.fmt {
        Format::Native => copy_n(dst, raw, rb),
        Format::Eight => to_eight(c, raw, n, dst),
    }
}

/// Copies the pixels of `src` (`bpp` bytes each) into `dst` at pixel x0, x0 + dx, x0 + 2 dx, ...
pub(crate) fn scatter_bytes(src: &[u8], bpp: usize, dst: &mut [u8], x0: usize, dx: usize) {
    if bpp == 0 {
        return;
    }
    for (i, px) in src.chunks_exact(bpp).enumerate() {
        let start = (x0 + i * dx) * bpp;
        if let Some(d) = dst.get_mut(start..start + bpp) {
            d.copy_from_slice(px);
        }
    }
}

/// As [`scatter_bytes`] for the `n` samples of `depth` (1, 2 or 4) bits of the stored row `raw`.
pub(crate) fn scatter_bits(
    raw: &[u8],
    n: usize,
    depth: usize,
    dst: &mut [u8],
    x0: usize,
    dx: usize,
) {
    for (i, v) in samples(raw, depth, n).enumerate() {
        let bit = (x0 + i * dx) * depth;
        let shift = (8 - depth).saturating_sub(bit % 8);
        if let Some(b) = dst.get_mut(bit / 8) {
            let mask = (0xffu8 >> (8 - depth)) << shift;
            *b = (*b & !mask) | (v << shift);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(w: u32, h: u32, ct: ColorType, depth: u8, interlaced: bool) -> Header {
        Header {
            width: w,
            height: h,
            bit_depth: depth,
            color_type: ct,
            interlaced,
        }
    }

    fn info(h: Header, trns: Option<Trns>, palette: Vec<[u8; 3]>) -> Info {
        Info {
            header: h,
            trns,
            palette,
            ..Info::default()
        }
    }

    #[test]
    fn adam7_pass_sizes() {
        // (width, height) -> (w, h) of the 7 passes; 0 means no pixels.
        #[allow(clippy::type_complexity)]
        let table: &[(u32, u32, [(usize, usize); 7])] = &[
            (
                1,
                1,
                [(1, 1), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
            ),
            (
                2,
                2,
                [(1, 1), (0, 0), (0, 0), (0, 0), (0, 0), (1, 1), (2, 1)],
            ),
            (
                5,
                3,
                [(1, 1), (1, 1), (0, 0), (1, 1), (3, 1), (2, 2), (5, 1)],
            ),
            (
                8,
                8,
                [(1, 1), (1, 1), (2, 1), (2, 2), (4, 2), (4, 4), (8, 4)],
            ),
            (
                9,
                9,
                [(2, 2), (1, 2), (3, 1), (2, 3), (5, 2), (4, 5), (9, 4)],
            ),
        ];
        for (w, h, want) in table {
            let hd = hdr(*w, *h, ColorType::Gray, 8, true);
            for (i, &(pw, ph)) in want.iter().enumerate() {
                match pass(&hd, i) {
                    Some(p) => assert_eq!((p.w, p.h), (pw, ph), "{w}x{h} pass {i}"),
                    None => assert_eq!((pw, ph), (0, 0), "{w}x{h} pass {i}"),
                }
            }
        }
        // Every pixel belongs to exactly one pass.
        for w in 1..=17u32 {
            for h in 1..=17u32 {
                let hd = hdr(w, h, ColorType::Gray, 8, true);
                let total: usize = (0..7).filter_map(|i| pass(&hd, i)).map(|p| p.w * p.h).sum();
                assert_eq!(total, (w * h) as usize, "{w}x{h}");
            }
        }
    }

    #[test]
    fn flat_pass_and_filtered_len() {
        let hd = hdr(3, 2, ColorType::Rgb, 8, false);
        let p = pass(&hd, 0).unwrap();
        assert_eq!((p.w, p.h, p.rb), (3, 2, 9));
        assert!(pass(&hd, 1).is_none());
        assert_eq!(filtered_len(&hd), Some(2 * 10));
        // 1x1 interlaced: only pass 0 has a row, so only one filter byte.
        let hd = hdr(1, 1, ColorType::Gray, 8, true);
        assert_eq!(filtered_len(&hd), Some(2));
        // Sub-byte rows round up.
        let hd = hdr(9, 1, ColorType::Gray, 1, false);
        assert_eq!(pass(&hd, 0).unwrap().rb, 2);
        assert_eq!(filter_bpp(&hd), 1);
        assert_eq!(filter_bpp(&hdr(1, 1, ColorType::Rgba, 16, false)), 8);
    }

    fn eight(i: &Info, profile: Profile, raw: &[u8], n: usize) -> Vec<u8> {
        let c = Conv::new(i, Format::Eight, profile);
        let mut out = vec![0xaa; n * c.chan];
        to_eight(&c, raw, n, &mut out);
        out
    }

    #[test]
    fn eight_gray_depths() {
        let i = info(hdr(4, 1, ColorType::Gray, 2, false), None, vec![]);
        assert_eq!(
            eight(&i, Profile::Go, &[0b00_01_10_11], 4),
            [0, 85, 170, 255]
        );
        let i = info(hdr(3, 1, ColorType::Gray, 1, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[0b1010_0000], 3), [255, 0, 255]);
        let i = info(hdr(2, 1, ColorType::Gray, 4, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[0x0f], 2), [0, 255]);
        let i = info(hdr(2, 1, ColorType::Gray, 8, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[7, 9], 2), [7, 9]);
        let i = info(hdr(2, 1, ColorType::Gray, 16, false), None, vec![]);
        assert_eq!(
            eight(&i, Profile::Go, &[0x12, 0x34, 0xab, 0xcd], 2),
            [0x12, 0xab]
        );
    }

    #[test]
    fn eight_other_color_types() {
        let i = info(hdr(2, 1, ColorType::GrayAlpha, 8, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[1, 2, 3, 4], 2), [1, 2, 3, 4]);
        let i = info(hdr(1, 1, ColorType::GrayAlpha, 16, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[1, 2, 3, 4], 1), [1, 3]);
        let i = info(hdr(1, 1, ColorType::Rgb, 8, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[1, 2, 3], 1), [1, 2, 3]);
        let i = info(hdr(1, 1, ColorType::Rgb, 16, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[1, 0, 2, 0, 3, 0], 1), [1, 2, 3]);
        let i = info(hdr(1, 1, ColorType::Rgba, 8, false), None, vec![]);
        assert_eq!(eight(&i, Profile::Go, &[1, 2, 3, 4], 1), [1, 2, 3, 4]);
        let i = info(hdr(1, 1, ColorType::Rgba, 16, false), None, vec![]);
        assert_eq!(
            eight(&i, Profile::Go, &[1, 0, 2, 0, 3, 0, 4, 0], 1),
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn eight_indexed() {
        let pal = vec![[10, 11, 12], [20, 21, 22]];
        let i = info(hdr(4, 1, ColorType::Indexed, 2, false), None, pal.clone());
        // Index 2 and 3 are past the palette: opaque black.
        assert_eq!(
            eight(&i, Profile::Go, &[0b00_01_10_11], 4),
            [10, 11, 12, 20, 21, 22, 0, 0, 0, 0, 0, 0]
        );
        // With tRNS: RGBA; entries past tRNS are opaque.
        let i = info(
            hdr(3, 1, ColorType::Indexed, 8, false),
            Some(Trns::Alpha(vec![7])),
            pal,
        );
        assert_eq!(
            eight(&i, Profile::Go, &[0, 1, 5], 3),
            [10, 11, 12, 7, 20, 21, 22, 255, 0, 0, 0, 255]
        );
    }

    #[test]
    fn trns_key() {
        // Gray 8: the low byte of the key.
        let i = info(
            hdr(3, 1, ColorType::Gray, 8, false),
            Some(Trns::Gray(0x0105)),
            vec![],
        );
        assert_eq!(
            eight(&i, Profile::Go, &[4, 5, 6], 3),
            [4, 255, 5, 0, 6, 255]
        );
        // Gray 16: the whole value.
        let i = info(
            hdr(2, 1, ColorType::Gray, 16, false),
            Some(Trns::Gray(0x0102)),
            vec![],
        );
        assert_eq!(eight(&i, Profile::Go, &[1, 2, 1, 3], 2), [1, 0, 1, 255]);
        // Gray 2: the key is scaled with the sample.
        let i = info(
            hdr(2, 1, ColorType::Gray, 2, false),
            Some(Trns::Gray(2)),
            vec![],
        );
        assert_eq!(eight(&i, Profile::Go, &[0x90], 2), [170, 0, 85, 255]);
        // Gray 1: Go scales the low byte and wraps, libpng masks the value first.
        let i = info(
            hdr(2, 1, ColorType::Gray, 1, false),
            Some(Trns::Gray(3)),
            vec![],
        );
        assert_eq!(eight(&i, Profile::Libpng, &[0x80], 2), [255, 0, 0, 255]);
        assert_eq!(eight(&i, Profile::Go, &[0x80], 2), [255, 255, 0, 255]);
        // Rgb 8 and 16.
        let i = info(
            hdr(2, 1, ColorType::Rgb, 8, false),
            Some(Trns::Rgb([1, 2, 3])),
            vec![],
        );
        assert_eq!(
            eight(&i, Profile::Go, &[1, 2, 3, 1, 2, 4], 2),
            [1, 2, 3, 0, 1, 2, 4, 255]
        );
        let i = info(
            hdr(2, 1, ColorType::Rgb, 16, false),
            Some(Trns::Rgb([0x0102, 0x0304, 0x0506])),
            vec![],
        );
        assert_eq!(
            eight(&i, Profile::Go, &[1, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 7], 2),
            [1, 3, 5, 0, 1, 3, 5, 255]
        );
    }

    #[test]
    fn scatter() {
        let mut row = [0u8; 4];
        scatter_bytes(&[1, 2], 1, &mut row, 1, 2);
        assert_eq!(row, [0, 1, 0, 2]);
        let mut row = [0u8; 2];
        // 2-bit samples 3 and 1 at x = 1 and 3: 00 11 00 01.
        scatter_bits(&[0xd0], 2, 2, &mut row, 1, 2);
        assert_eq!(row, [0b00_11_00_01, 0]);
        // Existing bits of other pixels stay.
        scatter_bits(&[0x80], 1, 2, &mut row, 0, 1);
        assert_eq!(row, [0b10_11_00_01, 0]);
    }
}
