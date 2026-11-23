#include <cstdint>
#include <vector>

#include "TestHarness.h"
#include "pl/ipc/RoadCoreAbi.h"

namespace {

RoadAllocStats readStats() {
  RoadAllocStats stats{};
  road_alloc_stats(&stats);
  return stats;
}

}  // namespace

PL_TEST(allocator_shims_route_through_the_core) {
  const auto before = readStats();
  void* pointer = road_alloc(4096, 64);
  PL_CHECK(pointer != nullptr);
  PL_CHECK_EQ(reinterpret_cast<uintptr_t>(pointer) % 64, uintptr_t{0});
  PL_CHECK(road_alloc_usable_size(pointer) >= 4096);
  const auto during = readStats();
  PL_CHECK(during.live_bytes >= before.live_bytes + 4096);
  road_free(pointer);
  const auto after = readStats();
  PL_CHECK(after.live_bytes < during.live_bytes);
}

PL_TEST(calloc_zeroes_memory) {
  auto* bytes = static_cast<uint8_t*>(road_calloc(1024, 16));
  PL_CHECK(bytes != nullptr);
  for (size_t index = 0; index < 1024; ++index) {
    PL_CHECK_EQ(bytes[index], uint8_t{0});
  }
  road_free(bytes);
}

PL_TEST(realloc_preserves_contents) {
  auto* bytes = static_cast<uint8_t*>(road_alloc(256, 16));
  PL_CHECK(bytes != nullptr);
  for (size_t index = 0; index < 256; ++index) {
    bytes[index] = static_cast<uint8_t>(index);
  }
  auto* resized = static_cast<uint8_t*>(road_realloc(bytes, 8192, 16));
  PL_CHECK(resized != nullptr);
  for (size_t index = 0; index < 256; ++index) {
    PL_CHECK_EQ(resized[index], static_cast<uint8_t>(index));
  }
  PL_CHECK(road_alloc_usable_size(resized) >= 8192);
  road_free(resized);
}

PL_TEST(cpp_new_and_delete_use_the_shared_heap) {
  const auto before = readStats();
  auto* values = new uint64_t[1024];
  PL_CHECK(values != nullptr);
  values[0] = 0xDEADBEEF;
  const auto during = readStats();
  PL_CHECK(during.live_bytes >= before.live_bytes + 8192);
  PL_CHECK_EQ(values[0], uint64_t{0xDEADBEEF});
  delete[] values;
  PL_CHECK(readStats().live_bytes < during.live_bytes);
}

PL_TEST(std_vector_uses_the_shared_heap) {
  const auto before = readStats();
  {
    std::vector<uint8_t> buffer(1 << 20, 0x5A);
    PL_CHECK_EQ(buffer.front(), uint8_t{0x5A});
    PL_CHECK(readStats().live_bytes >= before.live_bytes + (1 << 20));
  }
  PL_CHECK(readStats().live_bytes < before.live_bytes + (1 << 20));
}

PL_TEST(heap_limit_is_configurable) {
  const auto before = readStats();
  road_set_heap_limit(2ull * 1024 * 1024 * 1024);
  const auto raised = readStats();
  PL_CHECK_EQ(raised.limit_bytes, uint64_t{2} * 1024 * 1024 * 1024);
  road_set_heap_limit(0);  // restores the 1.5 GiB default
  const auto restored = readStats();
  PL_CHECK_EQ(restored.limit_bytes, uint64_t{1536} * 1024 * 1024);
  PL_CHECK(before.limit_bytes != 0);
}
