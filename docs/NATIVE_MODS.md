# Writing native 32-bit mods

A Road Loader mod is a native library or a Rust crate that runs inside the
companion process. It never touches the UI process directly; everything it needs
from the UI arrives through the shared region.

## 1. Where a mod runs

The companion process loads `libroad_core_32.so` (Rust) and
`libroadloader32.so` (JNI and hooks). A mod can be:

* a C++ shared library that links against `libroadloader32.so`, or
* a Rust `cdylib` that links the `road_core_32` C-ABI.

Both run with the same allocator (mimalloc) and the same 1.5 GiB soft limit, so a
mod must not assume it can allocate freely.

## 2. Resolving a game function

```cpp
#include "pl/memory/ModuleMap.h"
#include "pl/memory/Signature.h"

pl::ModuleMap modules;
const pl::Module* game = modules.find("libminecraftpe.so");
if (!game) return false;  // fail closed: never guess an address

pl::Signature tick{"55 48 89 E5 ?? ?? ?? ?? 0F B7", 0};
const uintptr_t address = tick.scan(*game);
if (address == 0) return false;

road_patch_offset("Level::tick", address);
```

`road_patch_offset` stores the address by name so other modules can reuse it and
so it shows up in diagnostics. A pattern must match exactly once; the scanner
reports duplicates instead of picking one.

## 3. Installing a hook

```cpp
#include "pl/hooks/InlineHook.h"

static pl::InlineHook tickHook;
static void (*originalTick)(void* level, float dt) = nullptr;

static void onTick(void* level, float dt) {
    // work before or after the original
    originalTick(level, dt);
}

bool install() {
    const uintptr_t address = road_patch_lookup("Level::tick");
    if (address == 0) return false;
    return tickHook.install(
        reinterpret_cast<void*>(address),
        reinterpret_cast<void*>(&onTick),
        reinterpret_cast<void**>(&originalTick));
}
```

The ARM32 hook saves the prologue, relocates PC-relative instructions into the
trampoline, writes a `B` and flushes the instruction cache. Every hook has a
matching `uninstall()`; call it before the process exits so the game can shut
down cleanly.

## 4. Talking to the UI

Use the region API rather than inventing a channel:

* read input with `road_host_push_input` from the UI side only;
* emit a log line with an `EVENT_LOG` entry (kind 2);
* ask for work with a command (`CMD_*` in `ipc/layout.rs`).

Never allocate inside the frame-slot write path. Write into the slot, publish
`READY`, and let the UI release it.

## 5. Safety rules

* Fail closed: if a signature does not resolve, disable the feature and log it.
* Keep the 32-bit address space in mind: no `Vec` growth in a tick loop, use
  fixed buffers sized from the region.
* Do not call back into the UI process; there is no shared address space.
* Respect `FLAG_SAFE_MODE`: skip optimisation hooks when it is set.
* Unhook on shutdown. A leaked hook keeps the game mapped and the companion
  alive.
