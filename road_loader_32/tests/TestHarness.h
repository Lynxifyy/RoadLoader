// Tiny dependency-free test harness for the host-side C++ tests.
#pragma once

#include <functional>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace pl::test {

struct TestCase {
  const char* name;
  void (*function)();
};

std::vector<TestCase>& registry();
void reportFailure(const char* file, int line, const std::string& message);
int failureCount();
int runAll();

struct Registrar {
  Registrar(const char* name, void (*function)()) { registry().push_back({name, function}); }
};

template <typename A, typename B>
std::string describe(const char* expression, const A& actual, const B& expected) {
  std::ostringstream stream;
  stream << expression << " (actual=" << actual << ", expected=" << expected << ")";
  return stream.str();
}

}  // namespace pl::test

#define PL_TEST(name)                                              \
  static void name();                                              \
  static ::pl::test::Registrar pl_registrar_##name(#name, name);   \
  static void name()

#define PL_CHECK(expression)                                                     \
  do {                                                                           \
    if (!(expression)) {                                                         \
      ::pl::test::reportFailure(__FILE__, __LINE__, "CHECK failed: " #expression); \
    }                                                                            \
  } while (0)

#define PL_CHECK_EQ(actual, expected)                                              \
  do {                                                                             \
    const auto& pl_actual = (actual);                                              \
    const auto& pl_expected = (expected);                                          \
    if (!(pl_actual == pl_expected)) {                                             \
      ::pl::test::reportFailure(__FILE__, __LINE__,                                \
                                ::pl::test::describe(#actual, pl_actual, pl_expected)); \
    }                                                                              \
  } while (0)
