#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#
# Clone the WPT (Web Platform Tests) suite at a specific tag into deps/wpt/.
#
# Usage:
#   ./scripts/clone-wpt.sh [--force]
#
# Uses a shallow clone to minimize download size (~200MB vs ~6GB for full history).

set -euo pipefail

WPT_TAG="epochs/daily/2024-10-02_01H"
WPT_REPO="https://github.com/web-platform-tests/wpt.git"
WPT_DIR="deps/wpt"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$ROOT_DIR/$WPT_DIR"

if [[ -d "$TARGET_DIR" ]] && [[ "${1:-}" != "--force" ]]; then
    echo "WPT suite already cloned at $TARGET_DIR"
    echo "Use --force to re-clone."
    exit 0
fi

if [[ "${1:-}" == "--force" ]] && [[ -d "$TARGET_DIR" ]]; then
    echo "Removing existing WPT clone..."
    rm -rf "$TARGET_DIR"
fi

echo "Cloning WPT suite (shallow, tag: $WPT_TAG)..."
echo "This may take a few minutes..."

mkdir -p "$(dirname "$TARGET_DIR")"
git clone \
    --depth 1 \
    --branch "$WPT_TAG" \
    --single-branch \
    "$WPT_REPO" \
    "$TARGET_DIR"

echo "WPT suite cloned to $TARGET_DIR"
echo "Disk usage: $(du -sh "$TARGET_DIR" | cut -f1)"
