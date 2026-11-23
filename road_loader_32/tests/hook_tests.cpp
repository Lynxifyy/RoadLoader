#include <cstring>

#include "TestHarness.h"
#include "pl/hooks/InlineHook.h"
#include "pl/runtime/GameHooks.h"

using pl::Status;
using pl::hooks::InlineHook;
using pl::runtime::GameHooks;
using pl::runtime::GameOptions;
using pl::runtime::GameTargets;

PL_TEST(inline_hook_rejects_null_arguments) {
  InlineHook hook;
  void* original = nullptr;
  PL_CHECK(hook.install(nullptr, reinterpret_cast<void*>(0x1000), &original) ==
           Status::InvalidArgument);
  PL_CHECK(hook.install(reinterpret_cast<void*>(0x1000), nullptr, &original) ==
           Status::InvalidArgument);
  PL_CHECK(hook.install(reinterpret_cast<void*>(0x1000), reinterpret_cast<void*>(0x2000), nullptr) ==
           Status::InvalidArgument);
}

PL_TEST(inline_hook_uninstall_before_install_is_ok) {
  InlineHook hook;
  PL_CHECK(hook.uninstall() == Status::Ok);
  PL_CHECK(!hook.installed());
}

PL_TEST(inline_hook_is_unsupported_off_arm) {
#if !defined(__arm__) && !defined(__aarch64__)
  InlineHook hook;
  void* original = nullptr;
  // Never executed: the backend refuses before touching the address.
  const Status status =
      hook.install(reinterpret_cast<void*>(0x1000), reinterpret_cast<void*>(0x2000), &original);
  PL_CHECK(status == Status::Unsupported);
  PL_CHECK(original == nullptr);
  PL_CHECK(!hook.installed());
#endif
}

PL_TEST(game_hooks_require_targets) {
  auto& hooks = GameHooks::instance();
  hooks.uninstallAll();
  const GameTargets targets{};
  PL_CHECK(hooks.install(targets, GameOptions{}) == Status::NotFound);
  PL_CHECK_EQ(hooks.activeCount(), size_t{0});
}

PL_TEST(game_hooks_safe_mode_installs_nothing) {
  auto& hooks = GameHooks::instance();
  hooks.uninstallAll();
  GameOptions options;
  options.safeMode = true;
  PL_CHECK(hooks.install(GameTargets{}, options) == Status::Ok);
  PL_CHECK_EQ(hooks.activeCount(), size_t{0});
  hooks.uninstallAll();
}

PL_TEST(distance_hook_matches_rust_math) {
  PL_CHECK_EQ(GameHooks::distanceHook(0.0f, 0.0f, 0.0f, 3.0f, 4.0f, 0.0f), 5.0f);
  PL_CHECK_EQ(GameHooks::distanceHook(1.0f, 1.0f, 1.0f, 1.0f, 1.0f, 1.0f), 0.0f);
}

PL_TEST(mesh_hook_builds_a_cube) {
  uint32_t dims[3] = {16, 16, 16};
  std::vector<uint8_t> blocks(16 * 16 * 16, 0);
  blocks[8 + 16 * (8 + 16 * 8)] = 1;
  uint8_t opaque[256] = {0};
  for (size_t index = 1; index < 256; ++index) {
    opaque[index] = 1;
  }
  std::vector<RoadMeshVertex> vertices(1024);
  const int count = GameHooks::meshHook(dims, blocks.data(), opaque, nullptr, 1.0f, vertices.data(),
                                        static_cast<uint32_t>(vertices.size()));
  PL_CHECK_EQ(count, 36);
  PL_CHECK(GameHooks::instance().stats().meshCalls >= 1);
}

PL_TEST(mesh_hook_rejects_null_buffers) {
  const auto before = GameHooks::instance().stats().meshFailures;
  const int count = GameHooks::meshHook(nullptr, nullptr, nullptr, nullptr, 1.0f, nullptr, 0);
  PL_CHECK_EQ(count, -1);
  PL_CHECK(GameHooks::instance().stats().meshFailures > before);
}

PL_TEST(hook_stats_are_monotonic) {
  const auto first = GameHooks::instance().stats();
  uint32_t dims[3] = {16, 16, 16};
  std::vector<uint8_t> blocks(16 * 16 * 16, 0);
  uint8_t opaque[256] = {0};
  for (size_t index = 1; index < 256; ++index) {
    opaque[index] = 1;
  }
  std::vector<RoadMeshVertex> vertices(64);
  GameHooks::meshHook(dims, blocks.data(), opaque, nullptr, 1.0f, vertices.data(),
                      static_cast<uint32_t>(vertices.size()));
  const auto second = GameHooks::instance().stats();
  PL_CHECK(second.meshCalls >= first.meshCalls + 1);
}
