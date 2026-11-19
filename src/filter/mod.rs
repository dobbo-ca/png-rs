//! PNG row unfilter: a scalar oracle plus AVX2 (x86_64) and NEON (aarch64) tiers.
//!
//! The tier is detected once; `PNG_SIMD=scalar` forces the scalar one. Every tier is bit-exact
//! with `scalar` on every input. `unsafe` is only in the per-tier dispatch fns below and in the
//! `x86_io` / `neon_io` load/store helpers; the kernels are safe `#[target_feature]` fns.
//!
//! The SIMD kernels follow the structure of libpng's `intel/filter_sse2_intrinsics.c` and
//! `arm/filter_neon_intrinsics.c` (libpng licence); see NOTICE.
mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
mod neon_io;
#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
mod x86_io;

#[cfg(test)]
mod tests;

use std::sync::OnceLock;

type Unfilter = fn(u8, usize, &mut [u8], &[u8]) -> bool;

/// The arguments the kernels assume: filter 0..=4, bpp 1..=8, equal row lengths.
fn valid(filter: u8, bpp: usize, cur: &[u8], prev: &[u8]) -> bool {
    filter <= 4 && (1..=8).contains(&bpp) && cur.len() == prev.len()
}

/// `s` bytes of zero fill then bytes 0.. of the source, as a byte-shuffle mask (0xff = zero).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[allow(clippy::indexing_slicing)] // const-evaluated, `k < 16`
const fn shift_mask(s: usize) -> [u8; 16] {
    let mut m = [0xffu8; 16];
    let mut k = s;
    while k < 16 {
        m[k] = (k - s) as u8;
        k += 1;
    }
    m
}

/// Byte `k` takes source byte `from + k % b`: the last pixel of a block, tiled.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[allow(clippy::indexing_slicing)] // const-evaluated, `k < 16`
const fn tile_mask(from: usize, b: usize) -> [u8; 16] {
    let mut m = [0u8; 16];
    let mut k = 0;
    while k < 16 {
        m[k] = (from + k % b) as u8;
        k += 1;
    }
    m
}

/// Reverses the row filter in place with the scalar code. False: filter > 4, bpp not in
/// 1..=8, or `cur.len() != prev.len()`. For the first row pass a zeroed `prev`.
pub(crate) fn unfilter_scalar(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
    if !valid(filter, bpp, cur, prev) {
        return false;
    }
    scalar::run(filter, bpp, cur, prev, 0);
    true
}

#[allow(unsafe_code)]
#[cfg(target_arch = "x86_64")]
fn unfilter_avx2(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
    if !valid(filter, bpp, cur, prev) {
        return false;
    }
    // SAFETY: this fn is only stored in `TIER` by `resolve`, after
    // `is_x86_feature_detected!("avx2")` returned true, so the AVX2 kernel may run.
    unsafe {
        x86::unfilter(filter, bpp, cur, prev);
    }
    true
}

#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code, unused_unsafe)] // NEON is in the aarch64 baseline: the call may need no `unsafe`
fn unfilter_neon(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
    if !valid(filter, bpp, cur, prev) {
        return false;
    }
    // SAFETY: this fn is only stored in `TIER` by `resolve`, after
    // `is_aarch64_feature_detected!("neon")` returned true, so the NEON kernel may run.
    unsafe {
        neon::unfilter(filter, bpp, cur, prev);
    }
    true
}

fn resolve() -> (&'static str, Unfilter) {
    if std::env::var("PNG_SIMD").is_ok_and(|v| v == "scalar") {
        return ("scalar", unfilter_scalar);
    }
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return ("avx2", unfilter_avx2);
    }
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("neon") {
        return ("neon", unfilter_neon);
    }
    ("scalar", unfilter_scalar)
}

fn resolved() -> &'static (&'static str, Unfilter) {
    static TIER: OnceLock<(&'static str, Unfilter)> = OnceLock::new();
    TIER.get_or_init(resolve)
}

/// The tier in use: "scalar", "avx2" or "neon".
pub(crate) fn tier() -> &'static str {
    resolved().0
}

/// [`unfilter_scalar`] on the dispatched tier.
pub(crate) fn unfilter_simd(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
    (resolved().1)(filter, bpp, cur, prev)
}

/// What the decoder calls: the dispatched tier.
pub(crate) fn unfilter(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
    unfilter_simd(filter, bpp, cur, prev)
}
