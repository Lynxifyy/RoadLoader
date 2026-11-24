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

enum class Isa { Unknown = 0, Arm, Thumb, Aarch64 };

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
///
/// Thumb is two halfwords, first halfword first: `0xF8DF` then `0xF000`
/// (`ldr.w pc, [pc, #0]`), followed by the literal. Writing the pair as one
/// little-endian word would swap them and decode as a different instruction.
std::vector<uint8_t> encodeAbsoluteJump(uintptr_t destination, Isa isa);

struct JumpPatch {
  /// Bytes to write at the entry point.
  std::vector<uint8_t> bytes;
  /// Leading padding included in `bytes`, counted in the overwritten length.
  size_t leadingNop = 0;
};

/// Encodes the patch written at `entry` to transfer control to `destination`.
///
/// Thumb `ldr.w pc, [pc, #0]` reads its literal from `Align(PC, 4)`, where PC is
/// the address of the instruction plus four. An entry at `2 mod 4` therefore
/// gets a 16-bit `nop` first so the load and its literal stay 4-byte aligned;
/// the same entry at `0 mod 4` needs no padding.
JumpPatch encodeJumpPatch(uintptr_t entry, uintptr_t destination, Isa isa);

}  // namespace pl::hooks
