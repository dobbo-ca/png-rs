# Benchmarks

`examples/bench.rs <dir> <reps> [max_threads]` decodes each file to `Format::Eight` with one
`Decoder` and `decode_into` (`bench.yml` runs it with `max_threads` 1 and with the default) and
prints TSV. The C tool `tests/parity/c/pngoracle.c`
has a `bench <dir> <reps>` mode with the same TSV (`name`, `bytes`, `ns`). `bench.yml` prints
png-rs against libpng per file-size class in the job summary. It is information only, not a gate.

Numbers below were measured on a self-hosted runner. New runs use GitHub-hosted runners (ubuntu-24.04 / ubuntu-24.04-arm) and are a new baseline, not comparable with them.

Results (main 08bdc94, `bench.yml` run 44492107682):

### png-rs vs libpng: decode time, ms per MB of output (lower is better)

775 files timed by all three runs (libpng, png-rs 1 thread, png-rs default), 5 repetitions. Information only, no gate.

| size class (output) | files | libpng | png-rs 1 thread | png-rs default | 1 thread / libpng | default / libpng |
|---|---|---|---|---|---|---|
| <= 4 KiB | 531 | 3.64 | 2.52 | 2.51 | 0.69 | 0.69 |
| <= 64 KiB | 198 | 5.63 | 0.92 | 0.92 | 0.16 | 0.16 |
| <= 1 MiB | 40 | 1.43 | 0.35 | 0.35 | 0.25 | 0.24 |
| <= 16 MiB | 6 | 2.55 | 1.13 | 1.22 | 0.44 | 0.48 |
| all | 775 | 2.48 | 1.00 | 1.06 | 0.40 | 0.43 |

Both tools report the mean of one decode; the libpng number includes `png_create`/destroy and the output `malloc` per call, while png-rs reuses one `Decoder` and output buffer. Information only.
