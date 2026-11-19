#![no_main]
use libfuzzer_sys::fuzz_target;
use png::__private::{unfilter_scalar, unfilter_simd};

// data = [filter][bpp][prev len/cur len layout][bytes]: prev and cur are equal-length rows built
// from the rest; the dispatched tier and the scalar code must agree on the bool and the bytes.
fuzz_target!(|src: &[u8]| {
    let Some((&[filter, bpp, split], rest)) = src.split_first_chunk::<3>() else {
        return;
    };
    let bpp = 1 + usize::from(bpp) % 8;
    let n = rest.len() / 2;
    let (prev, cur) = rest.split_at(n);
    let cur = &cur[..n];
    // Vary the filter byte past 0..=4 too (invalid filters must agree: false).
    let filter = if split & 1 == 0 { filter % 5 } else { filter };
    let (mut a, mut b) = (cur.to_vec(), cur.to_vec());
    let ra = unfilter_scalar(filter, bpp, &mut a, prev);
    let rb = unfilter_simd(filter, bpp, &mut b, prev);
    assert_eq!(ra, rb);
    if ra {
        assert_eq!(a, b);
    }
});
