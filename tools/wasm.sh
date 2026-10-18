#!/bin/sh
# Builds the engine for the browser with the rustup stable toolchain, which has the wasm32 target, even where
# another rust (Homebrew's) shadows it on PATH and the rustup proxies are not installed. Through `rustup run`
# the toolchain's own rust-lld finds its libLLVM; started directly, it does not.
set -e
cd "$(dirname "$0")/.."
sysroot="$(rustup run stable rustc --print sysroot)"
RUSTC="$sysroot/bin/rustc" rustup run stable cargo build --profile wasm --target wasm32-unknown-unknown --lib
ls -l target/wasm32-unknown-unknown/wasm/chess.wasm
