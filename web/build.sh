#!/bin/sh
# Builds the pond for the browser into web/www/duck_pond.wasm. Serve web/www
# from any static server, e.g. `python3 -m http.server -d web/www`.
set -e
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown -p duck-pond-web
cp target/wasm32-unknown-unknown/release/duck_pond_web.wasm web/www/duck_pond.wasm
if command -v wasm-opt >/dev/null; then
    wasm-opt -O3 web/www/duck_pond.wasm -o web/www/duck_pond.wasm
fi
echo "built web/www/duck_pond.wasm"
