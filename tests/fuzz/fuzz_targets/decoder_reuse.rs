#![no_main]
use libfuzzer_sys::fuzz_target;
use png::Decoder;

#[path = "../common.rs"]
mod common;
use common::{NO, limits, parse};

// data = [options][split byte][a][b]: a is the first `split % len` bytes. One Decoder decodes a
// then b into one reused buffer; each must equal a fresh decode (a stale byte or stale Info
// leaking out of reused scratch is a crash).
fuzz_target!(|src: &[u8]| {
    let Some((opts, fmt, rest)) = parse(src) else {
        return;
    };
    let Some((&s, rest)) = rest.split_first() else {
        return;
    };
    let (a, b) = rest.split_at(usize::from(s) % (rest.len() + 1));
    let l = limits(1);
    let mut dec = Decoder::new();
    let mut buf = Vec::new();
    for data in [a, b] {
        let fresh = png::decode(data, fmt, &opts, &l, &NO);
        // Dirty the reused buffer so unwritten bytes show up.
        buf.fill(0xA5);
        let need = fresh
            .as_ref()
            .ok()
            .and_then(|(i, _)| i.output_size(fmt))
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
