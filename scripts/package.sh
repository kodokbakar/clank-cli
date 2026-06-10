#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

package_field() {
  local field="$1"

  awk -F '=' -v field="$field" '
    /^\[package\]/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && $1 ~ "^[[:space:]]*" field "[[:space:]]*$" {
      value = $2
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      gsub(/^"|"$/, "", value)
      print value
      exit
    }
  ' Cargo.toml
}

PACKAGE_NAME="$(package_field name)"
VERSION="$(package_field version)"

if [ -z "$PACKAGE_NAME" ] || [ -z "$VERSION" ]; then
  echo "Failed to read package name/version from Cargo.toml" >&2
  exit 1
fi

if [ "$#" -eq 0 ]; then
  TARGETS=(
    "x86_64-unknown-linux-gnu"
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
    "x86_64-pc-windows-msvc"
  )
else
  TARGETS=("$@")
fi

DIST_DIR="$ROOT_DIR/dist"
mkdir -p "$DIST_DIR"

for target in "${TARGETS[@]}"; do
  binary_ext=""
  archive_ext="tar.gz"

  if [[ "$target" == *"windows"* ]]; then
    binary_ext=".exe"
    archive_ext="zip"
  fi

  binary_path="$ROOT_DIR/target/$target/release/$PACKAGE_NAME$binary_ext"
  package_name="$PACKAGE_NAME-$VERSION-$target"

  if [ ! -f "$binary_path" ]; then
    echo "Binary not found for target: $target" >&2
    echo "Expected: $binary_path" >&2
    echo "Build it first with:" >&2
    echo "  ./scripts/build.sh $target" >&2
    exit 1
  fi

  if [ ! -f README.md ]; then
    echo "README.md not found" >&2
    exit 1
  fi

  if [ ! -f LICENSE.md ]; then
    echo "LICENSE.md not found" >&2
    exit 1
  fi

  workdir="$(mktemp -d)"
  mkdir -p "$workdir/$package_name"

  cp "$binary_path" "$workdir/$package_name/"
  cp README.md "$workdir/$package_name/"
  cp LICENSE.md "$workdir/$package_name/"

  if [ "$archive_ext" = "zip" ]; then
    if ! command -v zip >/dev/null 2>&1; then
      echo "zip command not found. Install zip first." >&2
      rm -rf "$workdir"
      exit 1
    fi

    (
      cd "$workdir"
      zip -qr "$DIST_DIR/$package_name.zip" "$package_name"
    )
  else
    (
      cd "$workdir"
      tar -czf "$DIST_DIR/$package_name.tar.gz" "$package_name"
    )
  fi

  rm -rf "$workdir"

  echo "Created dist/$package_name.$archive_ext"
done
