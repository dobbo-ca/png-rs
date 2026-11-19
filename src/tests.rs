//! Unit tests over hand-made PNGs. The writer here uses stored (uncompressed) deflate blocks.

use crate::*;
use std::sync::atomic::AtomicBool;

const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn adler(d: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in d {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A zlib stream of stored blocks.
fn zlib(raw: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler(raw).to_be_bytes());
    z
}

fn chunk_with_crc(out: &mut Vec<u8>, typ: &[u8; 4], body: &[u8], flip: u32) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut t = typ.to_vec();
    t.extend_from_slice(body);
    out.extend_from_slice(&t);
    out.extend_from_slice(&(crate::crc::update(0, &t) ^ flip).to_be_bytes());
}

fn ihdr_raw(w: u32, h: u32, depth: u8, ct: u8, comp: u8, filt: u8, il: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[depth, ct, comp, filt, il]);
    v
}

fn ihdr(w: u32, h: u32, depth: u8, ct: u8, il: u8) -> Vec<u8> {
    ihdr_raw(w, h, depth, ct, 0, 0, il)
}

/// A PNG of the chunks, every CRC right.
fn build(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = SIG.to_vec();
    for (t, b) in chunks {
        chunk_with_crc(&mut out, t, b, 0);
    }
    out
}

/// IHDR, IDAT (the stored zlib stream of `raw`), IEND.
fn simple(w: u32, h: u32, depth: u8, ct: u8, il: u8, raw: &[u8]) -> Vec<u8> {
    build(&[
        (b"IHDR", ihdr(w, h, depth, ct, il)),
        (b"IDAT", zlib(raw)),
        (b"IEND", vec![]),
    ])
}

fn opts(profile: Profile) -> Options {
    Options { profile }
}

fn limits() -> Limits {
    Limits {
        max_threads: 1,
        ..Limits::default()
    }
}

fn run(data: &[u8], fmt: Format, profile: Profile) -> Result<(Info, Vec<u8>), Error> {
    decode(
        data,
        fmt,
        &opts(profile),
        &limits(),
        &AtomicBool::new(false),
    )
}

const BOTH: [Profile; 2] = [Profile::Go, Profile::Libpng];

#[test]
fn gray8_both_formats_and_filters() {
    // Row 0 None, row 1 Up, row 2 Sub.
    let raw = [0, 10, 20, 30, 2, 1, 1, 1, 1, 5, 1, 1];
    let data = simple(3, 3, 8, 0, 0, &raw);
    for p in BOTH {
        for fmt in [Format::Native, Format::Eight] {
            let (info, out) = run(&data, fmt, p).unwrap();
            assert_eq!(out, [10, 20, 30, 11, 21, 31, 5, 6, 7], "{p:?} {fmt:?}");
            assert_eq!(info.header.width, 3);
            assert!(info.trns.is_none() && info.palette.is_empty());
        }
    }
}

#[test]
fn paeth_matches_the_spec() {
    fn paeth(a: u8, b: u8, c: u8) -> u8 {
        let p = i32::from(a) + i32::from(b) - i32::from(c);
        let (pa, pb, pc) = (
            (p - i32::from(a)).abs(),
            (p - i32::from(b)).abs(),
            (p - i32::from(c)).abs(),
        );
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    }
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    };
    for bpp in 1..=8usize {
        for _ in 0..50 {
            let prev: Vec<u8> = (0..32).map(|_| next()).collect();
            let raw: Vec<u8> = (0..32).map(|_| next()).collect();
            let mut got = raw.clone();
            assert!(crate::filter::unfilter(4, bpp, &mut got, &prev));
            let mut want = raw.clone();
            for i in 0..want.len() {
                let (a, c) = if i >= bpp {
                    (want[i - bpp], prev[i - bpp])
                } else {
                    (0, 0)
                };
                want[i] = want[i].wrapping_add(paeth(a, prev[i], c));
            }
            assert_eq!(got, want, "bpp {bpp}");
        }
    }
}

#[test]
fn bad_filter_type() {
    let data = simple(1, 1, 8, 0, 0, &[5, 0]);
    for p in BOTH {
        assert_eq!(
            run(&data, Format::Native, p).unwrap_err(),
            Error::Format("bad filter type")
        );
    }
}

#[test]
fn every_depth_and_colour_type_decodes() {
    let combos: [(u8, u8); 15] = [
        (1, 0),
        (2, 0),
        (4, 0),
        (8, 0),
        (16, 0),
        (8, 2),
        (16, 2),
        (1, 3),
        (2, 3),
        (4, 3),
        (8, 3),
        (8, 4),
        (16, 4),
        (8, 6),
        (16, 6),
    ];
    for (depth, ct) in combos {
        let samples = match ct {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        };
        let row_bytes = (samples * usize::from(depth)).div_ceil(8);
        let mut raw = vec![0u8];
        raw.extend(std::iter::repeat_n(0u8, row_bytes));
        let mut chunks = vec![(b"IHDR", ihdr(1, 1, depth, ct, 0))];
        if ct == 3 {
            chunks.push((b"PLTE", vec![9, 8, 7]));
        }
        chunks.push((b"IDAT", zlib(&raw)));
        chunks.push((b"IEND", vec![]));
        let data = build(&chunks);
        for p in BOTH {
            for fmt in [Format::Native, Format::Eight] {
                let (info, out) = run(&data, fmt, p)
                    .unwrap_or_else(|e| panic!("depth {depth} ct {ct} {p:?} {fmt:?}: {e}"));
                assert_eq!(Some(out.len()), info.output_size(fmt));
                if fmt == Format::Native {
                    assert_eq!(out.len(), row_bytes);
                }
            }
        }
    }
    // Indexed, Eight: the palette entry.
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 3, 0)),
        (b"PLTE", vec![9, 8, 7]),
        (b"IDAT", zlib(&[0, 0])),
        (b"IEND", vec![]),
    ]);
    assert_eq!(run(&data, Format::Eight, Profile::Go).unwrap().1, [9, 8, 7]);
}

