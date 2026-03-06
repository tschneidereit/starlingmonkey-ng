#!/usr/bin/env bash
#
# Run the crown GC rooting linter on the workspace.
#
# Crown is a rustc compiler plugin that statically verifies GC rooting
# safety — ensuring that GC pointers are always stored in traced containers.
#
# This script:
#   1. Builds the crown binary (from crown/)
#   2. Creates a wrapper that sets LD_LIBRARY_PATH for the nightly sysroot
#   3. Runs `cargo check` with RUSTC_WRAPPER=crown and --features "js/crown"
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

# Step 1: Build crown (uses RUSTC_BOOTSTRAP=1 for rustc_private)
echo "=== Building crown linter ==="
RUSTC_BOOTSTRAP=1 cargo build --manifest-path "$ROOT_DIR/crown/Cargo.toml" --release 2>&1

CROWN_BIN="$ROOT_DIR/crown/target/release/crown"
if [[ ! -x "$CROWN_BIN" ]]; then
    echo "ERROR: crown binary not found at $CROWN_BIN"
    exit 1
fi
echo "Crown binary: $CROWN_BIN"

# Step 2: Find the sysroot for rustc shared libraries for crown's used toolchain
CROWN_SYSROOT="$(cd "$ROOT_DIR/crown" && rustc --print sysroot)"
CROWN_LIB="$CROWN_SYSROOT/lib"

# Step 3: Create a wrapper script that sets LD_LIBRARY_PATH before invoking crown
WRAPPER="$(mktemp)"
cat > "$WRAPPER" <<EOF
#!/usr/bin/env bash
export LD_LIBRARY_PATH="$CROWN_LIB:\${LD_LIBRARY_PATH:-}"
exec "$CROWN_BIN" "\$@"
EOF
chmod +x "$WRAPPER"
trap "rm -f '$WRAPPER'" EXIT

# Step 4: Run cargo check with crown as the compiler wrapper
echo ""
echo "=== Running crown check ==="
RUSTC_BOOTSTRAP="1" RUSTC_WRAPPER="$WRAPPER" cargo check \
    --manifest-path "$ROOT_DIR/Cargo.toml" \
    --features "js/crown" \
    "$@"

echo ""
echo "=== Crown check passed ==="
