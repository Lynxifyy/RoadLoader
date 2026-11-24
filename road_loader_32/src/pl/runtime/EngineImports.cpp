#include "pl/runtime/EngineImports.h"

#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <link.h>

#include <algorithm>
#include <cstdint>

#include <fstream>
#include <iterator>
#include <memory>
#include <mutex>
#include <vector>

#if defined(__ANDROID__)
#include <android/log.h>
#endif

#include "pl/hooks/ImportHook.h"
#include "pl/memory/ImportResolver.h"
#include "pl/memory/ModuleMap.h"

namespace pl::runtime {
namespace {

std::atomic<uint64_t> selfTestCount{0};

/// Hooks installed by `install()` live for the life of the process: they are
/// the frame-capture path, not probes.
std::mutex& persistentMutex() {
  static std::mutex mutex;
  return mutex;
}

std::vector<std::unique_ptr<hooks::ImportHook>>& persistentHooks() {
  static std::vector<std::unique_ptr<hooks::ImportHook>> hooks;
  return hooks;
}

#if defined(__ANDROID__)
extern "C" int roadSelfTestLogReplacement(int, const char*, const char*, ...) {
  selfTestCount.fetch_add(1, std::memory_order_relaxed);
  return 0;
}
#endif

std::vector<memory::Mapping> currentMappings() {
  std::ifstream maps("/proc/self/maps");
  std::string text((std::istreambuf_iterator<char>(maps)), std::istreambuf_iterator<char>());
  return memory::parseMaps(text);
}

struct LoadedObject {
  std::string name;
  uintptr_t base = 0;
  uintptr_t end = 0;
};

int collectProgramHeader(struct dl_phdr_info* info, size_t, void* data) {
  auto* objects = static_cast<std::vector<LoadedObject>*>(data);
  if (info->dlpi_name == nullptr || info->dlpi_name[0] == '\0') {
    return 0;
  }
  LoadedObject object;
  object.name = info->dlpi_name;
  uintptr_t lowest = UINTPTR_MAX;
  uintptr_t highest = 0;
  for (int index = 0; index < info->dlpi_phnum; ++index) {
    const auto& header = info->dlpi_phdr[index];
    if (header.p_type != PT_LOAD) {
      continue;
    }
    const uintptr_t start = static_cast<uintptr_t>(info->dlpi_addr + header.p_vaddr);
    const uintptr_t finish = start + static_cast<uintptr_t>(header.p_memsz);
    lowest = std::min(lowest, start);
    highest = std::max(highest, finish);
  }
  if (lowest == UINTPTR_MAX) {
    return 0;
  }
  object.base = lowest;
  object.end = highest;
  objects->push_back(std::move(object));
  return 0;
}

std::vector<LoadedObject> loadedObjects() {
  std::vector<LoadedObject> objects;
  dl_iterate_phdr(&collectProgramHeader, &objects);
  return objects;
}

/// Finds a loaded module by soname or by the path the loader reports.
///
/// `/proc/self/maps` is not authoritative on its own: a library loaded
/// straight out of an APK can appear under a path that does not end with the
/// soname the caller used. The loader's own list of loaded objects closes that
/// gap, and `dl_iterate_phdr` is the only way to ask for it on bionic, which
/// has neither `dlinfo` nor `RTLD_DI_LINKMAP`.
std::optional<memory::ModuleInfo> locateModule(const char* name) {
  if (name == nullptr || name[0] == '\0') {
    return std::nullopt;
  }
  const auto mappings = currentMappings();
  if (auto found = memory::findModule(mappings, name); found.has_value()) {
    return found;
  }
  // A library packaged uncompressed is mapped straight out of the APK, so
  // /proc/self/maps shows the APK path and nothing that ends with the soname.
  // The loader still knows the object by name, and its program headers give
  // the address range that /proc/self/maps cannot.
  const size_t nameLength = std::strlen(name);
  for (const auto& object : loadedObjects()) {
    if (object.name.size() < nameLength) {
      continue;
    }
    if (object.name.compare(object.name.size() - nameLength, nameLength, name) != 0) {
      continue;
    }
    memory::ModuleInfo info;
    info.path = object.name;
    info.base = object.base;
    info.size = object.end - object.base;
    for (const auto& mapping : mappings) {
      if (mapping.start >= object.base && mapping.end <= object.end &&
          mapping.permissions.find('x') != std::string::npos) {
        info.executable.push_back(mapping);
      }
    }
    return info;
  }
  return std::nullopt;
}

std::string selfModuleName() {
  Dl_info info{};
  if (dladdr(reinterpret_cast<void*>(&EngineImports::selfTest), &info) == 0 ||
      info.dli_fname == nullptr) {
    return "libroadloader32.so";
  }
  std::string path = info.dli_fname;
  const size_t slash = path.find_last_of('/');
  return slash == std::string::npos ? path : path.substr(slash + 1);
}

ImportProbe baseProbe(const char* module, const char* symbol) {
  ImportProbe probe;
  probe.module = module != nullptr ? module : "";
  probe.symbol = symbol != nullptr ? symbol : "";
  return probe;
}

}  // namespace

std::string ImportProbe::describe() const {
  std::string line = module;
  line += " ";
  line += symbol;
  line += ": ";
  if (!moduleFound) {
    line += "module not loaded";
  } else if (!symbolFound) {
    line += "symbol not imported";
  } else if (lazy) {
    line += "slot not bound yet";
  } else if (!installed) {
    line += "available";
  } else {
    line += "hooked";
  }
  char numbers[96];
  std::snprintf(numbers, sizeof(numbers), " slot=0x%llx target=0x%llx",
                static_cast<unsigned long long>(slot), static_cast<unsigned long long>(target));
  line += numbers;
  if (fired) line += " fired";
  if (restored) line += " restored";
  return line;
}

const char* EngineImports::gameModule() { return "libminecraftpe.so"; }

const char* EngineImports::swapBuffersSymbol() { return "eglSwapBuffers"; }

ImportProbe EngineImports::probe(const char* module, const char* symbol) {
  ImportProbe probe = baseProbe(module, symbol);
  if (module == nullptr || symbol == nullptr) {
    probe.status = Status::InvalidArgument;
    return probe;
  }
  const auto loaded = locateModule(module);
  if (!loaded.has_value()) {
    probe.status = Status::NotFound;
    return probe;
  }
  probe.moduleFound = true;
  auto table = memory::readImportsAt(loaded->base, loaded->size);
  if (!table.has_value()) {
    probe.status = Status::NotFound;
    return probe;
  }
  const auto* entry = table->find(symbol);
  if (entry == nullptr) {
    probe.status = Status::NotFound;
    return probe;
  }
  probe.symbolFound = true;
  probe.lazy = entry->lazy;
  probe.slot = entry->slot;
  probe.target = entry->target;
  probe.status = probe.lazy ? Status::Unsupported : Status::Ok;
  return probe;
}

ImportProbe EngineImports::install(const char* module, const char* symbol, void* replacement) {
  ImportProbe probe = baseProbe(module, symbol);
  if (module == nullptr || symbol == nullptr || replacement == nullptr) {
    probe.status = Status::InvalidArgument;
    return probe;
  }
  const auto loaded = locateModule(module);
  if (!loaded.has_value()) {
    probe.status = Status::NotFound;
    return probe;
  }
  probe.moduleFound = true;
  hooks::ImportHook hook;
  probe.status = hook.install(loaded->base, loaded->size, symbol, replacement);
  if (probe.status != Status::Ok) {
    // Record what the table said so the UI can distinguish "missing" from
    // "present but unusable".
    auto table = memory::readImportsAt(loaded->base, loaded->size);
    if (table.has_value()) {
      const auto* entry = table->find(symbol);
      if (entry != nullptr) {
        probe.symbolFound = true;
        probe.lazy = entry->lazy;
        probe.slot = entry->slot;
        probe.target = entry->target;
      }
    }
    return probe;
  }
  probe.symbolFound = true;
  probe.installed = true;
  probe.slot = hook.slot();
  probe.target = reinterpret_cast<uintptr_t>(hook.original());
  {
    std::lock_guard<std::mutex> lock(persistentMutex());
    persistentHooks().push_back(std::make_unique<hooks::ImportHook>(std::move(hook)));
  }
  return probe;
}

ImportProbe EngineImports::selfTest(const char* module, const char* symbol) {
#if defined(__ANDROID__)
  const std::string ownModule = selfModuleName();
  const char* targetModule = module != nullptr ? module : ownModule.c_str();
  const char* targetSymbol = symbol != nullptr ? symbol : "__android_log_print";
  ImportProbe probe = baseProbe(targetModule, targetSymbol);
  const auto loaded = locateModule(targetModule);
  if (!loaded.has_value()) {
    probe.status = Status::NotFound;
    return probe;
  }
  probe.moduleFound = true;
  hooks::ImportHook hook;
  probe.status = hook.install(loaded->base, loaded->size, targetSymbol,
                              reinterpret_cast<void*>(&roadSelfTestLogReplacement));
  if (probe.status != Status::Ok) {
    auto table = memory::readImportsAt(loaded->base, loaded->size);
    if (table.has_value()) {
      const auto* entry = table->find(targetSymbol);
      if (entry != nullptr) {
        probe.symbolFound = true;
        probe.lazy = entry->lazy;
        probe.slot = entry->slot;
        probe.target = entry->target;
      }
    }
    return probe;
  }
  probe.symbolFound = true;
  probe.installed = true;
  probe.slot = hook.slot();
  probe.target = reinterpret_cast<uintptr_t>(hook.original());

  const uint64_t before = selfTestCount.load(std::memory_order_relaxed);
  __android_log_print(ANDROID_LOG_INFO, "RoadLoader", "import hook self test");
  probe.fired = selfTestCount.load(std::memory_order_relaxed) > before;

  const Status restored = hook.uninstall();
  probe.restored = restored == Status::Ok;
  const uint64_t afterRestore = selfTestCount.load(std::memory_order_relaxed);
  __android_log_print(ANDROID_LOG_INFO, "RoadLoader", "import hook restored");
  // The replacement must not run once the original pointer is back.
  probe.restored = probe.restored && selfTestCount.load(std::memory_order_relaxed) == afterRestore;
  probe.status = (probe.fired && probe.restored) ? Status::Ok : Status::Unsupported;
  return probe;
#else
  ImportProbe probe = baseProbe(module, symbol);
  probe.status = Status::Unsupported;
  return probe;
#endif
}

std::string EngineImports::loadedModules(std::string_view filter, size_t limit) {
  std::string out;
  size_t seen = 0;
  for (const auto& mapping : currentMappings()) {
    if (mapping.path.empty() || mapping.path[0] == '[') {
      continue;
    }
    if (mapping.path.find(filter) == std::string::npos) {
      continue;
    }
    if (out.find(mapping.path) != std::string::npos) {
      continue;
    }
    if (seen >= limit) {
      out += "...";
      break;
    }
    if (seen != 0) {
      out += ' ';
    }
    out += mapping.path;
    ++seen;
  }
  return out;
}

std::string EngineImports::describeFile(const char* path, const char* symbol) {
  if (path == nullptr || path[0] == '\0') {
    return "no staged engine";
  }
  std::string name = path;
  const size_t slash = name.find_last_of('/');
  if (slash != std::string::npos) {
    name = name.substr(slash + 1);
  }
  const auto table = memory::readImportsFromFile(path);
  if (!table.has_value()) {
    return name + ": not readable as an ELF";
  }
  std::string out = name + " (staged) imports=" + std::to_string(table->slots.size());
  const auto* found = table->find(symbol != nullptr ? symbol : "eglSwapBuffers");
  if (found == nullptr) {
    out += " ";
    out += symbol != nullptr ? symbol : "eglSwapBuffers";
    out += ": not imported";
    return out;
  }
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), ": imported reloc=%u", found->relocType);
  out += " ";
  out += found->name;
  out += buffer;
  return out;
}

std::string EngineImports::moduleDebug(const char* name) {
  const auto mappings = currentMappings();
  char header[96];
  std::snprintf(header, sizeof(header), "mappings=%zu name=%s", mappings.size(),
                name != nullptr ? name : "(null)");
  std::string out = header;
  if (name == nullptr) {
    return out;
  }
  const size_t nameLength = std::strlen(name);
  size_t matches = 0;
  std::string first;
  for (const auto& mapping : mappings) {
    if (mapping.path.size() < nameLength) {
      continue;
    }
    if (mapping.path.compare(mapping.path.size() - nameLength, nameLength, name) != 0) {
      continue;
    }
    if (matches == 0) {
      first = mapping.path;
    }
    ++matches;
  }
  std::snprintf(header, sizeof(header), " suffix_matches=%zu", matches);
  out += header;
  out += " first=";
  out += first.empty() ? "none" : first;
  const auto located = locateModule(name);
  std::snprintf(header, sizeof(header), " located=%s", located.has_value() ? "yes" : "no");
  out += header;
  return out;
}

uint64_t EngineImports::selfTestCalls() {
  return selfTestCount.load(std::memory_order_relaxed);
}

}  // namespace pl::runtime