#[test]
fn indexed_with_trns() {
    let data = build(&[
        (b"IHDR", ihdr(4, 1, 2, 3, 0)),
        (b"PLTE", vec![1, 2, 3, 4, 5, 6]),
        (b"tRNS", vec![128]),
        (b"IDAT", zlib(&[0, 0b00_01_10_11])),
        (b"IEND", vec![]),
    ]);
    for p in BOTH {
        let (info, native) = run(&data, Format::Native, p).unwrap();
        assert_eq!(native, [0b00_01_10_11]);
        assert_eq!(info.palette, [[1, 2, 3], [4, 5, 6]]);
        assert_eq!(info.trns, Some(Trns::Alpha(vec![128])));
        let (_, eight) = run(&data, Format::Eight, p).unwrap();
        assert_eq!(
            eight,
            [1, 2, 3, 128, 4, 5, 6, 255, 0, 0, 0, 255, 0, 0, 0, 255]
        );
    }
}

#[test]
fn trns_colour_key() {
    let data = build(&[
        (b"IHDR", ihdr(3, 1, 8, 0, 0)),
        (b"tRNS", vec![0, 5]),
        (b"IDAT", zlib(&[0, 4, 5, 6])),
        (b"IEND", vec![]),
    ]);
    for p in BOTH {
        let (info, eight) = run(&data, Format::Eight, p).unwrap();
        assert_eq!(info.trns, Some(Trns::Gray(5)));
        assert_eq!(eight, [4, 255, 5, 0, 6, 255]);
        let (_, native) = run(&data, Format::Native, p).unwrap();
        assert_eq!(native, [4, 5, 6]);
    }
}

/// The 3x3 gray image 1..=9 as Adam7 passes 0, 3, 4, 5, 6 (the others have no pixels).
fn adam7_3x3() -> Vec<u8> {
    let raw = [0, 1, 0, 3, 0, 7, 9, 0, 2, 0, 8, 0, 4, 5, 6];
    simple(3, 3, 8, 0, 1, &raw)
}

/// A 3x1 2-bit gray image with the values 1, 2, 3 as Adam7 passes 0, 3 and 5.
fn adam7_2bit() -> Vec<u8> {
    simple(3, 1, 2, 0, 1, &[0, 0x40, 0, 0xc0, 0, 0x80])
}

#[test]
fn adam7_merge() {
    for p in BOTH {
        let (info, out) = run(&adam7_3x3(), Format::Native, p).unwrap();
        assert!(info.header.interlaced);
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8, 9], "{p:?}");
        assert_eq!(
            run(&adam7_2bit(), Format::Native, p).unwrap().1,
            [0b01_10_11_00]
        );
        assert_eq!(
            run(&adam7_2bit(), Format::Eight, p).unwrap().1,
            [85, 170, 255]
        );
    }
}

#[test]
fn rows_equal_decode() {
    let one = simple(3, 3, 8, 0, 0, &[0, 10, 20, 30, 2, 1, 1, 1, 1, 5, 1, 1]);
    for data in [one, adam7_3x3(), adam7_2bit()] {
        for p in BOTH {
            for fmt in [Format::Native, Format::Eight] {
                let want = run(&data, fmt, p).unwrap();
                let mut dec = Decoder::new();
                let cancel = AtomicBool::new(false);
                let mut rows = dec.rows(&data, fmt, &opts(p), &limits(), &cancel).unwrap();
                let mut buf = vec![0u8; rows.row_bytes()];
                let mut got = Vec::new();
                let mut n = 0;
                while let Some(y) = rows.next_row(&mut buf).unwrap() {
                    assert_eq!(y, n);
                    n += 1;
                    got.extend_from_slice(&buf);
                }
                assert_eq!(&got, &want.1);
                assert_eq!(rows.info(), &want.0);
                // Done stays done.
                assert_eq!(rows.next_row(&mut buf).unwrap(), None);
            }
        }
    }
}

#[test]
fn decoder_reuse_gives_the_same_output() {
    let a = adam7_3x3();
    let b = simple(2, 2, 8, 0, 0, &[0, 1, 2, 0, 3, 4]);
    let cancel = AtomicBool::new(false);
    let mut dec = Decoder::new();
    for _ in 0..2 {
        for data in [&a, &b, &a] {
            let want = run(data, Format::Native, Profile::Go).unwrap();
            let mut out = vec![0u8; want.1.len()];
            let info = dec
                .decode_into(
                    data,
                    Format::Native,
                    &mut out,
                    &Options::default(),
                    &limits(),
                    &cancel,
                )
                .unwrap();
            assert_eq!(info, &want.0);
            assert_eq!(out, want.1);
        }
    }
}

