# png-rs design (v1)

This is the build spec of the crate. It is binding for every module, oracle and workflow.

png-rs is a new crate, not a port. Its decode flow starts from byblos-rs
`src/image/png_read.rs` (byblos-rs main `7131963`, Apache-2.0, itself a port of Go 1.27
`image/png`, BSD), so that byblos-rs can switch to png-rs with an empty parity diff. Keep that
file's chunk order checks, CRC checks, the 4096-byte buffered IDAT reader that `zlib.NewReader`
puts around the decoder (so checksum and zlib errors come in Go's order), the "too much pixel
data" and "not enough pixel data" checks and the trailing-error rule. Replace its Go image types
with the output formats below and its `pdf::Mem` with the crate's own ledger.

## Hard rules

- No local builds: never run cargo build/test/check/clippy/bench/run/miri/fuzz, rustc, cc, gcc,
  python, exiftool, pngcheck or any other analysis tool on this Mac. Only edit, `cargo fmt`,
  `cargo generate-lockfile`, git. CI is the only build.
- No C dependency in the crate (`[dependencies]`, `[build-dependencies]`). libpng is used only by
  the CI-only oracle tool in `tests/parity/c/`. `libfuzzer-sys` only in `tests/fuzz/`.
- Never panic on hostile input. No `unwrap`/`expect`/`panic!`/indexing that can go out of
  bounds in non-test code (`clippy::unwrap_used`, `expect_used`, `panic`, `todo`,
  `unimplemented`, `indexing_slicing` denied in non-test code; use `get`, iterators,
  `chunks_exact`, `split_at_checked`). `checked_*` arithmetic on every size from the input.
- `#![deny(unsafe_code, unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]` at the root;
  `#[allow(unsafe_code)]` only on the per-tier SIMD dispatch and the load/store helper modules. Kernels are safe `#[target_feature]` fns.
- Bounded allocations: charge before allocate, `try_reserve_exact`, no `vec![0; n]` on an
  unchecked `n`.

## Crate

- Package `png`, repo `dobbo-ca/png-rs`, edition 2024, `rust-toolchain.toml` channel `1.99.1`
  with `rustfmt`, `clippy`. `license = "Apache-2.0"`, `publish = false`.
- Only dependency: `deflate = { git = "https://github.com/dobbo-ca/deflate-rs", rev = "<sha>" }`.
- Release profile `codegen-units = 1`. `[profile.dev.package."*"] opt-level = 2`.

## Public API (exact; lib.rs re-exports)

```rust
pub use error::Error;           // one enum, below
pub use limits::Limits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Peak working memory of one call, bytes, all threads, incl. deflate-rs. Default 1 GiB.
    pub max_alloc: usize,
    /// Most OS threads of one call, incl. the caller. 1 = no spawn. Default available_parallelism.
    pub max_threads: usize,
    /// Default 1 << 24 each.
    pub max_width: u32,
    pub max_height: u32,
    /// width * height. Default 1 << 28.
    pub max_pixels: u64,
    /// Sum of the ancillary data the decoder keeps (PLTE excluded): text chunks, eXIf, the
    /// decompressed iCCP profile, unknown nothing (unknown chunks are skipped, not kept).
    /// Default 16 MiB. More is a dropped chunk, in both profiles (as libpng's chunk cache
    /// limits; the Go profile ignores ancillary contents as Go does). Every kept text chunk
    /// counts at least 16 bytes.
    pub max_metadata: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Go 1.27 image/png and byblos-rs today (default): every chunk CRC checked, unknown
    /// chunks (critical too) skipped, extra pixel data after the last row is an error, an
    /// error after the last row (bad Adler-32, truncated stream) is an error.
    /// The contents of ancillary chunks are ignored as Go does: a bad or over-limit one is dropped,
    /// never an error.
    #[default]
    Go,
    /// libpng 1.6 defaults (the lenient option): a bad CRC in an ancillary chunk drops the
    /// chunk, a bad CRC in a critical chunk is an error; an unknown critical chunk is an error;
    /// extra compressed data after the last row is ignored; a bad Adler-32, corrupt data or a
    /// short zlib tail after the last row is an error or ignored as libpng 1.6.44 does (see
    /// docs/profiles.md); invalid ancillary chunks are dropped as libpng does.
    Libpng,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options { pub profile: Profile }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorType { Gray = 0, Rgb = 2, Indexed = 3, GrayAlpha = 4, Rgba = 6 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header { pub width: u32, pub height: u32, pub bit_depth: u8,
                    pub color_type: ColorType, pub interlaced: bool }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Rows as stored after unfiltering and Adam7 merge: samples at bit_depth, sub-byte
    /// samples packed MSB first with each row padded to a byte, 16-bit big-endian, palette
    /// indices as stored. The unused low bits of the last byte of a sub-byte row (PNG leaves
    /// them unspecified) are zero, also after the Adam7 merge. libpng with no transforms.
    Native,
    /// One byte per sample: 1/2/4-bit gray scaled to 0..255 (v * 255 / (2^d - 1)), 16-bit
    /// reduced to the high byte, palette expanded to RGB (RGBA if tRNS is present; entries
    /// past tRNS are alpha 255; an index past the palette is 0,0,0,255), a tRNS colour key on
    /// Gray / Rgb adds an alpha channel (0 where the sample equals the key at full depth,
    /// else 255). libpng with png_set_expand + png_set_strip_16.
    Eight,
}

impl Header {
    /// Channels of `fmt` output (Native: 1 for Indexed; Eight: 3/4 for Indexed, +1 for tRNS key).
    pub fn channels(&self, fmt: Format, has_trns: bool) -> usize;
    /// Bytes per output row, None on overflow.
    pub fn row_bytes(&self, fmt: Format, has_trns: bool) -> Option<usize>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub header: Header,                 // Header: Default impl with width 0 etc.
    pub palette: Vec<[u8; 3]>,          // PLTE
    pub trns: Option<Trns>,
    pub gamma: Option<u32>,             // gAMA, raw value
    pub chrm: Option<[u32; 8]>,         // cHRM raw: wx wy rx ry gx gy bx by
    pub srgb: Option<u8>,               // rendering intent
    pub iccp: Option<Iccp>,
    pub cicp: Option<Cicp>,
    pub exif: Option<Vec<u8>>,          // eXIf data, raw
    pub text: Vec<Text>,                // tEXt / zTXt / iTXt in file order, raw chunk data
    pub apng: Option<Apng>,             // acTL seen; decode returns the default image (IDAT)
}
pub enum Trns { Gray(u16), Rgb([u16; 3]), Alpha(Vec<u8>) }        // derive Debug Clone PartialEq Eq
pub struct Iccp { pub name: Vec<u8>, pub profile: Vec<u8> }         // profile decompressed
pub struct Cicp { pub primaries: u8, pub transfer: u8, pub matrix: u8, pub full_range: u8 }
pub enum TextKind { Text, Ztxt, Itxt }
pub struct Text { pub kind: TextKind, pub data: Vec<u8> }           // raw chunk data
pub struct Apng { pub num_frames: u32, pub num_plays: u32 }

impl Info { pub fn has_trns(&self) -> bool; pub fn row_bytes(&self, fmt: Format) -> Option<usize>;
            pub fn output_size(&self, fmt: Format) -> Option<usize>; }

/// Reusable decoder. A second decode of an image of the same or smaller size, into the same
/// output buffer, allocates nothing (all scratch: rows, IDAT buffer, inflate window via
/// deflate-rs `zlib::Reader::with_buffer` / `into_buffer`, the Info vectors keep capacity).
#[derive(Debug, Default)]
pub struct Decoder { /* private */ }

impl Decoder {
    pub fn new() -> Self;
    /// Chunks up to the first IDAT (PLTE/tRNS needed for Indexed). Unlike Go's DecodeConfig,
    /// which stops after IHDR (after tRNS for Indexed), every chunk up to IDAT is read and its
    /// CRC and order are checked; an ancillary problem never fails it (it is dropped).
    pub fn read_info(&mut self, data: &[u8], opts: &Options, limits: &Limits) -> Result<&Info, Error>;
    /// Whole image into `out` (len >= info.output_size(fmt), else Error::BufferTooSmall,
    /// checked after the header). Chunks after the IDATs up to IEND are read too (Info then
    /// holds post-IDAT text/eXIf). Cancel checked per chunk and per row.
    pub fn decode_into(&mut self, data: &[u8], fmt: Format, out: &mut [u8], opts: &Options,
                       limits: &Limits, cancel: &AtomicBool) -> Result<&Info, Error>;
    /// Row streaming: rows top to bottom. Non-interlaced: holds two rows plus the inflate
    /// window. Interlaced: holds the merged image (charged), documented.
    pub fn rows<'d>(&'d mut self, data: &'d [u8], fmt: Format, opts: &Options,
                    limits: &Limits, cancel: &'d AtomicBool) -> Result<Rows<'d>, Error>;
}
pub struct Rows<'d> { /* private */ }
impl Rows<'_> {
    pub fn info(&self) -> &Info;
    pub fn row_bytes(&self) -> usize;
    /// Writes the next row into out[..row_bytes]; Ok(None) after the last row (then the
    /// chunks to IEND have been read and checked). Error::BufferTooSmall if out is short.
    pub fn next_row(&mut self, out: &mut [u8]) -> Result<Option<u32>, Error>;
}

/// Convenience: one call, allocates the output (charged).
pub fn decode(data: &[u8], fmt: Format, opts: &Options, limits: &Limits, cancel: &AtomicBool)
    -> Result<(Info, Vec<u8>), Error>;
pub fn read_info(data: &[u8], opts: &Options, limits: &Limits) -> Result<Info, Error>;

/// Hidden, for the fuzz crate and the cross-tier tests only.
#[doc(hidden)] pub mod __private {
    pub fn unfilter_scalar(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool;
    pub fn unfilter_simd(filter: u8, bpp: usize, cur: &mut [u8], prev: &[u8]) -> bool; // dispatched tier
    pub fn tier() -> &'static str;      // "scalar" | "avx2" | "neon"
    /// decode() with the threaded inflate taken from `min_compressed` bytes of IDAT data (the
    /// default is 4 MiB), so that the fuzz target `threads` reaches it on small inputs.
    pub fn decode_threaded_min(data: &[u8], fmt: Format, opts: &Options, limits: &Limits,
                               cancel: &AtomicBool, min_compressed: usize)
        -> Result<(Info, Vec<u8>), Error>;
}
```

Decode and `decode_into` and `Rows` are the same code path (one implementation), so they give
the same output and the same error for every input and every thread count.

### Threads

Inflate is the only stage that can pay. After the chunks before the first IDAT are read and
the row buffers booked, the threaded path runs when `max_threads > 1`, the IDAT chunks that follow
each other from the first one are all in the input with at least 4 MiB of data (`PAR_MIN`, a
parameter of the engine that `__private::decode_threaded_min` overrides, together with deflate-rs's
1 MiB per-worker minimum, via `decode_parallel_chunked_into`), and the concatenated
IDAT data plus the whole filtered image fit in what is left of `max_alloc`. It books both
buffers, copies the IDAT run into one (CRC of each chunk checked; only the IDAT run is scanned),
inflates it with `deflate::zlib::decode_parallel_into(.., max_threads, ..)`, checks the inflated
length and that the zlib stream ends where the IDAT data ends (the last four bytes are the
Adler-32 of the output, found once), unfilters serially, and goes on at the chunk after the IDAT
run: the chunks up to IEND are read by the serial chunk loop. On success the buffers and their
booking stay until the call ends. If any step fails (a CRC, any deflate error including its own
`AllocLimit`, a length or trailer mismatch, a bad filter byte) the buffers are freed, their
booking is given back and the serial streaming path runs from the first IDAT chunk and gives
the result and the error. Output and errors are thus the serial ones by construction.
`max_threads = 1` (or 0) never spawns. Tests: output equal for threads 1, 2, default; thread peak
never above `max_threads`; the peak of memory against `max_alloc` on a 4.5 MiB file.

## Error

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    NotPng,                      // signature; Go "not a PNG file"
    Checksum,                    // chunk CRC; Go "invalid checksum"
    ChunkOrder,                  // Go "chunk out of order"
    Format(&'static str),        // other Go FormatError text after "png: invalid format: "
    Unsupported(&'static str),   // Go UnsupportedError text after "png: unsupported feature: "
    UnexpectedEof,               // io.ErrUnexpectedEOF
    Zlib(deflate::Error),        // zlib/DEFLATE error in the pixel rows (stream errors only)
    Trailing(deflate::Error),    // Go profile: stream error after the last row (Go FormatError with the text)
    WidthLimit, HeightLimit, PixelLimit, MetadataLimit,
    AllocLimit, OutOfMemory,
    Cancelled,
    BufferTooSmall,
}
```
Display gives Go's exact text where Go has one ("png: invalid format: invalid checksum", ...),
so byblos-rs can map one to one. A deflate `AllocLimit`/`OutOfMemory`/`Cancelled` from the
inflate maps to the png-rs variants, never to `Zlib`.

## Ledger

Own `src/ledger.rs` (copy the pattern of tiff-rs/jpeg-rs: an atomic charge counter with a peak,
charge on allocate, release on drop). The inflate gets `deflate::Limits { max_output, max_alloc:
remaining }`. Tests use a counting global allocator in an integration test to check the peak
against `max_alloc` for several values, and a thread counter for `max_threads` (1, 2, default).

## SIMD (`src/filter/`)

- `scalar.rs`: the oracle; Sub, Up, Average, Paeth for bpp 1..=8 (bpp = bytes per complete
  pixel, min 1). Go's/byblos's Paeth.
- `x86.rs` (AVX2 tier: `#[target_feature(enable = "avx2")]` safe kernels; 128-bit lanes are
  fine inside) and `neon.rs` (aarch64): Up (wide), Sub/Average/Paeth for bpp 3 and 4 (and 6/8
  if simple). Other cases call scalar. Port guide: libpng `intel/filter_sse2_intrinsics.c` and
  `arm/filter_neon_intrinsics.c` (libpng licence, permissive: attribute in the file and NOTICE)
  or image-rs/png `src/filter.rs` (MIT/Apache).
- `x86_io.rs` / `neon_io.rs`: the only `unsafe`: `load128(&[u8;16])`, `store128(&mut [u8;16])`
  etc., `#[allow(unsafe_code)]` module, SAFETY comments.
- `mod.rs`: `unfilter(filter, bpp, cur, prev) -> bool` (false: unknown filter type, bpp not in
  1..=8, or `cur.len() != prev.len()`), dispatch resolved once in a `OnceLock<fn>`;
  env `PNG_SIMD=scalar` forces scalar. The dispatch fn per tier is the only other `unsafe`.
- Cross-tier tests (`src/filter/tests.rs`): xorshift inputs, lengths 0, 1, bpp-1, bpp, 15, 16,
  17, 31, 32, 33, 1000, every filter, bpp 1..=8, tier vs scalar bit-exact; x86_64 asserts AVX2
  detected, aarch64 asserts NEON.

## Oracles and parity (CI only)

Corpus (oracle inputs), each file named by its relative path with `/` replaced by `__`:

- `tests/data/pngsuite/` (committed: PngSuite 2017jul19, 176 files, licence in NOTICE).
- `tests/data/byblos/` (committed: the 272 PNG files of byblos-rs main 7131963: `data/` = its tests/data, `fuzz-seeds/` = its tests/fuzz/seeds).
- Public sets fetched in CI into the BuildKit cache mount `id=png-rs-data` by
  `tests/parity/fetch.sh tests/parity/corpus.toml /data` (curl --limit-rate 20M, size +
  SHA-256 check, `.ok` stamps; only `.png` files are used):
  - libpng 1.6.44 source, `contrib/pngsuite/**` and `contrib/testpngs/**` (libpng licence):
    https://github.com/pnggroup/libpng/archive/refs/tags/v1.6.44.tar.gz
    sha256 0ef5b633d0c65f780c4fced27ff832998e71478c13b45dfb6e94f23a82f64f7c, 1558044 bytes
  - image-rs/image-png 0.17.16, `tests/**` (MIT OR Apache-2.0):
    https://github.com/image-rs/image-png/archive/refs/tags/v0.17.16.tar.gz
    sha256 3ee432b21f90a4940c5138e612ff6748e4a9d010d0eac87a667f346a7d54fbc9, 11234617 bytes
- Mutations: `cargo run --release --example oracle -- gen <in-dir> <out-dir>`: deterministic
  (own xorshift, seed from the file name): per input 4 truncations, 8 single bit flips (2 of
  them in an IDAT chunk payload), 2 chunk-length edits; at most 2000 inputs.
- Fuzz-grown files: `tests/data/fuzz-grown/` (empty at first; filled from fuzz runs later).

Output files per input `<name>` (all oracles, same names):

- `<name>.native`, `<name>.eight`: the output bytes of Format::Native / Format::Eight.
- `<name>.meta`: text, `\n` line ends, exactly these lines in this order:
  `size <w> <h>`, `depth <d>`, `color <ct>`, `interlace <0|1>`,
  `plte <n> <lowercase hex of the 3n bytes>` (only if PLTE kept),
  `trns <lowercase hex of the raw tRNS data>` (only if tRNS kept).
- `<name>.err` instead of all three when the decode fails: one class token and `\n`.
  Tokens: `notpng checksum chunk-order format unsupported eof zlib limit cancelled`.
  (`Trailing` -> `format`; any limit -> `limit`.) The libpng tool writes `error`; the
  libpng comparison checks only that both sides failed.

Oracle tools:

1. `examples/oracle.rs` (png-rs): `oracle [--profile go|libpng] [--threads N] <in-dir> <out-dir>`
   (walks in-dir recursively, `.png` and extensionless mutation files) and `oracle gen <in> <out>`.
   Limits: defaults with `max_threads` from `--threads` (default: available_parallelism).
2. `tests/parity/c/pngoracle.c` (libpng 1.6 from Debian bookworm `libpng-dev`, CI-only):
   classic API with setjmp; `png_set_user_limits(0x7fffffff,0x7fffffff)`,
   `png_set_chunk_malloc_max(0)`/cache max 0 (unlimited, documented); `.native`:
   `png_set_interlace_handling` only; `.eight`: a second read with `png_set_expand` +
   `png_set_strip_16` (+ interlace handling); `.meta` from IHDR/PLTE/tRNS of the first read.
   Mode `bench <dir> <reps>`: decode each file `reps` times, print TSV `name\tbytes\tns`.
   Built in a CI stage `FROM debian:bookworm` with gcc + libpng-dev; never linked into the crate.
3. `tests/parity/byblos/` (Cargo bin crate `byblos-oracle`, own Cargo.lock, depends on byblos
   `git = "https://github.com/dobbo-ca/byblos-rs", rev = "d0c3d7fa8da2e394ee20654c165a615c5b8ca4a6"` full sha): decodes with
   `byblos::image::png_read::decode(data, &Limits::default(), &cancel)` and writes `.eight`
   (converted from byblos's Go image types to the Eight layout using the IHDR colour type /
   depth / tRNS presence it reads from the file itself) or `.err` (tokens above, mapped from
   byblos's errors by Go text). It never writes `.native`/`.meta`.

Gates (parity job, all must be empty diffs):

- libpng vs `oracle --profile libpng`: same set of names with `.err`; for the rest, `.native`,
  `.eight`, `.meta` identical. Inputs: the unmutated corpus.
- byblos vs `oracle --profile go`: `.eight` and `.err` (with tokens) identical. Inputs: the
  unmutated corpus + the mutations.
- png-rs `--threads 1` vs default threads: identical trees.
- Counts (inputs, ok, err, compared files) go to the job summary and `docs/parity.md`.

## Fuzz (`tests/fuzz`, cargo-fuzz crate, own Cargo.lock)

Targets: `decode` (both profiles, both formats, small Limits), `rows`, `decoder_reuse` (two
decodes with one Decoder equal two fresh decodes), `read_info`, `unfilter_simd` (tier vs scalar),
`threads` (threads 1 vs 4 equal). `-rss_limit_mb=2048 -malloc_limit_mb=512`. Seeds: PngSuite +
byblos fixtures (copied at build time from tests/data, not duplicated in git).

## Bench

`examples/bench.rs <dir> <reps>`: decode_into with one Decoder, `max_threads` 1 and default,
TSV like the C tool. `bench.yml` prints png-rs vs libpng per file class (size buckets) in the job
summary. Information only, no gate.
