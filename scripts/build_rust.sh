#!/usr/bin/env bash
# Cross-compiles the Rust core for the two ABIs shipped in the APK.
#
# Usage: scripts/build_rust.sh [debug|release]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${1:-release}"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$ANDROID_HOME/ndk/27.2.12479018}}"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
API=26

if [[ ! -d "$TOOLCHAIN" ]]; then
  echo "NDK toolchain not found at $TOOLCHAIN" >&2
  echo "Set ANDROID_NDK_HOME or install ndk;27.2.12479018 with sdkmanager." >&2
  exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"
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