#[test]
fn header_validation_go() {
    let cases: [(Vec<u8>, Error); 9] = [
        (
            ihdr_raw(0, 1, 8, 0, 0, 0, 0),
            Error::Format("non-positive dimension"),
        ),
        (
            ihdr_raw(0x8000_0000, 1, 8, 0, 0, 0, 0),
            Error::Format("non-positive dimension"),
        ),
        (
            ihdr_raw(1, 1, 8, 0, 1, 0, 0),
            Error::Unsupported("compression method"),
        ),
        (
            ihdr_raw(1, 1, 8, 0, 0, 1, 0),
            Error::Unsupported("filter method"),
        ),
        (
            ihdr_raw(1, 1, 8, 0, 0, 0, 2),
            Error::Format("invalid interlace method"),
        ),
        (
            ihdr_raw(1, 1, 3, 0, 0, 0, 0),
            Error::UnsupportedDepthColor {
                bit_depth: 3,
                color_type: 0,
            },
        ),
        (
            ihdr_raw(1, 1, 8, 1, 0, 0, 0),
            Error::UnsupportedDepthColor {
                bit_depth: 8,
                color_type: 1,
            },
        ),
        (
            ihdr_raw(1, 1, 16, 3, 0, 0, 0),
            Error::UnsupportedDepthColor {
                bit_depth: 16,
                color_type: 3,
            },
        ),
        (
            ihdr_raw(1, 1, 1, 2, 0, 0, 0),
            Error::UnsupportedDepthColor {
                bit_depth: 1,
                color_type: 2,
            },
        ),
    ];
    for (body, want) in cases {
        let data = build(&[(b"IHDR", body)]);
        assert_eq!(
            read_info(&data, &opts(Profile::Go), &limits()).unwrap_err(),
            want
        );
        // libpng rejects every one of them too.
        assert!(read_info(&data, &opts(Profile::Libpng), &limits()).is_err());
    }
    let short = build(&[(b"IHDR", vec![0; 12])]);
    assert_eq!(
        read_info(&short, &Options::default(), &limits()).unwrap_err(),
        Error::Format("bad IHDR length")
    );
}

#[test]
fn signature_crc_and_truncation() {
    let good = simple(1, 1, 8, 0, 0, &[0, 7]);
    for p in BOTH {
        let mut bad = good.clone();
        bad[0] = 0;
        assert_eq!(run(&bad, Format::Native, p).unwrap_err(), Error::NotPng);
        assert_eq!(
            run(&good[..5], Format::Native, p).unwrap_err(),
            Error::UnexpectedEof
        );
        // A flipped bit of the IHDR CRC.
        let mut bad = good.clone();
        bad[29] ^= 1;
        assert_eq!(run(&bad, Format::Native, p).unwrap_err(), Error::Checksum);
        // IEND cut off.
        let cut = &good[..good.len() - 6];
        assert_eq!(
            run(cut, Format::Native, p).unwrap_err(),
            Error::UnexpectedEof
        );
    }
}

#[test]
fn chunk_order_go() {
    let plte = (b"PLTE", vec![1, 2, 3]);
    let idat = (b"IDAT", zlib(&[0, 0]));
    let end = (b"IEND", vec![]);
    // Paletted without PLTE; PLTE after IDAT; IDAT first; IEND first.
    let a = build(&[(b"IHDR", ihdr(1, 1, 8, 3, 0)), idat.clone(), end.clone()]);
    let b = build(&[
        (b"IHDR", ihdr(1, 1, 8, 3, 0)),
        plte.clone(),
        idat.clone(),
        plte.clone(),
        end.clone(),
    ]);
    let c = build(&[idat.clone(), end.clone()]);
    let d = build(&[(b"IHDR", ihdr(1, 1, 8, 0, 0)), end.clone()]);
    for data in [a, b, c, d] {
        assert_eq!(
            run(&data, Format::Native, Profile::Go).unwrap_err(),
            Error::ChunkOrder
        );
    }
}

#[test]
fn not_enough_pixel_data() {
    // Two rows promised, one given.
    let data = simple(2, 2, 8, 0, 0, &[0, 1, 2]);
    for p in BOTH {
        assert_eq!(
            run(&data, Format::Native, p).unwrap_err(),
            Error::Format("not enough pixel data")
        );
    }
}

#[test]
fn bad_idat_crc_is_an_error_in_both_profiles() {
    let mut data = SIG.to_vec();
    chunk_with_crc(&mut data, b"IHDR", &ihdr(1, 1, 8, 0, 0), 0);
    chunk_with_crc(&mut data, b"IDAT", &zlib(&[0, 1]), 1);
    chunk_with_crc(&mut data, b"IEND", &[], 0);
    for p in BOTH {
        assert_eq!(run(&data, Format::Native, p).unwrap_err(), Error::Checksum);
    }
}

