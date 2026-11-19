//! The 128-bit loads and stores of the NEON kernels, on array references: the array type is the
//! proof that the access is in bounds. With `unfilter_neon` in `mod.rs`, the only `unsafe` in
//! the NEON tier.
use std::arch::aarch64::*;

#[inline]
#[target_feature(enable = "neon")]
pub(super) fn load128(a: &[u8; 16]) -> uint8x16_t {
    // SAFETY: `a` is 16 readable bytes; `vld1q_u8` reads exactly 16, no alignment needed.
    unsafe { vld1q_u8(a.as_ptr()) }
}

#[inline]
#[target_feature(enable = "neon")]
pub(super) fn store128(a: &mut [u8; 16], v: uint8x16_t) {
    // SAFETY: `a` is 16 writable bytes; `vst1q_u8` writes exactly 16, no alignment needed.
    unsafe { vst1q_u8(a.as_mut_ptr(), v) }
}
