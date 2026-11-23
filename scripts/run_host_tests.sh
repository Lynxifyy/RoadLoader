#!/usr/bin/env bash
# Runs the Rust unit tests and the host-side C++ tests (including a real
# cross-process shared-memory test).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

echo "== Rust tests =="
(cd "$ROOT/rust" && cargo test --workspace)

echo "== Building host core =="
(cd "$ROOT/rust" && cargo build --release)

echo "== C++ host tests =="
CMAKE_BIN="${CMAKE_BIN:-}"
if [[ -z "$CMAKE_BIN" ]]; then
  if command -v cmake >/dev/null 2>&1; then
    CMAKE_BIN="$(command -v cmake)"
  elif [[ -x "${ANDROID_HOME:-/workspace/android-sdk}/cmake/3.22.1/bin/cmake" ]]; then
    CMAKE_BIN="${ANDROID_HOME:-/workspace/android-sdk}/cmake/3.22.1/bin/cmake"
  else
    echo "cmake not found" >&2
    exit 1
  fi
fi
"$CMAKE_BIN" -S "$ROOT/road_loader_32/tests" -B "$ROOT/build/host-tests" \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DROAD_CORE_LIB_DIR="$ROOT/rust/target/release"
"$CMAKE_BIN" --build "$ROOT/build/host-tests" -j "$(nproc)"
"$ROOT/build/host-tests/road_host_tests"