#[test]
fn libpng_profile_differences() {
    let ihdr_c = (b"IHDR", ihdr(1, 1, 8, 0, 0));
    let pixels = zlib(&[0, 7]);

    // A bad CRC in an ancillary chunk: Go fails, libpng drops the chunk.
    let mut d = SIG.to_vec();
    chunk_with_crc(&mut d, ihdr_c.0, &ihdr_c.1, 0);
    chunk_with_crc(&mut d, b"tEXt", b"k\0v", 1);
    chunk_with_crc(&mut d, b"IDAT", &pixels, 0);
    chunk_with_crc(&mut d, b"IEND", &[], 0);
    assert_eq!(
        run(&d, Format::Native, Profile::Go).unwrap_err(),
        Error::Checksum
    );
    let (info, out) = run(&d, Format::Native, Profile::Libpng).unwrap();
    assert!(info.text.is_empty());
    assert_eq!(out, [7]);

    // An unknown critical chunk: Go skips it, libpng fails. An unknown ancillary one is skipped.
    let unknown = |name: &[u8; 4]| {
        build(&[
            (b"IHDR", ihdr(1, 1, 8, 0, 0)),
            (name, vec![1, 2, 3]),
            (b"IDAT", pixels.clone()),
            (b"IEND", vec![]),
        ])
    };
    assert!(run(&unknown(b"ABCD"), Format::Native, Profile::Go).is_ok());
    assert_eq!(
        run(&unknown(b"ABCD"), Format::Native, Profile::Libpng).unwrap_err(),
        Error::Format("unknown critical chunk")
    );
    assert!(run(&unknown(b"abcd"), Format::Native, Profile::Libpng).is_ok());

    // Compressed data after the pixels, past the 4096 bytes Go's reader holds.
    let mut long = pixels.clone();
    long.extend_from_slice(&[0; 5000]);
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"IDAT", long),
        (b"IEND", vec![]),
    ]);
    assert_eq!(
        run(&data, Format::Native, Profile::Go).unwrap_err(),
        Error::Format("too much pixel data")
    );
    assert_eq!(run(&data, Format::Native, Profile::Libpng).unwrap().1, [7]);

    // A wrong Adler-32.
    let mut z = pixels.clone();
    let n = z.len();
    z[n - 1] ^= 1;
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"IDAT", z),
        (b"IEND", vec![]),
    ]);
    assert_eq!(
        run(&data, Format::Native, Profile::Go).unwrap_err(),
        Error::Trailing(deflate::Error::Checksum)
    );
    // libpng sees the trailer in the call of the last row: an error (see libpng_zlib_tail).
    assert_eq!(
        run(&data, Format::Native, Profile::Libpng).unwrap_err(),
        Error::Trailing(deflate::Error::Checksum)
    );

    // A palette longer than the bit depth allows: Go fails, libpng cuts it off.
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 1, 3, 0)),
        (b"PLTE", vec![1, 1, 1, 2, 2, 2, 3, 3, 3]),
        (b"IDAT", zlib(&[0, 0x80])),
        (b"IEND", vec![]),
    ]);
    assert_eq!(
        run(&data, Format::Native, Profile::Go).unwrap_err(),
        Error::Format("bad PLTE length")
    );
    let (info, out) = run(&data, Format::Eight, Profile::Libpng).unwrap();
    assert_eq!(info.palette.len(), 2);
    assert_eq!(out, [2, 2, 2]);

    // A tRNS longer than the palette: Go keeps it, libpng drops it.
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 3, 0)),
        (b"PLTE", vec![1, 2, 3]),
        (b"tRNS", vec![10, 20]),
        (b"IDAT", zlib(&[0, 0])),
        (b"IEND", vec![]),
    ]);
    assert!(
        run(&data, Format::Native, Profile::Go)
            .unwrap()
            .0
            .trns
            .is_some()
    );
    assert!(
        run(&data, Format::Native, Profile::Libpng)
            .unwrap()
            .0
            .trns
            .is_none()
    );

    // IEND missing: an error for both.
    let mut cut = simple(1, 1, 8, 0, 0, &[0, 7]);
    cut.truncate(cut.len() - 12);
    for p in BOTH {
        assert_eq!(
            run(&cut, Format::Native, p).unwrap_err(),
            Error::UnexpectedEof
        );
    }
}

#[test]
fn metadata_chunks() {
    let mut chrm = Vec::new();
    for v in [31270u32, 32900, 64000, 33000, 30000, 60000, 15000, 6000] {
        chrm.extend_from_slice(&v.to_be_bytes());
    }
    let mut profile = vec![0u8; 132];
    profile[..4].copy_from_slice(&132u32.to_be_bytes());
    let mut iccp = b"icc\0\0".to_vec();
    iccp.extend_from_slice(&zlib(&profile));
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"gAMA", 45455u32.to_be_bytes().to_vec()),
        (b"sRGB", vec![1]),
        (b"cHRM", chrm),
        (b"iCCP", iccp),
        (b"acTL", vec![0, 0, 0, 2, 0, 0, 0, 0]),
        (b"tEXt", b"Comment\0hello".to_vec()),
        (b"eXIf", b"II*\0abcd".to_vec()),
        (b"fcTL", vec![0; 26]),
        (b"IDAT", zlib(&[0, 7])),
        (b"tEXt", b"After\0x".to_vec()),
        (b"IEND", vec![]),
    ]);
    for p in BOTH {
        let before = read_info(&data, &opts(p), &limits()).unwrap();
        assert_eq!(before.text.len(), 1, "{p:?}");
        let (info, out) = run(&data, Format::Native, p).unwrap();
        assert_eq!(out, [7]);
        assert_eq!(info.gamma, Some(45455));
        assert_eq!(info.srgb, Some(1));
        assert_eq!(info.chrm.map(|c| c[0]), Some(31270));
        let icc = info.iccp.as_ref().unwrap();
        assert_eq!((icc.name.as_slice(), icc.profile.len()), (&b"icc"[..], 132));
        assert_eq!(
            info.apng,
            Some(Apng {
                num_frames: 2,
                num_plays: 0
            })
        );
        assert_eq!(info.exif.as_deref(), Some(&b"II*\0abcd"[..]));
        assert_eq!(info.text.len(), 2);
        assert!(info.text.iter().all(|t| t.kind == TextKind::Text));
        assert_eq!(info.text[1].data, b"After\0x");
    }
}

