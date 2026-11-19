//! max_threads: output equal for every thread count; no spawn with max_threads = 1; thread peak
//! within max_threads (read from /proc/self/status, so only checked on Linux). One test fn on
//! purpose: other tests would change the thread count.

use png::{Format, Limits, Options};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *t = c;
    }
    !data.iter().fold(!0u32, |c, &b| {
        table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8)
    })
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
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

/// 8-bit gray PNG, pseudo-random rows with every filter type, zlib stored blocks, >= 4 MiB IDAT.
fn synthetic_png(w: u32, h: u32) -> Vec<u8> {
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
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    assert!(z.len() >= 4 << 20);

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    for c in z.chunks(1 << 18) {
        chunk(&mut png, b"IDAT", c);
    }
    chunk(&mut png, b"IEND", &[]);
    png
}

fn threads() -> Option<usize> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("Threads:")?.trim().parse().ok())
}

static PEAK: AtomicUsize = AtomicUsize::new(0);

#[test]
fn max_threads() {
    let (w, h) = (2048u32, 2200u32);
    let data = synthetic_png(w, h);
    let (cancel, o) = (AtomicBool::new(false), Options::default());
    let many = std::thread::available_parallelism().map_or(1, |n| n.get());

    // Sampler thread; it is part of every baseline below.
    let stop = AtomicBool::new(false);
    let mut reference: Option<Vec<u8>> = None;
    std::thread::scope(|s| {
        s.spawn(|| {
            while !stop.load(SeqCst) {
                if let Some(t) = threads() {
                    PEAK.fetch_max(t, SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        });
        for n in [1usize, 2, 4, many] {
            let l = Limits {
                max_threads: n,
                ..Limits::default()
            };
            let base = threads();
            PEAK.store(base.unwrap_or(0), SeqCst);
            let mut last = None;
            for _ in 0..10 {
                last = Some(png::decode(&data, Format::Native, &o, &l, &cancel).unwrap());
            }
            let peak = PEAK.load(SeqCst);
            let (info, out) = last.unwrap();
            assert_eq!(out.len(), (w * h) as usize);
            assert_eq!((info.header.width, info.header.height), (w, h));
            match &reference {
                None => reference = Some(out),
                Some(r) => assert!(*r == out, "output differs for max_threads {n}"),
            }
            if let Some(base) = base {
                assert!(
                    peak < base + n,
                    "max_threads {n}: peak {peak} threads, baseline {base}"
                );
            }
        }
        stop.store(true, SeqCst);
    });
}
