// Byte-pattern scanning, mirroring SweetLoader's `src/pl/memory/Signature.cpp`.
//
// A pattern is a byte sequence where every byte is either an exact value or a
// wildcard. Both IDA syntax ("48 8B ?? ? ?") and escaped C syntax
// ("\x48\x8B") are accepted.
#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace pl::memory {

struct Pattern {
  std::vector<uint8_t> bytes;
  /// 1 = byte must match, 0 = wildcard.
  std::vector<uint8_t> mask;
  /// Human-readable form, used in logs and diagnostics.
  std::string text;

  size_t size() const { return bytes.size(); }
  bool empty() const { return bytes.empty(); }
};

/// Parses an IDA-style or escaped-C pattern. Returns nullopt on malformed input.
std::optional<Pattern> parsePattern(const std::string& text);

class SignatureScanner {
 public:
  /// Finds the first match in `[begin, begin + size)`.
  static std::optional<uintptr_t> find(const uint8_t* begin, size_t size, const Pattern& pattern);

  /// Finds up to `limit` matches.
  static std::vector<uintptr_t> findAll(const uint8_t* begin, size_t size, const Pattern& pattern,
                                        size_t limit);

  /// Convenience overload over an address range.
  static std::optional<uintptr_t> find(uintptr_t begin, size_t size, const Pattern& pattern) {
    return find(reinterpret_cast<const uint8_t*>(begin), size, pattern);
  }
};

}  // namespace pl::memory
