# SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#
# Starling-NG justfile
#
# Usage:
#   just build           Build the project (debug mode)
#   just test            Run all Rust tests
#   just clone-wpt-tests Clone the WPT test suite
#   just wpt-setup       Add the WPT hosts entries to /etc/hosts
#   just wpt-test        Run all WPT tests
#   just wpt-test base64 Run WPT tests matching "base64"
#   just wpt-update      Run WPT tests and update expectations
#   just test-wizer      Snapshot the wasm component with wizer and serve it
#   just fmt             Format all code
#   just clippy          Run clippy lints
#   just check           Run fmt check + clippy + tests

# Build in debug mode.
build *TARGET:
    cargo build --features debugmozjs {{TARGET}}

# Build in release mode.
build-release *TARGET:
    cargo build --release {{TARGET}}

# Run all Rust tests.
test *TARGET:
    cargo test --features debugmozjs --workspace {{TARGET}}

# Run all Rust tests in release mode.
test-release *TARGET:
    cargo test --release --workspace {{TARGET}}

# Run the starling shell with the given args.
run *ARGS:
    cargo run --features debugmozjs -- {{ARGS}}

# Clone the WPT test suite (shallow clone, ~200MB).
clone-wpt-tests *ARGS:
    ./scripts/clone-wpt.sh {{ARGS}}

# Add the hosts entries the WPT server needs to /etc/hosts.
wpt-setup *ARGS:
    cat deps/wpt-hosts | sudo tee -a /etc/hosts

# Run WPT tests, optionally filtering by pattern.
wpt-test *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs {{PATTERN}}

# Run WPT tests, optionally filtering by pattern.
wpt-test-release *PATTERN:
    @just build-release
    node tests/wpt-harness/run-wpt.mjs --runtime=target/release/starling {{PATTERN}}

# Run WPT tests with verbose output.
wpt-test-verbose *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs -vv {{PATTERN}}

# Run WPT tests and update expectation files.
wpt-update *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs --update-expectations {{PATTERN}}

# Run WPT tests with request restrictions disabled (the non-WPT default).
wpt-test-permissive *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs --permissive {{PATTERN}}

# Run permissive WPT tests and update `permissive_status` expectations.
wpt-update-permissive *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs --permissive --update-expectations {{PATTERN}}

# Format all code.
fmt:
    cargo fmt

# Check formatting without modifying files.
fmt-check *ARGS:
    cargo fmt --check {{ARGS}}

# Run clippy lints.
clippy *ARGS:
    cargo clippy {{ARGS}}

# Run GC zeal stress tests.
# Defaults to quick tests on the `js` and `core-runtime` packages.
# See `./scripts/test-gc-zeal.sh` for usage info.
gc-zeal *ARGS:
    ./scripts/test-gc-zeal.sh {{ARGS}}

# Run crown static GC analysis.
check-gc:
    ./scripts/check-crown.sh --workspace --all --examples

# Run basic checks: formatting, clippy, tests.
check:
    just fmt-check --all
    just clippy --all
    just test --examples

# Run most checks: `check` + `check-gc` + `gc-zeal`.
check-all:
    just check
    just check-gc
    just gc-zeal

# Run basic checks: formatting, clippy, tests.
check-wasm:
    just fmt-check --all
    just clippy --all
    just test-wasm --examples

# Build for wasm32-wasip2.
build-wasm *TARGET:
    cargo build --target wasm32-wasip2 --features debugmozjs {{TARGET}}

# Build for wasm32-wasip2 in release mode.
build-wasm-release *TARGET:
    cargo build --target wasm32-wasip2 --release {{TARGET}}

# Run all Rust tests. A run with no arguments also runs the wasm serve end-to-end suite;
# arguments (a filter, `--examples`) reach only the cargo tests and leave that suite out.
test-wasm *TARGET:
    cargo test --target wasm32-wasip2 --features debugmozjs --workspace {{TARGET}}
    @{{ if TARGET == "" { "just test-serve-wasm" } else { "echo 'Skipped the wasm serve end-to-end suite; run it with: just test-serve-wasm'" } }}

