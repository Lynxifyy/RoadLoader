---
title: Road Loader
---

Road Loader is the 32-bit companion engine for **CaramelClient**. A 64-bit UI
process keeps the modern client, and a separate companion process hosts the
armeabi-v7a engine, the shared-memory bridge and the Rust core. No Luau VM is
involved anywhere.

## What it does

* spawns the 32-bit engine as `android:process=":road32"` or as an optional
  satellite APK;
* streams RGBA frames from the engine to the 64-bit UI through shared memory,
  with a three-slot state machine that prevents tearing;
* pipes touch and controller input back into the engine through a lock-free ring;
* carries 48 kHz stereo float PCM audio the other way;
* replaces the engine's math, physics, meshing and entity ticking with a
  `mimalloc`-backed Rust core;
* injects ES 3.0 post-processing shaders transpiled from Java Edition GLSL.

## Documentation

* [Architecture, IPC and the C-ABI](ROAD_LOADER_ARCHITECTURE)
* [Writing native 32-bit mods](NATIVE_MODS)
* [Troubleshooting](TROUBLESHOOTING)

## Verified

Tested end to end on a cloud Android 14 phone: companion spawn, frame streaming,
input round trip, audio bridge, 512 KiB asset stream at 4 MiB/s, self-test
`0x8000001f`, and a cross-app satellite process using the same bridge.

Source: <https://github.com/Lynxifyy/RoadLoader>
