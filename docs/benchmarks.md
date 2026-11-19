# Benchmarks

Placeholder: this page is filled from `bench.yml`.

`examples/bench.rs <dir> <reps>` decodes each file with one `Decoder` and `decode_into`, with
`max_threads` 1 and with the default, and prints TSV. The C tool `tests/parity/c/pngoracle.c`
has a `bench <dir> <reps>` mode with the same TSV (`name`, `bytes`, `ns`). `bench.yml` prints
png-rs against libpng per file-size class in the job summary. It is information only, not a gate.

Results: TBD from CI.
