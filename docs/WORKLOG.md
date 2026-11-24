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

## M1 - ARM32 hook correctness (findings H1, H2, H3, H4, B3)

### Changes

1. **Thumb jump halfword order (H1).** `encodeAbsoluteJump(.., Isa::Thumb)` wrote
   `0xF8DFF000` as one little-endian word, so memory held `00 F0 DF F8`: the CPU
   decoded `0xF000` first, the opening halfword of a BL. The encoder now appends
   two halfwords, `0xF8DF` then `0xF000`, so the bytes are `DF F8 00 F0`.
2. **Thumb jump alignment (H2).** `ldr.w pc, [pc, #0]` reads its literal from
   `Align(PC, 4)` with `PC = address + 4`. At an entry of `2 mod 4` the literal
   landed on the instruction's own second halfword. `encodeJumpPatch(entry, ..)`
   now prefixes a 16-bit `nop` (`0xBF00`) for those entries; the patch is 10
   bytes instead of 8 and the extra 2 bytes are part of the overwritten and
   restored range.
3. **Explicit ISA (H3).** `InlineHook::install` takes an `Isa` and refuses
   `Isa::Unknown`; the ISA is no longer inferred from address bit 0. Each
   `GameTargets` entry is a `GameTarget {address, isa, expectedPrologue,
   prologueMask, expectedSize}`. A target without an ISA is refused.
4. **Prologue verification (B3).** `hooks::verifyPrologue(target, expected, mask,
   count)` compares the live bytes with the bytes the pattern matched, reporting
   the first difference. `GameHooks::install` refuses a target whose prologue does
   not match, and refuses targets with no expectation at all unless
   `GameOptions::allowUnverified` is set (default false).
5. **Relocator fail-closed rules (H4).** Refused prologues now also include
   ARM data-processing instructions that read r15 (`mov r0, pc`), Thumb
   high-register operations with PCs operand (`add/mov rX, pc`), IT blocks, and
   TBB/TBH table branches. `mrs`/`msr`/`bx`/`blx` keep their 1111 fields as
   status-register or register numbers and are still relocated.
6. **Page protection (H4).** The real protection of every page covering the
   patch is read from `/proc/self/maps`; the hook makes the pages read+write
   (execute removed) and restores exactly what was there. Read+write+execute is
   only a fallback, used when the first form is refused, and is logged.

### Verification

| Suite | Command | Result |
| --- | --- | --- |
| C++ host | `scripts/run_host_tests.sh` | `ALL TESTS PASSED (79 cases, 0 failures)` (was 55) |
| Kotlin | `./gradlew :road-loader-core:testDebugUnitTest :satellite:testDebugUnitTest` | BUILD SUCCESSFUL, 6 tests, 0 failures |
| Android | `./gradlew :app:assembleDebug` | BUILD SUCCESSFUL; `libroadloader32.so` for `armeabi-v7a` contains the new refusal strings |

The new tests fail against the M0 code:

- `thumb_jump_bytes_are_halfword_ordered` and the changed assertions in
  `absolute_jump_encodings` / `thumb_prologue_is_relocated` compare the bytes
  `DF F8 00 F0`, which the old single-word write could not produce.
- `thumb_jump_patch_pads_a_2_mod_4_entry` needs 10 bytes with a leading `0xBF00`;
  `encodeJumpPatch` did not exist.
- `arm_data_processing_from_pc_is_refused`, `thumb_high_register_read_of_pc_is_refused`,
  `thumb_it_block_is_refused` and `thumb_table_branch_is_refused` describe
  instructions the old relocator copied verbatim and reported as supported.
- `unknown_isa_is_refused_by_the_relocator`, `inline_hook_rejects_a_missing_isa`,
  `evaluate_target_needs_an_isa`, `game_hooks_refuse_a_target_without_an_isa`
  exercise an ISA argument that did not exist.
- `prologue_verification_*` and `game_hooks_refuse_an_unverified_target` /
  `game_hooks_refuse_a_prologue_mismatch` exercise `verifyPrologue` and the
  refusal path, neither of which existed.

### NOT RUN

- Any hook against a real `libminecraftpe.so`: no 32-bit device or emulator is
  attached, and the library is not present in this environment.
- Execution of a patched function end to end: the host is x86_64, where
  `InlineHook::install` returns `Unsupported` by design, so the tests cover the
  encoding, relocation, verification and refusal paths, not live execution.

### Risks and assumptions

- Relocation still copies instructions verbatim after the new checks. An
  instruction form that reads PC and is not in the list above would still be
  copied; the listed set covers the forms the compiler emits in function
  prologues.
- Rewriting a prologue while another thread executes it is inherently racy. The
  install path keeps the window to one `memcpy` and flushes the instruction
  cache, but it cannot stop another thread from executing the half-written
  prologue. Suspending threads around the patch is not implemented.
