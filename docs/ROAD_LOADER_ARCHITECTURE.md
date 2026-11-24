# Road Loader architecture

Road Loader lets a 64-bit client drive a 32-bit Minecraft engine (1.16 / 1.17)
without sharing an address space. The two processes exchange pixels, input,
audio and commands through one fixed-layout shared region; the Rust core owns the
layout and every other language talks to it through a small C-ABI.

## 1. Process model

```
+--------------------------- 64-bit UI process (arm64-v8a) ---------------------------+
| MainActivity  FrameSurfaceView (GLSurfaceView)  RoadAudioBridge (AudioTrack)        |
|      |                    |                              ^                          |
|      | touch/controller   | poll_frame + release_frame   | pop_audio                |
|      v                    v                              |                          |
|  RoadSession  ------------------> libroadloader32.so (JNI) ---------------------+   |
|                                         |                                       |   |
|                                   libroad_core_32.so (C-ABI)                    |   |
+-----------------------------------------|---------------------------------------|---+
                                          | mmap(PROT_READ|PROT_WRITE)            |
                                          v                                       |
+--------------------------- companion process (:road32 / satellite) ----------- | ---+
| CompanionServiceBase  ->  NativeCompanion.attach(fd, size, flags, heapLimit)   |   |
|                                 |                                              |   |
|                         libroad_core_32.so  (Region::attach)                   |   |
|                                 |                                              |   |
|   engine loop: render slot -> heartbeat -> stats -> audio tone (demo scene)    |   |
+--------------------------------------------------------------------------------+
```

The companion runs as `android:process=":road32"` inside the same APK, or in the
optional `com.caramel.roadloader.satellite` APK. Both use the same service base
class, so the bridge does not care which one hosts the engine. The companion is a
foreground service with a low-importance notification; it never appears in the
task switcher because it has no activity.

### Which process is actually 32-bit

An APK is installed at one ABI, the device's preferred one, and every process of
that APK runs at that ABI. A `:road32` service inside the main APK is therefore
**64-bit** on a 64-bit device: it is a second process, not a second ABI. The
name describes its role, not its pointer size.

| Install | `:road32` process | Real 32-bit engine? |
| --- | --- | --- |
| main APK (`armeabi-v7a` + `arm64-v8a`) on a 64-bit device | 64-bit | no |
| main APK on a 32-bit device | 32-bit | yes |
| satellite APK (`armeabi-v7a` only) on a 64-bit device with 32-bit support | 32-bit | yes |
| satellite APK on a 64-bit-only device | does not install | not possible |

The satellite ships `armeabi-v7a` only for that reason. Shipping `arm64-v8a`
there as well would let it install everywhere, but on a 64-bit device it would
then run 64-bit and provide nothing the main APK does not already provide.

The companion reports its own answer: after attaching, the service replies with
`Process.is64Bit()` and `NativeCompanion.abiInfo()` (the loaded library's ABI,
pointer size and page size). The UI shows `32-bit` or `64-bit` next to the input
counter, and says so in the status line when the in-APK companion is 64-bit.

### Attach security

The satellite service is exported so another app can drive it, which means the
attach path treats the caller as untrusted:

* the region size must be in `1..128 MiB` (`MAX_REGION_BYTES`); the core refuses
  a larger mapping before it is created, so a caller cannot make the 32-bit
  process reserve memory it can never use;
* the descriptor must carry the region magic, the matching ABI version, the
  header size the core expects, dimensions within `16..8192` and a mapping at
  least as large as the layout it describes;
* one session at a time: a second attach gets `-3` and cannot displace a running
  client;
* the caller's uid is logged with the attach and returned to the client;
* the service returns `START_NOT_STICKY`, so a killed companion is not silently
  restarted with no client.

The caller supplies the region, so attaching grants it no read access to engine
memory. What remains is denial of service: any app on the device can occupy the
single session. That is the cost of an exported engine, and the reason the
satellite is a separate, opt-in APK.