#[test]
fn metadata_in_the_wrong_place_or_invalid_is_dropped() {
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 1, 3, 0)),
        (b"PLTE", vec![1, 2, 3]),
        (b"gAMA", 45455u32.to_be_bytes().to_vec()), // after PLTE
        (b"sRGB", vec![9]),                         // invalid
        (b"IDAT", zlib(&[0, 0])),
        (b"IEND", vec![]),
    ]);
    for p in BOTH {
        let (info, _) = run(&data, Format::Native, p).unwrap();
        assert_eq!((info.gamma, info.srgb), (None, None), "{p:?}");
    }
}

#[test]
fn limits_and_cancel() {
    let data = simple(3, 2, 8, 0, 0, &[0, 1, 2, 3, 0, 4, 5, 6]);
    let cancel = AtomicBool::new(false);
    let run_with = |l: &Limits| decode(&data, Format::Native, &Options::default(), l, &cancel);
    let d = limits();
    assert_eq!(
        run_with(&Limits { max_width: 2, ..d }).unwrap_err(),
        Error::WidthLimit
    );
    assert_eq!(
        run_with(&Limits { max_height: 1, ..d }).unwrap_err(),
        Error::HeightLimit
    );
    assert_eq!(
        run_with(&Limits { max_pixels: 5, ..d }).unwrap_err(),
        Error::PixelLimit
    );
    assert!(run_with(&Limits { max_pixels: 6, ..d }).is_ok());
    // The output is 6 bytes.
    assert_eq!(
        run_with(&Limits { max_alloc: 5, ..d }).unwrap_err(),
        Error::AllocLimit
    );
    assert_eq!(
        run_with(&Limits { max_alloc: 0, ..d }).unwrap_err(),
        Error::AllocLimit
    );

    let mut out = [0u8; 5];
    assert_eq!(
        Decoder::new()
            .decode_into(
                &data,
                Format::Native,
                &mut out,
                &Options::default(),
                &d,
                &cancel
            )
            .unwrap_err(),
        Error::BufferTooSmall
    );

    let stop = AtomicBool::new(true);
    assert_eq!(
        decode(&data, Format::Native, &Options::default(), &d, &stop).unwrap_err(),
        Error::Cancelled
    );

    // max_metadata: both profiles drop the chunk.
    let text = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"tEXt", b"Comment\0hello".to_vec()),
        (b"IDAT", zlib(&[0, 7])),
        (b"IEND", vec![]),
    ]);
    let l = Limits {
        max_metadata: 4,
        ..d
    };
    for p in BOTH {
        let (info, out) = decode(&text, Format::Native, &opts(p), &l, &cancel).unwrap();
        assert!(info.text.is_empty(), "{p:?}");
        assert_eq!(out, [7]);
    }
}

#[test]
fn errors_display_go_text() {
    assert_eq!(
        Error::Checksum.to_string(),
        "png: invalid format: invalid checksum"
    );
    assert_eq!(
        Error::Unsupported("filter method").to_string(),
        "png: unsupported feature: filter method"
    );
    assert_eq!(
        Error::Trailing(deflate::Error::UnexpectedEof).to_string(),
        "png: invalid format: unexpected EOF"
    );
    assert_eq!(Error::UnexpectedEof.to_string(), "unexpected EOF");
    assert_eq!(
        Error::UnsupportedDepthColor {
            bit_depth: 3,
            color_type: 0
        }
        .to_string(),
        "png: unsupported feature: bit depth 3, color type 0"
    );
    assert_eq!(
        Error::BadChunkLength(0x8000_0000).to_string(),
        "png: invalid format: Bad chunk length: 2147483648"
    );
}

#[test]
fn native_padding_bits_are_zero() {
    // 1-bit gray, width 3: the five unused bits of the row byte are set in the file.
    for (il, raw) in [(0, vec![0, 0xff]), (1, vec![0, 0xff, 0, 0xff, 0, 0xff])] {
        let data = simple(3, 1, 1, 0, il, &raw);
        for p in BOTH {
            assert_eq!(run(&data, Format::Native, p).unwrap().1, [0xe0], "{p:?}");
        }
    }
    // 4-bit, width 3: the low nibble of the second byte.
    let data = simple(3, 1, 4, 0, 0, &[0, 0x12, 0x3f]);
    assert_eq!(
        run(&data, Format::Native, Profile::Go).unwrap().1,
        [0x12, 0x30]
    );
}

/// IHDR (1x1 gray), the `parts` as IDAT chunks, IEND.
fn idat_parts(parts: &[&[u8]]) -> Vec<u8> {
    let mut v = vec![(b"IHDR", ihdr(1, 1, 8, 0, 0))];
    v.extend(parts.iter().map(|p| (b"IDAT", p.to_vec())));
    v.push((b"IEND", vec![]));
    build(&v)
}

