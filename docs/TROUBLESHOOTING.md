# Troubleshooting

## The UI says "Native library failed to load"

`dlopen` failed for `libroadloader32.so`. The message names the missing library.
Common causes:

* `libroad_core_32.so` was not staged. Run `scripts/build_rust.sh release`
  before Gradle. The Rust build sets `-Wl,-soname,libroad_core_32.so`; without it
  the bridge records an absolute build path in `DT_NEEDED` and the APK cannot
  resolve it on the device.
* The APK was built for one ABI only. Check
  `unzip -l app-debug.apk | grep lib/` and confirm both `arm64-v8a` and
  `armeabi-v7a` are present.

Verify a built library:

```bash
llvm-readelf -d libroadloader32.so | grep NEEDED
# Shared library: [libroad_core_32.so]  <- must be the bare file name
```

## "Not allowed to write file descriptors here"

Android 14 refuses `ParcelFileDescriptor` extras in intents. Road Loader sends
the descriptor over a Binder `Messenger` instead. If you see this message, a
custom build reintroduced the intent extra; switch back to
`CompanionServiceBase.MSG_ATTACH`.

## "Companion refused the region (code ...)"

The companion received the descriptor but `road_companion_attach` rejected it:

* `-1` - null descriptor, zero size, bad magic or a mapping smaller than the
  layout needs. The UI and companion probably disagree about the frame size.
* `-2` - ABI mismatch; the UI and companion libraries come from different builds.
* `-3` - a companion is already attached in this process.

Rebuild both APK variants from the same commit.

## Frames stop but the process is alive

Check `hb` in the overlay. A growing heartbeat age means the companion's loop is
blocked. Usual causes: an asset chunk larger than the ring, or a hook that
deadlocks. Stop the companion, then relaunch in safe mode to confirm the hook is
the cause.

## "The 32-bit engine stopped responding"

The heartbeat did not advance for the configured timeout. The UI keeps running.
Press *Stop*, then relaunch; after three consecutive failures the crash guard
blocks launching until *Reset guard*.

## Audio is silent

The overlay reports the reason next to `audio pushed/played`. If the device has
no float PCM sink the bridge drains blocks and reports
`Audio sink unavailable`. Volume starts at 50; use the volume command to change
it.

## Input does nothing

`in 0/0` means the UI never queued an event: check that the touch landed inside
the surface. `in 2/0` means the companion has not consumed them; confirm the
companion is `running` and that input-sink mode is enabled (the demo enables it
on the first frame).

## Satellite not installed

The client looks for `com.caramel.roadloader.satellite`. Install
`satellite/build/outputs/apk/debug/satellite-debug.apk` on the same device. The
client declares the package in `<queries>`; removing that entry hides the
satellite again on Android 11+.

## Reading logs

```bash
adb logcat -s RoadLoader32 RoadAudio AndroidRuntime
```

The companion's `pid` and heartbeat are in the header, so `dumpsys meminfo
<package>:road32` shows the real 32-bit footprint on a 32-bit device.

## The companion says it is 64-bit

That is correct on a 64-bit device when you launch the in-APK companion. An APK
installs at one ABI and every process in it runs at that ABI, so `:road32` is a
second process, not a second pointer size.

Install `satellite/build/outputs/apk/release/satellite-release.apk` (armeabi-v7a
only) and press *Satellite* instead. If it refuses to install, the device cannot
execute 32-bit code at all (64-bit-only kernel), and no 32-bit engine is possible
on it.

The status line after attaching shows the ABI the service reported, for example
`64-bit (arm64-v8a 64-bit ptr=8 page=4096 abi=1)`.

## "Companion refused the region (larger than 128 MB)"

The client asked the companion to map a region larger than `MAX_REGION_BYTES`
(128 MiB, which covers a 3840x2160 triple buffer). Lower the render size; a
4K region is about 95 MiB and leaves little room in a 32-bit address space.

## "Companion already has a client attached"

Another app holds the single session. Close it, or stop the satellite with
`adb shell am force-stop com.caramel.roadloader.satellite`, then launch again.