## 2. Descriptor handoff

The UI creates the region (`road_host_create`), duplicates the descriptor and
hands it to the companion. Android 14 refuses to parcel a `ParcelFileDescriptor`
inside an intent (`Not allowed to write file descriptors here`), so the
descriptor travels over Binder:

1. `startForegroundService(intent)` starts the service with no extras.
2. `bindService(intent, connection, 0)` connects to the service's `Messenger`.
3. The client sends `MSG_ATTACH` with `descriptor`, `regionSize`, `flags` and
   `heapLimit` in the message bundle. Binder transactions carry descriptors.
4. The service calls `NativeCompanion.attach(...)`, replies `MSG_ATTACHED` with
   the attach status and closes its copy.
5. The client closes its duplicate; the mapping stays alive in both processes.

If no attach arrives within 15 s the service stops itself, which keeps a stray
start from holding a process open.

## 3. Region layout

The layout is defined once in `rust/road_core_32/src/ipc/layout.rs`. Every other
language goes through the C-ABI, so offsets cannot drift.

| Offset | Size | Contents |
| --- | --- | --- |
| `0x0000` | 4096 | `RoadHeader`: magic `ROAD`, ABI version, status, flags, pid, dimensions, heartbeat, counters, heap stats |
| `0x1000` | 4096 | frame header: per-slot state and sequence |
| `0x2000` | 3 x 2,076,672 | three RGBA8888 frame slots (960x540, 4096-aligned) |
| `0x5F3000` | 64 + 65,536 | input ring, 1024 x 64-byte `InputEvent` |
| `0x603040` | 64 + 65,536 | command ring, 256 x 256-byte `Command` |
| `0x613080` | 64 + 65,536 | event ring, 256 x 256-byte `Event` |
| `0x6230C0` | 64 + 262,144 | audio ring, 64 x 4096-byte `AudioBlock` |
| `0x664000` | | end (6,701,056 bytes, 6.4 MiB for 960x540) |

`RegionLayout::new(width, height)` derives every offset from the frame size, so
a different render resolution only changes the frame area. `required_size()`
validates attachments.

### Rings

Each ring is a single-producer/single-consumer queue in the mapping:

* 64-byte header with `head`, `tail`, `dropped` (relaxed loads, acquire/release
  on the index that crosses the boundary).
* Capacities are powers of two and fixed for the region's lifetime. The producer
  never grows a ring; when it is full the event is dropped and counted.

Ownership is strict. The UI produces input and commands, and the companion
consumes them. The companion produces events, frames and audio, and the UI
consumes them. A second consumer would break the non-atomic head update, which is
why the UI is the only reader of the event ring and drains it on every stats
poll.

### Commands

The UI drives the companion with commands. Each one has a defined effect; a
command that is accepted but changes nothing is treated as a bug, not as a
placeholder.

| Kind | Value | Payload | Effect |
| --- | --- | --- | --- |
| `CMD_SET_VOLUME` (1) | volume x 1000 | - | clamps to 0..1 and scales the demo tone |
| `CMD_PATCH_OFFSET` (2) | 32-bit address | symbol name (UTF-8) | records the offset in the registry `road_patch_lookup` reads |
| `CMD_SET_RENDER_DISTANCE` (3) | chunks | - | clamps to 2..32 and redraws the grid at that density |
| `CMD_REQUEST_SELF_TEST` (4) | - | - | reruns the startup self-test and republishes the mask |
| `CMD_SHUTDOWN` (5) | - | - | stops the loop; the process exits cleanly |
| `CMD_ASSET_CHUNK` (6) | (asset id << 32) or index | bytes | stages one chunk |
| `CMD_SET_SHADERS` (7) | 0/1 | - | sets or clears `FLAG_SHADERS` |
| `CMD_SET_SAFE_MODE` (8) | 0/1 | - | sets or clears `FLAG_SAFE_MODE`, both directions |
| `CMD_ASSET_BEGIN` (9) | (asset id << 32) or chunk count | - | opens a stream; refuses 0 or more than 16384 chunks |
| `CMD_ASSET_COMMIT` (10) | (asset id << 32) or expected bytes | CRC32, 4 bytes LE | verifies the staged asset and publishes the verdict |

