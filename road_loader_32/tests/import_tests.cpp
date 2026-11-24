#include <dlfcn.h>
#include <unistd.h>

#include <cstdio>
#include <string>

#include "TestHarness.h"
#include "pl/hooks/ImportHook.h"
#include "pl/memory/ImportResolver.h"
#include "pl/memory/ModuleMap.h"
#include "pl/runtime/EngineImports.h"

namespace {

constexpr int kProviderValue = 7;

int replacementCalls = 0;

extern "C" int pl_test_replacement() {
  ++replacementCalls;
  return 42;
}

/// The test binary is started from the repository root, so the libraries are
/// resolved next to the executable rather than through the loader search path.
std::string libraryPath(const char* name) {
  char exe[4096] = {0};
  const ssize_t length = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
  if (length <= 0) return name;
  std::string path(exe, static_cast<size_t>(length));
  const auto slash = path.find_last_of('/');
  if (slash == std::string::npos) return name;
  return path.substr(0, slash + 1) + name;
}

/// Loads the provider and the consumer for real. The provider goes first and
/// globally so the consumer's relocations resolve against it.
bool loadPair(void** provider, void** consumer) {
  const std::string providerPath = libraryPath("libroad_test_provider.so");
  *provider = dlopen(providerPath.c_str(), RTLD_NOW | RTLD_GLOBAL);
  if (*provider == nullptr) {
    std::fprintf(stderr, "dlopen(%s): %s\n", providerPath.c_str(), dlerror());
    return false;
  }
  const std::string consumerPath = libraryPath("libroad_test_consumer.so");
  *consumer = dlopen(consumerPath.c_str(), RTLD_NOW | RTLD_GLOBAL);
  if (*consumer == nullptr) {
    std::fprintf(stderr, "dlopen(%s): %s\n", consumerPath.c_str(), dlerror());
    return false;
  }
  return true;
}

std::optional<pl::memory::ModuleInfo> consumerModule() {
  return pl::memory::findLoadedModule("libroad_test_consumer.so");
}

}  // namespace

PL_TEST(imports_of_a_real_module_resolve_by_name) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (provider == nullptr || consumer == nullptr) return;

  auto module = consumerModule();
  PL_CHECK(module.has_value());
  if (!module.has_value()) return;

  auto table = pl::memory::readImportsAt(module->base, module->size);
  PL_CHECK(table.has_value());
  if (!table.has_value()) return;

  const auto* slot = table->find("pl_provider_value");
  PL_CHECK(slot != nullptr);
  if (slot == nullptr) return;

  const auto real = reinterpret_cast<uintptr_t>(dlsym(provider, "pl_provider_value"));
  PL_CHECK(real != 0);
  PL_CHECK_EQ(slot->target, real);
  // Bound by the linker at load time, so patching it is safe.
  PL_CHECK(!slot->lazy);
  PL_CHECK(pl::memory::isExecutableAddress(slot->target));

  const auto* sum = table->find("pl_provider_add");
  PL_CHECK(sum != nullptr);
  PL_CHECK(sum->slot != slot->slot);
}

PL_TEST(an_import_hook_intercepts_the_call_and_restores_it) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (provider == nullptr || consumer == nullptr) return;

  auto module = consumerModule();
  PL_CHECK(module.has_value());
  if (!module.has_value()) return;

  using ConsumerCall = int (*)();
  auto call = reinterpret_cast<ConsumerCall>(dlsym(consumer, "pl_consumer_call"));
  PL_CHECK(call != nullptr);
  if (call == nullptr) return;

  PL_CHECK_EQ(call(), kProviderValue);

  pl::hooks::ImportHook hook;
  const auto installed = hook.install(module->base, module->size, "pl_provider_value",
                                      reinterpret_cast<void*>(&pl_test_replacement));
  PL_CHECK(installed == pl::Status::Ok);
  if (installed != pl::Status::Ok) return;

  PL_CHECK(hook.installed());
  PL_CHECK_EQ(pl::hooks::activeImportHooks(), size_t{1});
  const int before = replacementCalls;
  PL_CHECK_EQ(call(), 42);
  PL_CHECK_EQ(replacementCalls, before + 1);

  PL_CHECK(hook.uninstall() == pl::Status::Ok);
  PL_CHECK(!hook.installed());
  PL_CHECK_EQ(pl::hooks::activeImportHooks(), size_t{0});
  PL_CHECK_EQ(call(), kProviderValue);
}

