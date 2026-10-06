#!/usr/bin/env bash
# Rebuild the engine for web and regenerate the wasm-bindgen glue (Task 1.4).
#
#   bash scripts/build-wasm.sh [debug|release]
#
# Toolchain choice: plain `wasm-bindgen --target web` (NOT wasm-pack) — the
# boundary is a few string-in/string-out methods, so wasm-pack's npm-packaging
# layer buys nothing here. The CLI version MUST match Cargo.lock exactly;
# this script enforces that before building.
set -euo pipefail

PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OUT="$ROOT/apps/vectra-web/src/wasm"

LOCK_VERSION="$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"/\1/p}' "$ROOT/Cargo.lock" | head -n 1)"
if [ -z "$LOCK_VERSION" ]; then
  echo "error: could not read wasm-bindgen version from Cargo.lock" >&2
  exit 1
fi
if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "error: wasm-bindgen CLI not found. Install the lockfile-pinned version:" >&2
  echo "  cargo install wasm-bindgen-cli --version $LOCK_VERSION" >&2
  exit 1
fi
CLI_VERSION="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$CLI_VERSION" != "$LOCK_VERSION" ]; then
  echo "error: wasm-bindgen CLI ($CLI_VERSION) != Cargo.lock ($LOCK_VERSION)." >&2
  echo "  cargo install wasm-bindgen-cli --version $LOCK_VERSION" >&2
  exit 1
fi

BUILD_ARGS=(--target wasm32-unknown-unknown -p vectra-wasm)
WASM_BIN="$ROOT/target/wasm32-unknown-unknown/debug/vectra_wasm.wasm"
if [ "$PROFILE" = "release" ]; then
  BUILD_ARGS+=(--release)
  WASM_BIN="$ROOT/target/wasm32-unknown-unknown/release/vectra_wasm.wasm"
fi

echo ">> cargo build ${BUILD_ARGS[*]}"
(cd "$ROOT" && cargo build "${BUILD_ARGS[@]}")

echo ">> wasm-bindgen --target web --out-dir $OUT"
mkdir -p "$OUT"
wasm-bindgen --target web --out-dir "$OUT" "$WASM_BIN"
ls -la "$OUT"
