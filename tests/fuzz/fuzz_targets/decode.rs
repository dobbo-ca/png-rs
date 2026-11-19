#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// data = [profile bit 0, format bit 1][png stream]. Must not panic; output length must match Info.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, data)) = parse(src) else {
        return;
    };
    if let Ok((info, out)) = png::decode(data, fmt, &opts, &limits(1), &NO) {
        assert_eq!(Some(out.len()), info.output_size(fmt));
    }
});
