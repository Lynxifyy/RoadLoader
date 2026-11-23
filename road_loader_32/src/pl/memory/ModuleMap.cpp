#include "pl/memory/ModuleMap.h"

#include <algorithm>
#include <charconv>
#include <fstream>
#include <sstream>

namespace pl::memory {
namespace {

bool parseHex(std::string_view text, uint64_t* out) {
  if (text.empty()) {
    return false;
  }
  uint64_t value = 0;
  const auto result = std::from_chars(text.data(), text.data() + text.size(), value, 16);
  if (result.ec != std::errc() || result.ptr != text.data() + text.size()) {
    return false;
  }
  *out = value;
  return true;
}

bool endsWith(std::string_view value, std::string_view suffix) {
  return value.size() >= suffix.size() &&
         value.substr(value.size() - suffix.size()) == suffix;
}

}  // namespace

std::vector<Mapping> parseMaps(std::string_view contents) {
  std::vector<Mapping> mappings;
  size_t position = 0;
  while (position < contents.size()) {
    const size_t newline = contents.find('\n', position);
    const size_t end = newline == std::string_view::npos ? contents.size() : newline;
    std::string_view line = contents.substr(position, end - position);
    position = end + 1;
    if (line.empty()) {
      continue;
    }
    std::istringstream stream{std::string(line)};
    std::string range;
    std::string permissions;
    std::string offsetText;
    std::string device;
    std::string inode;
    if (!(stream >> range >> permissions >> offsetText >> device >> inode)) {
      continue;
    }
    const size_t dash = range.find('-');
    if (dash == std::string::npos) {
      continue;
    }
    uint64_t start = 0;
    uint64_t stop = 0;
    uint64_t offset = 0;
    if (!parseHex(std::string_view(range).substr(0, dash), &start) ||
        !parseHex(std::string_view(range).substr(dash + 1), &stop) ||
        !parseHex(offsetText, &offset)) {
      continue;
    }
    Mapping mapping;
    mapping.start = static_cast<uintptr_t>(start);
    mapping.end = static_cast<uintptr_t>(stop);
    mapping.permissions = permissions;
    mapping.offset = offset;
    std::string path;
    std::getline(stream, path);
    const size_t first = path.find_first_not_of(' ');
    if (first != std::string::npos) {
      path.erase(0, first);
    } else {
      path.clear();
    }
    mapping.path = path;
    mappings.push_back(std::move(mapping));
  }
  return mappings;
}

std::optional<ModuleInfo> findModule(const std::vector<Mapping>& mappings, std::string_view name) {
  ModuleInfo info;
  uintptr_t lowest = UINTPTR_MAX;
  uintptr_t highest = 0;
  for (const auto& mapping : mappings) {
    if (mapping.path.empty() || !endsWith(mapping.path, name)) {
      continue;
    }
    // Avoid matching "libfoo.so" inside "libfoo.so.1".
    if (mapping.path.size() > name.size()) {
      const char previous = mapping.path[mapping.path.size() - name.size() - 1];
      if (previous != '/' && previous != ' ') {
        continue;
      }
    }
    if (info.path.empty()) {
      info.path = mapping.path;
    }
    lowest = std::min(lowest, mapping.start);
    highest = std::max(highest, mapping.end);
    if (mapping.permissions.find('x') != std::string::npos) {
      info.executable.push_back(mapping);
    }
  }
  if (lowest == UINTPTR_MAX) {
    return std::nullopt;
  }
  info.base = lowest;
  info.size = highest - lowest;
  return info;
}

std::optional<ModuleInfo> findLoadedModule(std::string_view name) {
  std::ifstream maps("/proc/self/maps");
  if (!maps) {
    return std::nullopt;
  }
  std::stringstream buffer;
  buffer << maps.rdbuf();
  const auto contents = buffer.str();
  return findModule(parseMaps(contents), name);
}

}  // namespace pl::memory
