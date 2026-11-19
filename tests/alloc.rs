//! Allocation behaviour under a counting global allocator. One test fn on purpose: the counters
//! are process-wide, so no other test may run in this binary.

use png::{Decoder, Error, Format, Limits, Options, Profile};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

fn grow(n: usize) {
    let c = CUR.fetch_add(n, SeqCst) + n;
    PEAK.fetch_max(c, SeqCst);
}

// SAFETY: forwards every call to `System` unchanged and only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            CALLS.fetch_add(1, SeqCst);
            grow(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            CALLS.fetch_add(1, SeqCst);
            grow(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        CUR.fetch_sub(l.size(), SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            CALLS.fetch_add(1, SeqCst);
            CUR.fetch_sub(l.size(), SeqCst);
            grow(new);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fn file(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/pngsuite")
            .join(format!("{name}.png")),
    )
    .unwrap()
}

/// Second decode_into with the same Decoder and output buffer allocates nothing.
fn zero_alloc(name: &str, fmt: Format) {
    let data = file(name);
    let (cancel, o, l) = (
        AtomicBool::new(false),
        Options::default(),
        Limits::default(),
    );
    let size = png::read_info(&data, &o, &l)
        .unwrap()
        .output_size(fmt)
        .unwrap();
    let mut out = vec![0u8; size];
    let mut dec = Decoder::new();
    assert!(
        dec.decode_into(&data, fmt, &mut out, &o, &l, &cancel)
            .is_ok()
    );
    let first = out.clone();

    let before = CALLS.load(SeqCst);
    let r = dec
        .decode_into(&data, fmt, &mut out, &o, &l, &cancel)
        .is_ok();
    let after = CALLS.load(SeqCst);
    assert!(r, "{name}");
    assert_eq!(
        after - before,
        0,
        "{name} {fmt:?}: second decode_into allocated"
    );
    assert_eq!(first, out, "{name}");
}

const VALUES: [usize; 9] = [0, 1, 64, 1024, 4096, 16 << 10, 64 << 10, 1 << 20, 1 << 30];

/// Peak bytes held during one decode call stay within max_alloc.
fn peak_decode(label: &str, data: &[u8], o: &Options, threads: usize, values: &[usize]) {
    let cancel = AtomicBool::new(false);
    for fmt in [Format::Native, Format::Eight] {
        for &max_alloc in values {
            let l = Limits {
                max_alloc,
                max_threads: threads,
                ..Limits::default()
            };
            let base = CUR.load(SeqCst);
            PEAK.store(base, SeqCst);
            let r = png::decode(data, fmt, o, &l, &cancel);
            let peak = PEAK.load(SeqCst).saturating_sub(base);
            let ok = r.is_ok();
            let err = r.err();
            assert!(
                peak <= max_alloc,
                "{label} {fmt:?} threads {threads} max_alloc {max_alloc}: peak {peak}"
            );
            if !ok {
                assert_eq!(
                    err,
                    Some(Error::AllocLimit),
                    "{label} {fmt:?} threads {threads} max_alloc {max_alloc}"
                );
            }
            if max_alloc == 1 << 30 {
                assert!(ok, "{label} {fmt:?} must fit in 1 GiB");
            }
            if max_alloc == 0 {
                assert!(!ok, "{label} {fmt:?} cannot decode with max_alloc 0");
            }
        }
    }
}

fn peak_within_max_alloc(name: &str, threads: usize) {
    peak_decode(name, &file(name), &Options::default(), threads, &VALUES);
}

/// The same for `Decoder::rows`, read to the end (the interlaced image is merged in memory).
fn peak_rows(name: &str) {
    let data = file(name);
    let (cancel, o) = (AtomicBool::new(false), Options::default());
    for fmt in [Format::Native, Format::Eight] {
        let rb = png::read_info(&data, &o, &Limits::default())
            .unwrap()
            .row_bytes(fmt)
            .unwrap();
        let mut buf = vec![0u8; rb];
        for max_alloc in VALUES {
            let l = Limits {
                max_alloc,
                max_threads: 1,
                ..Limits::default()
            };
            let base = CUR.load(SeqCst);
            PEAK.store(base, SeqCst);
            let mut dec = Decoder::new();
            let r = (|| {
                let mut rows = dec.rows(&data, fmt, &o, &l, &cancel)?;
                while rows.next_row(&mut buf)?.is_some() {}
                Ok::<(), Error>(())
            })();
            drop(dec);
            let peak = PEAK.load(SeqCst).saturating_sub(base);
            assert!(
                peak <= max_alloc,
                "rows {name} {fmt:?} max_alloc {max_alloc}: peak {peak}"
            );
            match r {
                Ok(()) => assert_ne!(max_alloc, 0, "rows {name} {fmt:?}"),
                Err(e) => {
                    assert_eq!(e, Error::AllocLimit, "rows {name} {fmt:?} {max_alloc}");
                    assert_ne!(max_alloc, 1 << 30, "rows {name} {fmt:?} must fit in 1 GiB");
                }
            }
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of stored blocks.
fn stored(raw: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(raw).to_be_bytes());
    z
}

/// An 8-bit gray PNG of `w` x `h` with pseudo-random rows of every filter type (stored zlib
/// blocks), IDAT chunks of 256 KiB, and the `before` chunks after IHDR and `after` ones after IDAT.
fn gray_png(
    w: u32,
    h: u32,
    before: &[(&[u8; 4], Vec<u8>)],
    after: &[(&[u8; 4], Vec<u8>)],
) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut raw = Vec::with_capacity((w as usize + 1) * h as usize);
    for y in 0..h {
        raw.push((y % 5) as u8);
        for _ in 0..w {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            raw.push((x >> 24) as u8);
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    for (t, d) in before {
        chunk(&mut png, t, d);
    }
    for c in stored(&raw).chunks(1 << 18) {
        chunk(&mut png, b"IDAT", c);
    }
    for (t, d) in after {
        chunk(&mut png, t, d);
    }
    chunk(&mut png, b"IEND", &[]);
    png
}

#[test]
fn allocation() {
    for name in ["basn2c08", "basn6a16", "basn3p08"] {
        zero_alloc(name, Format::Native);
        zero_alloc(name, Format::Eight);
    }
    for name in ["basi2c08", "basi6a16", "basi3p08"] {
        zero_alloc(name, Format::Native);
        zero_alloc(name, Format::Eight);
    }
    let many = std::thread::available_parallelism().map_or(1, |n| n.get());
    for name in ["PngSuite", "basi6a08", "basn2c16"] {
        peak_within_max_alloc(name, 1);
        peak_within_max_alloc(name, many);
    }

    // Rows, also the interlaced images (merged in memory).
    for name in [
        "basn2c08", "basn6a16", "basn3p08", "basi2c08", "basi6a16", "basi3p08",
    ] {
        peak_rows(name);
    }

    // Metadata: an iCCP that inflates to 300 KB (Go keeps it, Libpng drops it as implausible),
    // a tEXt before and one after the pixels.
    let mut iccp = b"profile\0\0".to_vec();
    iccp.extend_from_slice(&stored(&vec![0u8; 300_000]));
    let meta = gray_png(
        64,
        64,
        &[(b"iCCP", iccp), (b"tEXt", b"k\0before".to_vec())],
        &[(b"tEXt", b"k\0after".to_vec())],
    );
    for profile in [Profile::Go, Profile::Libpng] {
        let o = Options { profile };
        let values = [
            0usize,
            1,
            1024,
            4096,
            16 << 10,
            64 << 10,
            200 << 10,
            400 << 10,
            700 << 10,
            1 << 20,
            1 << 30,
        ];
        peak_decode(&format!("iccp {profile:?}"), &meta, &o, 1, &values);
        peak_decode(&format!("iccp {profile:?}"), &meta, &o, many, &values);
    }

    // The threaded path: 4.5 MiB of stored blocks, around the sizes where it is taken (output
    // 4.5 MiB, IDAT data and inflated data 9 MiB together), falls back or does not fit.
    let big = gray_png(2048, 2200, &[], &[(b"tEXt", b"k\0after".to_vec())]);
    let values = [
        1usize << 20,
        4 << 20,
        5 << 20,
        8 << 20,
        10 << 20,
        13 << 20,
        14 << 20,
        15 << 20,
        20 << 20,
        64 << 20,
        1 << 30,
    ];
    for threads in [1, 4, many] {
        peak_decode("threaded", &big, &Options::default(), threads, &values);
    }
}
