#include <iostream>

#include "TestHarness.h"

namespace pl::test {
namespace {
std::vector<TestCase> g_registry;
int g_failures = 0;
}  // namespace

std::vector<TestCase>& registry() { return g_registry; }

void reportFailure(const char* file, int line, const std::string& message) {
  ++g_failures;
  std::cerr << file << ":" << line << ": " << message << "\n";
}

int failureCount() { return g_failures; }

int runAll() {
  for (const auto& test : g_registry) {
    const int before = g_failures;
    std::cout << "[ RUN  ] " << test.name << "\n";
    test.function();
    std::cout << (g_failures == before ? "[  OK  ] " : "[ FAIL ] ") << test.name << "\n";
  }
  return g_failures;
}

}  // namespace pl::test

int main() {
  const int failures = pl::test::runAll();
  std::cout << (failures == 0 ? "ALL TESTS PASSED" : "TESTS FAILED") << " ("
            << pl::test::registry().size() << " cases, " << failures << " failures)\n";
  return failures == 0 ? 0 : 1;
}
