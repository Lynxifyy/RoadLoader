#include "pl/hooks/ImportHook.h"

#include <sys/mman.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <fstream>
#include <mutex>
#include <string>
#include <unordered_set>
#include <utility>

#include "pl/memory/ImportResolver.h"

namespace pl::hooks {
namespace {

std::mutex& registryMutex() {
  static std::mutex mutex;
  return mutex;
}

std::unordered_set<uintptr_t>& registry() {
  static std::unordered_set<uintptr_t> slots;
  return slots;
}

struct PageProtection {
  uintptr_t page = 0;
  size_t length = 0;
  int prot = PROT_READ;
  bool found = false;
};

/// Reads the current protection of the mapping that contains `address`, so the
/// page can be restored exactly rather than assumed.
PageProtection protectionOf(uintptr_t address) {
  PageProtection result;
  const long pageSize = sysconf(_SC_PAGESIZE);
  if (pageSize <= 0) return result;
  result.page = address & ~static_cast<uintptr_t>(pageSize - 1);
  result.length = static_cast<size_t>(pageSize);
  std::ifstream maps("/proc/self/maps");
  if (!maps) return result;
  std::string line;
  while (std::getline(maps, line)) {
    unsigned long long start = 0;
    unsigned long long end = 0;
    char permissions[8] = {0};
    if (std::sscanf(line.c_str(), "%llx-%llx %7s", &start, &end, permissions) != 3) {
      continue;
    }
    if (address < start || address >= end) continue;
    result.prot = 0;
    if (permissions[0] == 'r') result.prot |= PROT_READ;
    if (permissions[1] == 'w') result.prot |= PROT_WRITE;
    if (permissions[2] == 'x') result.prot |= PROT_EXEC;
    result.found = true;
    return result;
  }
  return result;
}

Status storeSlot(uintptr_t slot, uintptr_t value, uintptr_t* previous) {
  const PageProtection protection = protectionOf(slot);
  if (!protection.found) return Status::NotFound;
  const bool needsWrite = (protection.prot & PROT_WRITE) == 0;
  if (needsWrite && mprotect(reinterpret_cast<void*>(protection.page), protection.length,
                             protection.prot | PROT_WRITE) != 0) {
    return Status::PermissionDenied;
  }
  if (previous != nullptr) {
    *previous = *reinterpret_cast<const uintptr_t*>(slot);
  }
  __atomic_store_n(reinterpret_cast<uintptr_t*>(slot), value, __ATOMIC_SEQ_CST);
  __builtin___clear_cache(reinterpret_cast<char*>(slot),
                          reinterpret_cast<char*>(slot + sizeof(uintptr_t)));
  if (needsWrite &&
      mprotect(reinterpret_cast<void*>(protection.page), protection.length, protection.prot) != 0) {
    return Status::PermissionDenied;
  }
  return Status::Ok;
}

}  // namespace

ImportHook::~ImportHook() { uninstall(); }

ImportHook::ImportHook(ImportHook&& other) noexcept
    : slot_(other.slot_), original_(other.original_), installed_(other.installed_) {
  other.slot_ = 0;
  other.original_ = 0;
  other.installed_ = false;
}

ImportHook& ImportHook::operator=(ImportHook&& other) noexcept {
  if (this == &other) return *this;
  uninstall();
  slot_ = other.slot_;
  original_ = other.original_;
  installed_ = other.installed_;
  other.slot_ = 0;
  other.original_ = 0;
  other.installed_ = false;
  return *this;
}

Status ImportHook::install(uintptr_t moduleBase, size_t moduleSize, std::string_view symbol,
                           void* replacement) {
  if (installed_) return Status::AlreadyExists;
  if (replacement == nullptr || symbol.empty()) return Status::InvalidArgument;
  auto table = memory::readImportsAt(moduleBase, moduleSize);
  if (!table.has_value()) return Status::NotFound;
  const memory::ImportSlot* entry = table->find(symbol);
  if (entry == nullptr) return Status::NotFound;
  if (entry->lazy) return Status::Unsupported;
  if (entry->target == 0) return Status::NotFound;

  {
    std::lock_guard<std::mutex> lock(registryMutex());
    if (registry().count(entry->slot) != 0) return Status::AlreadyExists;
  }
  uintptr_t previous = 0;
  const Status status = storeSlot(entry->slot, reinterpret_cast<uintptr_t>(replacement), &previous);
  if (status != Status::Ok) return status;
  {
    std::lock_guard<std::mutex> lock(registryMutex());
    registry().insert(entry->slot);
  }
  slot_ = entry->slot;
  original_ = previous;
  installed_ = true;
  return Status::Ok;
}

Status ImportHook::uninstall() {
  if (!installed_) return Status::Ok;
  const Status status = storeSlot(slot_, original_, nullptr);
  {
    std::lock_guard<std::mutex> lock(registryMutex());
    registry().erase(slot_);
  }
  slot_ = 0;
  original_ = 0;
  installed_ = false;
  return status;
}

size_t activeImportHooks() {
  std::lock_guard<std::mutex> lock(registryMutex());
  return registry().size();
}

}  // namespace pl::hooks
