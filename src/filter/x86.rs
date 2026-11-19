//! AVX2 kernels (128-bit lanes). Safe `#[target_feature(enable = "avx2")]` fns with no `unsafe`;
//! memory is reached through `x86_io` on array references. Up is wide; Sub is a prefix sum per
//! 16-byte block; Average and Paeth are per pixel; those three for bpp 3 and 4 only. Every other
//! case calls scalar.
//!
//! Structure after libpng `intel/filter_sse2_intrinsics.c` (libpng licence, permissive); Paeth
//! is the 16-bit-lane form of the same selection and is bit-exact with `scalar::paeth`.
use super::scalar;
use super::x86_io::{load128, store128};
use super::{shift_mask, tile_mask};
use std::arch::x86_64::*;

/// The caller (`unfilter_avx2`) validated filter, bpp and lengths.
#[target_feature(enable = "avx2")]
pub(super) fn unfilter(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) {
    match (filter, bpp) {
        (2, _) => up(cur, prev),
        (1, 3) => sub::<3>(cur),
        (1, 4) => sub::<4>(cur),
        (3, 3) => avg::<3>(cur, prev),
        (3, 4) => avg::<4>(cur, prev),
        (4, 3) => paeth::<3>(cur, prev),
        (4, 4) => paeth::<4>(cur, prev),
        _ => scalar::run(filter, bpp, cur, prev, 0),
    }
}

#[inline]
#[target_feature(enable = "avx2")]
fn ld4(a: &[u8; 4]) -> __m128i {
    _mm_cvtsi32_si128(u32::from_le_bytes(*a).cast_signed())
}

/// Stores the low `B` bytes of `v` at `cur[i..]`.
#[inline]
#[target_feature(enable = "avx2")]
fn st<const B: usize>(cur: &mut [u8], i: usize, v: __m128i) {
    let b = _mm_cvtsi128_si32(v).cast_unsigned().to_le_bytes();
    if let Some(d) = cur.get_mut(i..i + B) {
        for (d, s) in d.iter_mut().zip(b) {
            *d = s;
        }
    }
}

#[target_feature(enable = "avx2")]
fn up(cur: &mut [u8], prev: &[u8]) {
    let (cs, _) = cur.as_chunks_mut::<16>();
    let (ps, _) = prev.as_chunks::<16>();
    for (c, p) in cs.iter_mut().zip(ps) {
        let v = _mm_add_epi8(load128(c), load128(p));
        store128(c, v);
    }
    scalar::up(cur, prev, cur.len() & !15);
}

/// Sub: a stride-`B` prefix sum over each block of `16 / B * B` bytes, plus the last pixel of the
/// block before it.
#[target_feature(enable = "avx2")]
fn sub<const B: usize>(cur: &mut [u8]) {
    let step = const { 16 / B * B };
    let m1 = load128(&const { shift_mask(B) });
    let m2 = load128(&const { shift_mask(2 * B) });
    let m3 = load128(&const { shift_mask(4 * B) });
    let tile = load128(&const { tile_mask(16 / B * B - B, B) });
    let mut acc = _mm_setzero_si128();
    let mut i = 0;
    while let Some(blk) = cur.get(i..).and_then(|s| s.first_chunk::<16>()) {
        let mut x = load128(blk);
        x = _mm_add_epi8(x, _mm_shuffle_epi8(x, m1));
        x = _mm_add_epi8(x, _mm_shuffle_epi8(x, m2));
        if B * 4 < 16 {
            x = _mm_add_epi8(x, _mm_shuffle_epi8(x, m3));
        }
        x = _mm_add_epi8(x, _mm_shuffle_epi8(acc, tile));
        acc = x;
        let mut out = [0u8; 16];
        store128(&mut out, x);
        if let Some(d) = cur.get_mut(i..i + step) {
            for (d, s) in d.iter_mut().zip(out) {
                *d = s;
            }
        }
        i += step;
    }
    scalar::sub(cur, B, i);
}

#[target_feature(enable = "avx2")]
fn avg<const B: usize>(cur: &mut [u8], prev: &[u8]) {
    let ones = _mm_set1_epi8(1);
    let mut left = _mm_setzero_si128();
    let mut i = 0;
    while let (Some(x), Some(u)) = (
        cur.get(i..).and_then(|s| s.first_chunk::<4>()),
        prev.get(i..).and_then(|s| s.first_chunk::<4>()),
    ) {
        let (xv, uv) = (ld4(x), ld4(u));
        // floor((left + up) / 2): the rounding-up average minus the carry bit.
        let avg = _mm_sub_epi8(
            _mm_avg_epu8(left, uv),
            _mm_and_si128(_mm_xor_si128(left, uv), ones),
        );
        left = _mm_add_epi8(xv, avg);
        st::<B>(cur, i, left);
        i += B;
    }
    scalar::avg(cur, prev, B, i);
}

/// `x <= y` per 16-bit lane (small non-negative values).
#[inline]
#[target_feature(enable = "avx2")]
fn le(x: __m128i, y: __m128i) -> __m128i {
    _mm_cmpeq_epi16(_mm_min_epi16(x, y), x)
}

#[target_feature(enable = "avx2")]
fn paeth<const B: usize>(cur: &mut [u8], prev: &[u8]) {
    let zero = _mm_setzero_si128();
    let ff = _mm_set1_epi16(0xff);
    let (mut aw, mut cw) = (zero, zero);
    let mut i = 0;
    while let (Some(x), Some(p)) = (
        cur.get(i..).and_then(|s| s.first_chunk::<4>()),
        prev.get(i..).and_then(|s| s.first_chunk::<4>()),
    ) {
        let xw = _mm_unpacklo_epi8(ld4(x), zero);
        let bw = _mm_unpacklo_epi8(ld4(p), zero);
        let pa_s = _mm_sub_epi16(bw, cw);
        let pb_s = _mm_sub_epi16(aw, cw);
        let pc = _mm_abs_epi16(_mm_add_epi16(pa_s, pb_s));
        let pa = _mm_abs_epi16(pa_s);
        let pb = _mm_abs_epi16(pb_s);
        let use_a = _mm_and_si128(le(pa, pb), le(pa, pc));
        let use_b = le(pb, pc);
        let r = _mm_blendv_epi8(_mm_blendv_epi8(cw, bw, use_b), aw, use_a);
        let out = _mm_and_si128(_mm_add_epi16(xw, r), ff);
        st::<B>(cur, i, _mm_packus_epi16(out, out));
        aw = out;
        cw = bw;
        i += B;
    }
    scalar::paeth(cur, prev, B, i);
}
