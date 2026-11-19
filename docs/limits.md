# Limits

`Limits` is a plain struct with public fields. Every entry point takes `&Limits` (`decode`,
`read_info`, `Decoder::read_info`, `Decoder::decode_into`, `Decoder::rows`). Decoding takes a
`&AtomicBool` cancel flag as well.

| Field | Default | Unit | Error when exceeded |
|---|---|---|---|
| `max_alloc` | 1 GiB | bytes | `AllocLimit` |
| `max_threads` | `available_parallelism()` | threads | none |
| `max_width` | `1 << 24` | pixels | `WidthLimit` |
| `max_height` | `1 << 24` | pixels | `HeightLimit` |
| `max_pixels` | `1 << 28` | width x height | `PixelLimit` |
| `max_metadata` | 16 MiB | bytes | `MetadataLimit` (Go profile) |

## What is checked, and when

- Width, height and pixel count are checked as soon as IHDR is read, before any buffer is sized
  from them.
- `max_alloc` is the peak working memory of one call. Each call books a fixed 1 KiB first, for the small short-lived allocations that are not charged one by one (such as the boxed `io::Error` deflate-rs makes for a bad IDAT chunk), so a `max_alloc` under 1 KiB is `AllocLimit` at once. Every buffer that scales with the input
  (row buffers, IDAT buffer, output from `decode`, the merged image for interlaced `Rows`,
  metadata vectors) is charged to a ledger before it is allocated, with `try_reserve_exact`, and
  released when it is dropped. The inflate stage gets what is left of the budget as
  `deflate::Limits::max_alloc`, so the total holds across crates and threads.
- `max_metadata` counts the ancillary data the decoder keeps: text chunks, eXIf, and the
  decompressed iCCP profile. PLTE is not counted. Unknown chunks are skipped, not kept. Under the
  Libpng profile an over-limit chunk is dropped instead of failing the decode.
- A failed allocation that the ledger allowed returns `OutOfMemory`.
- `Decoder::decode_into` returns `BufferTooSmall` if `out` is shorter than
  `info.output_size(fmt)`; this is checked right after the header.

## Cancellation

The flag is read with relaxed ordering per chunk and per row, and by every thread of a threaded
inflate. A set flag gives `Error::Cancelled`. A cancel from inside deflate-rs maps to
`Cancelled`, not to `Zlib`.

## Reuse

A `Decoder` keeps its scratch buffers. A second decode of an image of the same or smaller size,
into the same output buffer, allocates nothing.
