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

## M2 - Frame path and honest frame rate

### What was wrong

1. **The number labelled FPS was the presenter's draw rate.** `FrameRenderer`
   incremented `framesThisSecond` on every `onDrawFrame`, and the view runs with
   `RENDERMODE_CONTINUOUSLY`, so it reported the display refresh rate (or the
   software renderer's rate) no matter what the companion produced. A stopped
   companion still showed 60. The architecture doc then described that number as
   the demo scene's FPS, which made the mistake look like a measurement.
2. **The frame descriptor was not checked.** `RoadSession.pollFrame` trusted
   `width`, `height` and `stride` from shared memory and `FrameSurfaceView`
   uploaded `width * height * 4` bytes, ignoring `stride`. A padded stride would
   shear the image, and a slot smaller than the claimed size would upload past
   the end of the mapping.
3. **A refused frame leaked its slot.** The poll claims a slot (`READY ->
   READING`); the old code had no path that released it without drawing, so a
   descriptor that failed to map a buffer would burn one of the three slots per
   poll and stall the stream.
4. **Nothing measured frame age**, and nothing said the stream had stalled: the
   last frame stayed on screen while the overlay kept printing the last FPS.
5. **`RoadStats.describe()` hid the presentation counters** (presented and
   dropped frames existed in the header but were never shown).

### Changes

* `FrameContract` (new, `road-loader-core`): validates size, stride, declared
  length and slot capacity, and reports the reason instead of drawing.
* `FrameRateCounter` (new): frames per second from a monotonic counter, with a
  minimum window, a reset when the counter goes backwards, and no rate on the
  first sample.
* `RoadSession.pollFrame`: validates through `FrameContract`, releases the slot
  on every refusal, logs the first problem once, and returns `length` and
  `publishedNs` alongside the pixels. Frame-info indices moved into
  `RoadConstants` (`FRAME_INFO_*`) instead of magic numbers.
* `FrameSurfaceView`: counts only frames that were actually received and
  uploaded, skips a re-upload when the sequence did not change, repacks rows
  into a scratch buffer when `stride != width * 4` (logged once), and reports
  `PresentStats(fps, frameAgeMs, framesTotal)` once per second. `frameAgeMs` is
  measured from the companion's publish timestamp to the upload, on the same
  `CLOCK_MONOTONIC` the UI reads.
* `MainActivity`: the overlay now shows `FPS <engine> (shown <presented>)`,
  where engine is the `frame_count` rate between stats polls. Developer mode adds
  `age` and the presented total. The counters reset on stop, and `no frames`
  appears when the stream stalls after the first frame.
* `RoadStats.describe()` reports produced / presented / dropped frames.
* `docs/ROAD_LOADER_ARCHITECTURE.md` explains which number is which and states
  that input latency is not measured, because nothing records when the companion
  consumed an event.

### Verification

| Suite | Command | Result |
| --- | --- | --- |
| Kotlin | `./gradlew :road-loader-core:testDebugUnitTest` | 19 tests, 0 failures (was 6) |
| Rust | `cargo test --workspace` | 102 passed, 0 failed |
| C++ host | `scripts/run_host_tests.sh` | 79 cases, 0 failures |
| Android | `./gradlew :app:assembleDebug :satellite:assembleDebug` | BUILD SUCCESSFUL; new strings present in `classes.dex` |

The new Kotlin tests fail against the old code: `FrameContract` and
`FrameRateCounter` did not exist, so `FrameContractTest` and
`FrameRateCounterTest` could not compile, and `RoadStatsTest` did not see the
presentation counters.

### NOT RUN

* No device or emulator is attached, so the overlay was not read on screen and
  no frame age was observed on hardware. The frame-age value depends on the
  companion's `CLOCK_MONOTONIC` and the UI's `System.nanoTime` sharing an epoch,
  which is true on Android/Linux but was not exercised here.
* The stride repack path was not exercised end to end: the companion always
  publishes `stride == width * 4`, so only `FrameContract`'s arithmetic is
  covered by tests, not a real padded frame.

### Environment note

`~/.cargo` disappeared from this container between M1 and M2 (the M1 build used
it). Rust 1.100.0 was reinstalled with rustup and the `armv7-linux-androideabi`,
`aarch64-linux-android` and `x86_64-unknown-linux-gnu` targets, then all suites
were re-run from that toolchain.

## M3 - A real 32-bit process, and an attach path that trusts nothing

### What was wrong

1. **The `:road32` process is not 32-bit.** An APK installs at one ABI and every
   process in it runs at that ABI. The main APK ships `armeabi-v7a` and
   `arm64-v8a`, so on a 64-bit device the companion process was 64-bit and loaded
   the arm64 build of `libroadloader32.so`. Nothing said so: the service name,
   the manifest comment and the docs all claimed 32 bits, and the demo ran, so no
   test could notice.
2. **The satellite shipped both ABIs**, which defeated its purpose. With
   `arm64-v8a` present it installed as a 64-bit app on 64-bit devices, so the
   "32-bit engine" APK was not 32-bit either.
3. **The attach path trusted the caller.** The satellite service is exported, and
   it mapped whatever size the caller asked for. `Region::attach` validated the
   magic, ABI version and dimensions, but `SharedMemory::from_fd` mmapped the
   caller's `size` after only checking `fstat`, so a caller could ask a 32-bit
   process to map gigabytes.
4. **A failed attach leaked the descriptor.** `descriptor.close()` ran after
   `NativeCompanion.attach(...)`; a throw (for example `UnsatisfiedLinkError` in
   a broken build) skipped it.
5. **A killed companion was restarted by the system** with `START_STICKY`, then
   sat in a foreground notification for 15 s with no client before stopping.

### Changes

* `CompanionServiceBase` reports the truth: the attach reply carries
  `Process.is64Bit()` in `arg2` and a Bundle with `NativeCompanion.abiInfo()`
  (the loaded library's ABI, pointer size, page size) plus the caller's uid. It
  logs the caller uid, closes the descriptor in a `finally`, refuses
  `size <= 0` and `size > 128 MiB` before touching the core, returns
  `START_NOT_STICKY`, and answers `MSG_QUERY_ABI` for a client that wants the
  report without attaching.
* `CompanionAttach` (new): the attach codes and their sentences, shared by the
  service and the client, with `sizeAllowed()` next to `RoadConstants.MAX_REGION_BYTES`.
* `satellite/build.gradle.kts`: `abiFilters += "armeabi-v7a"`. The satellite APK
  now contains only `lib/armeabi-v7a`, so a 64-bit device with 32-bit support
  installs it as a genuine 32-bit app. It no longer installs on 64-bit-only
  hardware, which is correct: that hardware cannot run the code.
* Rust: `MAX_REGION_BYTES` (128 MiB) in `ipc/layout.rs`, `AttachError::TooLarge`,
  a check in `Region::attach` before anything is mapped, and the C-ABI code `-4`
  for it (documented in `RoadCoreAbi.h`).
* `MainActivity`: shows `32-bit` or `64-bit` in the overlay, prints the reported
  ABI in the status line, warns when the in-APK companion is 64-bit, and turns
  each attach code into a sentence. Developer mode adds the ABI string and uid.
* `ROAD_LOADER_ARCHITECTURE.md`: a table of which install yields a 32-bit
  process, and the attach security model with its residual risk. `TROUBLESHOOTING.md`:
  the 64-bit companion, the size refusal and the busy refusal.

### Verification

| Suite | Command | Result |
| --- | --- | --- |
| Kotlin | `:road-loader-core:testDebugUnitTest` | 23 tests, 0 failures (was 19) |
| Rust | `cargo test --workspace` | 105 passed, 0 failed (was 102) |
| C++ host | `scripts/run_host_tests.sh` | 79 cases, 0 failures |
| APK ABIs | `zipfile` listing of both debug APKs | `app`: `arm64-v8a` + `armeabi-v7a`; `satellite`: `armeabi-v7a` only |
| UI strings | `classes.dex` of `app-debug.apk` | ABI warning and attach sentences present |

New tests: `attach_rejects_a_region_larger_than_the_cap`,
`the_cap_covers_the_largest_accepted_resolution`,
`the_cap_is_below_a_32_bit_address_space`, `CompanionAttachTest` (4 cases).

### NOT RUN

* No device or emulator, so nothing here was observed on hardware: the reply
  `arg2`/Bundle path, the satellite installing as 32-bit, and `Process.is64Bit()`
  returning false were not executed. They are the parts most worth checking on
  the cloud phone.
* The 64-bit-only-device case (satellite refuses to install) is untested.

### Residual risk

The satellite is exported so another app can drive it, so any app on the device
can take the single session and deny it to the real client. Making the service
signature-protected would break the cross-app client this project is built to
serve, so the choice is documented rather than hidden.
