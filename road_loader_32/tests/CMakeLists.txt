cmake_minimum_required(VERSION 3.22)
project(road_host_tests CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

if(NOT ROAD_CORE_LIB_DIR)
  set(ROAD_CORE_LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../rust/target/release")
endif()
if(NOT EXISTS "${ROAD_CORE_LIB_DIR}/libroad_core_32.so")
  message(FATAL_ERROR "Build the host core first: (cd rust && cargo build --release)")
endif()

set(ROAD_SOURCES
  ../src/pl/memory/Signature.cpp
  ../src/pl/memory/ModuleMap.cpp
  ../src/pl/memory/AllocatorOverride.cpp
  ../src/pl/hooks/ArmRelocator.cpp
  ../src/pl/hooks/InlineHook.cpp
  ../src/pl/ipc/SharedRegion.cpp
  ../src/pl/runtime/GameHooks.cpp)

add_executable(road_host_tests
  TestMain.cpp
  signature_tests.cpp
  modulemap_tests.cpp
  relocator_tests.cpp
  hook_tests.cpp
  ipc_tests.cpp
  allocator_tests.cpp
  abi_tests.cpp
  ${ROAD_SOURCES})

target_include_directories(road_host_tests PRIVATE ../include)
target_compile_options(road_host_tests PRIVATE -Wall -Wextra -Wpedantic)
# The allocator test replaces operator new/delete. Without -rdynamic those
# definitions stay out of the dynamic symbol table and the PLT resolves to
# libstdc++ instead, so the test would silently measure the wrong allocator.
target_link_options(road_host_tests PRIVATE -rdynamic)
target_link_libraries(road_host_tests PRIVATE
  "${ROAD_CORE_LIB_DIR}/libroad_core_32.so"
  dl
  pthread)
set_target_properties(road_host_tests PROPERTIES BUILD_RPATH "${ROAD_CORE_LIB_DIR}")

enable_testing()
add_test(NAME road_host_tests COMMAND road_host_tests)
