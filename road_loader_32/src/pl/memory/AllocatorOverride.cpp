// Routes every C++ allocation through the companion's mimalloc heap (Phase 7).
//
// The shims live in the Rust core so both languages share one heap and one set
// of accounting counters. `road_alloc` stores a small header that lets
// `road_free` rebuild the original layout, which is what makes sized and
// unsized delete equivalent here.
#include <new>

#include "pl/ipc/RoadCoreAbi.h"
#include "pl/memory/AllocatorOverride.h"

namespace pl::memory {

void* cxx_allocate(std::size_t size) { return ::operator new(size); }

void cxx_release(void* pointer) noexcept { ::operator delete(pointer); }

}  // namespace pl::memory

void* operator new(std::size_t size) {
  if (size == 0) {
    size = 1;
  }
  void* pointer = road_alloc(size, alignof(std::max_align_t));
  if (pointer == nullptr) {
    throw std::bad_alloc();
  }
  return pointer;
}

void* operator new[](std::size_t size) {
  if (size == 0) {
    size = 1;
  }
  void* pointer = road_alloc(size, alignof(std::max_align_t));
  if (pointer == nullptr) {
    throw std::bad_alloc();
  }
  return pointer;
}

void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  return road_alloc(size == 0 ? 1 : size, alignof(std::max_align_t));
}

void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
  return road_alloc(size == 0 ? 1 : size, alignof(std::max_align_t));
}

void* operator new(std::size_t size, std::align_val_t alignment) {
  void* pointer = road_alloc(size == 0 ? 1 : size, static_cast<std::size_t>(alignment));
  if (pointer == nullptr) {
    throw std::bad_alloc();
  }
  return pointer;
}

void* operator new[](std::size_t size, std::align_val_t alignment) {
  void* pointer = road_alloc(size == 0 ? 1 : size, static_cast<std::size_t>(alignment));
  if (pointer == nullptr) {
    throw std::bad_alloc();
  }
  return pointer;
}

void operator delete(void* pointer) noexcept { road_free(pointer); }
void operator delete[](void* pointer) noexcept { road_free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { road_free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { road_free(pointer); }
void operator delete(void* pointer, const std::nothrow_t&) noexcept { road_free(pointer); }
void operator delete[](void* pointer, const std::nothrow_t&) noexcept { road_free(pointer); }
void operator delete(void* pointer, std::align_val_t) noexcept { road_free(pointer); }
void operator delete[](void* pointer, std::align_val_t) noexcept { road_free(pointer); }
void operator delete(void* pointer, std::size_t, std::align_val_t) noexcept { road_free(pointer); }
void operator delete[](void* pointer, std::size_t, std::align_val_t) noexcept { road_free(pointer); }
