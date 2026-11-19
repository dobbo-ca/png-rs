# png-rs

PNG decoding under strict resource limits, in Rust. It decodes every colour type and bit depth,
Adam7 interlacing, and the ancillary chunks (tRNS, gAMA, cHRM, sRGB, iCCP, cICP, eXIf, text, acTL),
into either the stored sample layout or 8 bits per sample. The decode flow comes from the byblos-rs
PNG reader (itself a port of Go 1.27 `image/png`), so errors and output match it. A second
profile follows libpng's lenient defaults. Pure Rust, no C dependency; the only `unsafe` is the
SIMD filter dispatch and its load/store helpers. Apache-2.0; see `NOTICE`.

Dependency: `png = { git = "https://github.com/dobbo-ca/png-rs", rev = "<sha>" }`

## Quick start

```rust
use std::sync::atomic::AtomicBool;
use png::{Format, Limits, Options};

let data = std::fs::read("photo.png")?;
// At most 2 threads and 256 MiB for this call.
let limits = Limits {
    max_threads: 2,
    max_alloc: 256 << 20,
    ..Limits::default()
};
let cancel = AtomicBool::new(false);
let (info, pixels) = png::decode(&data, Format::Eight, &Options::default(), &limits, &cancel)?;
println!("{}x{}, {} bytes", info.header.width, info.header.height, pixels.len());
```

To reuse buffers across images, keep a `png::Decoder` and call `Decoder::decode_into` with your
own output slice (`info.output_size(Format::Eight)` bytes).

## Resource limits

There is no unlimited entry point. Every call takes a `Limits`, and a zero field is a real cap.

| Field | Default | Unit | Over it |
|---|---|---|---|
| `max_alloc` | 1 GiB | bytes, peak of one call | `Error::AllocLimit` (`Error::OutOfMemory` if the allocator itself fails) |
| `max_threads` | `std::thread::available_parallelism()` | threads, calling thread included | no error |
| `max_width` | 16 777 216 (`1 << 24`) | pixels | `Error::WidthLimit` |
| `max_height` | 16 777 216 (`1 << 24`) | pixels | `Error::HeightLimit` |
| `max_pixels` | 268 435 456 (`1 << 28`) | width x height | `Error::PixelLimit` |
| `max_metadata` | 16 MiB | bytes of kept ancillary data (PLTE excluded) | `Error::MetadataLimit` (Go profile); chunk dropped (Libpng profile) |

- `max_alloc` is the enforced peak working memory of one call: all threads together, including
  the memory deflate-rs uses. Allocations are charged before they are made and released on drop.
- `max_threads = 1` spawns no thread. There is no thread pool; a call that uses threads starts
  them for that call only and never exceeds `max_threads`.
- Cancellation: every decode takes `&AtomicBool`. It is checked per chunk and per row; when set,
  the call returns `Error::Cancelled`.
- Output and errors are the same for every `max_threads` value.

## Docs

- [docs/design.md](docs/design.md): the build spec
- [docs/limits.md](docs/limits.md): limits, charging, errors
- [docs/threading.md](docs/threading.md): when threads are used
- [docs/profiles.md](docs/profiles.md): Go profile vs Libpng profile
- [docs/safety.md](docs/safety.md): `unsafe` inventory, hostile input
- [docs/dependencies.md](docs/dependencies.md): deflate-rs, and why nothing else
- [docs/parity.md](docs/parity.md): oracles, corpus, gates
- [docs/fuzzing.md](docs/fuzzing.md): fuzz targets
- [docs/benchmarks.md](docs/benchmarks.md): benchmarks