fn flip_last(mut z: Vec<u8>) -> Vec<u8> {
    if let Some(b) = z.last_mut() {
        *b ^= 1;
    }
    z
}

#[test]
fn libpng_zlib_tail() {
    let lp = |d: &[u8]| run(d, Format::Native, Profile::Libpng);
    let go = |d: &[u8]| run(d, Format::Native, Profile::Go);
    // The stream is 13 bytes: header 2, stored block header 5, pixels 2, Adler-32 4.
    let z = zlib(&[0, 7]);
    let bad = flip_last(z.clone());

    // A good stream split in two chunks.
    assert_eq!(lp(&idat_parts(&[&z[..9], &z[9..]])).unwrap().1, [7]);

    // A bad Adler-32 that arrives with the end of the data is an error; one that arrives in
    // the next chunk (another zlib input piece) is a warning.
    assert_eq!(
        lp(&idat_parts(&[&bad])).unwrap_err(),
        Error::Trailing(deflate::Error::Checksum)
    );
    assert_eq!(lp(&idat_parts(&[&bad[..9], &bad[9..]])).unwrap().1, [7]);
    assert!(go(&idat_parts(&[&bad[..9], &bad[9..]])).is_err());

    // A missing trailer is an error: libpng wants more IDAT data and finds IEND.
    assert!(lp(&idat_parts(&[&z[..9]])).is_err());
    assert!(lp(&idat_parts(&[&z[..11]])).is_err());

    // Corrupt data after the last row, in the input libpng's last row call has: an error.
    // A stored block (not the last) with the pixels, then a final block of the invalid type 3.
    let corrupt = [0x78, 0x01, 0, 2, 0, 0xfd, 0xff, 0, 7, 0x07];
    assert!(lp(&idat_parts(&[&corrupt])).is_err());

    // Extra pixel data ends that call early: a bad Adler-32 or corrupt data after it is a
    // warning, as is the extra data itself.
    let more = zlib(&[0, 7, 9]);
    assert!(go(&idat_parts(&[&more])).is_err());
    assert_eq!(lp(&idat_parts(&[&more])).unwrap().1, [7]);
    assert_eq!(lp(&idat_parts(&[&flip_last(more.clone())])).unwrap().1, [7]);
    let corrupt = [0x78, 0x01, 0, 3, 0, 0xfc, 0xff, 0, 7, 9, 0x07];
    assert_eq!(lp(&idat_parts(&[&corrupt])).unwrap().1, [7]);
}

fn lp(d: &[u8]) -> Result<(Info, Vec<u8>), Error> {
    run(d, Format::Native, Profile::Libpng)
}

#[test]
fn libpng_unknown_chunk_before_ihdr_is_skipped() {
    let with = |typ: &[u8; 4]| {
        build(&[
            (typ, vec![1, 2, 3]),
            (b"IHDR", ihdr(1, 1, 8, 0, 0)),
            (b"IDAT", zlib(&[0, 7])),
            (b"IEND", vec![]),
        ])
    };
    // png_handle_unknown does not look for IHDR; cICP and acTL are unknown to libpng 1.6.44.
    for t in [b"zzZz", b"cICP", b"acTL"] {
        let (info, out) = lp(&with(t)).unwrap();
        assert_eq!(out, [7]);
        assert!(info.cicp.is_none() && info.apng.is_none());
    }
    // The chunks libpng has a handler for fail there; an unknown critical one fails too.
    for t in [b"tEXt", b"gAMA", b"PLTE", b"tRNS", b"IDAT"] {
        assert_eq!(lp(&with(t)).unwrap_err(), Error::Format("missing IHDR"));
    }
    assert_eq!(
        lp(&with(b"ABCD")).unwrap_err(),
        Error::Format("unknown critical chunk")
    );
    // The CRC of the skipped chunk is checked: a bad one is dropped, as for any ancillary chunk.
    let mut d = SIG.to_vec();
    chunk_with_crc(&mut d, b"zzZz", &[1], 1);
    chunk_with_crc(&mut d, b"IHDR", &ihdr(1, 1, 8, 0, 0), 0);
    chunk_with_crc(&mut d, b"IDAT", &zlib(&[0, 7]), 0);
    chunk_with_crc(&mut d, b"IEND", &[], 0);
    assert_eq!(lp(&d).unwrap().1, [7]);
}

#[test]
fn libpng_plte_rules() {
    let rgb = |chunks: &[(&[u8; 4], Vec<u8>)]| {
        let mut v = vec![(b"IHDR", ihdr(1, 1, 8, 2, 0))];
        v.extend(chunks.iter().cloned());
        build(&v)
    };
    let pixels = (b"IDAT", zlib(&[0, 1, 2, 3]));
    let end = (b"IEND", vec![]);
    let plte = (b"PLTE", vec![9, 9, 9]);
    // A duplicate PLTE is an error, also after IDAT (libpng checks it before the place).
    let dup = rgb(&[plte.clone(), pixels.clone(), plte.clone(), end.clone()]);
    assert_eq!(lp(&dup).unwrap_err(), Error::ChunkOrder);
    // A first PLTE after IDAT is ignored.
    let late = rgb(&[pixels.clone(), plte.clone(), end.clone()]);
    assert_eq!(lp(&late).unwrap().1, [1, 2, 3]);
    // An empty PLTE is "Invalid palette" for every colour type that reads it.
    let empty = (b"PLTE", Vec::<u8>::new());
    assert_eq!(
        lp(&rgb(&[empty.clone(), pixels.clone(), end.clone()])).unwrap_err(),
        Error::Format("Invalid palette")
    );
    let indexed = build(&[
        (b"IHDR", ihdr(1, 1, 8, 3, 0)),
        empty.clone(),
        (b"IDAT", zlib(&[0, 0])),
        end.clone(),
    ]);
    assert_eq!(lp(&indexed).unwrap_err(), Error::Format("Invalid palette"));
    // A gray image ignores its PLTE, whatever it holds.
    let gray = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        empty,
        (b"IDAT", zlib(&[0, 7])),
        end,
    ]);
    assert_eq!(lp(&gray).unwrap().1, [7]);
}

