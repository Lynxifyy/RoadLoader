#include "pl/runtime/EngineImports.h"

#include <cstdio>
#include <cstring>
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
  auto table = memory::readModuleImports(module);
  if (!table.has_value()) {
    probe.status = Status::NotFound;
    return probe;
  }
  probe.moduleFound = true;
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
  auto mappings = memory::parseMaps([] {
    std::string text;
    std::ifstream maps("/proc/self/maps");
    text.assign((std::istreambuf_iterator<char>(maps)), std::istreambuf_iterator<char>());
    return text;
  }());
  auto loaded = memory::findModule(mappings, module);
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
  const char* targetModule = module != nullptr ? module : "libroadloader32.so";
  const char* targetSymbol = symbol != nullptr ? symbol : "__android_log_print";
  ImportProbe probe = baseProbe(targetModule, targetSymbol);
  auto mappings = memory::parseMaps([] {
    std::string text;
    std::ifstream maps("/proc/self/maps");
    text.assign((std::istreambuf_iterator<char>(maps)), std::istreambuf_iterator<char>());
    return text;
  }());
  auto loaded = memory::findModule(mappings, targetModule);
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

uint64_t EngineImports::selfTestCalls() {
  return selfTestCount.load(std::memory_order_relaxed);
}

}  // namespace pl::runtime
