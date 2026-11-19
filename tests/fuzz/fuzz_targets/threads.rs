#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// max_threads 1 and 4 give the same Info, bytes and error.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, data)) = parse(src) else {
        return;
    };
    let a = png::decode(data, fmt, &opts, &limits(1), &NO);
    let b = png::decode(data, fmt, &opts, &limits(4), &NO);
    assert_eq!(a, b);
});