#[test]
fn libpng_rgb_trns_before_plte_is_cancelled() {
    let trns = (b"tRNS", vec![0, 1, 0, 2, 0, 3]);
    let plte = (b"PLTE", vec![9, 9, 9]);
    let make = |chunks: &[(&[u8; 4], Vec<u8>)]| {
        let mut v = vec![(b"IHDR", ihdr(1, 1, 8, 2, 0))];
        v.extend(chunks.iter().cloned());
        v.push((b"IDAT", zlib(&[0, 1, 2, 3])));
        v.push((b"IEND", vec![]));
        build(&v)
    };
    let before = make(&[trns.clone(), plte.clone()]);
    let after = make(&[plte.clone(), trns.clone()]);
    // tRNS first: libpng sets num_trans to 0 when the PLTE comes, so no alpha channel; the
    // chunk is still reported.
    let (info, eight) = run(&before, Format::Eight, Profile::Libpng).unwrap();
    assert_eq!(eight, [1, 2, 3]);
    assert!(info.trns.is_some() && !info.has_trns());
    assert_eq!(info.output_size(Format::Eight), Some(3));
    // PLTE first: the key applies (the pixel equals it, so it is transparent).
    let (info, eight) = run(&after, Format::Eight, Profile::Libpng).unwrap();
    assert_eq!(eight, [1, 2, 3, 0]);
    assert!(info.has_trns());
    // Without a PLTE at all the key applies, and Go cannot see the first case at all.
    let plain = make(&[trns]);
    assert_eq!(
        run(&plain, Format::Eight, Profile::Libpng).unwrap().1,
        [1, 2, 3, 0]
    );
    assert_eq!(
        run(&before, Format::Eight, Profile::Go).unwrap_err(),
        Error::ChunkOrder
    );
    // The flag does not leak into the next call of a reused decoder.
    let cancel = AtomicBool::new(false);
    let mut dec = Decoder::new();
    let mut out = [0u8; 4];
    let o = opts(Profile::Libpng);
    let mut three = [0u8; 3];
    dec.decode_into(&before, Format::Eight, &mut three, &o, &limits(), &cancel)
        .unwrap();
    assert!(
        dec.decode_into(&plain, Format::Eight, &mut out, &o, &limits(), &cancel)
            .unwrap()
            .has_trns()
    );
}

/// An iCCP chunk of `name` and a profile of `n` zero bytes (stored zlib blocks).
fn iccp_zeros(n: usize) -> (&'static [u8; 4], Vec<u8>) {
    let mut body = b"p\0\0".to_vec();
    body.extend_from_slice(&zlib(&vec![0u8; n]));
    (b"iCCP", body)
}

#[test]
fn go_profile_never_fails_on_an_ancillary_chunk() {
    let mut bad_icc = b"icc\0\0".to_vec();
    bad_icc.extend_from_slice(b"garbage");
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"gAMA", vec![1]),
        (b"zTXt", b"k\0\0not zlib".to_vec()),
        (b"iCCP", bad_icc),
        iccp_zeros(64 << 10), // over max_metadata
        (b"eXIf", vec![1]),
        (b"tEXt", vec![b'a'; 200]), // over max_metadata
        (b"IDAT", zlib(&[0, 7])),
        (b"IEND", vec![]),
    ]);
    let l = Limits {
        max_metadata: 100,
        ..limits()
    };
    let cancel = AtomicBool::new(false);
    let go = opts(Profile::Go);
    let info = read_info(&data, &go, &l).unwrap();
    assert!(info.iccp.is_none() && info.exif.is_none() && info.gamma.is_none());
    // Go does not look inside zTXt: it is kept raw.
    assert_eq!(info.text.len(), 1);
    let (info, out) = decode(&data, Format::Native, &go, &l, &cancel).unwrap();
    assert_eq!((out.as_slice(), info.text.len()), (&[7u8][..], 1));
    // The Libpng profile inflates it, finds it bad and drops it.
    let (info, _) = decode(&data, Format::Native, &opts(Profile::Libpng), &l, &cancel).unwrap();
    assert!(info.text.is_empty());
}

#[test]
fn go_profile_drops_an_iccp_that_does_not_fit_max_alloc() {
    // The profile inflates to 3 MiB, max_alloc is 1 MiB: the chunk is dropped and the booking
    // of its steps is given back, so the pixels still decode.
    let data = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        iccp_zeros(3 << 20),
        (b"IDAT", zlib(&[0, 7])),
        (b"IEND", vec![]),
    ]);
    let l = Limits {
        max_alloc: 1 << 20,
        ..limits()
    };
    let (info, out) = decode(
        &data,
        Format::Native,
        &opts(Profile::Go),
        &l,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(info.iccp.is_none());
    assert_eq!(out, [7]);
}

