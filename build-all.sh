#!/usr/bin/env bash
# Build odoo-xml-rpc for Linux, Windows, macOS from Linux.
#
# First-time setup:
#   sudo apt install gcc-mingw-w64-x86-64   # Windows cross-linker
#   pip install ziglang                      # Zig (for macOS cross-compilation)
#   cargo install cargo-zigbuild
#   rustup target add \
#     x86_64-pc-windows-gnu \
#     x86_64-apple-darwin \
#     aarch64-apple-darwin
set -euo pipefail

DIST="dist"
mkdir -p "$DIST"

build() {
    local target="$1"
    local name="$2"
    local use_zig="${3:-false}"

    echo "=== Building $target ==="
    if [[ "$use_zig" == "true" ]]; then
        cargo zigbuild --release --target "$target"
    else
        cargo build --release --target "$target"
    fi

    local src="target/$target/release/$name"
    cp "$src" "$DIST/$name-$target"
    echo "  → $DIST/$name-$target"
}

# Linux x86_64 (native)
build x86_64-unknown-linux-gnu odoo-xml-rpc

# Windows x86_64 (MinGW cross-compiler)
build x86_64-pc-windows-gnu odoo-xml-rpc.exe

# macOS Intel + Apple Silicon (via Zig)
build x86_64-apple-darwin  odoo-xml-rpc true
build aarch64-apple-darwin odoo-xml-rpc true

echo ""
echo "Done. Artifacts in ./$DIST/:"
ls -lh "$DIST/"