Every handled command logs an event, so the UI can show what the 32-bit side
did without another round trip. `CMD_PATCH_OFFSET` writes to the same registry
as the `road_patch_*` C functions, which is what the hook layer reads before it
patches a prologue.

### Asset streams

A stream is `BEGIN`, one `CHUNK` per 192 bytes, then `COMMIT`:

```
CMD_ASSET_BEGIN   value = (id << 32) | chunks
CMD_ASSET_CHUNK   value = (id << 32) | index, payload = up to 192 bytes
CMD_ASSET_COMMIT  value = (id << 32) | expected bytes, payload = crc32 LE
```

The companion assembles into a preallocated slot vector (`AssetAssembler`),
never reallocating per chunk. The commit compares the staged length and the
CRC32 of the whole payload against what the UI says it sent, then publishes
`asset_bytes`, `asset_crc` and one of the `ASSET_STATUS_*` codes in the header.
The UI polls those three fields, so "streamed" means "the 32-bit side proved it
assembled exactly those bytes".

### Frame slots and tearing

Frames use three slots with an explicit state machine:

```
FREE -> WRITING -> READY -> READING -> FREE
```

The companion writes only in `WRITING`, then publishes `READY` with a release
store. The UI takes the oldest `READY` slot, marks it `READING` and uploads the
texture; `release_frame` returns it to `FREE`. The producer never touches a slot
that is not `FREE`, so a frame cannot be overwritten while it is being uploaded.
If all slots are busy the producer counts a dropped frame and skips the tick.

## 4. Heartbeat and death detection

The companion stores a monotonically increasing sequence and a timestamp in the
header on every tick. The UI compares the sequence between polls:

* sequence advanced -> healthy, `hb` shows the age of the last beat in ms.
* sequence unchanged for more than `HEARTBEAT_TIMEOUT_MS` -> the status text
  turns into an error, the surface is dimmed and `CrashGuard` records the event.
* `poll_frame` returning `SLOT_FREE` for every slot after the timeout is treated
  as a dead producer.

`CrashGuard` counts consecutive failures (persisted in `SharedPreferences`).
After three, launching is refused with a "32-bit Engine Failure" dialog until the
user presses *Reset guard* or starts again in safe mode.

## 5. C-ABI of `libroad_core_32.so`

Host side (called by the UI process; `session` is an opaque pointer):

| Symbol | Purpose |
| --- | --- |
| `void* road_host_create(uint32_t width, uint32_t height)` | create and initialise a region |
| `int road_host_dup_fd(void* session)` | duplicate the region descriptor |
| `int road_host_fd(void* session)` | borrowed descriptor |
| `uint64_t road_host_region_size(void* session)` | mapping size |
| `int road_host_poll_frame(void* session, RoadFrameInfo* out)` | take a ready slot |
| `void road_host_release_frame(void* session, uint32_t slot)` | return a slot |
| `int road_host_push_input(void* session, const InputEvent* event)` | enqueue touch/controller |
| `int road_host_push_command(void* session, const Command* command)` | enqueue a command |
| `int road_host_pop_event(void* session, Event* out)` | read one event |
| `int road_host_pop_audio(void* session, AudioBlock* out)` | read one PCM block |
| `int road_host_stats(void* session, RoadStats* out)` | snapshot counters |
| `void road_host_destroy(void* session)` | unmap and free |

Companion side (called inside the 32-bit process):