#[test]
fn each_kept_text_chunk_counts_at_least_16_bytes() {
    let mut v = vec![(b"IHDR", ihdr(1, 1, 8, 0, 0))];
    v.extend(std::iter::repeat_n((b"tEXt", Vec::<u8>::new()), 5));
    v.push((b"IDAT", zlib(&[0, 7])));
    v.push((b"IEND", vec![]));
    let data = build(&v);
    let l = Limits {
        max_metadata: 40,
        ..limits()
    };
    for p in BOTH {
        let (info, _) =
            decode(&data, Format::Native, &opts(p), &l, &AtomicBool::new(false)).unwrap();
        assert_eq!(info.text.len(), 2, "{p:?}");
    }
}

#[test]
fn a_reused_decoder_does_not_charge_a_pooled_iccp_that_a_fresh_one_would_not_hold() {
    let wrap = |icc: (&'static [u8; 4], Vec<u8>)| {
        build(&[
            (b"IHDR", ihdr(1, 1, 8, 0, 0)),
            icc,
            (b"IDAT", zlib(&[0, 7])),
            (b"IEND", vec![]),
        ])
    };
    let (big, small) = (wrap(iccp_zeros(600_000)), wrap(iccp_zeros(200)));
    let cancel = AtomicBool::new(false);
    let o = Options::default();
    let tight = Limits {
        max_alloc: 400_000,
        ..limits()
    };
    let mut out = [0u8; 1];
    let fresh = Decoder::new()
        .decode_into(&small, Format::Native, &mut out, &o, &tight, &cancel)
        .map(|i| i.iccp.is_some());
    assert_eq!(fresh, Ok(true));
    // The first decode leaves a profile vector of about 1 MiB in the pool.
    let mut dec = Decoder::new();
    dec.decode_into(&big, Format::Native, &mut out, &o, &limits(), &cancel)
        .unwrap();
    let reused = dec
        .decode_into(&small, Format::Native, &mut out, &o, &tight, &cancel)
        .map(|i| i.iccp.is_some());
    assert_eq!(reused, Ok(true));
}

#[test]
fn decode_reads_the_cancel_flag_in_its_first_pass() {
    // The header pass used a flag of its own: a bad file gave its own error, not Cancelled.
    let stop = AtomicBool::new(true);
    assert_eq!(
        decode(
            b"not a png",
            Format::Native,
            &Options::default(),
            &limits(),
            &stop
        )
        .unwrap_err(),
        Error::Cancelled
    );
}

#[test]
fn the_threaded_path_threshold_is_a_parameter() {
    let threaded = Limits {
        max_threads: 4,
        ..Limits::default()
    };
    let bad_adler = build(&[
        (b"IHDR", ihdr(1, 1, 8, 0, 0)),
        (b"IDAT", flip_last(zlib(&[0, 7]))),
        (b"IEND", vec![]),
    ]);
    let good = simple(3, 3, 8, 0, 0, &[0, 10, 20, 30, 2, 1, 1, 1, 1, 5, 1, 1]);
    for data in [&good, &bad_adler, &adam7_3x3(), &adam7_2bit()] {
        for p in BOTH {
            for fmt in [Format::Native, Format::Eight] {
                let serial = run(data, fmt, p);
                // Threshold 0: the threaded path is tried on this small input; a fallback or
                // not, the result is the serial one.
                let par = crate::decoder::decode_with(
                    data,
                    fmt,
                    &opts(p),
                    &threaded,
                    &AtomicBool::new(false),
                    0,
                );
                assert_eq!(serial, par, "{p:?} {fmt:?}");
            }
        }
    }
}

/// A reused `Decoder` keeps the `Info` vectors but charges them as a fresh one does: the same
/// file twice under a tight `max_alloc` gives what a fresh decoder gives.
#[test]
fn reused_decoder_charges_kept_vectors() {
    let mut c: Vec<(&[u8; 4], Vec<u8>)> =
        vec![(b"IHDR", ihdr(1, 1, 8, 3, 0)), (b"PLTE", vec![7; 600])];
    c.extend((0..3000).map(|_| (b"tEXt", b"k\0".to_vec())));
    c.push((b"IDAT", zlib(&[0, 0])));
    c.push((b"IEND", vec![]));
    let data = build(&c);
    let big = limits();
    let tight = Limits {
        max_alloc: 1024 + 100_000,
        ..limits()
    };
    let run = |dec: &mut Decoder, l: &Limits, p| {
        let mut out = vec![0u8; 16];
        dec.decode_into(
            &data,
            Format::Native,
            &mut out,
            &opts(p),
            l,
            &AtomicBool::new(false),
        )
        .cloned()
    };
    for p in BOTH {
        let fresh_big = run(&mut Decoder::new(), &big, p);
        let fresh_tight = run(&mut Decoder::new(), &tight, p);
        assert_ne!(fresh_big, fresh_tight, "{p:?}");
        let mut dec = Decoder::new();
        assert_eq!(run(&mut dec, &big, p), fresh_big, "{p:?}");
        assert_eq!(run(&mut dec, &tight, p), fresh_tight, "{p:?}");
        assert_eq!(run(&mut dec, &tight, p), fresh_tight, "{p:?}");
    }
}
