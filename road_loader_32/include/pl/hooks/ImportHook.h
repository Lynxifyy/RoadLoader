// Import (GOT) hooks: swap the pointer an imported symbol resolves to.
//
// An inline hook needs a function address, which for a stripped game means a
// signature and a guess. An import hook needs neither: the code calls through a
// pointer the linker wrote, and every build of the game has one. The trade-off
// is scope - only calls made through that slot are intercepted - which is
// exactly what an engine boundary like `eglSwapBuffers` needs.
//
// The patch is a single aligned pointer store, so a thread that is inside the
// import either sees the old target or the new one, never a mixture. Pages that
// the linker marked read-only (full RELRO) are made writable for the store and
// restored to their original protection afterwards.
#pragma once

#include <cstdint>
#include <string_view>

#include "../Result.h"

namespace pl::hooks {

class ImportHook {
 public:
  ImportHook() = default;
  ~ImportHook();

  ImportHook(const ImportHook&) = delete;
  ImportHook& operator=(const ImportHook&) = delete;
  ImportHook(ImportHook&& other) noexcept;
  ImportHook& operator=(ImportHook&& other) noexcept;

  /// Installs a detour on the GOT slot of `symbol` in the module described by
  /// `base`/`size`. On success `original()` is the address that was there.
  ///
  /// Refused when the slot is not yet bound to its provider (`ImportSlot::lazy`):
  /// the dynamic resolver would overwrite the patch the first time the import
  /// is used. Call the import once, or link the module with `-z now`.
  Status install(uintptr_t moduleBase, size_t moduleSize, std::string_view symbol,
                 void* replacement);

  /// Restores the original pointer. Safe to call more than once.
  Status uninstall();

  bool installed() const { return installed_; }
  uintptr_t slot() const { return slot_; }
  void* original() const { return reinterpret_cast<void*>(original_); }

 private:
  uintptr_t slot_ = 0;
  uintptr_t original_ = 0;
  bool installed_ = false;
};

/// Number of import hooks currently installed in this process.
size_t activeImportHooks();

}  // namespace pl::hooks
