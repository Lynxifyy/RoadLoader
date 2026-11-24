// Reads the imports of a *loaded* module out of its dynamic section.
//
// This is what makes a version-independent hook possible: a game that wants
// EGL must import `eglSwapBuffers`, and the import lives in a writable slot the
// linker filled in. Finding that slot needs no signature, no offset and no
// knowledge of the build - only the module's own relocation tables.
#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace pl::memory {

/// One relocated import: a GOT slot and the symbol it points at.
struct ImportSlot {
  std::string name;
  /// Address of the pointer the code calls through.
  uintptr_t slot = 0;
  /// Value currently stored there. Equal to the resolved function once the
  /// relocation has run; a PLT resolver stub before that on lazy-binding ABIs.
  uintptr_t target = 0;
  /// R_*_JUMP_SLOT (PLT) or R_*_GLOB_DAT (data reference).
  uint32_t relocType = 0;
  bool lazy = false;
};

/// Imports of one module, in relocation-table order.
struct ImportTable {
  uintptr_t base = 0;
  std::string path;
  std::vector<ImportSlot> slots;

  bool valid() const { return base != 0 && !slots.empty(); }
  const ImportSlot* find(std::string_view symbol) const;
  /// Number of distinct symbols whose slot resolves to executable code.
  size_t resolved() const;
};

/// Parses the dynamic section at `base`. `size` bounds every read; pass 0 to
/// skip the bound (the caller already validated the mapping).
///
/// Fails closed: a malformed or absent dynamic section returns nullopt rather
/// than a partially filled table.
std::optional<ImportTable> readImportsAt(uintptr_t base, size_t size = 0);

/// Convenience: finds a module by name in `/proc/self/maps` and reads its
/// imports. `name` is matched as a path suffix.
std::optional<ImportTable> readModuleImports(std::string_view name);

/// True when `address` lies inside an executable mapping of this process.
/// Used to tell a resolved import from a not-yet-bound PLT stub.
bool isExecutableAddress(uintptr_t address);

}  // namespace pl::memory
