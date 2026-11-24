// Reversible inline hooks.
//
// The patch is an absolute jump written over the prologue; the relocated
// prologue is executed from an executable trampoline. Installation is atomic
// from the caller's point of view: the page is made writable, the patch is
// written, the instruction cache is flushed, and the page protection is
// restored. Anything the relocator cannot handle is refused.
#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>

#include "../Result.h"
#include "ArmRelocator.h"

namespace pl::hooks {

class InlineHook {
 public:
  InlineHook() = default;
  ~InlineHook();

  InlineHook(const InlineHook&) = delete;
  InlineHook& operator=(const InlineHook&) = delete;
  InlineHook(InlineHook&& other) noexcept;
  InlineHook& operator=(InlineHook&& other) noexcept;

  /// Installs a detour from `target` to `replacement`. On success, `original`
  /// receives a callable pointer to the trampoline.
  ///
  /// `isa` must be stated by the caller. It is never inferred from the low bit
  /// of `target`: addresses coming from a signature scan carry no ISA
  /// information, and guessing turns a Thumb function into a corrupt ARM hook.
  /// `Isa::Unknown` is refused.
  Status install(void* target, void* replacement, void** original, Isa isa);

  /// Restores the original bytes. Safe to call more than once.
  Status uninstall();

  bool installed() const { return installed_; }
  void* target() const { return target_; }
  void* trampoline() const { return trampoline_; }

 private:
  void* target_ = nullptr;
  void* replacement_ = nullptr;
  void* trampoline_ = nullptr;
  size_t trampolineSize_ = 0;
  std::vector<uint8_t> saved_;
  bool installed_ = false;
};

}  // namespace pl::hooks
