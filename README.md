# RoadLoader

Road Loader is the 32-bit companion engine for CaramelClient. The 64-bit UI keeps
the modern client, while a separate companion process hosts the armeabi-v7a
engine, the shared-memory bridge and the Rust core. Nothing in this repository
depends on a Luau VM.

The project is at release-candidate stage: the IPC bridge, the Rust engine, the
Android client and the optional satellite APK all build and run, and the client
has been exercised end to end on a cloud Android 14 phone.

| Piece | Path | Language |
| --- | --- | --- |
| Rust core (`libroad_core_32.so`) | `rust/road_core_32` | Rust 2021 |
| JNI + hook layer (`libroadloader32.so`) | `road_loader_32` | C++17 / NDK |
| Kotlin bridge and UI | `road-loader-core`, `app` | Kotlin / Java |
| Optional 32-bit satellite APK | `satellite` | Kotlin |

## Documentation

* [Architecture, IPC and the C-ABI](docs/ROAD_LOADER_ARCHITECTURE.md)
* [Writing native 32-bit mods](docs/NATIVE_MODS.md)
* [Troubleshooting](docs/TROUBLESHOOTING.md)
* Rendered site: <https://lynxifyy.github.io/RoadLoader/>

## Build

Requirements: JDK 21, Android SDK with platform 35, NDK `27.2.12479018`, a Rust
toolchain with the `armv7-linux-androideabi` and `aarch64-linux-android` targets.

```bash
rustup target add armv7-linux-androideabi aarch64-linux-android
scripts/build_rust.sh release          # stages jniLibs for both ABIs
./gradlew :app:assembleDebug :satellite:assembleDebug
```

`local.properties` needs `sdk.dir=/path/to/android-sdk`. The Rust artifacts are
staged into `road-loader-core/src/main/jniLibs` and are not committed; always run
`scripts/build_rust.sh` before Gradle in a fresh checkout.

## Test

```bash
cd rust && cargo test --workspace     # 102 Rust unit tests
scripts/run_host_tests.sh             # 55 C++ hook, signature, IPC and allocator tests
./gradlew :road-loader-core:testDebugUnitTest   # 6 Kotlin unit tests
```

## What runs where

* The UI process (arm64-v8a) owns the shared region, renders frames with OpenGL
  ES and plays audio.
* The companion (`android:process=":road32"`, or the satellite APK) maps the
  region read/write, ticks the engine, renders frames into the slots and pushes
  PCM blocks back.
* The file descriptor travels over a Binder `Messenger`, because Android 14
  refuses to parcel descriptors inside intents.

## Verified on hardware

Validated on a Mobilebox cloud phone (Android 14, 2 GB, SwiftShader GPU):

* companion spawn, frame streaming and input round trip (`in 2/2`)
* audio bridge (`audio 2/2`, both blocks played by the client `AudioTrack`)
* asset streaming: 512 KiB in 125 ms (4.0 MiB/s), 2260/2260 chunks accepted
* self-test `0x8000001f` (mesh, math, physics, alloc, ipc)
* cross-app satellite process streaming frames through the same bridge
* clean stop, no crash records in `logcat`

The demo renderer draws a synthetic scene; hooking a real `libminecraftpe.so`
still needs a device with the 1.17 game binary installed.
