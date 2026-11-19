//! NEON kernels, the same shapes as `x86.rs`. Safe `#[target_feature(enable = "neon")]` fns with
//! no `unsafe`; 16-byte memory access goes through `neon_io` on array references. Up is wide;
//! Sub is a prefix sum per 16-byte block; Average and Paeth are per pixel; those three for bpp
//! 3 and 4 only. Every other case calls scalar.
//!
//! Structure after libpng `arm/filter_neon_intrinsics.c` (libpng licence, permissive); Paeth is
//! the 16-bit-lane form of the same selection and is bit-exact with `scalar::paeth`.
use super::neon_io::{load128, store128};
use super::scalar;
use super::{shift_mask, tile_mask};
use std::arch::aarch64::*;

/// The caller (`unfilter_neon`) validated filter, bpp and lengths.
#[target_feature(enable = "neon")]
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

/// Four bytes in the low lanes of a 64-bit vector.
#[inline]
#[target_feature(enable = "neon")]
fn ld4(a: &[u8; 4]) -> uint8x8_t {
    vcreate_u8(u64::from(u32::from_le_bytes(*a)))
}

/// Stores the low `B` bytes of `v` at `cur[i..]`.
#[inline]
#[target_feature(enable = "neon")]
fn st<const B: usize>(cur: &mut [u8], i: usize, v: uint8x8_t) {
    let b = vget_lane_u32::<0>(vreinterpret_u32_u8(v)).to_le_bytes();
    if let Some(d) = cur.get_mut(i..i + B) {
        for (d, s) in d.iter_mut().zip(b) {
            *d = s;
        }
    }
}

#[target_feature(enable = "neon")]
fn up(cur: &mut [u8], prev: &[u8]) {
    let (cs, _) = cur.as_chunks_mut::<16>();
    let (ps, _) = prev.as_chunks::<16>();
    for (c, p) in cs.iter_mut().zip(ps) {
        let v = vaddq_u8(load128(c), load128(p));
        store128(c, v);
    }
    scalar::up(cur, prev, cur.len() & !15);
}

/// Sub: a stride-`B` prefix sum over each block of `16 / B * B` bytes, plus the last pixel of the
/// block before it. Table lookups with an index of 16 or more give 0.
#[target_feature(enable = "neon")]
fn sub<const B: usize>(cur: &mut [u8]) {
    let step = const { 16 / B * B };
    let m1 = load128(&const { shift_mask(B) });
    let m2 = load128(&const { shift_mask(2 * B) });
    let m3 = load128(&const { shift_mask(4 * B) });
    let tile = load128(&const { tile_mask(16 / B * B - B, B) });
    let mut acc = vdupq_n_u8(0);
    let mut i = 0;
    while let Some(blk) = cur.get(i..).and_then(|s| s.first_chunk::<16>()) {
        let mut x = load128(blk);
        x = vaddq_u8(x, vqtbl1q_u8(x, m1));
        x = vaddq_u8(x, vqtbl1q_u8(x, m2));
        if B * 4 < 16 {
            x = vaddq_u8(x, vqtbl1q_u8(x, m3));
        }
        x = vaddq_u8(x, vqtbl1q_u8(acc, tile));
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

#[target_feature(enable = "neon")]
fn avg<const B: usize>(cur: &mut [u8], prev: &[u8]) {
    let mut left = vdup_n_u8(0);
    let mut i = 0;
    while let (Some(x), Some(u)) = (
        cur.get(i..).and_then(|s| s.first_chunk::<4>()),
        prev.get(i..).and_then(|s| s.first_chunk::<4>()),
    ) {
        let (xv, uv) = (ld4(x), ld4(u));
        left = vadd_u8(xv, vhadd_u8(left, uv)); // vhadd: floor((a + b) / 2)
        st::<B>(cur, i, left);
        i += B;
    }
    scalar::avg(cur, prev, B, i);
}

#[inline]
#[target_feature(enable = "neon")]
fn widen(v: uint8x8_t) -> int16x8_t {
    vreinterpretq_s16_u16(vmovl_u8(v))
}

#[target_feature(enable = "neon")]
fn paeth<const B: usize>(cur: &mut [u8], prev: &[u8]) {
    let zero = vdupq_n_s16(0);
    let (mut aw, mut cw) = (zero, zero);
    let mut i = 0;
    while let (Some(x), Some(p)) = (
        cur.get(i..).and_then(|s| s.first_chunk::<4>()),
        prev.get(i..).and_then(|s| s.first_chunk::<4>()),
    ) {
        let xw = widen(ld4(x));
        let bw = widen(ld4(p));
        let pa = vabdq_s16(bw, cw);
        let pb = vabdq_s16(aw, cw);
        let pc = vabsq_s16(vaddq_s16(vsubq_s16(bw, cw), vsubq_s16(aw, cw)));
        let use_a = vandq_u16(vcleq_s16(pa, pb), vcleq_s16(pa, pc));
        let use_b = vcleq_s16(pb, pc);
        let r = vbslq_s16(use_a, aw, vbslq_s16(use_b, bw, cw));
        let sum = vaddq_s16(xw, r);
        let out = vmovn_u16(vreinterpretq_u16_s16(sum));
        st::<B>(cur, i, out);
        // The next left pixel: the new bytes, widened (the low four lanes are the pixel).
        aw = widen(out);
        cw = bw;
        i += B;
    }
    scalar::paeth(cur, prev, B, i);
}