# Run all Rust tests in release mode. A run with no arguments also runs the wasm serve
# end-to-end suite.
test-wasm-release *TARGET:
    cargo test --target wasm32-wasip2 --release --workspace {{TARGET}}
    @{{ if TARGET == "" { "just test-serve-wasm-release" } else { "echo 'Skipped the wasm serve end-to-end suite; run it with: just test-serve-wasm-release'" } }}

# Snapshot the component with `wasmtime wizer` and serve the result. Needs wasmtime on PATH.
test-wizer *ARGS:
    ./scripts/test-wizer.sh {{ARGS}}

# Serve the component under `wasmtime serve` and assert the same observables the native serve
# suite does, plus the behaviors only a real `wasi:http` host exercises. Builds the component
# first, so the suite never tests a stale one. Needs wasmtime on PATH; without it the suite
# skips loudly.
test-serve-wasm *ARGS:
    @just build-wasm -p starlingmonkey
    STARLING_WASM_COMPONENT="${CARGO_TARGET_DIR:-{{justfile_directory()}}/target}/wasm32-wasip2/debug/starling.wasm" \
        cargo test -p serve-test-support --test serve_wasm_e2e {{ARGS}}

# `test-serve-wasm` against the release component. The harness itself stays a debug build.
test-serve-wasm-release *ARGS:
    @just build-wasm-release -p starlingmonkey
    STARLING_WASM_COMPONENT="${CARGO_TARGET_DIR:-{{justfile_directory()}}/target}/wasm32-wasip2/release/starling.wasm" \
        cargo test -p serve-test-support --test serve_wasm_e2e {{ARGS}}

# Run WPT tests against the wasm binary.
wpt-test-wasm *PATTERN:
    @just build-wasm
    node tests/wpt-harness/run-wpt.mjs --target=wasm {{PATTERN}}

# Run WPT tests against the wasm binary with verbose output.
wpt-test-wasm-verbose *PATTERN:
    @just build-wasm
    node tests/wpt-harness/run-wpt.mjs --target=wasm -vv {{PATTERN}}

# Run WPT tests against the wasm binary and update expectations.
wpt-update-wasm *PATTERN:
    @just build-wasm
    node tests/wpt-harness/run-wpt.mjs --target=wasm --update-expectations {{PATTERN}}

# Run WPT tests through a serve-mode runtime: each test runs inside a `fetch` handler rather than
# as a one-shot command, which is the shape a deployed server has. Same binary as the command-mode
# recipes above — the component exports `wasi:cli/run` and `wasi:http/handler` both.
#
# The native server runs each request in its own global (`--serve-isolated`) and one at a time, so
# tests can't collide through shared global state. The wasm server asks its host for the same
# property with `--max-instance-reuse-count 1`, which it has to: a WASIp3 host reuses an instance
# for many requests by default.
wpt-test-serve *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs --mode=serve {{PATTERN}}

# Run WPT tests against the wasm binary through a serve-mode runtime, pre-initialized with Wizer.
# This is the configuration a deployed server has: inside a request handler, against a snapshot
# whose engine and content script are already stood up. Drop `--wizen` to skip the snapshot step.
wpt-test-wasm-serve *PATTERN:
    @just build-wasm
    node tests/wpt-harness/run-wpt.mjs --target=wasm --mode=serve --wizen {{PATTERN}}

# Run WPT across every configuration: both targets, each as a command and as a server, from one
# build per target.
wpt-test-all *PATTERN:
    @just wpt-test {{PATTERN}}
    @just wpt-test-wasm {{PATTERN}}
    @just wpt-test-wasm-serve {{PATTERN}}
    @just wpt-test-serve {{PATTERN}}
