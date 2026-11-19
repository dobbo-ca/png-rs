//! The 128-bit loads and stores of the AVX2 kernels, on array references: the array type is the
//! proof that the access is in bounds; the access is unaligned. With `unfilter_avx2` in
//! `mod.rs`, the only `unsafe` in the x86 tier.
use std::arch::x86_64::*;

#[inline]
#[target_feature(enable = "avx2")]
pub(super) fn load128(a: &[u8; 16]) -> __m128i {
    // SAFETY: `a` is 16 readable bytes; the load is unaligned and reads exactly 16.
    unsafe { _mm_loadu_si128(a.as_ptr().cast()) }
}

#[inline]
#[target_feature(enable = "avx2")]
pub(super) fn store128(a: &mut [u8; 16], v: __m128i) {
    // SAFETY: `a` is 16 writable bytes; the store is unaligned and writes exactly 16.
    unsafe { _mm_storeu_si128(a.as_mut_ptr().cast(), v) }
}
