//! Caps on one call.

/// Caps on one call. Go had none; the defaults are finite. Every entry point takes `&Limits`
/// and, for decoding, a cancel flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Peak working memory of one call, in bytes, all threads together, including deflate-rs
    /// (default 1 GiB). Every buffer that scales with the input is charged before it is
    /// allocated. More is [`Error::AllocLimit`](crate::Error::AllocLimit).
    pub max_alloc: usize,
    /// Most OS threads of one call, the calling thread included (default
    /// `std::thread::available_parallelism`). 1 spawns no thread.
    pub max_threads: usize,
    /// Most pixels per row (default `1 << 24`). More is
    /// [`Error::WidthLimit`](crate::Error::WidthLimit).
    pub max_width: u32,
    /// Most rows (default `1 << 24`). More is [`Error::HeightLimit`](crate::Error::HeightLimit).
    pub max_height: u32,
    /// Most `width * height` (default `1 << 28`). More is
    /// [`Error::PixelLimit`](crate::Error::PixelLimit).
    pub max_pixels: u64,
    /// Sum of the ancillary data the decoder keeps, in bytes (default 16 MiB): text chunks,
    /// eXIf, the decompressed iCCP profile. PLTE is not counted and unknown chunks are skipped,
    /// not kept. More is [`Error::MetadataLimit`](crate::Error::MetadataLimit) in the Go
    /// profile; the Libpng profile drops the chunk.
    pub max_metadata: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_alloc: 1 << 30,
            max_threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            max_width: 1 << 24,
            max_height: 1 << 24,
            max_pixels: 1 << 28,
            max_metadata: 16 << 20,
        }
    }
}
