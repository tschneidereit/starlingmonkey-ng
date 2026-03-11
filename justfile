# SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#
# Starling-NG justfile
#
# Usage:
#   just build           Build the project (debug mode)
#   just test            Run all Rust tests
#   just wpt-setup       Clone the WPT test suite
#   just wpt-test        Run all WPT tests
#   just wpt-test base64 Run WPT tests matching "base64"
#   just wpt-update      Run WPT tests and update expectations
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

# Clone the WPT test suite (shallow clone, ~200MB).
wpt-setup:
    ./scripts/clone-wpt.sh

# Force re-clone of WPT suite.
wpt-setup-force:
    ./scripts/clone-wpt.sh --force

# Run WPT tests, optionally filtering by pattern.
wpt-test *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs {{PATTERN}}

# Run WPT tests with verbose output.
wpt-test-verbose *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs -vv {{PATTERN}}

# Run WPT tests and update expectation files.
wpt-update *PATTERN:
    @just build
    node tests/wpt-harness/run-wpt.mjs --update-expectations {{PATTERN}}

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
gc-zeal:
    ./scripts/test-gc-zeal.sh

# Run crown lint checker.
crown:
    ./scripts/check-crown.sh

# Run most checks: formatting, clippy, tests.
check:
    just fmt-check --all
    just clippy --all
    just test --examples

# Run most checks: `check` + `crown` + `gc-zeal`.
check-all:
    just check
    just crown
    just gc-zeal

# Run all checks: formatting, clippy, tests.
check-wasm:
    just fmt-check --all --exclude tools
    just clippy --all
    just test --examples --target=wasm32-wasip2

# Build for wasm32-wasip2.
build-wasm *TARGET:
    cargo build --target wasm32-wasip2 --features debugmozjs {{TARGET}}

# Build for wasm32-wasip2 in release mode.
build-wasm-release *TARGET:
    cargo build --target wasm32-wasip2 --release {{TARGET}}

# Run all Rust tests.
test-wasm *TARGET:
    cargo test --target wasm32-wasip2 --features debugmozjs --workspace {{TARGET}}

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
