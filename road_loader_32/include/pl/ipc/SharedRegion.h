// RAII wrapper over a host-side shared region.
#pragma once

#include <cstdint>
#include <memory>

#include "../Result.h"
#include "RoadCoreAbi.h"

namespace pl::ipc {

/// Owns the shared-memory region created for one companion session.
class SharedRegion {
 public:
  static std::unique_ptr<SharedRegion> create(uint32_t width, uint32_t height);

  ~SharedRegion();

  SharedRegion(const SharedRegion&) = delete;
  SharedRegion& operator=(const SharedRegion&) = delete;

  void* handle() const { return handle_; }
  int duplicateFd() const;
  uint64_t size() const;
  bool pollFrame(RoadFrameInfo* out);
  void releaseFrame(uint32_t slot);
  bool pushInput(const RoadInputEvent& event);
  bool pushCommand(uint32_t kind, uint64_t value);
  bool stats(RoadStats* out) const;

  /// Verifies the runtime ABI sizes reported by the core.
  static Status checkAbi();

 private:
  explicit SharedRegion(void* handle) : handle_(handle) {}
  void* handle_ = nullptr;
};

}  // namespace pl::ipc
