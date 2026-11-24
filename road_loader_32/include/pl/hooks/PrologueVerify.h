// Prologue verification before an inline hook is installed.
//
// A pattern that no longer matches the binary would patch the wrong function,
// so the expected first bytes are compared with the live ones first. Modelled
// on SweetLoader's `include/pl/memory/PrologueVerify.hpp`: a mismatch is a
// refusal, never a warning.
#pragma once

#include <cstddef>
#include <cstdint>

namespace pl::hooks {

struct PrologueCheck {
  /// True only when every masked byte matched.
  bool ok = false;
  /// Number of bytes compared (the expectation length).
  size_t compared = 0;
  /// Offset of the first difference when `ok` is false.
  size_t mismatchOffset = 0;
  /// Why the check failed, for logs.
  const char* reason = "";
};

/// Compares `count` bytes at `target` with `expected`. `mask` may be null (all
/// bytes must match) or one byte per position, where 0xFF requires equality and
/// 0x00 ignores the byte. An empty expectation is a failure, not a pass.
PrologueCheck verifyPrologue(const uint8_t* target, const uint8_t* expected, const uint8_t* mask,
                             size_t count);

}  // namespace pl::hooks
