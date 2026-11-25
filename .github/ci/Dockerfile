# CI image: Rust runs here, built with docker buildx on GitHub-hosted runners (ubuntu-24.04 for
# x86_64, ubuntu-24.04-arm for the native arm64 stages).
# The crate depends on dobbo-ca/deflate-rs by git rev; a read-only GitHub App token fetches it while
# that repo is private. Its fetch is the only step
# of the crate stages that needs the GitHub App token, passed as the BuildKit secret `gh_token`
# (never a build arg, never left in a layer); every later step runs offline. The fuzz crate's own
# dependencies (libfuzzer-sys from crates.io) are fetched in the same step.
# Every tool is installed in a stage before `COPY . .`, so a new commit does not install it again,
# and the fetch runs from the manifests and lockfiles alone.
FROM rust:1.99.1-bookworm AS toolchain
ARG TARGETARCH
# On x86_64, gcc-aarch64-linux-gnu is the linker of `cargo build --target aarch64-unknown-linux-gnu`
# (the aarch64 lint and link check in `ci`). On arm64 that target is the host: no cross toolchain.
RUN rustup component add rustfmt clippy && rustup target add aarch64-unknown-linux-gnu \
    && if [ "$TARGETARCH" = amd64 ]; then \
         apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
         && rm -rf /var/lib/apt/lists/* \
         && printf '[target.aarch64-unknown-linux-gnu]\nlinker = "aarch64-linux-gnu-gcc"\n' >> "$CARGO_HOME/config.toml"; \
       fi
WORKDIR /src
# The manifests and lockfiles of the crate and of the fuzz crate, with stub targets, so the fetch
# is a layer that only a change to them rebuilds.
COPY Cargo.toml Cargo.lock ./
COPY tests/fuzz/Cargo.toml tests/fuzz/Cargo.lock tests/fuzz/
RUN mkdir -p src tests/fuzz/fuzz_targets && touch src/lib.rs \
    && for t in decode rows decoder_reuse read_info unfilter_simd threads; do touch tests/fuzz/fuzz_targets/$t.rs; done
RUN --mount=type=secret,id=gh_token \
    git config --global url."https://x-access-token:$(cat /run/secrets/gh_token)@github.com/".insteadOf "https://github.com/" \
    && CARGO_NET_GIT_FETCH_WITH_CLI=true cargo fetch --locked \
    && CARGO_NET_GIT_FETCH_WITH_CLI=true cargo fetch --locked --manifest-path tests/fuzz/Cargo.toml \
    && git config --global --unset-all url."https://x-access-token:$(cat /run/secrets/gh_token)@github.com/".insteadOf \
    && rm -rf src tests/fuzz/fuzz_targets

FROM toolchain AS src
ENV CARGO_NET_OFFLINE=true
COPY . .

# The checks run in the build, so a red check fails the build; a rerun of the same source is a
# BuildKit cache hit. The target dir is a cache mount, so it persists across builds. The target
# cache is shared by every branch and cargo compares mtimes: another branch's newer build would be
# reused as this one's, so every RUN touches the .rs files itself, just before cargo starts.
FROM src AS ci
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo fmt --check
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo clippy --locked --all-targets -- -D warnings
# The NEON modules are not compiled for x86_64, so lint the aarch64 target too.
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo clippy --locked --all-targets --target aarch64-unknown-linux-gnu -- -D warnings
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo test --locked --no-fail-fast
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && PNG_SIMD=scalar cargo test --locked --no-fail-fast
# The examples are built too, so the cross linker runs (a lib alone is not linked).
RUN --mount=type=cache,id=png-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo build --locked --release --target aarch64-unknown-linux-gnu --examples

# The arm64 stages are built natively on an arm64 runner (ubuntu-24.04-arm): no emulator.
FROM toolchain AS native-arm64
ENV CARGO_NET_OFFLINE=true
COPY . .

# aarch64 tests, only those that the target changes: the NEON unfilter kernels and
# their cross-tier tests (src/filter/tests.rs), both tiers (PNG_SIMD=scalar forces the scalar one).
# The rest of the suite is the same code on both architectures and runs in `ci`.
FROM native-arm64 AS arm64
RUN --mount=type=cache,id=png-rs-target-arm64,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo test --locked --target aarch64-unknown-linux-gnu --lib filter:: \
    && PNG_SIMD=scalar cargo test --locked --target aarch64-unknown-linux-gnu --lib filter::

# libpng 1.6 oracle (CI only, never linked into the crate), static so no libpng runtime is needed
# in the stages that run it. Copies tests/parity/c only, so a crate change does not rebuild it.
FROM debian:bookworm AS c-oracle
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev libpng-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
COPY tests/parity/c /c
RUN gcc -O2 -Wall -static -o /usr/local/bin/pngoracle /c/pngoracle.c -lpng -lz -lm

# byblos-rs (Go-parity reference of the Go profile) and its private dependencies. Own token scope
# (byblos-rs, pdf-rs, jpeg-rs, deflate-rs), so the job that builds this stage mints a wider token.
# Copies tests/parity/byblos only.
FROM rust:1.99.1-bookworm AS byblos-oracle
WORKDIR /byblos
COPY tests/parity/byblos .
RUN --mount=type=secret,id=gh_token \
    git config --global url."https://x-access-token:$(cat /run/secrets/gh_token)@github.com/".insteadOf "https://github.com/" \
    && CARGO_NET_GIT_FETCH_WITH_CLI=true cargo fetch --locked \
    && git config --global --unset-all url."https://x-access-token:$(cat /run/secrets/gh_token)@github.com/".insteadOf
RUN --mount=type=cache,id=png-rs-target-byblos,sharing=locked,target=/byblos/target \
    find . -name '*.rs' -exec touch {} + \
    && CARGO_NET_OFFLINE=true cargo build --locked --release \
    && cp target/release/byblos-oracle /usr/local/bin/byblos-oracle

# The public corpus (tests/parity/corpus.toml: sizes and SHA-256 are checked) in a cache mount, so
# a build downloads it once. sharing=locked: two builds never download at once. Only a list of the
# .png files reaches the layer; the stages that read the corpus mount the same cache and depend on
# this stage through that list.
# A cache mount is not saved to the Actions cache, and each hosted job starts a new builder, so the
# mount is empty there. CORPUS_RUN (the workflow run id) makes the fetch run again in each build,
# so a fetch layer restored from the Actions cache never leaves the readers with an empty mount.
FROM debian:bookworm-slim AS data
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY tests/parity/fetch.sh tests/parity/corpus.toml /parity/
ARG CORPUS_RUN=0
RUN --mount=type=cache,id=png-rs-data,target=/data,sharing=locked \
    echo "corpus run $CORPUS_RUN" && sh /parity/fetch.sh /parity/corpus.toml /data \
    && (cd /data/x && find . -type f -name '*.png' | sort) > /data.list && wc -l /data.list

# The two reference trees of png-rs on PngSuite (both profiles, one thread), x86_64: what the
# aarch64 oracle must reproduce byte for byte.
FROM src AS x86-ref
RUN --mount=type=cache,id=png-rs-target-parity,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example oracle && cp target/release/examples/oracle /usr/local/bin/png-oracle
RUN sh tests/parity/run.sh ref /ref

# Parity gates (tests/parity/run.sh full): libpng vs --profile libpng, byblos vs --profile go (on
# the corpus and the mutations), --threads 1 vs default threads. The stage does not fail on a
# diff: the report (summary.md, *.diff, status) is the build output (target parity-report), and
# the workflow fails on `status`, as the report is wanted most when the gate is red.
FROM src AS parity
COPY --from=c-oracle /usr/local/bin/pngoracle /usr/local/bin/pngoracle
COPY --from=byblos-oracle /usr/local/bin/byblos-oracle /usr/local/bin/byblos-oracle
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=png-rs-target-parity,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example oracle && cp target/release/examples/oracle /usr/local/bin/png-oracle
RUN --mount=type=cache,id=png-rs-data,target=/data,sharing=locked,readonly \
    mkdir -p /report && { sh tests/parity/run.sh full /report; echo $? > /report/status; }

FROM scratch AS parity-report
COPY --from=parity /report /

# The x86_64 reference trees as a build output (job parity-ref uploads them for parity-arm64).
FROM scratch AS x86-ref-out
COPY --from=x86-ref /ref /

# The oracle built natively for aarch64, run on PngSuite, both profiles and both PNG_SIMD
# tiers, compared with the x86_64 trees of `x86-ref-out`. The workflow passes them as the named
# build context `x86ref`.
FROM native-arm64 AS parity-arm64
RUN --mount=type=cache,id=png-rs-target-arm64,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --target aarch64-unknown-linux-gnu --example oracle \
    && cp target/aarch64-unknown-linux-gnu/release/examples/oracle /usr/local/bin/png-oracle-arm64
RUN --mount=type=bind,from=x86ref,target=/ref,rw \
    sh tests/parity/run.sh arm /ref

# Fuzzing (fuzz.yml): nightly and cargo-fuzz, with the targets built into the image. The workflow
# runs it with `docker create` + `docker start`, so a BuildKit cache hit cannot skip the fuzzing.
FROM toolchain AS fuzz-tools
RUN rustup toolchain install nightly --profile minimal \
    && cargo install cargo-fuzz --locked --version 0.13.1
ENV CARGO_NET_OFFLINE=true

# Seeds are PngSuite and the byblos fixtures, copied from tests/data here, not kept twice in git.
# Each gets one 0x00 option byte at the end (the targets read their options from the last byte).
FROM fuzz-tools AS fuzz
COPY . .
RUN sh tests/parity/run.sh gather tests/fuzz/seeds tests/data pngsuite byblos \
    && for f in tests/fuzz/seeds/*; do printf '\0' >> "$f"; done \
    && cargo +nightly fuzz build --fuzz-dir tests/fuzz

# unfilter_simd built natively on an arm64 runner with the default sanitizer, run by fuzz.yml: it
# fuzzes the NEON unfilter kernels.
FROM fuzz-tools AS fuzz-arm64
COPY . .
RUN cargo +nightly fuzz build --fuzz-dir tests/fuzz unfilter_simd

# Benchmark (bench.yml): libpng (C oracle, bench mode) and examples/bench on the same corpus, in
# one container, so the ratio compares the same machine. BENCH_RUN changes per run, so a cache hit
# of the same source does not reuse an old timing.
FROM src AS bench
COPY --from=c-oracle /usr/local/bin/pngoracle /usr/local/bin/pngoracle
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=png-rs-target-bench,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example bench && cp target/release/examples/bench /usr/local/bin/png-bench
ARG BENCH_RUN=0
RUN --mount=type=cache,id=png-rs-data,target=/data,sharing=locked,readonly \
    echo "run $BENCH_RUN" && mkdir -p /report && { sh tests/parity/bench.sh /report; echo $? > /report/status; }

FROM scratch AS bench-report
COPY --from=bench /report /
