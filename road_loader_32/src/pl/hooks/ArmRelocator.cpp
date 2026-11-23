#include "pl/hooks/ArmRelocator.h"

#include <cstring>

namespace pl::hooks {
namespace {

uint32_t readU32(const uint8_t* pointer) {
  uint32_t value = 0;
  std::memcpy(&value, pointer, sizeof(value));
  return value;
}

uint16_t readU16(const uint8_t* pointer) {
  uint16_t value = 0;
  std::memcpy(&value, pointer, sizeof(value));
  return value;
}

void appendU32(std::vector<uint8_t>& out, uint32_t value) {
  const size_t offset = out.size();
  out.resize(offset + sizeof(value));
  std::memcpy(out.data() + offset, &value, sizeof(value));
}

void appendU64(std::vector<uint8_t>& out, uint64_t value) {
  const size_t offset = out.size();
  out.resize(offset + sizeof(value));
  std::memcpy(out.data() + offset, &value, sizeof(value));
}

void appendBytes(std::vector<uint8_t>& out, const uint8_t* bytes, size_t size) {
  out.insert(out.end(), bytes, bytes + size);
}

}  // namespace

std::vector<uint8_t> encodeAbsoluteJump(uintptr_t destination, Isa isa) {
  std::vector<uint8_t> jump;
  switch (isa) {
    case Isa::Arm:
      // ldr pc, [pc, #-4] ; .word destination
      appendU32(jump, 0xE51FF004u);
      appendU32(jump, static_cast<uint32_t>(destination));
      break;
    case Isa::Thumb:
      // ldr.w pc, [pc, #0] ; .word destination (bit 0 keeps Thumb state)
      appendU32(jump, 0xF8DFF000u);
      appendU32(jump, static_cast<uint32_t>(destination | 1u));
      break;
    case Isa::Aarch64:
      // ldr x16, #8 ; br x16 ; .quad destination
      appendU32(jump, 0x58000050u);
      appendU32(jump, 0xD61F0200u);
      appendU64(jump, static_cast<uint64_t>(destination));
      break;
  }
  return jump;
}

Relocation relocatePrologue(const uint8_t* target, size_t minBytes, Isa isa) {
  Relocation result;
  if (target == nullptr) {
    result.reason = "null target";
    return result;
  }
  const uintptr_t base = reinterpret_cast<uintptr_t>(target);

  if (isa == Isa::Arm) {
    size_t copied = 0;
    while (copied < minBytes) {
      const uint32_t instruction = readU32(target + copied);
      // B / BL: bits 27..25 == 0b101.
      if ((instruction & 0x0E000000u) == 0x0A000000u) {
        result.reason = "pc-relative branch in prologue";
        return result;
      }
      // LDR/STR with PC as base register.
      if ((instruction & 0x0C000000u) == 0x04000000u &&
          ((instruction >> 16) & 0xFu) == 0xFu) {
        result.reason = "pc-relative literal load in prologue";
        return result;
      }
      // ADR (add/sub rd, pc, #imm).
      if ((instruction & 0x0FBF0000u) == 0x028F0000u ||
          (instruction & 0x0FBF0000u) == 0x024F0000u) {
        result.reason = "ADR in prologue";
        return result;
      }
      appendBytes(result.trampoline, target + copied, 4);
      copied += 4;
    }
    result.supported = true;
    result.overwritten = copied;
    const auto jump = encodeAbsoluteJump(base + copied, Isa::Arm);
    result.trampoline.insert(result.trampoline.end(), jump.begin(), jump.end());
    return result;
  }

  if (isa == Isa::Thumb) {
    size_t copied = 0;
    while (copied < minBytes) {
      const uint16_t first = readU16(target + copied);
      const bool wide = (first >> 11) >= 0x1Du;
      if (!wide) {
        // 16-bit instructions.
        const bool unconditionalBranch = (first & 0xF800u) == 0xE000u;
        const bool conditionalBranch = (first & 0xF000u) == 0xD000u;
        const bool literalLoad = (first & 0xF800u) == 0x4800u;
        const bool adr = (first & 0xF800u) == 0xA000u;
        const bool compareBranch = (first & 0xF500u) == 0xB100u;
        if (unconditionalBranch || conditionalBranch || literalLoad || adr || compareBranch) {
          result.reason = "pc-relative 16-bit instruction in prologue";
          return result;
        }
        appendBytes(result.trampoline, target + copied, 2);
        copied += 2;
        continue;
      }
      const uint16_t second = readU16(target + copied + 2);
      const bool bl = (first & 0xF800u) == 0xF000u && (second & 0x8000u) != 0;
      const bool bWide = (first & 0xF800u) == 0xF800u && (second & 0x8000u) == 0;
      const bool literalLoadWide = (first & 0xFF7Fu) == 0xF85Fu;
      const bool adrWide = (first & 0xFBFFu) == 0xF20Fu || (first & 0xFBFFu) == 0xF2AFu;
      if (bl || bWide || literalLoadWide || adrWide) {
        result.reason = "pc-relative 32-bit thumb instruction in prologue";
        return result;
      }
      appendBytes(result.trampoline, target + copied, 4);
      copied += 4;
    }
    result.supported = true;
    result.overwritten = copied;
    // Keep the literal pool 4-byte aligned.
    if ((copied & 2u) != 0) {
      appendBytes(result.trampoline, reinterpret_cast<const uint8_t*>("\x00\xBF"), 2);
    }
    const auto jump = encodeAbsoluteJump(base + copied, Isa::Thumb);
    result.trampoline.insert(result.trampoline.end(), jump.begin(), jump.end());
    return result;
  }

  // AArch64: four fixed-width instructions, no PC-relative forms.
  size_t copied = 0;
  while (copied < minBytes) {
    const uint32_t instruction = readU32(target + copied);
    const bool branch = (instruction & 0x7C000000u) == 0x14000000u;
    const bool adr = (instruction & 0x1F000000u) == 0x10000000u;
    const bool literal = (instruction & 0x3B000000u) == 0x18000000u;
    if (branch || adr || literal) {
      result.reason = "pc-relative instruction in aarch64 prologue";
      return result;
    }
    appendBytes(result.trampoline, target + copied, 4);
    copied += 4;
  }
  result.supported = true;
  result.overwritten = copied;
  const auto jump = encodeAbsoluteJump(base + copied, Isa::Aarch64);
  result.trampoline.insert(result.trampoline.end(), jump.begin(), jump.end());
  return result;
}

}  // namespace pl::hooks
