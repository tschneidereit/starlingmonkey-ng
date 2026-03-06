#!/usr/bin/env bash
#
# Run tests under various SpiderMonkey GC zeal configurations.
#
# GC zeal is controlled by the JS_GC_ZEAL environment variable:
#   JS_GC_ZEAL="mode,frequency" or "mode1;mode2,frequency"
#
# Productive zeal modes:
#   1  - GC on roots change
#   2  - GC on every N-th allocation (most aggressive)
#   4  - Verify pre-barriers
#   5  - Verify post-barriers
#   7  - Generational GC stress
#  10  - Incremental GC (multiple slices)
#  14  - Compact on every GC
#  15  - Check heap after GC
#  18  - Check gray marking
#
# Usage:
#   ./scripts/test-gc-zeal.sh                         # Defaults: full zeal, core-runtime tests
#   ./scripts/test-gc-zeal.sh --zeal quick             # Quick zeal subset
#   ./scripts/test-gc-zeal.sh --zeal "2,1"             # Specific zeal setting
#   ./scripts/test-gc-zeal.sh --zeal quick -p core-runtime
#   ./scripts/test-gc-zeal.sh --zeal full --examples --workspace
set -euo pipefail

# Zeal configurations, ordered from fastest to slowest.
QUICK_CONFIGS=(
    "1,1"          # GC on roots change
    "4,1"          # Verify pre-barriers
    "15,1"         # Check heap after GC
    "18,1"         # Check gray marking
)

FULL_CONFIGS=(
    "${QUICK_CONFIGS[@]}"
    "2,1"          # GC on every allocation
    "5,1"          # Verify post-barriers
    "7,1"          # Generational GC stress
    "10,1"         # Incremental GC (multiple slices)
    "14,1"         # Compact on every GC
    "2;14,1"       # Allocate + compact (catches untraced Heap pointers)
    "1;2;4;5,1"    # Combined: roots + alloc + barriers
    "1;2;4;5;7;10;14,1"  # All productive modes at once
)

# --- Parse arguments ---
# Extract --zeal <mode> from the argument list; everything else is
# forwarded to `cargo test`.
MODE="full"
CARGO_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --zeal)
            MODE="${2:?--zeal requires a value (quick, full, or a JS_GC_ZEAL string)}"
            shift 2
            ;;
        *)
            CARGO_ARGS+=("$1")
            shift
            ;;
    esac
done

# Default cargo test arguments when none are provided.
if [[ ${#CARGO_ARGS[@]} -eq 0 ]]; then
    CARGO_ARGS=(-p core-runtime)
fi

# Ensure debugmozjs is enabled for GC zeal testing
CARGO_ARGS=("${CARGO_ARGS[@]}" --features debugmozjs)

PASSED=0
FAILED=0
FAILURES=()

if [[ "$MODE" == "quick" ]]; then
    CONFIGS=("${QUICK_CONFIGS[@]}")
elif [[ "$MODE" == "full" ]]; then
    CONFIGS=("${FULL_CONFIGS[@]}")
else
    # User-provided zeal setting
    CONFIGS=("$MODE")
fi

echo "Running tests with ${#CONFIGS[@]} GC zeal configuration(s)..."
echo "  cargo test ${CARGO_ARGS[*]}"
echo ""

for zeal in "${CONFIGS[@]}"; do
    # printf "  JS_GC_ZEAL=%-25s " "\"$zeal\""
    if JS_GC_ZEAL="$zeal" cargo test "${CARGO_ARGS[@]}"; then
        # echo "✓ pass"
        PASSED=$((PASSED + 1))
    else
        # echo "✗ FAIL"
        FAILED=$((FAILED + 1))
        FAILURES+=("$zeal")
    fi
done

echo ""
echo "Results: $PASSED passed, $FAILED failed out of ${#CONFIGS[@]} configurations"

if [[ $FAILED -gt 0 ]]; then
    echo ""
    echo "Failed configurations:"
    for z in "${FAILURES[@]}"; do
        echo "  JS_GC_ZEAL=\"$z\""
    done
    echo ""
    echo "To debug a failure, run:"
    echo "  JS_GC_ZEAL=\"${FAILURES[0]}\" cargo test ${CARGO_ARGS[*]} -- --nocapture"
    exit 1
fi
