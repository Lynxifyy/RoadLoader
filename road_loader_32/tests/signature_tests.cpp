#include "TestHarness.h"
#include "pl/memory/Signature.h"

using pl::memory::SignatureScanner;
using pl::memory::parsePattern;

namespace {

const uint8_t kBuffer[] = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
                           0x11, 0x22, 0x99, 0x33, 0x44, 0x55, 0x00, 0x00};

}  // namespace

PL_TEST(parse_ida_pattern_with_wildcards) {
  const auto pattern = parsePattern("11 22 ?? 33 ? 44");
  PL_CHECK(pattern.has_value());
  PL_CHECK_EQ(pattern->size(), size_t{6});
  PL_CHECK_EQ(pattern->bytes[0], uint8_t{0x11});
  PL_CHECK_EQ(pattern->mask[2], uint8_t{0});
  PL_CHECK_EQ(pattern->mask[4], uint8_t{0});
  PL_CHECK_EQ(pattern->mask[5], uint8_t{1});
}

PL_TEST(parse_escaped_c_pattern) {
  const auto pattern = parsePattern("\\x11\\x22\\x??\\x33");
  PL_CHECK(pattern.has_value());
  PL_CHECK_EQ(pattern->size(), size_t{4});
  PL_CHECK_EQ(pattern->mask[2], uint8_t{0});
}

PL_TEST(parse_rejects_malformed_patterns) {
  PL_CHECK(!parsePattern("").has_value());
  PL_CHECK(!parsePattern("1").has_value());
  PL_CHECK(!parsePattern("ZZ").has_value());
  PL_CHECK(!parsePattern("\\x1").has_value());
  PL_CHECK(!parsePattern("hello").has_value());
}

PL_TEST(find_returns_first_match) {
  const auto pattern = parsePattern("11 22 ?? 33");
  PL_CHECK(pattern.has_value());
  const auto match = SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern);
  PL_CHECK(match.has_value());
  PL_CHECK_EQ(*match - reinterpret_cast<uintptr_t>(kBuffer), uintptr_t{8});
}

PL_TEST(find_handles_wildcard_prefix) {
  const auto pattern = parsePattern("?? 44 55");
  PL_CHECK(pattern.has_value());
  const auto match = SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern);
  PL_CHECK(match.has_value());
  PL_CHECK_EQ(*match - reinterpret_cast<uintptr_t>(kBuffer), uintptr_t{3});
}

PL_TEST(find_returns_none_when_absent) {
  const auto pattern = parsePattern("DE AD BE EF");
  PL_CHECK(pattern.has_value());
  PL_CHECK(!SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern).has_value());
}

PL_TEST(find_all_respects_limit) {
  const auto pattern = parsePattern("11 22");
  PL_CHECK(pattern.has_value());
  const auto matches = SignatureScanner::findAll(kBuffer, sizeof(kBuffer), *pattern, 4);
  PL_CHECK_EQ(matches.size(), size_t{2});
  const auto limited = SignatureScanner::findAll(kBuffer, sizeof(kBuffer), *pattern, 1);
  PL_CHECK_EQ(limited.size(), size_t{1});
}

PL_TEST(find_rejects_pattern_larger_than_buffer) {
  const auto pattern = parsePattern("11 22 33 44 55 66 77 88 99 AA BB CC DD EE FF 00 11 22");
  PL_CHECK(pattern.has_value());
  PL_CHECK(!SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern).has_value());
}

PL_TEST(find_does_not_read_past_the_buffer) {
  const auto pattern = parsePattern("55 00 00 00 00");
  PL_CHECK(pattern.has_value());
  PL_CHECK(!SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern).has_value());
}

PL_TEST(all_wildcard_pattern_matches_at_zero) {
  const auto pattern = parsePattern("?? ??");
  PL_CHECK(pattern.has_value());
  const auto match = SignatureScanner::find(kBuffer, sizeof(kBuffer), *pattern);
  PL_CHECK(match.has_value());
  PL_CHECK_EQ(*match, reinterpret_cast<uintptr_t>(kBuffer));
}
