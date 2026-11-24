# Worklog

Results per milestone: what ran, the counts that came back, and what could not
run here. Counts are verbatim from the command output; nothing is summarised as
"passing".

## Environment used for these runs

| Item | Value |
| --- | --- |
| Host | Linux x86_64, 4 cores |
| Rust | rustup 1.100.0, `stable-x86_64-unknown-linux-gnu`, targets `armv7-linux-androideabi`, `aarch64-linux-android` |
| JDK | Temurin 21.0.13.1 at `/workspace/tools/jdk21` |
| Android SDK | `/workspace/android-sdk` (platform 35, NDK 27.2.12479018, CMake 3.22.1) |
| Device | none attached - no emulator, no phone |

## M0 - baseline and tooling (finding M5)

### Baseline, before any change

| Suite | Command | Result |
| --- | --- | --- |
| Rust | `cd rust && cargo test --workspace` | 102 passed, 0 failed, 0 ignored |
| C++ host | `scripts/run_host_tests.sh` | `ALL TESTS PASSED (55 cases, 0 failures)` |
| Kotlin | `./gradlew :road-loader-core:testDebugUnitTest --rerun-tasks` | 6 tests, 0 failures (RoadStatsTest) |
| Android | `./gradlew :app:assembleDebug :satellite:assembleDebug` | BUILD SUCCESSFUL, `app-debug.apk` 6314838 bytes, `satellite-debug.apk` 6298102 bytes |

`--rerun-tasks` was needed for the Kotlin suite: the first invocation reported
`:road-loader-core:testDebugUnitTest UP-TO-DATE` and ran no tests.

### Changes

`scripts/build_rust.sh` (finding M5):

1. The NDK path is no longer assembled with `$ANDROID_HOME/ndk/...` under
   `set -u`. The script now checks `ANDROID_NDK_HOME`, then `ANDROID_NDK_ROOT`,
   then `ANDROID_HOME`, and fails with the list of what was unset.
2. The host toolchain tag is detected instead of hard-coded to
   `linux-x86_64`: `uname -s` / `uname -m` maps to `linux-x86_64`,
   `linux-aarch64`, `darwin-x86_64` or `darwin-arm64`; if that directory has no
   `bin/llvm-ar` the script falls back to the only `prebuilt/*` directory that
   does, and refuses to guess when several exist.
3. Distinct, readable errors for: no NDK variable set, NDK directory without
   `toolchains/llvm/prebuilt`, no usable host toolchain, and `cargo` missing
   from `PATH`.

`road-loader-core/build.gradle.kts` (finding M5): `cargoBuild` now declares
inputs (`rust/**/*.rs`, `Cargo.toml`, `Cargo.lock`, `scripts/build_rust.sh`,
`ndkVersion`) and outputs (the two staged `libroad_core_32.so` files) instead of
`outputs.upToDateWhen { false }`.

### Verification of the M0 changes

NDK error paths, all `exit=1` with one distinguishable message each:

| Invocation | Message |
| --- | --- |
| `env -u ANDROID_NDK_HOME -u ANDROID_NDK_ROOT -u ANDROID_HOME bash scripts/build_rust.sh` | `error: no Android NDK: ANDROID_NDK_HOME, ANDROID_NDK_ROOT and ANDROID_HOME are unset` |
| `ANDROID_NDK_HOME=/tmp/no-such-ndk` | `error: NDK toolchain not found at /tmp/no-such-ndk/toolchains/llvm/prebuilt` |
| `ANDROID_NDK_HOME=/tmp/fake-ndk2` (prebuilt present, empty) | `error: no usable NDK host toolchain under ...` with the detected host |
| `PATH=/usr/bin:/bin` | `error: cargo not found on PATH (looked in /tmp/nohome/.cargo/bin)` |
| `ANDROID_HOME=/workspace/android-sdk`, normal run | `Using NDK host toolchain /workspace/android-sdk/ndk/27.2.12479018/toolchains/llvm/prebuilt/linux-x86_64`, then `Rust core staged` |

`cargoBuild` incrementality:

| Scenario | Observed |
| --- | --- |
| First run after editing the script | `> Task :road-loader-core:cargoBuild`, `BUILD SUCCESSFUL in 2s` |
| Second run, nothing changed | `> Task :road-loader-core:cargoBuild UP-TO-DATE`, `BUILD SUCCESSFUL in 832ms` |
| Rust source changed (marker line added to `src/lib.rs`) | task executed, `Compiling road_core_32 v2.1.0` for both targets, `BUILD SUCCESSFUL in 3s` |
| Marker reverted | task executed again, `Compiling road_core_32 v2.1.0` for both targets, `BUILD SUCCESSFUL in 3s` |
| Nothing changed again | `UP-TO-DATE` |

The cross-compile is now real rather than replayed from a stale fingerprint:
`cargo` previously reported `Finished release profile in 0.02s` because the
`armv7-linux-androideabi` / `aarch64-linux-android` targets were absent from the
freshly installed toolchain. Both targets were added, after which the script
compiled both ABIs from source.

### NOT RUN

- Any device or emulator step: no device is attached in this environment.
- `pl_test`-style on-device hook tests: they need an ARM device.

### Risks and assumptions

- The host-tag detection assumes one NDK install per machine. A machine with two
  `prebuilt/*` directories that both contain `llvm-ar` is rejected rather than
  guessed; the message tells the caller to pin `ANDROID_NDK_HOME`.
- `inputs.files(...).withPathSensitivity(RELATIVE)` means a Rust source change is
  detected by content, not mtime, so `touch` alone does not trigger a rebuild.
  That is intended: the staged `.so` only needs to change when the sources do.
