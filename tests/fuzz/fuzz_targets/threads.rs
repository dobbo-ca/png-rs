#![no_main]
use libfuzzer_sys::fuzz_target;
use png::__private::decode_threaded_min;
use png::Error;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// max_threads 4 with the parallel threshold at 0 (so tiny inputs take the threaded path) gives
// the same Info, bytes and error as max_threads 1. A memory limit may legitimately depend on the
// thread count, so that pair is skipped.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, data)) = parse(src) else {
        return;
    };
    let a = png::decode(data, fmt, &opts, &limits(1), &NO);
    let b = decode_threaded_min(data, fmt, &opts, &limits(4), &NO, 0);
    if matches!(a, Err(Error::AllocLimit)) || matches!(b, Err(Error::AllocLimit)) {
        return;
    }
    assert_eq!(a, b);
});
