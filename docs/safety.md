# Safety

## Hostile input

- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` or bounds-unchecked indexing in
  non-test code (denied by clippy). Slices are read with `get`, iterators, `chunks_exact` and
  `split_at_checked`.
- Every size from the input uses `checked_*` arithmetic.
- Nothing is allocated from an input size before it passes `Limits`. Allocation uses
  `try_reserve_exact`, never `vec![0; n]` on an unchecked `n`.
- Fuzzing runs with `-rss_limit_mb=2048 -malloc_limit_mb=512` (see [fuzzing.md](fuzzing.md)).

## `unsafe` inventory

The crate root has `#![deny(unsafe_code, unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]`.
`#[allow(unsafe_code)]` appears in two places only, and every block has a `// SAFETY:` comment:

1. Filter dispatch (`src/filter/mod.rs`): the one function per SIMD tier that calls the
   `#[target_feature]` kernels, next to the CPU feature detection that proves the feature is
   present. The tier is resolved once in a `OnceLock`. `PNG_SIMD=scalar` forces the scalar tier.
2. IO helpers (`src/filter/x86_io.rs`, `src/filter/neon_io.rs`): small load and store helpers such
   as `load128(&[u8; 16])` and `store128(&mut [u8; 16])`. The array reference type proves the
   bounds; callers get the arrays from slices in safe code.

The SIMD kernels (`src/filter/x86.rs`, `src/filter/neon.rs`) are safe `#[target_feature]`
functions and contain no `unsafe`. `src/filter/scalar.rs` is always compiled and is the reference.

## Kernel checks

- `unfilter(filter, bpp, cur, prev)` returns `false` for an unknown filter type, `bpp` outside
  1..=8, or `cur.len() != prev.len()`; the checks are in safe code before any kernel runs.
- Cross-tier tests compare each tier with scalar, bit for bit, on xorshift inputs of many lengths
  (0, 1, bpp-1, bpp, 15, 16, 17, 31, 32, 33, 1000), every filter, bpp 1..=8.
- The fuzz target `unfilter_simd` does the same under fuzzing.
