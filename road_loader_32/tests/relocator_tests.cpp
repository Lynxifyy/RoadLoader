#include <cstring>
#include <vector>

#include "TestHarness.h"
#include "pl/hooks/ArmRelocator.h"

using pl::hooks::encodeAbsoluteJump;
using pl::hooks::encodeJumpPatch;
using pl::hooks::Isa;
using pl::hooks::relocatePrologue;

namespace {

uint32_t readU32(const std::vector<uint8_t>& bytes, size_t offset) {
  uint32_t value = 0;
  std::memcpy(&value, bytes.data() + offset, sizeof(value));
  return value;
}

uint16_t readU16(const std::vector<uint8_t>& bytes, size_t offset) {
  uint16_t value = 0;
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
  // Thumb-2 stores the first halfword first: 0xF8DF is `ldr.w pc, [pc, #0]`
  // and 0xF000 is Rt=pc with imm12=0.
  PL_CHECK_EQ(readU16(thumb, 0), 0xF8DFu);
  PL_CHECK_EQ(readU16(thumb, 2), 0xF000u);
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
  PL_CHECK_EQ(readU16(relocation.trampoline, 8), 0xF8DFu);
  PL_CHECK_EQ(readU16(relocation.trampoline, 10), 0xF000u);
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

PL_TEST(thumb_jump_bytes_are_halfword_ordered) {
  const auto thumb = encodeAbsoluteJump(0x12345678u, Isa::Thumb);
  const uint8_t expected[8] = {0xDF, 0xF8, 0x00, 0xF0, 0x79, 0x56, 0x34, 0x12};
  PL_CHECK_EQ(thumb.size(), size_t{8});
  for (size_t index = 0; index < 8; ++index) {
    PL_CHECK_EQ(static_cast<int>(thumb[index]), static_cast<int>(expected[index]));
  }
}

PL_TEST(thumb_jump_patch_pads_a_2_mod_4_entry) {
  const auto aligned = encodeJumpPatch(0x1000u, 0x2000u, Isa::Thumb);
  PL_CHECK_EQ(aligned.bytes.size(), size_t{8});
  PL_CHECK_EQ(aligned.leadingNop, size_t{0});
  PL_CHECK_EQ(readU16(aligned.bytes, 0), 0xF8DFu);
  PL_CHECK_EQ((0x1000u + aligned.leadingNop + 4u) % 4u, 0u);

  // ldr.w pc, [pc, #0] reads Align(PC, 4); at 0x1002 the literal would land on
  // the instruction's own second halfword without the leading nop.
  const auto offset = encodeJumpPatch(0x1002u, 0x2000u, Isa::Thumb);
  PL_CHECK_EQ(offset.bytes.size(), size_t{10});
  PL_CHECK_EQ(offset.leadingNop, size_t{2});
  PL_CHECK_EQ(readU16(offset.bytes, 0), 0xBF00u);
  PL_CHECK_EQ(readU16(offset.bytes, 2), 0xF8DFu);
  PL_CHECK_EQ(readU16(offset.bytes, 4), 0xF000u);
  PL_CHECK_EQ(readU32(offset.bytes, 6), 0x2001u);
  PL_CHECK_EQ((0x1002u + offset.leadingNop + 4u) % 4u, 0u);
}

PL_TEST(arm_jump_patch_needs_no_padding) {
  const auto patch = encodeJumpPatch(0x1004u, 0x2000u, Isa::Arm);
  PL_CHECK_EQ(patch.bytes.size(), size_t{8});
  PL_CHECK_EQ(patch.leadingNop, size_t{0});
  PL_CHECK_EQ(readU32(patch.bytes, 0), 0xE51FF004u);
  PL_CHECK_EQ(readU32(patch.bytes, 4), 0x2000u);
}

PL_TEST(arm_data_processing_from_pc_is_refused) {
  const uint32_t prologue[] = {0xE1A0000Fu, 0xE1A00001u};  // mov r0, pc
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(!relocation.supported);
}

PL_TEST(arm_write_to_pc_is_still_relocated) {
  // push {r4, lr} ; mov pc, lr - PC is written, never read.
  const uint32_t prologue[] = {0xE92D4010u, 0xE1A0F00Eu};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(relocation.supported);
  PL_CHECK_EQ(relocation.overwritten, size_t{8});
}

PL_TEST(arm_mrs_is_not_mistaken_for_a_pc_read) {
  // mrs r0, apsr ; bx lr - the 1111 in the Rn field is a status register.
  const uint32_t prologue[] = {0xE10F0000u, 0xE12FFF1Eu};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Arm);
  PL_CHECK(relocation.supported);
}

PL_TEST(thumb_high_register_read_of_pc_is_refused) {
  const uint16_t addFromPc[] = {0x4478u, 0x3001u};    // add r0, pc
  PL_CHECK(!relocatePrologue(reinterpret_cast<const uint8_t*>(addFromPc), 4, Isa::Thumb)
                .supported);
  const uint16_t movFromPc[] = {0x4678u, 0x3001u};    // mov r0, pc
  PL_CHECK(!relocatePrologue(reinterpret_cast<const uint8_t*>(movFromPc), 4, Isa::Thumb)
                .supported);
  const uint16_t moveToPc[] = {0x46F7u, 0x3001u};     // mov pc, lr
  PL_CHECK(relocatePrologue(reinterpret_cast<const uint8_t*>(moveToPc), 4, Isa::Thumb)
               .supported);
}

PL_TEST(thumb_it_block_is_refused) {
  const uint16_t prologue[] = {0xBF08u, 0x4608u, 0x3001u, 0x4770u};  // it eq
  PL_CHECK(!relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb)
                .supported);
}

PL_TEST(thumb_nop_is_relocated) {
  // 0xBF00 has a zero condition mask, so it is nop and not an IT header.
  const uint16_t prologue[] = {0xBF00u, 0x4608u, 0x3001u, 0x4770u};
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb);
  PL_CHECK(relocation.supported);
  PL_CHECK_EQ(relocation.overwritten, size_t{8});
}

PL_TEST(thumb_table_branch_is_refused) {
  const uint16_t prologue[] = {0xE8DFu, 0xF000u, 0x4770u};  // tbb [pc, r0]
  PL_CHECK(!relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Thumb)
                .supported);
}

PL_TEST(unknown_isa_is_refused_by_the_relocator) {
  const uint32_t prologue[] = {0xE1A00001u, 0xE1A00001u};
  PL_CHECK(!relocatePrologue(reinterpret_cast<const uint8_t*>(prologue), 8, Isa::Unknown)
                .supported);
}
