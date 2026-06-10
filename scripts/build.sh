#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

default_targets_for_host() {
  case "$(uname -s)" in
    Linux)
      echo "x86_64-unknown-linux-gnu"
      ;;
    Darwin)
      echo "x86_64-apple-darwin"
      echo "aarch64-apple-darwin"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      echo "x86_64-pc-windows-msvc"
      ;;
    *)
      echo "Unsupported host OS: $(uname -s)" >&2
      exit 1
      ;;
  esac
}

if [ "$#" -gt 0 ]; then
  TARGETS=("$@")
else
  mapfile -t TARGETS < <(default_targets_for_host)
fi

for target in "${TARGETS[@]}"; do
  if ! rustup target list --installed | grep -qx "$target"; then
    echo "Rust target is not installed: $target" >&2
    echo "Install it with:" >&2
    echo "  rustup target add $target" >&2
    exit 1
  fi

  echo "Building release binary for $target..."
  cargo build --release --target "$target"
done
