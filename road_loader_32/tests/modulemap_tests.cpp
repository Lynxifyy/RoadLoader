#include "TestHarness.h"
#include "pl/memory/ModuleMap.h"

using pl::memory::findLoadedModule;
using pl::memory::findModule;
using pl::memory::parseMaps;

namespace {

const char* kMaps =
    "12c00000-12c20000 r-xp 00000000 fd:00 1234 /data/app/lib/arm/libminecraftpe.so\n"
    "12c20000-12c30000 r--p 00020000 fd:00 1234 /data/app/lib/arm/libminecraftpe.so\n"
    "12c30000-12c40000 rw-p 00030000 fd:00 1234 /data/app/lib/arm/libminecraftpe.so\n"
    "12d00000-12d10000 r-xp 00000000 fd:00 5678 /system/lib/libc.so\n"
    "12d10000-12d20000 r--p 00010000 fd:00 5678 /system/lib/libc.so.6\n"
    "12e00000-12e01000 rw-p 00000000 00:00 0 [anon:dalvik]\n";

}  // namespace

PL_TEST(parse_maps_reads_ranges_and_permissions) {
  const auto mappings = parseMaps(kMaps);
  PL_CHECK_EQ(mappings.size(), size_t{6});
  PL_CHECK_EQ(mappings[0].start, uintptr_t{0x12c00000});
  PL_CHECK_EQ(mappings[0].end, uintptr_t{0x12c20000});
  PL_CHECK_EQ(mappings[0].permissions, std::string("r-xp"));
  PL_CHECK_EQ(mappings[0].offset, uint64_t{0});
  PL_CHECK_EQ(mappings[0].path, std::string("/data/app/lib/arm/libminecraftpe.so"));
  PL_CHECK_EQ(mappings[5].path, std::string("[anon:dalvik]"));
}

PL_TEST(find_module_reports_base_size_and_exec_ranges) {
  const auto mappings = parseMaps(kMaps);
  const auto module = findModule(mappings, "libminecraftpe.so");
  PL_CHECK(module.has_value());
  PL_CHECK_EQ(module->base, uintptr_t{0x12c00000});
  PL_CHECK_EQ(module->size, size_t{0x40000});
  PL_CHECK_EQ(module->executable.size(), size_t{1});
  PL_CHECK_EQ(module->executable[0].start, uintptr_t{0x12c00000});
  PL_CHECK(module->valid());
}

PL_TEST(find_module_does_not_match_a_longer_suffix) {
  const auto mappings = parseMaps(kMaps);
  PL_CHECK(findModule(mappings, "libc.so").has_value());
  PL_CHECK(!findModule(mappings, "libc.so.7").has_value());
}

PL_TEST(find_module_returns_none_for_unknown_names) {
  const auto mappings = parseMaps(kMaps);
  PL_CHECK(!findModule(mappings, "libmissing.so").has_value());
}

PL_TEST(parse_maps_ignores_malformed_lines) {
  const auto mappings = parseMaps("garbage line\n\n12c00000-12c10000 r-xp 0 fd:00 1 /x\n");
  PL_CHECK_EQ(mappings.size(), size_t{1});
}

PL_TEST(real_process_map_contains_libc) {
  // On the host this exercises the real /proc/self/maps path.
  const auto libc = findLoadedModule("libc.so.6");
  PL_CHECK(libc.has_value());
  PL_CHECK(libc->valid());
}
