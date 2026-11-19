# Dependencies

The only dependency is deflate-rs:

```toml
deflate = { git = "https://github.com/dobbo-ca/deflate-rs", rev = "<sha>" }
```

Why: PNG pixel data is a zlib stream. deflate-rs is the same-owner pure-Rust DEFLATE and zlib
decoder. It brings `zlib::Reader` (a streaming reader whose buffers png-rs can reuse with
`with_buffer` and `into_buffer`), `zlib::decode_parallel_into` (the threaded path, see
[threading.md](threading.md)), and its own `Limits` (`max_output`, `max_alloc`), so the memory
budget reaches the inflate. Writing a second inflater here would duplicate it.

No other crate, and no C code, in `[dependencies]` or `[build-dependencies]`:

- libpng is used only by the CI parity oracle in `tests/parity/c/`. It is never linked into the
  crate.
- `libfuzzer-sys` is used only in the separate `tests/fuzz/` crate.
- The byblos oracle in `tests/parity/byblos/` is its own Cargo crate with its own `Cargo.lock`.

Git dependencies on private repos need the CI read token; see `.github/workflows/ci.yml`.
