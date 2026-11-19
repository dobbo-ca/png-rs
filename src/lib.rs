//! A PNG decoder with strict resource limits, no C dependency and no panics on hostile input.
//!
//! The decode flow is Go 1.27 `image/png` (through byblos-rs's port of it): the same chunk order
//! checks, CRC checks, 4096-byte buffered IDAT reader, "too much" / "not enough pixel data"
//! checks and trailing-error rule, so that byblos-rs can switch to this crate with an empty
//! parity diff ([`Profile::Go`], the default). [`Profile::Libpng`] follows libpng 1.6's lenient
//! defaults instead. Output is [`Format::Native`] (the rows as stored, as libpng with no
//! transforms) or [`Format::Eight`] (8 bits per sample, palette and tRNS expanded).
//!
//! # Resource limits
//!
//! Every call takes a [`Limits`] and (to decode) a cancel flag (`&AtomicBool`, set from another
//! thread). `Limits` has two resource fields and four caps:
//!
//! - `max_alloc` (bytes, default 1 GiB): the peak working memory of the call, all threads
//!   together, deflate-rs included. Every buffer that scales with the input is charged before it
//!   is allocated and released when it is dropped, so the peak is enforced, not estimated. More
//!   is [`Error::AllocLimit`]; an allocator that refuses a size within the limit is
//!   [`Error::OutOfMemory`].
//! - `max_threads` (default `available_parallelism`): the most OS threads of the call, the
//!   calling thread included. 1 spawns no thread, and 0 is treated as 1 (it is not an error).
//!   Only the inflate of a large image (at least 4 MiB of compressed data) uses more than one;
//!   the output is the same for every value.
//! - `max_width`, `max_height` (default `1 << 24`) and `max_pixels` (default `1 << 28`) are
//!   checked as soon as IHDR is read: [`Error::WidthLimit`], [`Error::HeightLimit`],
//!   [`Error::PixelLimit`].
//! - `max_metadata` (bytes, default 16 MiB): the ancillary data the decoder keeps (text chunks,
//!   eXIf, the decompressed iCCP profile). A chunk that would pass it is dropped, in both
//!   profiles: the contents of an ancillary chunk never fail a decode.
//!
//! The cancel flag is read per chunk and per row (and inside the inflate): a set flag is
//! [`Error::Cancelled`].
//!
//! ```
//! use png::{Format, Limits, Options};
//! use std::sync::atomic::AtomicBool;
//!
//! # fn crc32(data: &[u8]) -> u32 {
//! #     let mut c = !0u32;
//! #     for &b in data {
//! #         c ^= u32::from(b);
//! #         for _ in 0..8 {
//! #             c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
//! #         }
//! #     }
//! #     !c
//! # }
//! # fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
//! #     png.extend_from_slice(&(body.len() as u32).to_be_bytes());
//! #     let start = png.len();
//! #     png.extend_from_slice(kind);
//! #     png.extend_from_slice(body);
//! #     let crc = crc32(&png[start..]);
//! #     png.extend_from_slice(&crc.to_be_bytes());
//! # }
//! # fn tiny_png() -> Vec<u8> {
//! #     let raw = [0u8, 10, 200]; // filter type None, two gray pixels
//! #     let (mut a, mut b) = (1u32, 0u32);
//! #     for &x in &raw {
//! #         a += u32::from(x);
//! #         b += a;
//! #     }
//! #     let mut z = vec![0x78, 0x01, 0x01, 3, 0, 0xfc, 0xff]; // one stored block
//! #     z.extend_from_slice(&raw);
//! #     z.extend_from_slice(&((b << 16) | a).to_be_bytes());
//! #     let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
//! #     chunk(&mut png, b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
//! #     chunk(&mut png, b"IDAT", &z);
//! #     chunk(&mut png, b"IEND", &[]);
//! #     png
//! # }
//! # fn main() -> Result<(), png::Error> {
//! let data = tiny_png();
//! let limits = Limits {
//!     max_threads: 2,
//!     max_alloc: 64 << 20,
//!     ..Limits::default()
//! };
//! let cancel = AtomicBool::new(false);
//! let opts = Options::default();
//! let (info, pixels) = png::decode(&data, Format::Native, &opts, &limits, &cancel)?;
//! assert_eq!((info.header.width, info.header.height), (2, 1));
//! assert_eq!(pixels, [10, 200]);
//!
//! // Every call books 1 KiB first, so a `max_alloc` of 1 byte is over the limit at once.
//! let tight = Limits { max_alloc: 1, ..limits };
//! let err = png::decode(&data, Format::Native, &opts, &tight, &cancel).unwrap_err();
//! assert_eq!(err, png::Error::AllocLimit);
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Every entry point returns the one [`Error`] enum. Its `Display` is Go's text where Go has one.
//! The functions document which variants they can return; the limit variants are distinct so a
//! caller can match on them.
//!
//! # Entry points
//!
//! [`decode`] decodes in one call, [`read_info`] reads only the header and the metadata before
//! the image data, [`Decoder`] reuses its buffers across calls ([`Decoder::decode_into`] writes
//! into the caller's buffer without allocating on the second call) and streams rows
//! ([`Decoder::rows`]).
#![deny(
    unsafe_code,
    unsafe_op_in_unsafe_fn,
    clippy::undocumented_unsafe_blocks
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::indexing_slicing
    )
)]

mod chunks;
mod crc;
mod decoder;
mod engine;
mod error;
mod filter;
mod info;
mod ledger;
mod limits;
mod meta;
mod pixels;
mod source;
#[cfg(test)]
mod tests;

pub use decoder::{Decoder, Rows, decode, read_info};
pub use error::Error;
pub use info::{
    Apng, Cicp, ColorType, Format, Header, Iccp, Info, Options, Profile, Text, TextKind, Trns,
};
pub use limits::Limits;

/// Hidden, for the fuzz crate and the cross-tier tests only.
#[doc(hidden)]
pub mod __private {
    /// The scalar unfilter (the oracle).
    pub fn unfilter_scalar(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
        crate::filter::unfilter_scalar(filter, bpp, cur, prev)
    }

    /// The unfilter of the dispatched tier.
    pub fn unfilter_simd(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool {
        crate::filter::unfilter_simd(filter, bpp, cur, prev)
    }

    /// "scalar", "avx2" or "neon".
    pub fn tier() -> &'static str {
        crate::filter::tier()
    }

    /// [`decode`](crate::decode) with the threaded inflate taken from `min_compressed` bytes of
    /// IDAT data instead of 4 MiB, so that small inputs reach it (the fuzz crate).
    pub fn decode_threaded_min(
        data: &[u8],
        fmt: crate::Format,
        opts: &crate::Options,
        limits: &crate::Limits,
        cancel: &std::sync::atomic::AtomicBool,
        min_compressed: usize,
    ) -> Result<(crate::Info, Vec<u8>), crate::Error> {
        crate::decoder::decode_with(data, fmt, opts, limits, cancel, min_compressed)
    }
}
