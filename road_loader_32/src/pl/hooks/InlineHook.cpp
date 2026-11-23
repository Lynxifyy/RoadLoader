#include "pl/hooks/InlineHook.h"

#include <sys/mman.h>
#include <unistd.h>

#include <cstring>
#include <vector>

#include "pl/Log.h"

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

bool makeWritable(void* address, size_t size, int* savedProtection) {
  const uintptr_t start = reinterpret_cast<uintptr_t>(address) & ~(pageSize() - 1);
  const uintptr_t end = (reinterpret_cast<uintptr_t>(address) + size + pageSize() - 1) &
                        ~(pageSize() - 1);
  // Read the current protection from /proc/self/maps; default to read+exec.
  *savedProtection = PROT_READ | PROT_EXEC;
  if (mprotect(reinterpret_cast<void*>(start), end - start, PROT_READ | PROT_WRITE | PROT_EXEC) !=
      0) {
    PL_LOGE("mprotect(RWX) failed for %p", reinterpret_cast<void*>(start));
    return false;
  }
  return true;
}

void restoreProtection(void* address, size_t size, int protection) {
  const uintptr_t start = reinterpret_cast<uintptr_t>(address) & ~(pageSize() - 1);
  const uintptr_t end = (reinterpret_cast<uintptr_t>(address) + size + pageSize() - 1) &
                        ~(pageSize() - 1);
  if (mprotect(reinterpret_cast<void*>(start), end - start, protection) != 0) {
    PL_LOGE("mprotect(restore) failed for %p", reinterpret_cast<void*>(start));
  }
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

Status InlineHook::install(void* target, void* replacement, void** original) {
  if (target == nullptr || replacement == nullptr || original == nullptr) {
    return Status::InvalidArgument;
  }
  if (installed_) {
    return Status::AlreadyExists;
  }

#if defined(PL_HOST_ARM) || defined(PL_HOST_AARCH64)
#if defined(PL_HOST_ARM)
  const bool thumb = (reinterpret_cast<uintptr_t>(target) & 1u) != 0;
  const Isa isa = thumb ? Isa::Thumb : Isa::Arm;
  const size_t patchSize = 8;
#else
  const bool thumb = false;
  const Isa isa = Isa::Aarch64;
  const size_t patchSize = 16;
#endif
  void* entry = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(target) &
                                        ~static_cast<uintptr_t>(1));
  const auto relocation =
      relocatePrologue(reinterpret_cast<const uint8_t*>(entry), patchSize, isa);
  if (!relocation.supported) {
    PL_LOGW("cannot hook %p: %s", entry, relocation.reason);
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

  const auto patch = encodeAbsoluteJump(reinterpret_cast<uintptr_t>(replacement), isa);
  if (patch.size() != patchSize) {
    munmap(trampoline, allocation);
    return Status::Unsupported;
  }

  saved_.assign(reinterpret_cast<const uint8_t*>(entry),
                reinterpret_cast<const uint8_t*>(entry) + relocation.overwritten);
  int savedProtection = PROT_READ | PROT_EXEC;
  if (!makeWritable(entry, relocation.overwritten, &savedProtection)) {
    munmap(trampoline, allocation);
    saved_.clear();
    return Status::PermissionDenied;
  }
  std::memcpy(entry, patch.data(), patch.size());
  // Anything between the patch and the relocated length keeps its original
  // bytes; the trampoline jumps over them.
  flushInstructionCache(entry, relocation.overwritten);
  restoreProtection(entry, relocation.overwritten, savedProtection);

  target_ = target;
  replacement_ = replacement;
  trampoline_ = trampoline;
  trampolineSize_ = allocation;
  installed_ = true;
  // A Thumb trampoline must be called with bit 0 set.
  *original = thumb ? reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(trampoline) | 1u)
                    : trampoline;
  PL_LOGI("hooked %p -> %p (trampoline %p)", entry, replacement, trampoline);
  return Status::Ok;
#else
  (void)target;
  (void)replacement;
  (void)original;
  return Status::Unsupported;
#endif
}

Status InlineHook::uninstall() {
  if (!installed_) {
    return Status::Ok;
  }
  void* entry = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(target_) &
                                        ~static_cast<uintptr_t>(1));
  int savedProtection = PROT_READ | PROT_EXEC;
  if (!makeWritable(entry, saved_.size(), &savedProtection)) {
    return Status::PermissionDenied;
  }
  std::memcpy(entry, saved_.data(), saved_.size());
  flushInstructionCache(entry, saved_.size());
  restoreProtection(entry, saved_.size(), savedProtection);
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
