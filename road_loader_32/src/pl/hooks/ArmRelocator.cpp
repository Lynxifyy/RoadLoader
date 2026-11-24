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

void appendU16(std::vector<uint8_t>& out, uint16_t value) {
  const size_t offset = out.size();
  out.resize(offset + sizeof(value));
  std::memcpy(out.data() + offset, &value, sizeof(value));
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

// Instructions whose meaning depends on their own address, or that carry a
// control-flow side effect a verbatim copy would change. Anything listed here
// is refused rather than relocated.
const char* armUnrelocatableReason(uint32_t instruction) {
  // B / BL: bits 27..25 == 0b101.
  if ((instruction & 0x0E000000u) == 0x0A000000u) {
    return "pc-relative branch in prologue";
  }
  // LDR/STR with PC as the base register.
  if ((instruction & 0x0C000000u) == 0x04000000u && ((instruction >> 16) & 0xFu) == 0xFu) {
    return "pc-relative literal load in prologue";
  }
  // ADR (add/sub rd, pc, #imm).
  if ((instruction & 0x0FBF0000u) == 0x028F0000u || (instruction & 0x0FBF0000u) == 0x024F0000u) {
    return "ADR in prologue";
  }
  // The 000 group (data processing, multiply, extra load/store, PSR) reads PC
  // whenever it names r15 as a source operand: `mov r0, pc` and `add r0, pc, #4`
  // are both silently wrong once copied to another address.
  if ((instruction & 0x0C000000u) != 0u) {
    return nullptr;
  }
  const uint32_t opcode = (instruction >> 21) & 0xFu;
  const uint32_t rn = (instruction >> 16) & 0xFu;
  const uint32_t rs = (instruction >> 8) & 0xFu;
  const uint32_t rm = instruction & 0xFu;
  // MRS, MSR, BX, BLX, CLZ and BKPT share the 000 group; bits 27..23 == 00010
  // identifies them. Their 1111 patterns are status registers or register
  // numbers, never PC operands.
  if (((instruction >> 23) & 0x1Fu) == 0x02u) {
    return nullptr;
  }
  if (opcode == 0x0u) {
    // Multiply reads the operand fields, not Rn (which must be zero);
    // extra load/store uses Rn as its base.
    const bool multiply = (instruction & 0x0FC000F0u) == 0x00000090u;
    if (multiply && (rs == 15u || rm == 15u)) {
      return "multiply reads pc in prologue";
    }
    if (!multiply && rn == 15u) {
      return "extra load/store relative to pc in prologue";
    }
    return nullptr;
  }
  if (rn == 15u || rm == 15u) {
    return "data-processing instruction reads pc in prologue";
  }
  return nullptr;
}

// Thumb special data processing with r15 as a source register:
// `add rX, pc`, `cmp rX, pc`, `mov rX, pc`, all of which fold PC into a value.
bool thumbReadsPcInHighRegisterOp(uint16_t instruction) {
  return (instruction & 0xFCF8u) == 0x4478u;
}

// IT is 0xBFxx with a non-zero condition mask; 0xBF00 is `nop`.
bool thumbIsItBlock(uint16_t instruction) {
  return (instruction & 0xFF00u) == 0xBF00u && (instruction & 0x000Fu) != 0u;
}

// TBB/TBH jump through a byte or halfword table relative to their own PC.
bool thumbIsTableBranch(uint16_t first, uint16_t second) {
  // tbb [Rn, Rm] is 0xE8D0|Rn followed by 0xF000|(H<<4)|Rm.
  return (first & 0xFFF0u) == 0xE8D0u && (second & 0xFFE0u) == 0xF000u;
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
      // ldr.w pc, [pc, #0] ; .word destination (bit 0 keeps Thumb state).
      // A Thumb-2 instruction is two halfwords stored first halfword first.
      appendU16(jump, 0xF8DFu);
      appendU16(jump, 0xF000u);
      appendU32(jump, static_cast<uint32_t>(destination | 1u));
      break;
    case Isa::Unknown:
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

JumpPatch encodeJumpPatch(uintptr_t entry, uintptr_t destination, Isa isa) {
  JumpPatch patch;
  if (isa == Isa::Thumb && (entry & 2u) != 0u) {
    // `nop` (0xBF00) is not an IT block: the IT mask is zero.
    appendU16(patch.bytes, 0xBF00u);
    patch.leadingNop = 2;
  }
  const auto jump = encodeAbsoluteJump(destination, isa);
  patch.bytes.insert(patch.bytes.end(), jump.begin(), jump.end());
  return patch;
}

Relocation relocatePrologue(const uint8_t* target, size_t minBytes, Isa isa) {
  Relocation result;
  if (isa == Isa::Unknown) {
    result.reason = "no explicit ISA";
    return result;
  }
  if (target == nullptr) {
    result.reason = "null target";
    return result;
  }
  const uintptr_t base = reinterpret_cast<uintptr_t>(target);

  if (isa == Isa::Arm) {
    size_t copied = 0;
    while (copied < minBytes) {
      const uint32_t instruction = readU32(target + copied);
      if (const char* reason = armUnrelocatableReason(instruction); reason != nullptr) {
        result.reason = reason;
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
        if (thumbReadsPcInHighRegisterOp(first)) {
          result.reason = "16-bit high-register operation reads pc in prologue";
          return result;
        }
        if (thumbIsItBlock(first)) {
          result.reason = "IT block in prologue";
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
      if (thumbIsTableBranch(first, second)) {
        result.reason = "table branch in prologue";
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
