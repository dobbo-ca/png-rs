# Fuzzing

`tests/fuzz/` is a cargo-fuzz crate with its own `Cargo.lock`. Run with `--fuzz-dir tests/fuzz`.

| Target | What it checks |
|---|---|
| `decode` | both profiles, both formats, small `Limits`; no panic, no allocation over the cap |
| `rows` | the row-streaming path |
| `decoder_reuse` | two decodes with one `Decoder` equal two decodes with fresh ones |
| `read_info` | header and chunk reading up to the first IDAT |
| `unfilter_simd` | dispatched SIMD tier against scalar, bit-exact |
| `threads` | `max_threads` 1 against 4 gives equal output and errors |

All targets run with `-rss_limit_mb=2048 -malloc_limit_mb=512`, so an allocation outside the
limits is a crash.

Seeds are PngSuite and the byblos fixtures. They are copied from `tests/data` at build time, not
duplicated in git. Inputs that a fuzz run grows and keeps go to `tests/data/fuzz-grown/` (empty
at first).

`.github/workflows/fuzz.yml` runs on a weekly schedule and on `workflow_dispatch`, never on push
or pull request. Fuzz hours so far: TBD from CI.
