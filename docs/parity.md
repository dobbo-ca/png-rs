# Parity

All of this runs in CI only (the `parity` job). The counts below are from the `parity` job on main at d976dcb; the job summary of each run has the current ones.

## Oracles

1. `examples/oracle.rs` (png-rs): `oracle [--profile go|libpng] [--threads N] <in-dir> <out-dir>`,
   and `oracle gen <in> <out>` for mutations.
2. `tests/parity/c/pngoracle.c`: libpng 1.6 (Debian bookworm `libpng-dev`), CI only, never linked
   into the crate. User limits are set to unlimited so libpng does not reject what png-rs decodes
   under its own limits.
3. `tests/parity/byblos/`: a separate Cargo crate that decodes with byblos-rs at
   `7131963a975b85b76710f61f638de9d8c7834f85` and writes `.eight` or `.err`.

Per input `<name>` the oracles write `<name>.native`, `<name>.eight` and `<name>.meta`, or
`<name>.err` (one token: `notpng checksum chunk-order format unsupported eof zlib limit cancelled`;
libpng writes `error`). byblos writes only `.eight` and `.err`.

## Corpus

| Set | Files | Licence | Source and SHA-256 |
|---|---|---|---|
| PngSuite 2017jul19 | 176, committed in `tests/data/pngsuite/` | Willem van Schaik's permissive PngSuite licence (`PngSuite.LICENSE`) | committed |
| byblos-rs fixtures | 272, committed in `tests/data/byblos/` | Apache-2.0, same owner | byblos-rs main `7131963`; committed |
| libpng 1.6.44 `contrib/pngsuite/**`, `contrib/testpngs/**` | 405 together with the row below | libpng licence | `https://github.com/pnggroup/libpng/archive/refs/tags/v1.6.44.tar.gz`, SHA-256 `0ef5b633d0c65f780c4fced27ff832998e71478c13b45dfb6e94f23a82f64f7c`, 1558044 bytes |
| image-rs/image-png 0.17.16 `tests/**` | (see the row above) | MIT OR Apache-2.0 | `https://github.com/image-rs/image-png/archive/refs/tags/v0.17.16.tar.gz`, SHA-256 `3ee432b21f90a4940c5138e612ff6748e4a9d010d0eac87a667f346a7d54fbc9`, 11234617 bytes |
| Mutations | up to 2000 | n/a | made by `oracle gen`: per input 4 truncations, 8 single bit flips (2 in an IDAT payload), 2 chunk-length edits; deterministic |
| Fuzz-grown | 0 at first | n/a | `tests/data/fuzz-grown/` |

The public sets are fetched by `tests/parity/fetch.sh tests/parity/corpus.toml /data` with a size
and SHA-256 check. Only `.png` files are used.

## Gates

Each gate is an empty `diff -r`.

1. libpng against `oracle --profile libpng`, unmutated corpus: the same set of names with `.err`;
   for the rest, `.native`, `.eight` and `.meta` identical.
2. byblos against `oracle --profile go`, unmutated corpus plus mutations: `.eight` and `.err`
   (with tokens) identical.
3. `oracle --threads 1` against default threads: identical trees.

## Counts

| | Inputs | ok | err | Compared files |
|---|---|---|---|---|
| libpng gate | 853 | 781 | 72 | 2343 (.native .eight .meta) |
| byblos gate | 2841 (853 + 1988 mutations) | 777 | 2064 | 2841 (.eight .err) |
| threads gate | 853 (libpng profile) and 2841 (Go profile) | as above | as above | every output file |
