//! Allocation behaviour under a counting global allocator. One test fn on purpose: the counters
//! are process-wide, so no other test may run in this binary.

use png::{Decoder, Error, Format, Limits, Options};
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

/// Peak bytes held during one decode call stay within max_alloc.
fn peak_within_max_alloc(name: &str, threads: usize) {
    let data = file(name);
    let (cancel, o) = (AtomicBool::new(false), Options::default());
    for fmt in [Format::Native, Format::Eight] {
        for max_alloc in [
            0usize,
            1,
            64,
            1024,
            4096,
            16 << 10,
            64 << 10,
            1 << 20,
            1 << 30,
        ] {
            let l = Limits {
                max_alloc,
                max_threads: threads,
                ..Limits::default()
            };
            let base = CUR.load(SeqCst);
            PEAK.store(base, SeqCst);
            let r = png::decode(&data, fmt, &o, &l, &cancel);
            let peak = PEAK.load(SeqCst).saturating_sub(base);
            let ok = r.is_ok();
            let err = r.err();
            assert!(
                peak <= max_alloc,
                "{name} {fmt:?} max_alloc {max_alloc}: peak {peak}"
            );
            if !ok {
                assert_eq!(
                    err,
                    Some(Error::AllocLimit),
                    "{name} {fmt:?} max_alloc {max_alloc}"
                );
            }
            if max_alloc == 1 << 30 {
                assert!(ok, "{name} {fmt:?} must fit in 1 GiB");
            }
            if max_alloc == 0 {
                assert!(!ok, "{name} {fmt:?} cannot decode with max_alloc 0");
            }
        }
    }
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
}
