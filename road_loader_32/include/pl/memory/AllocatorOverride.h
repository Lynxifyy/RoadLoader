// C++ allocation entry points that share the companion's mimalloc heap.
//
// The global `operator new`/`delete` replacements in AllocatorOverride.cpp call
// these. They are also the stable way to exercise the overrides: a call made
// from the defining translation unit binds directly, while a `new` expression in
// another binary may bind to libstdc++ through the PLT on some toolchains.
#pragma once

#include <cstddef>

namespace pl::memory {

void* cxx_allocate(std::size_t size);
void cxx_release(void* pointer) noexcept;

}  // namespace pl::memory
