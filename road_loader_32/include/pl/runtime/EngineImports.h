// Engine-facing import probes.
//
// The 32-bit game must import `eglSwapBuffers` to present anything, so that
// import - not a signature - is the frame-capture anchor. This layer reports
// what it found and installs the hook, keeping the failure modes apart: engine
// absent, symbol absent, slot not yet bound, hook installed.
#pragma once

#include <atomic>
#include <cstdint>
#include <string>

#include "../Result.h"

namespace pl::runtime {

struct ImportProbe {
  std::string module;
  std::string symbol;
  bool moduleFound = false;
  bool symbolFound = false;
  bool lazy = false;
  bool installed = false;
  bool fired = false;
  bool restored = false;
  uintptr_t slot = 0;
  uintptr_t target = 0;
  Status status = Status::NotFound;

  /// One line for the UI and the logs. Never claims more than the flags say.
  std::string describe() const;
};

class EngineImports {
 public:
  /// Name of the engine module the companion looks for.
  static const char* gameModule();
  /// The import the frame-capture hook is installed on.
  static const char* swapBuffersSymbol();

  /// Reads the import table without touching it.
  static ImportProbe probe(const char* module, const char* symbol);

  /// Installs `replacement` on the import. Refused, with the reason recorded in
  /// `status`, when the module is absent, the symbol is missing, or the slot is
  /// still pointing at the lazy resolver.
  static ImportProbe install(const char* module, const char* symbol, void* replacement);

  /// Paths from `/proc/self/maps` that contain `filter`, one per line, for the
  /// developer overlay. Empty when the maps file cannot be read.
  static std::string loadedModules(std::string_view filter, size_t limit = 24);

  /// One compact line explaining how a module name matched, or did not.
  static std::string moduleDebug(const char* name);

  /// Proves the mechanism inside the running process: installs a counting hook
  /// on a symbol this library really imports, calls it, checks the counter, then
  /// restores the original and calls again. `module`/`symbol` default to this
  /// library and `__android_log_print`.
  static ImportProbe selfTest(const char* module = nullptr, const char* symbol = nullptr);

  /// Counter of self-test hook invocations, exposed for the JNI report.
  static uint64_t selfTestCalls();
};

}  // namespace pl::runtime
