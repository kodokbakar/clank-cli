#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TARGET="${1:-$(rustc -vV | awk '/^host:/ { print $2 }')}"

binary_ext=""
if [[ "$TARGET" == *"windows"* ]]; then
  binary_ext=".exe"
fi

binary_path="target/$TARGET/release/clank-cli$binary_ext"

echo "Verifying clank-cli for target: $TARGET"

echo "Checking format..."
cargo fmt --check

echo "Running tests with dev profile..."
cargo test --target "$TARGET"

echo "Running clippy with dev profile..."
cargo clippy --target "$TARGET" --all-targets

echo "Building release binary..."
./scripts/build.sh "$TARGET"

echo "Checking release binary exists..."
if [ ! -f "$binary_path" ]; then
  echo "Release binary not found: $binary_path" >&2
  exit 1
fi

echo "Checking binary size..."
binary_size_bytes="$(wc -c < "$binary_path")"
max_size_bytes="$((10 * 1024 * 1024))"

if [ "$binary_size_bytes" -gt "$max_size_bytes" ]; then
  echo "Binary is too large: $binary_size_bytes bytes" >&2
  echo "Expected <= $max_size_bytes bytes" >&2
  exit 1
fi

echo "Checking --version..."
"$binary_path" --version

echo "Checking --help..."
"$binary_path" --help >/dev/null

echo "Packaging release artifact..."
./scripts/package.sh "$TARGET"

echo "Verification completed for $TARGET"
