//! The one error enum. `Display` gives Go's text where Go has one.

use std::fmt;

/// Why a call failed.
///
/// `Display` is Go 1.27 `image/png`'s text for the error classes Go has, so a caller can map
/// errors one to one. The limit variants are distinct so a caller can match on them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The 8-byte signature is wrong (Go: "not a PNG file").
    NotPng,
    /// A chunk CRC does not match (Go: "invalid checksum").
    Checksum,
    /// A chunk is where the PNG rules do not allow it (Go: "chunk out of order").
    ChunkOrder,
    /// Any other Go `FormatError`; the text follows "png: invalid format: ".
    Format(&'static str),
    /// Go's `UnsupportedError`; the text follows "png: unsupported feature: ".
    Unsupported(&'static str),
    /// The file ends inside a chunk (Go: `io.ErrUnexpectedEOF`).
    UnexpectedEof,
    /// A zlib or DEFLATE error in the pixel rows (stream errors only; limits and cancellation
    /// never come as `Zlib`).
    Zlib(deflate::Error),
    /// Go profile: a stream error after the last row (Go's `FormatError` carrying its text).
    Trailing(deflate::Error),
    /// The width is over `Limits::max_width`.
    WidthLimit,
    /// The height is over `Limits::max_height`.
    HeightLimit,
    /// `width * height` is over `Limits::max_pixels`.
    PixelLimit,
    /// The ancillary data to keep is over `Limits::max_metadata`. Not returned today: both
    /// profiles drop such a chunk; the variant stays for callers that match on it.
    MetadataLimit,
    /// The working memory would pass `Limits::max_alloc`.
    AllocLimit,
    /// The allocator refused a size that is within the limits.
    OutOfMemory,
    /// The cancel flag was set.
    Cancelled,
    /// The output buffer is shorter than `Info::output_size` (or one row).
    BufferTooSmall,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPng => f.write_str("png: invalid format: not a PNG file"),
            Self::Checksum => f.write_str("png: invalid format: invalid checksum"),
            Self::ChunkOrder => f.write_str("png: invalid format: chunk out of order"),
            Self::Format(s) => write!(f, "png: invalid format: {s}"),
            Self::Unsupported(s) => write!(f, "png: unsupported feature: {s}"),
            Self::UnexpectedEof => f.write_str("unexpected EOF"),
            Self::Zlib(e) => write!(f, "{e}"),
            Self::Trailing(e) => write!(f, "png: invalid format: {e}"),
            Self::WidthLimit => f.write_str("png: image width over the limit"),
            Self::HeightLimit => f.write_str("png: image height over the limit"),
            Self::PixelLimit => f.write_str("png: image pixel count over the limit"),
            Self::MetadataLimit => f.write_str("png: metadata over the limit"),
            Self::AllocLimit => f.write_str("png: memory limit exceeded"),
            Self::OutOfMemory => f.write_str("png: out of memory"),
            Self::Cancelled => f.write_str("png: cancelled"),
            Self::BufferTooSmall => f.write_str("png: output buffer too small"),
        }
    }
}

impl std::error::Error for Error {}

/// Maps a deflate-rs error that is not about the stream: the limits and cancellation.
pub(crate) fn resource(e: &deflate::Error) -> Option<Error> {
    match e {
        deflate::Error::OutputLimit | deflate::Error::AllocLimit => Some(Error::AllocLimit),
        deflate::Error::OutOfMemory => Some(Error::OutOfMemory),
        deflate::Error::Cancelled => Some(Error::Cancelled),
        _ => None,
    }
}
