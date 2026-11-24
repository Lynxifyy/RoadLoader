#include "pl/hooks/InlineHook.h"

#include <sys/mman.h>
#include <unistd.h>

#include <algorithm>
#include <cstring>
#include <fstream>
#include <sstream>
#include <vector>

#include "pl/Log.h"
#include "pl/memory/ModuleMap.h"

#if defined(__aarch64__)
#define PL_HOST_AARCH64 1
#elif defined(__arm__)
#define PL_HOST_ARM 1
#endif

namespace pl::hooks {
namespace {

size_t pageSize() {
  static const size_t size = static_cast<size_t>(sysconf(_SC_PAGESIZE));
  return size;
}

uintptr_t pageStart(uintptr_t address) { return address & ~(pageSize() - 1); }

uintptr_t pageEnd(uintptr_t address, size_t size) {
  return pageStart(address + size + pageSize() - 1);
}

struct ProtectionSpan {
  uintptr_t start = 0;
  size_t size = 0;
  int protection = 0;
};

int toProtection(const std::string& permissions) {
  int protection = 0;
  if (permissions.find('r') != std::string::npos) {
    protection |= PROT_READ;
  }
  if (permissions.find('w') != std::string::npos) {
    protection |= PROT_WRITE;
  }
  if (permissions.find('x') != std::string::npos) {
    protection |= PROT_EXEC;
  }
  return protection;
}

/// Fills `spans` with the real protection of every mapping covering
/// `[start, end)`. Returns false when /proc/self/maps does not describe the
/// whole range, so the caller can fall back to read+execute.
bool readProtectionSpans(uintptr_t start, uintptr_t end, std::vector<ProtectionSpan>* spans) {
  std::ifstream maps("/proc/self/maps");
  if (!maps) {
    return false;
  }
  std::stringstream buffer;
  buffer << maps.rdbuf();
  uintptr_t cursor = start;
  for (const auto& mapping : memory::parseMaps(buffer.str())) {
    if (mapping.end <= cursor || mapping.start >= end) {
      continue;
    }
    if (mapping.start > cursor) {
      return false;
    }
    const uintptr_t spanEnd = std::min<uintptr_t>(mapping.end, end);
    spans->push_back({cursor, static_cast<size_t>(spanEnd - cursor), toProtection(mapping.permissions)});
    cursor = spanEnd;
  }
  return cursor >= end;
}

void restoreProtection(const std::vector<ProtectionSpan>& spans) {
  for (const auto& span : spans) {
    if (mprotect(reinterpret_cast<void*>(span.start), span.size, span.protection) != 0) {
      PL_LOGE("mprotect(restore) failed for [%zx, +%zu)", static_cast<size_t>(span.start),
              span.size);
    }
  }
}

/// Makes `[address, address + size)` writable, remembering the protection that
/// was really there. Read+write without execute is tried first; read+write+
/// execute is a logged fallback for pages that refuse the first form.
bool makeWritable(void* address, size_t size, std::vector<ProtectionSpan>* saved) {
  const uintptr_t start = pageStart(reinterpret_cast<uintptr_t>(address));
  const uintptr_t end = pageEnd(reinterpret_cast<uintptr_t>(address), size);
  if (!readProtectionSpans(start, end, saved)) {
    PL_LOGW("/proc/self/maps did not describe [%zx, %zx); assuming r-x",
            static_cast<size_t>(start), static_cast<size_t>(end));
    saved->clear();
    saved->push_back({start, static_cast<size_t>(end - start), PROT_READ | PROT_EXEC});
  }
  for (size_t index = 0; index < saved->size(); ++index) {
    const auto& span = (*saved)[index];
    void* base = reinterpret_cast<void*>(span.start);
    if (mprotect(base, span.size, (span.protection | PROT_WRITE) & ~PROT_EXEC) == 0) {
      continue;
    }
    if (mprotect(base, span.size, span.protection | PROT_WRITE | PROT_EXEC) == 0) {
      PL_LOGW("mprotect(rw) refused for [%zx, +%zu); retrying with rwx",
              static_cast<size_t>(span.start), span.size);
      continue;
    }
    PL_LOGE("mprotect failed for [%zx, +%zu)", static_cast<size_t>(span.start), span.size);
    for (size_t done = 0; done < index; ++done) {
      const auto& previous = (*saved)[done];
      mprotect(reinterpret_cast<void*>(previous.start), previous.size, previous.protection);
    }
    saved->clear();
    return false;
  }
  return true;
}

void flushInstructionCache(void* address, size_t size) {
  __builtin___clear_cache(reinterpret_cast<char*>(address),
                          reinterpret_cast<char*>(address) + size);
}

}  // namespace

InlineHook::~InlineHook() { uninstall(); }

InlineHook::InlineHook(InlineHook&& other) noexcept
    : target_(other.target_),
      replacement_(other.replacement_),
      trampoline_(other.trampoline_),
      trampolineSize_(other.trampolineSize_),
      saved_(std::move(other.saved_)),
      installed_(other.installed_) {
  other.target_ = nullptr;
  other.replacement_ = nullptr;
  other.trampoline_ = nullptr;
  other.trampolineSize_ = 0;
  other.installed_ = false;
  other.saved_.clear();
}

InlineHook& InlineHook::operator=(InlineHook&& other) noexcept {
  if (this != &other) {
    uninstall();
    target_ = other.target_;
    replacement_ = other.replacement_;
    trampoline_ = other.trampoline_;
    trampolineSize_ = other.trampolineSize_;
    saved_ = std::move(other.saved_);
    installed_ = other.installed_;
    other.target_ = nullptr;
    other.replacement_ = nullptr;
    other.trampoline_ = nullptr;
    other.trampolineSize_ = 0;
    other.installed_ = false;
    other.saved_.clear();
  }
  return *this;
}

Status InlineHook::install(void* target, void* replacement, void** original, Isa isa) {
  if (target == nullptr || replacement == nullptr || original == nullptr) {
    return Status::InvalidArgument;
  }
  if (installed_) {
    return Status::AlreadyExists;
  }
  if (isa == Isa::Unknown) {
    PL_LOGW("hook at %p refused: no explicit ISA", target);
    return Status::InvalidArgument;
  }

#if defined(PL_HOST_ARM) || defined(PL_HOST_AARCH64)
#if defined(PL_HOST_ARM)
  if (isa == Isa::Aarch64) {
    PL_LOGW("hook at %p refused: aarch64 target on a 32-bit host", target);
    return Status::Unsupported;
  }
#else
  if (isa != Isa::Aarch64) {
    PL_LOGW("hook at %p refused: 32-bit target on a 64-bit host", target);
    return Status::Unsupported;
  }
#endif
  // A function pointer may carry bit 0; the code itself starts at the even
  // address, and the ISA already told us how to decode it.
  void* entry = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(target) &
                                        ~static_cast<uintptr_t>(1));
  const auto patch = encodeJumpPatch(reinterpret_cast<uintptr_t>(entry),
                                     reinterpret_cast<uintptr_t>(replacement), isa);
  const auto relocation = relocatePrologue(reinterpret_cast<const uint8_t*>(entry),
                                           patch.bytes.size(), isa);
  if (!relocation.supported) {
    PL_LOGW("cannot hook %p: %s", entry, relocation.reason);
    return Status::Unsupported;
  }
  if (relocation.overwritten < patch.bytes.size()) {
    PL_LOGW("cannot hook %p: %zu-byte patch exceeds the %zu-byte relocation", entry,
            patch.bytes.size(), relocation.overwritten);
    return Status::Unsupported;
  }