| Symbol | Purpose |
| --- | --- |
| `int road_companion_attach(int fd, uint64_t size, uint32_t flags, uint64_t heap_limit)` | map an existing region |
| `int road_companion_stop(void)` | stop the engine loop |
| `int road_companion_is_running(void)` | loop state |
| `uint32_t road_companion_self_test(void)` | run mesh/math/physics/alloc/ipc checks |
| `void road_companion_set_input_sink(int enabled)` | route input into the engine |

Shared helpers:

| Symbol | Purpose |
| --- | --- |
| `uint32_t road_core_version(void)` | `major<<16 | minor<<8 | patch` |
| `uint32_t road_abi_version(void)` | region ABI, currently 1 |
| `void road_set_heap_limit(uint64_t bytes)` | soft allocator limit |
| `int road_alloc_stats(AllocStats* out)` | live/peak/limit/failed/RSS |
| `uint32_t road_stats_size(void)` and the other `*_size` probes | struct sizes for the JNI layer |
| `float rust_math_distance(float x1, float y1, float z1, float x2, float y2, float z2)` | glam distance |
| `float rust_vec3_length(float x, float y, float z)` | glam length |
| `uint32_t rust_asset_chunk_count(size_t len)` | asset chunking helper |
| `int road_patch_offset(const char* name, size_t address)` | register a resolved offset |
| `size_t road_patch_lookup(const char* name)` | read it back |
| `uint32_t road_patch_count(void)` / `void road_patch_clear(void)` | inspect/reset the table |

Verify the exports of a built library with:

```bash
llvm-nm -D --defined-only libroad_core_32.so | grep ' T '
```

## 6. Hooks and signature scanning

`road_loader_32` mirrors the SweetLoader layout:

* `include/pl/memory/Signature.h` + `src/pl/memory/Signature.cpp` - pattern
  scanner with `??` wildcards and a bounded scan window.
* `include/pl/memory/ModuleMap.h` - parses `/proc/self/maps` to find
  `libminecraftpe.so`, its base and its executable ranges.
* `include/pl/hooks/InlineHook.h` + `src/pl/hooks/ArmRelocator.cpp` - ARM32
  inline hook: saves the prologue, relocates PC-relative instructions, writes a
  `B` to the trampoline and flushes the instruction cache.
* `src/pl/runtime/GameHooks.cpp` - installs and removes the hooks
  (`Level::tick`, `Actor::render`, `ScreenView::setup`, `AppPlatform::swapBuffers`,
  `eglSwapBuffers`), each with an `unhook` path.

Hooks are installed only after the scanner resolves a unique address. A miss is
reported, never guessed: patching a wrong address in a 32-bit process is an
instant SIGSEGV. `road_patch_offset` lets a mod register resolved offsets by
name so a later session can reuse them.

The demo build has no `libminecraftpe.so`; the hook layer is exercised by host
tests (`scripts/run_host_tests.sh`) against a synthetic ARM32 function, and the
UI falls back to the Rust demo renderer.

## 7. Rust engine modules

| Module | Responsibility |
| --- | --- |
| `math.rs` | `glam`-based `Vec3`/`AABB`/`Mat4` with `rust_math_distance`, `rust_vec3_length` |
| `physics.rs` | swept-AABB collision solver (`parry3d`-style, f32, IEEE-754) |
| `mesh.rs` | greedy mesher with face culling, NEON (`std::arch::arm`) fast paths, mesh size cap |
| `entities.rs` | pathfinding and entity ticks, `rayon` work splitting |
| `assets.rs` | chunked asset streaming and caching |
| `shader.rs` | Java GLSL -> ES 3.0 transpiler and shader validation |
| `audio.rs` | 48 kHz stereo f32 block packing |
| `render.rs` | demo scene, safe mode, frame slot writer |
| `alloc.rs` | mimalloc global allocator, accounting and soft limit |
| `crash.rs` | panic hook, signal-safe breadcrumb |

Every module is unit tested on the host (`cargo test --workspace`), including the
cross-mapping audio test and the ring-buffer stress tests.

## 8. Audio bridge

