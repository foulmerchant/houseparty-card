#!/usr/bin/env bash
# Release build: Rust -> WASM -> wasm-bindgen -> wasm-opt -> shim bundle -> checksums.
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE=houseparty_card_ui
TARGET=wasm32-unknown-unknown
# Some mounted workspaces do not support Cargo's hardlinks; callers can move
# the build cache without changing where pkg/ and dist/ are produced.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}"

cargo build -p houseparty-card-ui --target "$TARGET" --release --locked

# The wasm-bindgen CLI must match the library version in Cargo.lock exactly.
want=$(cargo metadata --format-version 1 --locked \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="wasm-bindgen"))')
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [[ "$have" != "$want" ]]; then
  echo "wasm-bindgen CLI is '${have:-missing}', library is $want." >&2
  echo "Run: cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

rm -rf pkg dist
wasm-bindgen --target web --out-dir pkg "$CARGO_TARGET_DIR/$TARGET/release/$CRATE.wasm"

if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz \
    --enable-bulk-memory --enable-sign-ext --enable-nontrapping-float-to-int \
    --enable-mutable-globals --enable-reference-types \
    -o "pkg/${CRATE}_bg.wasm" "pkg/${CRATE}_bg.wasm"
else
  echo "warning: wasm-opt not found, so the binary will be larger. Install binaryen." >&2
fi

# --ignore-scripts: no npm install scripts run on the build machine.
(cd shim && npm ci --ignore-scripts && npm run build)

(cd dist && sha256sum -- * > SHA256SUMS)
ls -lh dist
