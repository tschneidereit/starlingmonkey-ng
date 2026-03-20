#!/usr/bin/env bash
#
# Run the crown GC rooting linter on the workspace.
#
# Crown is a rustc compiler plugin that statically verifies GC rooting
# safety — ensuring that GC pointers are always stored in traced containers.
#
# This script:
#   1. Builds the crown binary (from crown/) with rpath baked in
#   2. Runs `cargo check` with RUSTC_WRAPPER=crown and --features "js/crown"
#
# The `crown` Cargo feature propagates through the workspace:
#   starling → libstarling → core-runtime + simple-http → mozjs → mozjs_sys
# enabling the `#[must_root]` / `#[allow_unrooted_interior]` annotations
# on GC-related types.
#
# Usage:
#   ./scripts/check-crown.sh                # Check the default workspace member (starling)
#   ./scripts/check-crown.sh -p core-runtime # Check a specific package
#   ./scripts/check-crown.sh --workspace     # Check the entire workspace
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# All crown operations run from the crown/ directory so cargo picks up
# crown/rust-toolchain.toml (which may differ from the workspace toolchain).
CROWN_DIR="$ROOT_DIR/crown"
CROWN_BIN="$CROWN_DIR/target/release/crown"
CROWN_SYSROOT="$(cd "$CROWN_DIR" && rustc --print sysroot)"
CROWN_LIB="$CROWN_SYSROOT/lib"

# Step 1: Build crown with the rpath baked in so it always finds librustc_driver
echo "=== Building crown linter ==="
(cd "$CROWN_DIR" && \
    RUSTFLAGS="-C link-arg=-Wl,-rpath,$CROWN_LIB" \
    RUSTC_BOOTSTRAP=1 cargo build --release 2>&1)

if [[ ! -x "$CROWN_BIN" ]]; then
    echo "ERROR: crown binary not found at $CROWN_BIN"
    exit 1
fi
echo "Crown binary: $CROWN_BIN"

# Step 2: Run cargo check with crown as the compiler wrapper
echo ""
echo "=== Running crown check ==="
RUSTC_BOOTSTRAP="1" RUSTC_WRAPPER="$CROWN_BIN" cargo check \
    --manifest-path "$ROOT_DIR/Cargo.toml" \
    --features "js/crown" \
    "$@"

echo ""
echo "=== Crown check passed ==="
