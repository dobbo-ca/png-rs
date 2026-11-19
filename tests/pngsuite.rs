//! PngSuite integration tests: every file, both profiles, both formats; Rows vs decode_into;
//! decoder reuse; cancel; limits; known metadata.

use png::{Decoder, Error, Format, Limits, Options, Profile, TextKind, Trns};
use std::path::Path;
use std::sync::atomic::AtomicBool;

const PROFILES: [Profile; 2] = [Profile::Go, Profile::Libpng];
const FORMATS: [Format; 2] = [Format::Native, Format::Eight];

fn files() -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pngsuite");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    paths.sort();
    assert!(paths.len() > 100, "PngSuite files missing");
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .collect()
}

fn file(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/pngsuite")
            .join(format!("{name}.png")),
    )
    .unwrap()
}

fn opts(profile: Profile) -> Options {
    Options { profile }
}

fn is_bad(name: &str) -> bool {
    name.starts_with('x')
}

#[test]
fn every_file_decodes_or_fails() {
    let cancel = AtomicBool::new(false);
    let limits = Limits::default();
    for (name, data) in files() {
        for profile in PROFILES {
            for fmt in FORMATS {
                let r = png::decode(&data, fmt, &opts(profile), &limits, &cancel);
                if is_bad(&name) {
                    // xcsn0g01 may carry only a bad Adler-32, which the Libpng profile ignores
                    // by design; every other x file is an error in both profiles.
                    if name == "xcsn0g01" && profile == Profile::Libpng {
                        continue;
                    }
                    assert!(r.is_err(), "{name} {profile:?} {fmt:?} should fail");
                } else {
                    let (info, out) =
                        r.unwrap_or_else(|e| panic!("{name} {profile:?} {fmt:?}: {e:?}"));
                    assert_eq!(Some(out.len()), info.output_size(fmt), "{name}");
                }
            }
        }
    }
}

#[test]
fn rows_equal_decode_into() {
    let cancel = AtomicBool::new(false);
    let limits = Limits::default();
    for (name, data) in files().into_iter().filter(|(n, _)| !is_bad(n)) {
        for profile in PROFILES {
            for fmt in FORMATS {
                let o = opts(profile);
                let mut dec = Decoder::new();
                let size = png::read_info(&data, &o, &limits)
                    .unwrap()
                    .output_size(fmt)
                    .unwrap();
                let mut whole = vec![0u8; size];
                let info = dec
                    .decode_into(&data, fmt, &mut whole, &o, &limits, &cancel)
                    .unwrap()
                    .clone();

                let mut dec2 = Decoder::new();
                let mut rows = dec2.rows(&data, fmt, &o, &limits, &cancel).unwrap();
                assert_eq!(rows.info(), &info, "{name}");
                let rb = rows.row_bytes();
                let mut buf = vec![0u8; rb];
                let mut got = Vec::new();
                let mut y = 0u32;
                while let Some(row) = rows.next_row(&mut buf).unwrap() {
                    assert_eq!(row, y, "{name} row index");
                    y += 1;
                    got.extend_from_slice(&buf);
                }
                assert_eq!(y, info.header.height, "{name}");
                assert_eq!(got, whole, "{name} {profile:?} {fmt:?}");
            }
        }
    }
}

#[test]
fn decoder_reuse_equals_fresh() {
    let cancel = AtomicBool::new(false);
    let limits = Limits::default();
    let all = files();
    for profile in PROFILES {
        for fmt in FORMATS {
            let o = opts(profile);
            let mut dec = Decoder::new();
            // Two passes: the second reuses a decoder that has seen every file, bad ones included.
            for _ in 0..2 {
                for (name, data) in &all {
                    let fresh = png::decode(data, fmt, &o, &limits, &cancel);
                    let mut out = match &fresh {
                        Ok((_, b)) => vec![0u8; b.len()],
                        Err(_) => vec![0u8; 4 << 20],
                    };
                    let reused = dec
                        .decode_into(data, fmt, &mut out, &o, &limits, &cancel)
                        .cloned();
                    match (fresh, reused) {
                        (Ok((fi, fb)), Ok(ri)) => {
                            assert_eq!(fi, ri, "{name}");
                            assert_eq!(fb, out, "{name}");
                        }
                        (Err(a), Err(b)) => assert_eq!(a, b, "{name}"),
                        (a, b) => panic!(
                            "{name}: fresh {:?} vs reused {:?}",
                            a.map(|_| ()),
                            b.map(|_| ())
                        ),
                    }
                }
            }
        }
    }
}

#[test]
fn cancel_flag() {
    let data = file("basn2c08");
    let cancel = AtomicBool::new(true);
    let limits = Limits::default();
    let o = Options::default();
    assert_eq!(
        png::decode(&data, Format::Native, &o, &limits, &cancel).unwrap_err(),
        Error::Cancelled
    );
    let mut out = vec![0u8; 1 << 16];
    let mut dec = Decoder::new();
    assert_eq!(
        dec.decode_into(&data, Format::Native, &mut out, &o, &limits, &cancel)
            .unwrap_err(),
        Error::Cancelled
    );
    // Rows may refuse at creation or at the first row.
    let mut dec = Decoder::new();
    let mut buf = vec![0u8; 1 << 12];
    let err = match dec.rows(&data, Format::Native, &o, &limits, &cancel) {
        Err(e) => e,
        Ok(mut rows) => loop {
            match rows.next_row(&mut buf) {
                Err(e) => break e,
                Ok(Some(_)) => {}
                Ok(None) => panic!("rows finished despite cancel"),
            }
        },
    };
    assert_eq!(err, Error::Cancelled);
}

