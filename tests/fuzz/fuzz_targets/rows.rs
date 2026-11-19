#![no_main]
use libfuzzer_sys::fuzz_target;
use png::{Decoder, Error, Info};

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

fn by_rows(data: &[u8], fmt: png::Format, opts: &png::Options) -> Result<(Info, Vec<u8>), Error> {
    let mut dec = Decoder::new();
    let mut rows = dec.rows(data, fmt, opts, &limits(1), &NO)?;
    let n = rows.row_bytes();
    let mut row = vec![0u8; n];
    let mut out = Vec::new();
    while rows.next_row(&mut row)?.is_some() {
        out.extend_from_slice(&row);
    }
    Ok((rows.info().clone(), out))
}

// The row stream and the one-shot decode are one code path: same result, same error.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, data)) = parse(src) else {
        return;
    };
    let a = by_rows(data, fmt, &opts);
    let b = png::decode(data, fmt, &opts, &limits(1), &NO);
    assert_eq!(a, b);
});