  const size_t allocation = (relocation.trampoline.size() + pageSize() - 1) & ~(pageSize() - 1);
  void* trampoline =
      mmap(nullptr, allocation, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (trampoline == MAP_FAILED) {
    PL_LOGE("trampoline mmap failed");
    return Status::OutOfMemory;
  }
  std::memcpy(trampoline, relocation.trampoline.data(), relocation.trampoline.size());
  if (mprotect(trampoline, allocation, PROT_READ | PROT_EXEC) != 0) {
    PL_LOGE("trampoline mprotect(RX) failed");
    munmap(trampoline, allocation);
    return Status::PermissionDenied;
  }
  flushInstructionCache(trampoline, relocation.trampoline.size());

  saved_.assign(reinterpret_cast<const uint8_t*>(entry),
                reinterpret_cast<const uint8_t*>(entry) + relocation.overwritten);
  std::vector<ProtectionSpan> savedProtection;
  if (!makeWritable(entry, relocation.overwritten, &savedProtection)) {
    munmap(trampoline, allocation);
    saved_.clear();
    return Status::PermissionDenied;
  }
  std::memcpy(entry, patch.bytes.data(), patch.bytes.size());
  // Anything between the patch and the relocated length keeps its original
  // bytes; the trampoline jumps over them.
  flushInstructionCache(entry, relocation.overwritten);
  restoreProtection(savedProtection);

  target_ = target;
  replacement_ = replacement;
  trampoline_ = trampoline;
  trampolineSize_ = allocation;
  installed_ = true;
  // A Thumb trampoline must be called with bit 0 set.
  *original = (isa == Isa::Thumb)
                  ? reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(trampoline) | 1u)
                  : trampoline;
  PL_LOGI("hooked %p -> %p (trampoline %p)", entry, replacement, trampoline);
  return Status::Ok;
#else
  (void)target;
  (void)replacement;
  (void)original;
  (void)isa;
  return Status::Unsupported;
#endif
}

Status InlineHook::uninstall() {
  if (!installed_) {
    return Status::Ok;
  }
  void* entry = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(target_) &
                                        ~static_cast<uintptr_t>(1));
  std::vector<ProtectionSpan> savedProtection;
  if (!makeWritable(entry, saved_.size(), &savedProtection)) {
    return Status::PermissionDenied;
  }
  std::memcpy(entry, saved_.data(), saved_.size());
  flushInstructionCache(entry, saved_.size());
  restoreProtection(savedProtection);
  if (trampoline_ != nullptr) {
    munmap(trampoline_, trampolineSize_);
  }
  trampoline_ = nullptr;
  trampolineSize_ = 0;
  target_ = nullptr;
  replacement_ = nullptr;
  saved_.clear();
  installed_ = false;
  return Status::Ok;
}

}  // namespace pl::hooks
