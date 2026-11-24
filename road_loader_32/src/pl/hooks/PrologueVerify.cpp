#include "pl/hooks/PrologueVerify.h"

namespace pl::hooks {

PrologueCheck verifyPrologue(const uint8_t* target, const uint8_t* expected, const uint8_t* mask,
                             size_t count) {
  PrologueCheck check;
  check.compared = count;
  if (count == 0 || expected == nullptr) {
    check.reason = "no expected prologue";
    return check;
  }
  if (target == nullptr) {
    check.reason = "null target";
    return check;
  }
  for (size_t index = 0; index < count; ++index) {
    const uint8_t required = mask != nullptr ? mask[index] : static_cast<uint8_t>(0xFF);
    if (required != 0 && (target[index] & required) != (expected[index] & required)) {
      check.mismatchOffset = index;
      check.reason = "prologue mismatch";
      return check;
    }
  }
  check.ok = true;
  return check;
}

}  // namespace pl::hooks