The companion packs interleaved f32 PCM into 4096-byte `AudioBlock`s (32-byte
header + 4064 bytes, about 21 ms of 48 kHz stereo). The UI pops blocks on a
`THREAD_PRIORITY_URGENT_AUDIO`-equivalent thread (`Thread.MAX_PRIORITY`), writes
them through a direct `ByteBuffer` (float PCM cannot be written from a
`ByteArray`) and releases the track on stop. If the device has no float sink the
bridge keeps draining so the ring never fills, and the UI reports why audio is
silent instead of dropping blocks silently.

Volume is set with `CMD_SET_VOLUME` (0-1000) from the UI.

## 9. Shader injection

`ShaderLab` transpiles Java Edition GLSL to ES 3.0: `#version 300 es`,
`in`/`out` instead of `attribute`/`varying`, `texture()` instead of
`texture2D()`, explicit `precision` and `layout(location = 0) out`. The
transpiled source is validated before use. `FLAG_SHADERS` and `CMD_SET_SHADERS`
switch the post-processor on and off in the companion; `SAFE MODE` disables it
along with the Rust optimisations.

## 10. Memory limits

* Rust and C++ allocate through mimalloc (`RoadAllocator` wraps `MiMalloc`).
* The allocator refuses allocations above `ROAD_HEAP_LIMIT` (default 1.5 GiB)
  and counts failures instead of letting the kernel OOM-kill the process.
* `road_host_stats` reports RSS as `heap_used`, plus the tracked peak.
* Ring capacities and frame slots are fixed at creation; nothing grows at
  runtime.
* The mesher caps a chunk mesh and the UI warns once when the companion passes
  1.2 GiB ("Low memory: reduce render distance").

## 11. Crash handling and safe mode

* The Rust panic hook writes a breadcrumb and aborts (panic = abort).
* The UI detects the lost heartbeat and shows "The 32-bit engine stopped
  responding" instead of a frozen surface.
* `CrashGuard` persists consecutive failures; three in a row block the launch
  until reset.
* Safe mode (`FLAG_SAFE_MODE`, `CMD_SET_SAFE_MODE`) runs the companion with the
  vanilla renderer, no shaders and no Rust optimisation hooks.

## 12. Verification log

Cloud phone: Android 14, 2 GB RAM, SwiftShader (software GPU), 2400x1080.

| Check | Result |
| --- | --- |
| Companion spawn (`:road32`) | pid reported, status `running` |
| Frame stream | 400-1600 frames per run, render 190-310 us avg, engine 60 FPS |
| Input round trip | `in 2/2` after a tap (down + up) |
| Audio bridge | `audio 2/2`, both blocks written to `AudioTrack` |
| Asset stream | 512 KiB staged and CRC-verified, 2733/2733 commands queued |
| Self-test | `0x8000001f` (mesh, math, physics, alloc, ipc) |
| Satellite (cross-app) | frames streamed from the satellite process |
| Stop | "Companion stopped", no crash records in logcat |

### What the numbers mean

The overlay shows `FPS <engine> (shown <presented>)`:

* **engine** is the rate the companion produced frames, computed from the
  `frame_count` counter between two stats polls. It is the game's rate.
* **shown** is the rate frames were actually received and uploaded by the GL
  thread. It is lower than the engine rate when the UI drops frames, and it is
  `0` when nothing arrives.

Developer mode adds `age`, the age of the newest frame when it was uploaded,
measured from the companion's publish timestamp (the same `CLOCK_MONOTONIC` the
UI reads). It covers capture, the shared slot and the texture upload.

On the cloud phone the software GPU (SwiftShader) drew the demo scene at 9-16
presented frames per second while the companion kept producing at 60. Earlier
notes called that draw rate "FPS" without qualification; that was the presenter,
not the engine. The engine rate on that device is 60.

Input latency is **not** measured: an input event carries the UI timestamp, but
nothing records when the companion consumed it, so no input-to-photon number is
reported anywhere. Do not quote one until the consume timestamp is published.
