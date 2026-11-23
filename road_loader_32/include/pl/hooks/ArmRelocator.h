// Prologue relocation for inline hooks on ARM (A32) and Thumb-2 (T32).
//
// Only instructions that do not depend on their original PC are copied
// verbatim. PC-relative branches and literal loads are rejected so a hook either
// works correctly or is not installed at all (fail-closed).
#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>

namespace pl::hooks {

enum class Isa { Arm, Thumb, Aarch64 };

struct Relocation {
  /// True when the whole prologue could be relocated.
  bool supported = false;
  /// Bytes that must be overwritten at the target (always >= `minBytes`).
  size_t overwritten = 0;
  /// Relocated instructions followed by an absolute jump back to
  /// `target + overwritten`.
  std::vector<uint8_t> trampoline;
  /// Why relocation was rejected, for logs.
  const char* reason = "";
};

/// Relocates at least `minBytes` of the prologue at `target` into a trampoline.
///
/// `thumb` must be true when `target` is a Thumb function. `minBytes` is the
/// size of the patch that will replace the prologue (8 bytes for the ARM/Thumb
/// absolute jump, 16 for the AArch64 one).
Relocation relocatePrologue(const uint8_t* target, size_t minBytes, Isa isa);

/// Encodes an absolute jump that never clobbers a general-purpose register.
/// Returns 8 bytes for Arm/Thumb and 16 bytes for AArch64.
std::vector<uint8_t> encodeAbsoluteJump(uintptr_t destination, Isa isa);

}  // namespace pl::hooks
