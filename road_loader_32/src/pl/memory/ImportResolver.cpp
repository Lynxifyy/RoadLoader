#include "pl/memory/ImportResolver.h"

#include <elf.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <fstream>
#include <limits>

#include "pl/memory/ModuleMap.h"

namespace pl::memory {
namespace {

#if defined(__arm__)
constexpr uint32_t kJumpSlot = R_ARM_JUMP_SLOT;
constexpr uint32_t kGlobDat = R_ARM_GLOB_DAT;
#elif defined(__aarch64__)
constexpr uint32_t kJumpSlot = R_AARCH64_JUMP_SLOT;
constexpr uint32_t kGlobDat = R_AARCH64_GLOB_DAT;
#elif defined(__x86_64__)
constexpr uint32_t kJumpSlot = R_X86_64_JUMP_SLOT;
constexpr uint32_t kGlobDat = R_X86_64_GLOB_DAT;
#elif defined(__i386__)
constexpr uint32_t kJumpSlot = R_386_JMP_SLOT;
constexpr uint32_t kGlobDat = R_386_GLOB_DAT;
#else
#error "ImportResolver has no relocation types for this architecture"
#endif

#if UINTPTR_MAX == 0xFFFFFFFF
using Ehdr = Elf32_Ehdr;
using Phdr = Elf32_Phdr;
using Dyn = Elf32_Dyn;
using Sym = Elf32_Sym;
using Rel = Elf32_Rel;
using Rela = Elf32_Rela;
constexpr uint32_t kRelType = DT_REL;
constexpr uint32_t kRelSize = DT_RELSZ;
constexpr uint32_t kRelaType = DT_RELA;
constexpr uint32_t kRelaSize = DT_RELASZ;
#else
using Ehdr = Elf64_Ehdr;
using Phdr = Elf64_Phdr;
using Dyn = Elf64_Dyn;
using Sym = Elf64_Sym;
using Rel = Elf64_Rel;
using Rela = Elf64_Rela;
constexpr uint32_t kRelType = DT_REL;
constexpr uint32_t kRelSize = DT_RELSZ;
constexpr uint32_t kRelaType = DT_RELA;
constexpr uint32_t kRelaSize = DT_RELASZ;
#endif

bool inRange(uintptr_t address, uintptr_t start, size_t size) {
  if (size == 0) return address >= start;
  return address >= start && address - start < size;
}

uint32_t relocType(uintptr_t info) {
#if UINTPTR_MAX == 0xFFFFFFFF
  return static_cast<uint32_t>(info & 0xffU);
#else
  return static_cast<uint32_t>(info & 0xffffffffU);
#endif
}

uint32_t relocSymbol(uintptr_t info) {
#if UINTPTR_MAX == 0xFFFFFFFF
  return static_cast<uint32_t>(info >> 8);
#else
  return static_cast<uint32_t>(info >> 32);
#endif
}

/// Dynamic entries hold absolute addresses once the linker has loaded the
/// object; a few linkers leave them relative. Both are accepted.
uintptr_t biased(uintptr_t value, uintptr_t base) {
  if (value == 0) return 0;
  return value < base ? base + value : value;
}

const char* safeString(uintptr_t strtab, size_t strsz, uintptr_t offset) {
  if (strtab == 0 || offset >= strsz) return nullptr;
  const auto* text = reinterpret_cast<const char*>(strtab + offset);
  // The table is NUL terminated as a whole; bound the scan to what is left.
  const size_t available = strsz - static_cast<size_t>(offset);
  if (memchr(text, '\0', available) == nullptr) return nullptr;
  return text;
}

}  // namespace

const ImportSlot* ImportTable::find(std::string_view symbol) const {
  for (const auto& entry : slots) {
    if (entry.name == symbol) return &entry;
  }
  return nullptr;
}

size_t ImportTable::resolved() const {
  size_t count = 0;
  for (const auto& entry : slots) {
    if (entry.target != 0) ++count;
  }
  return count;
}

bool isExecutableAddress(uintptr_t address) {
  if (address == 0) return false;
  std::ifstream maps("/proc/self/maps");
  if (!maps) return false;
  std::string line;
  while (std::getline(maps, line)) {
    unsigned long long start = 0;
    unsigned long long end = 0;
    char permissions[8] = {0};
    if (std::sscanf(line.c_str(), "%llx-%llx %7s", &start, &end, permissions) != 3) {
      continue;
    }
    if (address >= start && address < end) {
      return permissions[2] == 'x';
    }
  }
  return false;
}

std::optional<ImportTable> readImportsAt(uintptr_t base, size_t size) {
  if (base == 0) return std::nullopt;
  const auto* header = reinterpret_cast<const Ehdr*>(base);
  if (memcmp(header->e_ident, ELFMAG, SELFMAG) != 0) return std::nullopt;
  if (header->e_ident[EI_CLASS] != (UINTPTR_MAX == 0xFFFFFFFF ? ELFCLASS32 : ELFCLASS64)) {
    return std::nullopt;
  }
  if (header->e_phoff == 0 || header->e_phnum == 0) return std::nullopt;
  if (size != 0 && !inRange(base + header->e_phoff, base, size)) return std::nullopt;

  const auto* phdrs = reinterpret_cast<const Phdr*>(base + header->e_phoff);
  const Dyn* dynamic = nullptr;
  size_t dynamicBytes = 0;
  for (int index = 0; index < header->e_phnum; ++index) {
    if (phdrs[index].p_type != PT_DYNAMIC) continue;
    dynamic = reinterpret_cast<const Dyn*>(base + phdrs[index].p_vaddr);
    dynamicBytes = static_cast<size_t>(phdrs[index].p_memsz);
    break;
  }
  if (dynamic == nullptr || dynamicBytes < sizeof(Dyn)) return std::nullopt;

  uintptr_t symtab = 0;
  uintptr_t strtab = 0;
  size_t strsz = 0;
  uintptr_t jmprel = 0;
  size_t pltrelsz = 0;
  uintptr_t rel = 0;
  size_t relsz = 0;
  uintptr_t rela = 0;
  size_t relasz = 0;
  size_t syment = sizeof(Sym);
  uintptr_t pltrel = 0;

  const size_t entries = dynamicBytes / sizeof(Dyn);
  for (size_t index = 0; index < entries; ++index) {
    const Dyn& entry = dynamic[index];
    if (entry.d_tag == DT_NULL) break;
    switch (entry.d_tag) {
      case DT_SYMTAB:
        symtab = biased(entry.d_un.d_ptr, base);
        break;
      case DT_STRTAB:
        strtab = biased(entry.d_un.d_ptr, base);
        break;
      case DT_STRSZ:
        strsz = static_cast<size_t>(entry.d_un.d_val);
        break;
      case DT_SYMENT:
        syment = static_cast<size_t>(entry.d_un.d_val);
        break;
      case DT_JMPREL:
        jmprel = biased(entry.d_un.d_ptr, base);
        break;
      case DT_PLTRELSZ:
        pltrelsz = static_cast<size_t>(entry.d_un.d_val);
        break;
      case DT_PLTREL:
        pltrel = static_cast<uintptr_t>(entry.d_un.d_val);
        break;
      case DT_REL:
        rel = biased(entry.d_un.d_ptr, base);
        break;
      case DT_RELSZ:
        relsz = static_cast<size_t>(entry.d_un.d_val);
        break;
      case DT_RELA:
        rela = biased(entry.d_un.d_ptr, base);
        break;
      case DT_RELASZ:
        relasz = static_cast<size_t>(entry.d_un.d_val);
        break;
      default:
        break;
    }
  }
  if (symtab == 0 || strtab == 0 || strsz == 0) return std::nullopt;
  if (syment < sizeof(Sym)) return std::nullopt;

  ImportTable table;
  table.base = base;

  auto addRelocations = [&](uintptr_t address, size_t bytes, bool withAddend) {
    if (address == 0 || bytes == 0) return;
    const size_t entrySize = withAddend ? sizeof(Rela) : sizeof(Rel);
    const size_t count = bytes / entrySize;
    for (size_t index = 0; index < count; ++index) {
      uintptr_t offset = 0;
      uintptr_t info = 0;
      if (withAddend) {
        const auto& entry = reinterpret_cast<const Rela*>(address)[index];
        offset = static_cast<uintptr_t>(entry.r_offset);
        info = static_cast<uintptr_t>(entry.r_info);
      } else {
        const auto& entry = reinterpret_cast<const Rel*>(address)[index];
        offset = static_cast<uintptr_t>(entry.r_offset);
        info = static_cast<uintptr_t>(entry.r_info);
      }
      const uint32_t type = relocType(info);
      if (type != kJumpSlot && type != kGlobDat) continue;
      const uint32_t symbolIndex = relocSymbol(info);
      if (symbolIndex == 0) continue;
      const auto* symbol = reinterpret_cast<const Sym*>(symtab + symbolIndex * syment);
      const char* name = safeString(strtab, strsz, static_cast<uintptr_t>(symbol->st_name));
      if (name == nullptr || name[0] == '\0') continue;
      ImportSlot slot;
      slot.name = name;
      slot.slot = base + offset;
      slot.relocType = type;
      // SAFETY: the slot is the address the linker relocated; the dynamic
      // section described it, and the caller passed a validated module base.
      slot.target = *reinterpret_cast<const uintptr_t*>(slot.slot);
      // A slot still pointing inside the module has not been bound to the
      // provider yet (lazy PLT stub). Callers must not patch those: the
      // resolver would overwrite the patch on first use.
      slot.lazy = size != 0 && inRange(slot.target, base, size);
      table.slots.push_back(std::move(slot));
    }
  };

  if (jmprel != 0 && pltrelsz != 0) {
    const bool withAddend = pltrel == kRelaType;
    addRelocations(jmprel, pltrelsz, withAddend);
  }
  if (rel != 0 && relsz != 0) addRelocations(rel, relsz, false);
  if (rela != 0 && relasz != 0) addRelocations(rela, relasz, true);

  if (table.slots.empty()) return std::nullopt;
  return table;
}

std::optional<ImportTable> readModuleImports(std::string_view name) {
  auto mappings = parseMaps([] {
    std::ifstream maps("/proc/self/maps");
    std::string text((std::istreambuf_iterator<char>(maps)), std::istreambuf_iterator<char>());
    return text;
  }());
  auto module = findModule(mappings, name);
  if (!module.has_value()) return std::nullopt;
  auto table = readImportsAt(module->base, module->size);
  if (!table.has_value()) return std::nullopt;
  table->path = module->path;
  return table;
}

}  // namespace pl::memory