PL_TEST(hooking_a_missing_symbol_is_refused) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (consumer == nullptr) return;
  auto module = consumerModule();
  if (!module.has_value()) return;

  pl::hooks::ImportHook hook;
  const auto status = hook.install(module->base, module->size, "pl_not_imported_at_all",
                                   reinterpret_cast<void*>(&pl_test_replacement));
  PL_CHECK(status == pl::Status::NotFound);
  PL_CHECK(!hook.installed());
}

PL_TEST(a_second_hook_on_the_same_slot_is_refused) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (consumer == nullptr) return;
  auto module = consumerModule();
  if (!module.has_value()) return;

  pl::hooks::ImportHook first;
  PL_CHECK(first.install(module->base, module->size, "pl_provider_value",
                         reinterpret_cast<void*>(&pl_test_replacement)) == pl::Status::Ok);
  pl::hooks::ImportHook second;
  const auto status = second.install(module->base, module->size, "pl_provider_value",
                                     reinterpret_cast<void*>(&pl_test_replacement));
  PL_CHECK(status == pl::Status::AlreadyExists);
  PL_CHECK(!second.installed());
  PL_CHECK(first.uninstall() == pl::Status::Ok);
}

PL_TEST(a_destroyed_hook_restores_the_import) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (consumer == nullptr) return;
  auto module = consumerModule();
  if (!module.has_value()) return;
  using ConsumerSum = int (*)(int, int);
  auto sum = reinterpret_cast<ConsumerSum>(dlsym(consumer, "pl_consumer_sum"));
  PL_CHECK(sum != nullptr);
  if (sum == nullptr) return;
  PL_CHECK_EQ(sum(2, 3), 5);
  {
    pl::hooks::ImportHook hook;
    PL_CHECK(hook.install(module->base, module->size, "pl_provider_add",
                          reinterpret_cast<void*>(&pl_test_replacement)) == pl::Status::Ok);
    PL_CHECK_EQ(pl::hooks::activeImportHooks(), size_t{1});
  }
  // The destructor must not leave the slot pointing at the replacement.
  PL_CHECK_EQ(pl::hooks::activeImportHooks(), size_t{0});
  PL_CHECK_EQ(sum(2, 3), 5);
}

PL_TEST(an_absent_engine_is_reported_not_guessed) {
  // The game is not loaded in this process, so the probe must say so rather
  // than resolve something that merely looks close.
  const auto probe = pl::runtime::EngineImports::probe(
      pl::runtime::EngineImports::gameModule(), pl::runtime::EngineImports::swapBuffersSymbol());
  PL_CHECK(!probe.moduleFound);
  PL_CHECK(!probe.installed);
  PL_CHECK(probe.status == pl::Status::NotFound);
  PL_CHECK(probe.describe().find("module not loaded") != std::string::npos);
}

PL_TEST(a_present_module_reports_the_import_it_would_hook) {
  void* provider = nullptr;
  void* consumer = nullptr;
  PL_CHECK(loadPair(&provider, &consumer));
  if (consumer == nullptr) return;
  const auto probe =
      pl::runtime::EngineImports::probe("libroad_test_consumer.so", "pl_provider_value");
  PL_CHECK(probe.moduleFound);
  PL_CHECK(probe.symbolFound);
  PL_CHECK(!probe.lazy);
  PL_CHECK(!probe.installed);
  PL_CHECK(probe.slot != 0);
  PL_CHECK(probe.status == pl::Status::Ok);
  PL_CHECK(probe.describe().find("available") != std::string::npos);

  const auto missing =
      pl::runtime::EngineImports::probe("libroad_test_consumer.so", "pl_not_imported_at_all");
  PL_CHECK(missing.moduleFound);
  PL_CHECK(!missing.symbolFound);
  PL_CHECK(missing.describe().find("symbol not imported") != std::string::npos);
}

PL_TEST(the_self_test_reports_unsupported_off_device) {
  const auto probe = pl::runtime::EngineImports::selfTest();
  PL_CHECK(probe.status == pl::Status::Unsupported);
  PL_CHECK(!probe.installed);
}
