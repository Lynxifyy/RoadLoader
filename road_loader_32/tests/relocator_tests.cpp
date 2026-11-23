#include <cstring>
#include <vector>

#include "TestHarness.h"
#include "pl/hooks/ArmRelocator.h"

using pl::hooks::encodeAbsoluteJump;
using pl::hooks::Isa;
using pl::hooks::relocatePrologue;

namespace {

uint32_t readU32(const std::vector<uint8_t>& bytes, size_t offset) {
  uint32_t value = 0;
  std::memcpy(&value, bytes.data() + offset, sizeof(value));
  return value;
}

}  // namespace

PL_TEST(absolute_jump_encodings) {
  const auto arm = encodeAbsoluteJump(0x12345678u, Isa::Arm);
  PL_CHECK_EQ(arm.size(), size_t{8});
  PL_CHECK_EQ(readU32(arm, 0), 0xE51FF004u);
  PL_CHECK_EQ(readU32(arm, 4), 0x12345678u);

  const auto thumb = encodeAbsoluteJump(0x12345678u, Isa::Thumb);
  PL_CHECK_EQ(thumb.size(), size_t{8});
  PL_CHECK_EQ(readU32(thumb, 0), 0xF8DFF000u);
  // Bit 0 is set to keep the CPU in Thumb state.
  PL_CHECK_EQ(readU32(thumb, 4), 0x12345679u);

  const auto aarch64 = encodeAbsoluteJump(0x1122334455667788ull, Isa::Aarch64);
  PL_CHECK_EQ(aarch64.size(), size_t{16});
  PL_CHECK_EQ(readU32(aarch64, 0), 0x58000050u);
  PL_CHECK_EQ(readU32(aarch64, 4), 0xD61F0200u);
}

PL_TEST(arm_prologue_is_relocated) {
  // push {r4, lr} ; mov r0, r1 ; bx lr
  const uint32_t prologue[] = {0xE92D4010u, 0xE1A00001u, 0xE12FFF1Eu};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(relocation.supported);
  PL_CHECK_EQ(relocation.overwritten, size_t{8});
  // Two instructions plus the 8-byte absolute jump.
  PL_CHECK_EQ(relocation.trampoline.size(), size_t{16});
  PL_CHECK_EQ(readU32(relocation.trampoline, 0), 0xE92D4010u);
  PL_CHECK_EQ(readU32(relocation.trampoline, 4), 0xE1A00001u);
  PL_CHECK_EQ(readU32(relocation.trampoline, 8), 0xE51FF004u);
  const uintptr_t expectedReturn = reinterpret_cast<uintptr_t>(prologue) + 8;
  PL_CHECK_EQ(readU32(relocation.trampoline, 12), static_cast<uint32_t>(expectedReturn));
}

PL_TEST(arm_branch_in_prologue_is_refused) {
  const uint32_t prologue[] = {0xEA000010u, 0xE1A00001u};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(!relocation.supported);
  PL_CHECK(relocation.reason != nullptr);
}

PL_TEST(arm_literal_load_in_prologue_is_refused) {
  const uint32_t prologue[] = {0xE59F0004u, 0xE1A00001u};  // ldr r0, [pc, #4]
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(!relocation.supported);
}

PL_TEST(arm_adr_in_prologue_is_refused) {
  const uint32_t prologue[] = {0xE28F0004u, 0xE1A00001u};  // add r0, pc, #4
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(!relocation.supported);
}

PL_TEST(thumb_prologue_is_relocated) {
  // push {r4, lr} ; mov r0, r1 ; add r0, #1 ; bx lr
  const uint16_t prologue[] = {0xB510u, 0x4608u, 0x3001u, 0x4770u};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb);
  PL_CHECK(relocation.supported);
  PL_CHECK_EQ(relocation.overwritten, size_t{8});
  PL_CHECK_EQ(relocation.trampoline.size(), size_t{16});
  PL_CHECK_EQ(readU32(relocation.trampoline, 8), 0xF8DFF000u);
}

PL_TEST(thumb_branch_in_prologue_is_refused) {
  const uint16_t prologue[] = {0xE010u, 0x4608u, 0x3001u, 0x4770u};  // b +32
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb);
  PL_CHECK(!relocation.supported);
}

PL_TEST(thumb_literal_load_is_refused) {
  const uint16_t prologue[] = {0x4804u, 0x4608u, 0x3001u, 0x4770u};  // ldr r0, [pc, #16]
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb);
  PL_CHECK(!relocation.supported);
}

PL_TEST(thumb_wide_branch_is_refused) {
  // bl (32-bit): f000 f800
  const uint16_t prologue[] = {0xF000u, 0xF800u, 0x4608u, 0x3001u, 0x4770u};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb);
  PL_CHECK(!relocation.supported);
}

PL_TEST(aarch64_prologue_is_relocated) {
  const uint32_t prologue[] = {0xD503201Fu, 0xD503201Fu, 0xD503201Fu, 0xD503201Fu};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 16, Isa::Aarch64);
  PL_CHECK(relocation.supported);
  PL_CHECK_EQ(relocation.overwritten, size_t{16});
  PL_CHECK_EQ(relocation.trampoline.size(), size_t{32});
}

PL_TEST(aarch64_branch_in_prologue_is_refused) {
  const uint32_t prologue[] = {0x14000010u, 0xD503201Fu, 0xD503201Fu, 0xD503201Fu};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 16, Isa::Aarch64);
  PL_CHECK(!relocation.supported);
}

PL_TEST(null_target_is_refused) {
  const auto relocation = relocatePrologue(nullptr, 8, Isa::Arm);
  PL_CHECK(!relocation.supported);
}
