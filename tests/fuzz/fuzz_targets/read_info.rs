#![no_main]
use libfuzzer_sys::fuzz_target;
use png::Decoder;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// read_info (free fn and Decoder) agree; when the full decode succeeds the pre-IDAT Info is a
// prefix-consistent view: same header and palette.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, data)) = parse(src) else {
        return;
    };
    let l = limits(1);
    let a = png::read_info(data, &opts, &l);
    let b = Decoder::new().read_info(data, &opts, &l).map(|i| i.clone());
    assert_eq!(a, b);
    if let (Ok(i), Ok((f, _))) = (&a, png::decode(data, fmt, &opts, &l, &NO)) {
        assert_eq!(i.header, f.header);
        assert_eq!(i.palette, f.palette);
    }
});
