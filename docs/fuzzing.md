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

`threads` calls `png::__private::decode_threaded_min` with the parallel threshold at 0, so small
inputs take the threaded path (the real threshold is 4 MiB of compressed data, out of reach of a
fuzzer); it skips a pair where either side is `AllocLimit`, which may depend on the thread count.

Every target except `unfilter_simd` reads its options from the **last** byte of the input (bit 0
profile, bit 1 format; `decoder_reuse` also takes bits 2..8 as the split point). A raw PNG plus one
byte is therefore a valid input; the Dockerfile appends a 0x00 byte to each seed.

All targets run with `-rss_limit_mb=2048 -malloc_limit_mb=512`, so an allocation outside the
limits is a crash.

Seeds are PngSuite and the byblos fixtures. They are copied from `tests/data` at build time, not
duplicated in git. Inputs that a fuzz run grows go to the first corpus directory, `tests/fuzz/corpus/<target>`. The
workflow keeps it in an Actions cache between runs and uploads it as the `corpus-<target>` artifact
(14 days). `tests/data/fuzz-grown/` is empty and is not written by the fuzzer; a useful input is
copied there by hand to join the parity corpus.

The `fuzz-arm64` job builds `unfilter_simd` for aarch64 (no sanitizer: ASan does not run under
qemu-user) and runs it under qemu-user, so the NEON unfilter kernels are fuzzed against scalar. The
other targets run on x86_64 only; the NEON code is not reached by them.

`.github/workflows/fuzz.yml` runs on a weekly schedule (Wednesday 12:23 UTC) and on `workflow_dispatch`, never on push
or pull request. Fuzz hours so far: TBD from CI.
