#!/usr/bin/env bash
# Cross-compiles the Rust core for the two ABIs shipped in the APK.
#
# Usage: scripts/build_rust.sh [debug|release]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${1:-release}"
NDK_VERSION=27.2.12479018
API=26

if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  NDK="$ANDROID_NDK_HOME"
elif [[ -n "${ANDROID_NDK_ROOT:-}" ]]; then
  NDK="$ANDROID_NDK_ROOT"
elif [[ -n "${ANDROID_HOME:-}" ]]; then
  NDK="$ANDROID_HOME/ndk/$NDK_VERSION"
else
  echo "error: no Android NDK: ANDROID_NDK_HOME, ANDROID_NDK_ROOT and ANDROID_HOME are unset" >&2
  echo "       set ANDROID_NDK_HOME=<path to ndk;$NDK_VERSION> or ANDROID_HOME=<sdk root>" >&2
  exit 1
fi

PREBUILT="$NDK/toolchains/llvm/prebuilt"
if [[ ! -d "$PREBUILT" ]]; then
  echo "error: NDK toolchain not found at $PREBUILT" >&2
  echo "       NDK is '$NDK'; install ndk;$NDK_VERSION with sdkmanager or set ANDROID_NDK_HOME" >&2
  exit 1
fi

# The NDK ships one host tag per build machine: linux-x86_64, linux-aarch64,
# darwin-x86_64 or darwin-arm64. Pick the one this machine can actually use
# instead of assuming linux-x86_64.
case "$(uname -s 2>/dev/null)-$(uname -m 2>/dev/null)" in
  Linux-x86_64) HOST_TAG=linux-x86_64 ;;
  Linux-aarch64|Linux-arm64) HOST_TAG=linux-aarch64 ;;
  Darwin-x86_64) HOST_TAG=darwin-x86_64 ;;
  Darwin-arm64) HOST_TAG=darwin-arm64 ;;
  *) HOST_TAG="" ;;
esac

if [[ -z "$HOST_TAG" || ! -x "$PREBUILT/$HOST_TAG/bin/llvm-ar" ]]; then
  HOST_TAG=""
  for candidate in "$PREBUILT"/*/; do
    [[ -x "${candidate}bin/llvm-ar" ]] || continue
    if [[ -n "$HOST_TAG" ]]; then
      echo "error: several NDK host toolchains in $PREBUILT; set ANDROID_NDK_HOME to one" >&2
      ls -1 "$PREBUILT" >&2
      exit 1
    fi
    HOST_TAG="$(basename "$candidate")"
  done
fi

if [[ -z "$HOST_TAG" ]]; then
  echo "error: no usable NDK host toolchain under $PREBUILT" >&2
  echo "       host is $(uname -s)/$(uname -m); expected linux-x86_64, linux-aarch64," >&2
  echo "       darwin-x86_64 or darwin-arm64 with bin/llvm-ar" >&2
  exit 1
fi

TOOLCHAIN="$PREBUILT/$HOST_TAG"
echo "Using NDK host toolchain $TOOLCHAIN"

export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH (looked in $HOME/.cargo/bin); install Rust from https://rustup.rs" >&2
  exit 1
fi
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$TOOLCHAIN/bin/armv7a-linux-androideabi${API}-clang"
export CC_armv7_linux_androideabi="$TOOLCHAIN/bin/armv7a-linux-androideabi${API}-clang"
export AR_armv7_linux_androideabi="$TOOLCHAIN/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/bin/aarch64-linux-android${API}-clang"
export CC_aarch64_linux_android="$TOOLCHAIN/bin/aarch64-linux-android${API}-clang"
export AR_aarch64_linux_android="$TOOLCHAIN/bin/llvm-ar"

FLAGS=()
if [[ "$PROFILE" == "release" ]]; then
  FLAGS+=(--release)
fi

cd "$ROOT/rust"
cargo build "${FLAGS[@]}" --target armv7-linux-androideabi
cargo build "${FLAGS[@]}" --target aarch64-linux-android

DEST="$ROOT/road-loader-core/src/main/jniLibs"
mkdir -p "$DEST/armeabi-v7a" "$DEST/arm64-v8a"
cp "target/armv7-linux-androideabi/$PROFILE/libroad_core_32.so" "$DEST/armeabi-v7a/"
cp "target/aarch64-linux-android/$PROFILE/libroad_core_32.so" "$DEST/arm64-v8a/"

"$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$DEST/armeabi-v7a/libroad_core_32.so"
"$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$DEST/arm64-v8a/libroad_core_32.so"

echo "Rust core staged in $DEST"
ls -l "$DEST/armeabi-v7a/libroad_core_32.so" "$DEST/arm64-v8a/libroad_core_32.so"
