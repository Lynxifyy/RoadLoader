cmake_minimum_required(VERSION 3.22.1)
project(road_loader_32 CXX C)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# The Rust core is built by scripts/build_rust.sh and staged into jniLibs so the
# same artifact is packaged in the APK. CMake links against that exact file.
set(ROAD_CORE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../road-loader-core/src/main/jniLibs/${ANDROID_ABI}")
if(NOT EXISTS "${ROAD_CORE_DIR}/libroad_core_32.so")
  message(FATAL_ERROR
    "libroad_core_32.so missing for ${ANDROID_ABI}.\n"
    "Run scripts/build_rust.sh before configuring the native build.")
endif()
add_library(road_core_32 SHARED IMPORTED)
set_target_properties(road_core_32 PROPERTIES IMPORTED_LOCATION
  "${ROAD_CORE_DIR}/libroad_core_32.so")

add_library(roadloader32 SHARED
  src/jni/JniBridge32.cpp
  src/pl/ipc/SharedRegion.cpp
  src/pl/memory/Signature.cpp
  src/pl/memory/ModuleMap.cpp
  src/pl/memory/ImportResolver.cpp
  src/pl/memory/AllocatorOverride.cpp
  src/pl/hooks/ArmRelocator.cpp
  src/pl/hooks/InlineHook.cpp
  src/pl/hooks/ImportHook.cpp
  src/pl/hooks/PrologueVerify.cpp
  src/pl/runtime/GameHooks.cpp
  src/pl/runtime/EngineImports.cpp)

target_include_directories(roadloader32 PRIVATE include)
target_compile_definitions(roadloader32 PRIVATE PL_ENABLE_GL=1)
target_compile_options(roadloader32 PRIVATE
  -Wall -Wextra -Wpedantic
  -fvisibility=hidden
  -fstack-protector-strong)
target_link_options(roadloader32 PRIVATE -Wl,--no-undefined -Wl,-z,relro -Wl,-z,now)

target_link_libraries(roadloader32 PRIVATE
  road_core_32
  android
  log
  GLESv3
  dl)

# Keep the exported JNI surface small and explicit.
target_link_options(roadloader32 PRIVATE
  -Wl,--version-script=${CMAKE_CURRENT_SOURCE_DIR}/roadloader32.map)
