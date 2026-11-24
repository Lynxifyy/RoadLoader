// The two modules the import tests hook into are real shared libraries built by
// this test project, so the resolver and the hook run against relocation tables
// produced by the real toolchain instead of a fixture.

extern "C" int pl_provider_value() { return 7; }

extern "C" int pl_provider_add(int left, int right) { return left + right; }
