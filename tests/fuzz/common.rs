//! Shared by the fuzz targets: small limits, and the options read from the end of the input.
#![allow(dead_code)]

use png::{Format, Limits, Options, Profile};
use std::sync::atomic::AtomicBool;

pub static NO: AtomicBool = AtomicBool::new(false);

/// Small, so a hostile image cannot reach the fuzzer's malloc limit.
pub fn limits(max_threads: usize) -> Limits {
    Limits {
        max_alloc: 256 << 20,
        max_threads,
        max_pixels: 1 << 22,
        max_metadata: 1 << 20,
        ..Limits::default()
    }
}

/// Last byte: bit 0 profile, bit 1 format. The rest is the PNG stream, so a raw PNG plus one byte
/// is a valid seed.
pub fn parse(src: &[u8]) -> Option<(Options, Format, &[u8])> {
    let (&b, data) = src.split_last()?;
    let profile = if b & 1 == 0 {
        Profile::Go
    } else {
        Profile::Libpng
    };
    let fmt = if b & 2 == 0 {
        Format::Native
    } else {
        Format::Eight
    };
    Some((Options { profile }, fmt, data))
}
