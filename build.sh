#!/bin/sh
# Builds every module (or the ones named) to dist/<module>.wasm in a Rust container.
# Usage: ./build.sh [module ...]
set -e
cd "$(dirname "$0")"
mods="${*:-template pdf-to-json identifier-validator fixed-width-parser fixed-width-writer xml-to-json json-repair text-extractor email-parser gs1-barcode-parser pii-redactor csv-cleaner text-chunker record-diff}"
mkdir -p dist
for m in $mods; do
  docker run --rm -v "$PWD/$m":/src -v cargo-cache:/usr/local/cargo/registry -w /src rust:1-slim \
    sh -c 'rustup target add wasm32-unknown-unknown >/dev/null 2>&1; cargo build --release --target wasm32-unknown-unknown --lib'
  f=$(ls "$m"/target/wasm32-unknown-unknown/release/*.wasm | head -1)
  cp "$f" "dist/$m.wasm"
  rm -rf "$m/target"   # build cache; the only copy kept is dist/<module>.wasm
  echo "$m: $(wc -c < "dist/$m.wasm" | tr -d ' ') bytes"
done
