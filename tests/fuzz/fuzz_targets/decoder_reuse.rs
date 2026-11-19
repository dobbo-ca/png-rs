#![no_main]
use libfuzzer_sys::fuzz_target;
use png::Decoder;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// data = [a b][option byte]: the option byte's bits 2..8 place the split, `a` is the first
// `(byte >> 2) * len / 64` bytes. One Decoder decodes a then b into one reused buffer; each must
// give the same Result as a fresh decode (a stale byte or stale Info leaking out of reused scratch
// is a crash). Bytes are compared only when both are Ok.
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, rest)) = parse(src) else {
        return;
    };
    let Some(&last) = src.last() else {
        return;
    };
    let (a, b) = rest.split_at(usize::from(last >> 2) * rest.len() / 64);
    let l = limits(1);
    let mut dec = Decoder::new();
    let mut buf = Vec::new();
    for data in [a, b] {
        let fresh = png::decode(data, fmt, &opts, &l, &NO);
        // Dirty the reused buffer so unwritten bytes show up.
        buf.fill(0xA5);
        // Sized from the header, not from the fresh result: a PNG that fails after its IDAT
        // header still needs a buffer that fits, or the reused call fails early with BufferTooSmall.
        let need = png::read_info(data, &opts, &l)
            .ok()
            .and_then(|i| i.output_size(fmt))
            .unwrap_or(0);
        if buf.len() < need {
            buf.resize(need, 0xA5);
        }
        let reused = dec
            .decode_into(data, fmt, &mut buf, &opts, &l, &NO)
            .map(|i| i.clone());
        match (reused, fresh) {
            (Ok(i), Ok((fi, fo))) => {
                assert_eq!(i, fi);
                assert_eq!(buf.get(..fo.len()), Some(&fo[..]));
            }
            (Err(e), Err(f)) => assert_eq!(e, f),
            (r, f) => panic!("reuse {:?} != fresh {:?}", r.map(|_| ()), f.map(|_| ())),
        }
    }
});
