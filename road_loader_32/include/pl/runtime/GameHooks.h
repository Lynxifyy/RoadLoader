// Game hook installation and hook bodies.
//
// `GameTargets` carries the addresses resolved by the signature scanner for one
// specific `libminecraftpe.so`. `GameHooks` installs every available detour and
// keeps the trampolines alive until `uninstallAll()`.
#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <string>
#include <vector>

#include "../Result.h"
#include "../hooks/InlineHook.h"
#include "../hooks/PrologueVerify.h"
#include "../ipc/RoadCoreAbi.h"

namespace pl::runtime {

/// One resolved address plus everything needed to hook it safely.
///
/// `isa` is mandatory: the scanner returns numbers, and a number cannot say
/// whether the code is ARM or Thumb. `expectedPrologue` is the first bytes the
/// pattern matched; they are compared with the live ones before patching, so a
/// stale offset is refused instead of corrupting another function.
struct GameTarget {
  uintptr_t address = 0;
  hooks::Isa isa = hooks::Isa::Unknown;
  const uint8_t* expectedPrologue = nullptr;
  const uint8_t* prologueMask = nullptr;
  size_t expectedSize = 0;

  bool present() const { return address != 0; }
};

struct GameTargets {
  GameTarget levelTick;
  GameTarget actorRender;
  GameTarget screenViewSetup;
  GameTarget swapBuffers;
  GameTarget processMouseEvent;
  GameTarget chunkMesher;
  GameTarget actorMove;
  GameTarget vec3Distance;
  GameTarget soundEnginePlay;

  size_t count() const;
};

struct GameOptions {
  bool safeMode = false;
  /// Install hooks whose prologue was not verified. Off by default: an
  /// unverified address is a guess, and a wrong guess corrupts the game.
  bool allowUnverified = false;
  bool enableMesh = true;
  bool enablePhysics = true;
  bool enableAudio = true;
  bool enableShaders = true;
  uint32_t width = 960;
  uint32_t height = 540;
};

struct HookStats {
  uint64_t framesCaptured = 0;
  uint64_t meshCalls = 0;
  uint64_t physicsCalls = 0;
  uint64_t audioBlocks = 0;
  uint64_t inputEvents = 0;
  uint64_t meshFailures = 0;
};

/// What `install()` would do with one target, before any memory is touched.
enum class TargetVerdict {
  Install,
  NoAddress,
  NoIsa,
  Unverified,
};

TargetVerdict evaluateTarget(const GameTarget& target, const GameOptions& options);

class GameHooks {
 public:
  static GameHooks& instance();

  /// Installs every hook whose target is non-zero. Individual failures are
  /// reported but do not abort the remaining hooks.
  Status install(const GameTargets& targets, const GameOptions& options);

  /// Restores every installed prologue and releases the trampolines.
  void uninstallAll();

  size_t activeCount() const;
  bool isInstalled(const char* name) const;
  HookStats stats() const;
  void setVolume(float volume);

  /// Hook bodies, public so they can be exercised by host tests without
  /// installing a real detour.
  static float distanceHook(float x1, float y1, float z1, float x2, float y2, float z2);
  static int meshHook(const uint32_t* dims, const uint8_t* blocks, const uint8_t* opaque,
                      const float* origin, float scale, RoadMeshVertex* out, uint32_t capacity);

 private:
  GameHooks() = default;
  ~GameHooks();

  struct Entry {
    std::string name;
    hooks::InlineHook hook;
    void* original = nullptr;
  };

  Status installOne(const char* name, const GameTarget& target, void* replacement);
  void* original(const char* name) const;

  mutable std::mutex mutex_;
  std::vector<Entry> entries_;
  GameOptions options_;
  mutable HookStats stats_;
};

}  // namespace pl::runtime
