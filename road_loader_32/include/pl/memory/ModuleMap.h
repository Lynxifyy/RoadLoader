// Reads the process module map from /proc/self/maps and locates loaded
// libraries, mirroring the module bookkeeping SweetLoader needs before it can
// scan `libminecraftpe.so`.
#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace pl::memory {

struct Mapping {
  uintptr_t start = 0;
  uintptr_t end = 0;
  std::string permissions;
  uint64_t offset = 0;
  std::string path;
};

struct ModuleInfo {
  std::string path;
  uintptr_t base = 0;
  size_t size = 0;
  /// Executable ranges of the module, used to bound signature scans.
  std::vector<Mapping> executable;

  bool valid() const { return base != 0 && size != 0; }
  uintptr_t end() const { return base + size; }
};

/// Parses a `/proc/self/maps` document. Exposed for tests.
std::vector<Mapping> parseMaps(std::string_view contents);

/// Finds a module whose path ends with `name` inside the given mappings.
std::optional<ModuleInfo> findModule(const std::vector<Mapping>& mappings, std::string_view name);

/// Convenience: parses the real `/proc/self/maps` and finds `name`.
std::optional<ModuleInfo> findLoadedModule(std::string_view name);

}  // namespace pl::memory