#[test]
fn limits() {
    let cancel = AtomicBool::new(false);
    let o = Options::default();
    let d = Limits::default();
    let data = file("basn0g01"); // 32x32
    let run = |l: &Limits| png::decode(&data, Format::Native, &o, l, &cancel).unwrap_err();
    assert_eq!(run(&Limits { max_width: 16, ..d }), Error::WidthLimit);
    assert_eq!(
        run(&Limits {
            max_height: 16,
            ..d
        }),
        Error::HeightLimit
    );
    assert_eq!(
        run(&Limits {
            max_pixels: 100,
            ..d
        }),
        Error::PixelLimit
    );
    // At the limit is fine.
    assert!(
        png::decode(
            &data,
            Format::Native,
            &o,
            &Limits {
                max_width: 32,
                max_height: 32,
                max_pixels: 1024,
                ..d
            },
            &cancel
        )
        .is_ok()
    );

    let text = file("ctzn0g04"); // zTXt
    // Over max_metadata the chunk is dropped; the decode goes on.
    let (info, _) = png::decode(
        &text,
        Format::Native,
        &o,
        &Limits {
            max_metadata: 0,
            ..d
        },
        &cancel,
    )
    .unwrap();
    assert!(info.text.is_empty());

    let mut out = vec![0u8; 10];
    let mut dec = Decoder::new();
    assert_eq!(
        dec.decode_into(&data, Format::Native, &mut out, &o, &d, &cancel)
            .unwrap_err(),
        Error::BufferTooSmall
    );
    let mut dec = Decoder::new();
    let mut rows = dec.rows(&data, Format::Native, &o, &d, &cancel).unwrap();
    let mut small = vec![0u8; 1];
    assert_eq!(
        rows.next_row(&mut small).unwrap_err(),
        Error::BufferTooSmall
    );
}

fn info(name: &str) -> png::Info {
    png::read_info(&file(name), &Options::default(), &Limits::default()).unwrap()
}

#[test]
fn known_metadata() {
    for (name, gamma) in [
        ("g03n0g16", 35000),
        ("g03n2c08", 35000),
        ("g03n3p04", 35000),
        ("g04n0g16", 45000),
        ("g05n2c08", 55000),
        ("g07n3p04", 70000),
        ("g10n0g16", 100000),
        ("g25n2c08", 250000),
    ] {
        assert_eq!(info(name).gamma, Some(gamma), "{name}");
    }
    // The basic PngSuite files carry gAMA 1.0.
    assert_eq!(info("basn0g01").gamma, Some(100000));
    assert!(info("exif2c08").gamma.is_none());

    assert!(info("ccwn2c08").chrm.is_some());
    assert!(info("ccwn3p08").chrm.is_some());
    assert!(info("basn2c08").chrm.is_none());
    assert!(info("exif2c08").exif.is_some());
    assert!(info("basn2c08").exif.is_none());

    assert!(info("ct0n0g04").text.is_empty());
    let t = info("ct1n0g04").text;
    assert!(!t.is_empty() && t.iter().all(|t| matches!(t.kind, TextKind::Text)));
    let t = info("ctzn0g04").text;
    assert!(t.iter().any(|t| matches!(t.kind, TextKind::Ztxt)));
    for name in ["cten0g04", "ctfn0g04", "ctgn0g04", "cthn0g04", "ctjn0g04"] {
        let t = info(name).text;
        assert!(t.iter().any(|t| matches!(t.kind, TextKind::Itxt)), "{name}");
    }

    assert!(matches!(info("tbwn0g16").trns, Some(Trns::Gray(_))));
    assert!(matches!(info("tbbn2c16").trns, Some(Trns::Rgb(_))));
    assert!(matches!(info("tbbn3p08").trns, Some(Trns::Alpha(_))));
    assert!(matches!(info("tm3n3p02").trns, Some(Trns::Alpha(_))));
    assert!(info("basn2c08").trns.is_none());

    assert_eq!(info("basn3p01").palette.len(), 2);
    assert!(info("basn3p08").palette.len() > 2);
    assert!(info("basn2c08").palette.is_empty());

    let h = info("basn6a16").header;
    assert_eq!(
        (h.width, h.height, h.bit_depth, h.interlaced),
        (32, 32, 16, false)
    );
    assert!(info("basi6a08").header.interlaced);
}

#[test]
fn eight_output_shapes() {
    let cancel = AtomicBool::new(false);
    let o = Options::default();
    let l = Limits::default();

    let (info, out) = png::decode(&file("basn0g01"), Format::Eight, &o, &l, &cancel).unwrap();
    assert_eq!(out.len(), (info.header.width * info.header.height) as usize);
    assert!(out.iter().all(|&b| b == 0x00 || b == 0xff));
    assert!(out.contains(&0x00) && out.contains(&0xff));

    let (info, out) = png::decode(&file("basn3p08"), Format::Eight, &o, &l, &cancel).unwrap();
    assert!(info.trns.is_none());
    assert_eq!(
        info.row_bytes(Format::Eight),
        Some(info.header.width as usize * 3)
    );
    assert_eq!(
        out.len(),
        (info.header.width * info.header.height) as usize * 3
    );
}
