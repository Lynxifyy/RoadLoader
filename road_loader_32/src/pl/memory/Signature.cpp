#include "pl/memory/Signature.h"

#include <algorithm>
#include <cctype>
#include <cstring>

namespace pl::memory {
namespace {

bool isHexDigit(char value) {
  return (value >= '0' && value <= '9') || (value >= 'a' && value <= 'f') ||
         (value >= 'A' && value <= 'F');
}

int hexValue(char value) {
  if (value >= '0' && value <= '9') {
    return value - '0';
  }
  if (value >= 'a' && value <= 'f') {
    return value - 'a' + 10;
  }
  if (value >= 'A' && value <= 'F') {
    return value - 'A' + 10;
  }
  return -1;
}

bool isSeparator(char value) {
  return value == ' ' || value == '\t' || value == '\n' || value == '\r';
}

}  // namespace

std::optional<Pattern> parsePattern(const std::string& text) {
  Pattern pattern;
  pattern.text = text;
  size_t index = 0;
  while (index < text.size()) {
    while (index < text.size() && isSeparator(text[index])) {
      ++index;
    }
    if (index >= text.size()) {
      break;
    }
    if (text[index] == '\\') {
      // Escaped C form: \xAB, \x??
      if (index + 3 < text.size() && (text[index + 1] == 'x' || text[index + 1] == 'X')) {
        const char high = text[index + 2];
        const char low = text[index + 3];
        if (high == '?' && low == '?') {
          pattern.bytes.push_back(0);
          pattern.mask.push_back(0);
        } else if (isHexDigit(high) && isHexDigit(low)) {
          pattern.bytes.push_back(static_cast<uint8_t>(hexValue(high) * 16 + hexValue(low)));
          pattern.mask.push_back(1);
        } else {
          return std::nullopt;
        }
        index += 4;
        continue;
      }
      return std::nullopt;
    }
    if (text[index] == '?' || text[index] == '*') {
      // "??" or "?" both mean one wildcard byte.
      pattern.bytes.push_back(0);
      pattern.mask.push_back(0);
      ++index;
      if (index < text.size() && text[index] == '?') {
        ++index;
      }
      continue;
    }
    if (isHexDigit(text[index])) {
      // Accept both "48" and "4 8" spellings of a byte.
      const char high = text[index];
      if (index + 1 >= text.size() || !isHexDigit(text[index + 1])) {
        return std::nullopt;
      }
      const char low = text[index + 1];
      pattern.bytes.push_back(static_cast<uint8_t>(hexValue(high) * 16 + hexValue(low)));
      pattern.mask.push_back(1);
      index += 2;
      continue;
    }
    return std::nullopt;
  }
  if (pattern.bytes.empty()) {
    return std::nullopt;
  }
  return pattern;
}

std::optional<uintptr_t> SignatureScanner::find(const uint8_t* begin, size_t size,
                                                const Pattern& pattern) {
  if (begin == nullptr || pattern.empty() || pattern.size() > size) {
    return std::nullopt;
  }
  // Anchor on the first concrete byte; wildcard prefixes are matched by the
  // bounded loop below.
  size_t anchor = 0;
  while (anchor < pattern.size() && pattern.mask[anchor] == 0) {
    ++anchor;
  }
  const size_t limit = size - pattern.size();
  if (anchor >= pattern.size()) {
    // All-wildcard pattern: every offset matches.
    return reinterpret_cast<uintptr_t>(begin);
  }
  const uint8_t anchorByte = pattern.bytes[anchor];
  for (size_t offset = 0; offset <= limit;) {
    const uint8_t* candidate = begin + offset;
    const uint8_t* found = static_cast<const uint8_t*>(
        std::memchr(candidate + anchor, anchorByte, limit - offset - anchor + 1));
    if (found == nullptr) {
      return std::nullopt;
    }
    const size_t matchOffset = static_cast<size_t>(found - begin) - anchor;
    bool matched = true;
    for (size_t index = 0; index < pattern.size(); ++index) {
      if (pattern.mask[index] != 0 && begin[matchOffset + index] != pattern.bytes[index]) {
        matched = false;
        break;
      }
    }
    if (matched) {
      return reinterpret_cast<uintptr_t>(begin + matchOffset);
    }
    offset = matchOffset + 1;
  }
  return std::nullopt;
}

std::vector<uintptr_t> SignatureScanner::findAll(const uint8_t* begin, size_t size,
                                                 const Pattern& pattern, size_t limit) {
  std::vector<uintptr_t> matches;
  if (begin == nullptr || pattern.empty() || pattern.size() > size || limit == 0) {
    return matches;
  }
  size_t offset = 0;
  while (offset <= size - pattern.size() && matches.size() < limit) {
    const auto match = find(begin + offset, size - offset, pattern);
    if (!match.has_value()) {
      break;
    }
    const auto address = *match;
    matches.push_back(address);
    offset = static_cast<size_t>(address - reinterpret_cast<uintptr_t>(begin)) + 1;
  }
  return matches;
}

}  // namespace pl::memory
