#include <cstring>

#include "TestHarness.h"
#include "pl/hooks/InlineHook.h"
#include "pl/hooks/PrologueVerify.h"
#include "pl/runtime/GameHooks.h"

using pl::Status;
using pl::hooks::InlineHook;
using pl::hooks::Isa;
using pl::hooks::verifyPrologue;
using pl::runtime::GameHooks;
using pl::runtime::GameOptions;
using pl::runtime::GameTarget;
using pl::runtime::GameTargets;

PL_TEST(inline_hook_rejects_null_arguments) {
  InlineHook hook;
  void* original = nullptr;
  PL_CHECK(hook.install(nullptr, reinterpret_cast<void*>(0x1000), &original, Isa::Arm) ==
           Status::InvalidArgument);
  PL_CHECK(hook.install(reinterpret_cast<void*>(0x1000), nullptr, &original, Isa::Arm) ==
           Status::InvalidArgument);
  PL_CHECK(hook.install(reinterpret_cast<void*>(0x1000), reinterpret_cast<void*>(0x2000), nullptr,
                        Isa::Arm) == Status::InvalidArgument);
}

PL_TEST(inline_hook_rejects_a_missing_isa) {
  InlineHook hook;
  void* original = nullptr;
  // The ISA is never guessed from bit 0 of the address.
  PL_CHECK(hook.install(reinterpret_cast<void*>(0x1001), reinterpret_cast<void*>(0x2000), &original,
                        Isa::Unknown) == Status::InvalidArgument);
  PL_CHECK(original == nullptr);
  PL_CHECK(!hook.installed());
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
  const Status status = hook.install(reinterpret_cast<void*>(0x1000),
                                     reinterpret_cast<void*>(0x2000), &original, Isa::Arm);
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

// push {r4, lr} ; mov r0, r1
constexpr uint8_t kArmPrologue[8] = {0x10, 0x40, 0x2D, 0xE9, 0x01, 0x00, 0xA0, 0xE1};

PL_TEST(prologue_verification_matches_the_expected_bytes) {
  const auto check = verifyPrologue(kArmPrologue, kArmPrologue, nullptr, 8);
  PL_CHECK(check.ok);
  PL_CHECK_EQ(check.compared, size_t{8});
}

PL_TEST(prologue_verification_reports_the_first_difference) {
  uint8_t wrong[8];
  std::memcpy(wrong, kArmPrologue, sizeof(wrong));
  wrong[5] = 0xFF;
  const auto check = verifyPrologue(kArmPrologue, wrong, nullptr, 8);
  PL_CHECK(!check.ok);
  PL_CHECK_EQ(check.mismatchOffset, size_t{5});
  PL_CHECK(check.reason != nullptr);
}

PL_TEST(prologue_verification_honours_wildcards) {
  const uint8_t mask[8] = {0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF};
  uint8_t actual[8];
  std::memcpy(actual, kArmPrologue, sizeof(actual));
  actual[2] = 0x00;
  actual[3] = 0x00;
  PL_CHECK(verifyPrologue(actual, kArmPrologue, mask, 8).ok);
  actual[0] = 0x00;
  PL_CHECK(!verifyPrologue(actual, kArmPrologue, mask, 8).ok);
}

PL_TEST(prologue_verification_refuses_an_empty_expectation) {
  PL_CHECK(!verifyPrologue(kArmPrologue, nullptr, nullptr, 0).ok);
  PL_CHECK(!verifyPrologue(nullptr, kArmPrologue, nullptr, 8).ok);
}

PL_TEST(evaluate_target_needs_an_address) {
  GameTarget target;
  target.isa = Isa::Thumb;
  target.expectedPrologue = kArmPrologue;
  target.expectedSize = 8;
  PL_CHECK(pl::runtime::evaluateTarget(target, GameOptions{}) ==
           pl::runtime::TargetVerdict::NoAddress);
}

PL_TEST(evaluate_target_needs_an_isa) {
  GameTarget target;
  target.address = 0x1000;
  target.expectedPrologue = kArmPrologue;
  target.expectedSize = 8;
  PL_CHECK(pl::runtime::evaluateTarget(target, GameOptions{}) ==
           pl::runtime::TargetVerdict::NoIsa);
}

PL_TEST(evaluate_target_refuses_an_unverified_prologue) {
  GameTarget target;
  target.address = 0x1000;
  target.isa = Isa::Thumb;
  PL_CHECK(pl::runtime::evaluateTarget(target, GameOptions{}) ==
           pl::runtime::TargetVerdict::Unverified);
  GameOptions permissive;
  permissive.allowUnverified = true;
  PL_CHECK(pl::runtime::evaluateTarget(target, permissive) ==
           pl::runtime::TargetVerdict::Install);
}

PL_TEST(evaluate_target_installs_a_verified_target) {
  GameTarget target;
  target.address = 0x1000;
  target.isa = Isa::Arm;
  target.expectedPrologue = kArmPrologue;
  target.expectedSize = 8;
  PL_CHECK(pl::runtime::evaluateTarget(target, GameOptions{}) ==
           pl::runtime::TargetVerdict::Install);
}

PL_TEST(game_hooks_refuse_a_target_without_an_isa) {
  auto& hooks = GameHooks::instance();
  hooks.uninstallAll();
  GameTargets targets;
  targets.swapBuffers.address = reinterpret_cast<uintptr_t>(kArmPrologue);
  targets.swapBuffers.expectedPrologue = kArmPrologue;
  targets.swapBuffers.expectedSize = 8;
  PL_CHECK(hooks.install(targets, GameOptions{}) == Status::NotFound);
  PL_CHECK_EQ(hooks.activeCount(), size_t{0});
}

PL_TEST(game_hooks_refuse_an_unverified_target) {
  auto& hooks = GameHooks::instance();
  hooks.uninstallAll();
  GameTargets targets;
  targets.swapBuffers.address = reinterpret_cast<uintptr_t>(kArmPrologue);
  targets.swapBuffers.isa = Isa::Arm;
  PL_CHECK(hooks.install(targets, GameOptions{}) == Status::NotFound);
  PL_CHECK_EQ(hooks.activeCount(), size_t{0});
}

PL_TEST(game_hooks_refuse_a_prologue_mismatch) {
  auto& hooks = GameHooks::instance();
  hooks.uninstallAll();
  uint8_t expected[8];
  std::memcpy(expected, kArmPrologue, sizeof(expected));
  expected[2] = 0x00;
  GameTargets targets;
  targets.swapBuffers.address = reinterpret_cast<uintptr_t>(kArmPrologue);
  targets.swapBuffers.isa = Isa::Arm;
  targets.swapBuffers.expectedPrologue = expected;
  targets.swapBuffers.expectedSize = 8;
  PL_CHECK(hooks.install(targets, GameOptions{}) == Status::NotFound);
  PL_CHECK_EQ(hooks.activeCount(), size_t{0});
}

PL_TEST(game_hooks_count_targets_with_an_address) {
  GameTargets targets;
  PL_CHECK_EQ(targets.count(), size_t{0});
  targets.levelTick.address = 0x1000;
  targets.actorMove.address = 0x2000;
  PL_CHECK_EQ(targets.count(), size_t{2});
}
