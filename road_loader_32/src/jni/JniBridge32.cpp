// JNI bridge for both sides of Road Loader.
//
// The same shared object is loaded in the 64-bit UI process (host role) and in
// the 32-bit companion process (companion role). It is responsible for loading
// `libroad_core_32.so`, validating the C ABI once, and marshalling the few
// primitives Kotlin needs.
#include <jni.h>

#include <dlfcn.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "pl/Log.h"
#include "pl/ipc/RoadCoreAbi.h"
#include "pl/memory/Signature.h"
#include "pl/runtime/EngineImports.h"

namespace {

using CreateSessionFn = void* (*)(uint32_t, uint32_t);
using CompanionAttachFn = int (*)(int, size_t, uint32_t, uint64_t);

void* g_coreLibrary = nullptr;
std::string g_coreStatus = "not loaded";
CreateSessionFn g_createSession = nullptr;
CompanionAttachFn g_companionAttach = nullptr;

bool ensureCore() {
  if (g_coreLibrary != nullptr) {
    return true;
  }
  dlerror();
  g_coreLibrary = dlopen("libroad_core_32.so", RTLD_NOW | RTLD_LOCAL);
  if (g_coreLibrary == nullptr) {
    const char* error = dlerror();
    g_coreStatus = std::string("dlopen failed: ") + (error != nullptr ? error : "unknown");
    PL_LOGE("%s", g_coreStatus.c_str());
    return false;
  }
  g_createSession = reinterpret_cast<CreateSessionFn>(dlsym(g_coreLibrary, "road_host_create"));
  g_companionAttach =
      reinterpret_cast<CompanionAttachFn>(dlsym(g_coreLibrary, "road_companion_attach"));
  const auto version = reinterpret_cast<uint32_t (*)()>(dlsym(g_coreLibrary, "road_core_version"));
  if (g_createSession == nullptr || g_companionAttach == nullptr || version == nullptr) {
    g_coreStatus = "dlopen succeeded but required symbols are missing";
    PL_LOGE("%s", g_coreStatus.c_str());
    dlclose(g_coreLibrary);
    g_coreLibrary = nullptr;
    return false;
  }
  char buffer[96];
  std::snprintf(buffer, sizeof(buffer), "loaded core %u.%u", version() >> 16, version() & 0xFFFF);
  g_coreStatus = buffer;
  PL_LOGI("%s", g_coreStatus.c_str());
  return true;
}

void* sessionFromHandle(jlong handle) { return reinterpret_cast<void*>(static_cast<intptr_t>(handle)); }

std::string toString(JNIEnv* env, jstring value) {
  if (value == nullptr) {
    return {};
  }
  const char* chars = env->GetStringUTFChars(value, nullptr);
  if (chars == nullptr) {
    return {};
  }
  std::string result(chars);
  env->ReleaseStringUTFChars(value, chars);
  return result;
}

void fillStats(JNIEnv* env, jlongArray out, const RoadStats& stats) {
  jlong values[35] = {0};
  values[0] = stats.status;
  values[1] = stats.flags;
  values[2] = stats.pid;
  values[3] = stats.width;
  values[4] = stats.height;
  values[5] = stats.heartbeat_seq;
  values[6] = stats.heartbeat_age_ms;
  values[7] = static_cast<jlong>(stats.frame_count);
  values[8] = static_cast<jlong>(stats.presented_count);
  values[9] = stats.dropped_frames;
  values[10] = stats.input_pushed;
  values[11] = stats.input_consumed;
  values[12] = stats.input_dropped;
  values[13] = stats.cmd_pushed;
  values[14] = stats.cmd_consumed;
  values[15] = stats.event_pushed;
  values[16] = stats.event_consumed;
  values[17] = static_cast<jlong>(stats.heap_used);
  values[18] = static_cast<jlong>(stats.heap_peak);
  values[19] = static_cast<jlong>(stats.heap_limit);
  values[20] = stats.render_us_avg;
  values[21] = stats.render_us_max;
  values[22] = stats.self_test;
  values[23] = stats.crash_signal;
  values[24] = static_cast<jlong>(stats.crash_addr);
  values[25] = stats.last_input_seq;
  values[26] = stats.last_input_x;
  values[27] = stats.last_input_y;
  values[28] = stats.audio_pushed;
  values[29] = stats.audio_consumed;
  values[30] = stats.audio_dropped;
  values[31] = stats.last_input_kind;
  values[32] = stats.asset_bytes;
  values[33] = stats.asset_crc;
  values[34] = stats.asset_status;
  env->SetLongArrayRegion(out, 0, 35, values);
}

struct ElfSummary {
  bool valid = false;
  bool is64 = false;
  uint16_t machine = 0;
  uint64_t entry = 0;
  uint64_t size = 0;
  std::string error;
};

ElfSummary inspectElf(const std::string& path) {
  ElfSummary summary;
  const int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC);
  if (fd < 0) {
    summary.error = "open failed";
    return summary;
  }
  struct stat statBuffer {};
  if (fstat(fd, &statBuffer) != 0) {
    summary.error = "fstat failed";
    close(fd);
    return summary;
  }
  summary.size = static_cast<uint64_t>(statBuffer.st_size);
  if (statBuffer.st_size < 52) {
    summary.error = "file too small";
    close(fd);
    return summary;
  }
  void* mapping = mmap(nullptr, static_cast<size_t>(statBuffer.st_size), PROT_READ, MAP_PRIVATE, fd, 0);
  close(fd);
  if (mapping == MAP_FAILED) {
    summary.error = "mmap failed";
    return summary;
  }
  const auto* bytes = static_cast<const uint8_t*>(mapping);
  if (bytes[0] != 0x7F || bytes[1] != 'E' || bytes[2] != 'L' || bytes[3] != 'F') {
    summary.error = "not an ELF file";
    munmap(mapping, static_cast<size_t>(statBuffer.st_size));
    return summary;
  }
  summary.valid = true;
  summary.is64 = bytes[4] == 2;
  if (summary.is64) {
    uint16_t machine = 0;
    uint64_t entry = 0;
    std::memcpy(&machine, bytes + 18, sizeof(machine));
    std::memcpy(&entry, bytes + 24, sizeof(entry));
    summary.machine = machine;
    summary.entry = entry;
  } else {
    uint16_t machine = 0;
    uint32_t entry = 0;
    std::memcpy(&machine, bytes + 18, sizeof(machine));
    std::memcpy(&entry, bytes + 24, sizeof(entry));
    summary.machine = machine;
    summary.entry = entry;
  }
  munmap(mapping, static_cast<size_t>(statBuffer.st_size));
  return summary;
}

const char* machineName(uint16_t machine) {
  switch (machine) {
    case 40:
      return "arm";
    case 183:
      return "aarch64";
    case 3:
      return "x86";
    case 62:
      return "x86_64";
    default:
      return "unknown";
  }
}

}  // namespace

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM*, void*) {
  ensureCore();
  return JNI_VERSION_1_6;
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_coreStatus(JNIEnv* env, jobject) {
  ensureCore();
  return env->NewStringUTF(g_coreStatus.c_str());
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_coreVersion(JNIEnv*, jobject) {
  return static_cast<jint>(road_core_version());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_abiInfo(JNIEnv* env, jobject) {
  char buffer[160];
#if defined(__aarch64__)
  const char* abi = "arm64-v8a";
#elif defined(__arm__)
  const char* abi = "armeabi-v7a";
#else
  const char* abi = "unknown";
#endif
  std::snprintf(buffer, sizeof(buffer), "%s %d-bit ptr=%zu page=%ld abi=%u", abi,
                static_cast<int>(sizeof(void*) * 8), sizeof(void*),
                static_cast<long>(sysconf(_SC_PAGESIZE)), road_abi_version());
  return env->NewStringUTF(buffer);
}

extern "C" JNIEXPORT jlong JNICALL
Java_dev_roadloader_core_NativeHost_createSession(JNIEnv*, jobject, jint width, jint height) {
  if (!ensureCore() || g_createSession == nullptr) {
    return 0;
  }
  void* session = g_createSession(static_cast<uint32_t>(width), static_cast<uint32_t>(height));
  return static_cast<jlong>(reinterpret_cast<intptr_t>(session));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_sessionFd(JNIEnv*, jobject, jlong handle) {
  return road_host_fd(sessionFromHandle(handle));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_duplicateFd(JNIEnv*, jobject, jlong handle) {
  return road_host_dup_fd(sessionFromHandle(handle));
}

extern "C" JNIEXPORT jlong JNICALL
Java_dev_roadloader_core_NativeHost_regionSize(JNIEnv*, jobject, jlong handle) {
  return static_cast<jlong>(road_host_region_size(sessionFromHandle(handle)));
}

extern "C" JNIEXPORT jobject JNICALL
Java_dev_roadloader_core_NativeHost_frameBuffer(JNIEnv* env, jobject, jlong handle, jint slot) {
  size_t length = 0;
  uint8_t* pointer = road_host_frame_buffer(sessionFromHandle(handle), static_cast<uint32_t>(slot),
                                            &length);
  if (pointer == nullptr || length == 0) {
    return nullptr;
  }
  return env->NewDirectByteBuffer(pointer, static_cast<jlong>(length));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_pollFrame(JNIEnv* env, jobject, jlong handle, jlongArray out) {
  if (out == nullptr || env->GetArrayLength(out) < 8) {
    return -1;
  }
  RoadFrameInfo info{};
  const int result = road_host_poll_frame(sessionFromHandle(handle), &info);
  if (result != 1) {
    return result;
  }
  jlong values[8];
  values[0] = static_cast<jlong>(reinterpret_cast<intptr_t>(info.ptr));
  values[1] = static_cast<jlong>(info.seq);
  values[2] = info.slot;
  values[3] = info.width;
  values[4] = info.height;
  values[5] = info.stride;
  values[6] = static_cast<jlong>(info.len);
  values[7] = static_cast<jlong>(info.published_ns);
  env->SetLongArrayRegion(out, 0, 8, values);
  return 1;
}

extern "C" JNIEXPORT void JNICALL
Java_dev_roadloader_core_NativeHost_releaseFrame(JNIEnv*, jobject, jlong handle, jint slot) {
  road_host_release_frame(sessionFromHandle(handle), static_cast<uint32_t>(slot));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_pushInput(JNIEnv*, jobject, jlong handle, jint kind,
                                              jint pointerId, jint action, jfloat x, jfloat y,
                                              jfloat pressure, jfloat size, jlong timestampNs,
                                              jint button, jint keyCode) {
  RoadInputEvent event{};
  event.kind = static_cast<uint32_t>(kind);
  event.pointer_id = static_cast<uint32_t>(pointerId);
  event.action = static_cast<uint32_t>(action);
  event.x = x;
  event.y = y;
  event.pressure = pressure;
  event.size = size;
  event.timestamp_ns = static_cast<uint64_t>(timestampNs);
  event.button = static_cast<uint32_t>(button);
  event.key_code = static_cast<uint32_t>(keyCode);
  return road_host_push_input(sessionFromHandle(handle), &event);
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_pushCommand(JNIEnv*, jobject, jlong handle, jint kind,
                                                jlong value) {
  RoadCommand command{};
  command.kind = static_cast<uint32_t>(kind);
  command.value = static_cast<uint64_t>(value);
  return road_host_push_command(sessionFromHandle(handle), &command);
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_pushCommandPayload(JNIEnv* env, jobject, jlong handle, jint kind,
                                                       jlong value, jbyteArray payload) {
  RoadCommand command{};
  command.kind = static_cast<uint32_t>(kind);
  command.value = static_cast<uint64_t>(value);
  if (payload != nullptr) {
    const jsize length = env->GetArrayLength(payload);
    const jsize copy = std::min<jsize>(length, static_cast<jsize>(sizeof(command.payload)));
    env->GetByteArrayRegion(payload, 0, copy, reinterpret_cast<jbyte*>(command.payload));
    command.length = static_cast<uint32_t>(copy);
  }
  return road_host_push_command(sessionFromHandle(handle), &command);
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_pollEvent(JNIEnv* env, jobject, jlong handle) {
  RoadEvent event{};
  if (road_host_pop_event(sessionFromHandle(handle), &event) != 1) {
    return nullptr;
  }
  std::string payload;
  const size_t length = event.length < sizeof(event.payload) ? event.length : sizeof(event.payload);
  for (size_t index = 0; index < length; ++index) {
    const uint8_t value = event.payload[index];
    payload.push_back(value >= 32 && value < 127 ? static_cast<char>(value) : '.');
  }
  char header[64];
  std::snprintf(header, sizeof(header), "%u|%u|%llu|", event.kind, event.flags,
                static_cast<unsigned long long>(event.value));
  return env->NewStringUTF((std::string(header) + payload).c_str());
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_popAudio(
    JNIEnv* env, jobject, jlong handle, jbyteArray out, jlongArray meta) {
  if (out == nullptr) {
    return -1;
  }
  RoadAudioBlock block{};
  if (road_host_pop_audio(sessionFromHandle(handle), &block) != 1) {
    return 0;
  }
  const jsize capacity = env->GetArrayLength(out);
  const jsize length = static_cast<jsize>(std::min<uint32_t>(block.length, static_cast<uint32_t>(capacity)));
  env->SetByteArrayRegion(out, 0, length, reinterpret_cast<const jbyte*>(block.data));
  // The consumer needs the block's own format and production time: assuming
  // 48 kHz stereo would hide a producer that changed either, and the timestamp
  // is what makes the bridge latency measurable instead of guessed.
  if (meta != nullptr && env->GetArrayLength(meta) >= 4) {
    const jlong values[4] = {
        static_cast<jlong>(block.frames),
        static_cast<jlong>(block.channels),
        static_cast<jlong>(block.sample_rate),
        static_cast<jlong>(block.timestamp_ns),
    };
    env->SetLongArrayRegion(meta, 0, 4, values);
  }
  return length;
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_stats(JNIEnv* env, jobject, jlong handle, jlongArray out) {
  if (out == nullptr || env->GetArrayLength(out) < 32) {
    return -1;
  }
  RoadStats stats{};
  if (road_host_stats(sessionFromHandle(handle), &stats) != 0) {
    return -1;
  }
  fillStats(env, out, stats);
  return 0;
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_selfTest(JNIEnv*, jobject, jlong handle) {
  RoadStats stats{};
  if (road_host_stats(sessionFromHandle(handle), &stats) != 0) {
    return 0;
  }
  return static_cast<jint>(stats.self_test);
}

extern "C" JNIEXPORT void JNICALL
Java_dev_roadloader_core_NativeHost_destroySession(JNIEnv*, jobject, jlong handle) {
  road_host_destroy(sessionFromHandle(handle));
}

extern "C" JNIEXPORT void JNICALL
Java_dev_roadloader_core_NativeHost_setHeapLimit(JNIEnv*, jobject, jlong bytes) {
  road_set_heap_limit(static_cast<uint64_t>(bytes));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_allocStats(JNIEnv* env, jobject, jlongArray out) {
  if (out == nullptr || env->GetArrayLength(out) < 4) {
    return -1;
  }
  RoadAllocStats stats{};
  if (road_alloc_stats(&stats) != 0) {
    return -1;
  }
  const jlong values[4] = {static_cast<jlong>(stats.live_bytes), static_cast<jlong>(stats.peak_bytes),
                           static_cast<jlong>(stats.limit_bytes),
                           static_cast<jlong>(stats.failed_allocs)};
  env->SetLongArrayRegion(out, 0, 4, values);
  return 0;
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_probeElf(JNIEnv* env, jobject, jstring path) {
  const std::string file = toString(env, path);
  const ElfSummary summary = inspectElf(file);
  if (!summary.valid) {
    return env->NewStringUTF(("error:" + summary.error).c_str());
  }
  char buffer[256];
  std::snprintf(buffer, sizeof(buffer),
                "class=%d machine=%s entry=0x%llx size=%llu", summary.is64 ? 64 : 32,
                machineName(summary.machine), static_cast<unsigned long long>(summary.entry),
                static_cast<unsigned long long>(summary.size));
  return env->NewStringUTF(buffer);
}

extern "C" JNIEXPORT jlong JNICALL
Java_dev_roadloader_core_NativeHost_scanElf(JNIEnv* env, jobject, jstring path, jstring patternText) {
  const std::string file = toString(env, path);
  const std::string text = toString(env, patternText);
  const auto pattern = pl::memory::parsePattern(text);
  if (!pattern.has_value()) {
    return -1;
  }
  const int fd = open(file.c_str(), O_RDONLY | O_CLOEXEC);
  if (fd < 0) {
    return -1;
  }
  struct stat statBuffer {};
  if (fstat(fd, &statBuffer) != 0 || statBuffer.st_size <= 0) {
    close(fd);
    return -1;
  }
  void* mapping = mmap(nullptr, static_cast<size_t>(statBuffer.st_size), PROT_READ, MAP_PRIVATE, fd, 0);
  close(fd);
  if (mapping == MAP_FAILED) {
    return -1;
  }
  const auto match = pl::memory::SignatureScanner::find(
      static_cast<const uint8_t*>(mapping), static_cast<size_t>(statBuffer.st_size), *pattern);
  const jlong result = match.has_value()
                           ? static_cast<jlong>(*match - reinterpret_cast<uintptr_t>(mapping))
                           : -1;
  munmap(mapping, static_cast<size_t>(statBuffer.st_size));
  return result;
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_engineFileReport(JNIEnv* env, jobject, jstring path) {
  const char* text = path != nullptr ? env->GetStringUTFChars(path, nullptr) : nullptr;
  std::string report = pl::runtime::EngineImports::describeFile(text);
  if (text != nullptr) {
    env->ReleaseStringUTFChars(path, text);
  }
  return env->NewStringUTF(report.c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_engineImportReport(JNIEnv* env, jobject) {
  const auto probe = pl::runtime::EngineImports::probe(
      pl::runtime::EngineImports::gameModule(), pl::runtime::EngineImports::swapBuffersSymbol());
  return env->NewStringUTF(probe.describe().c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_loadedModuleReport(JNIEnv* env, jobject) {
  return env->NewStringUTF(pl::runtime::EngineImports::loadedModules("/data/").c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_moduleDebug(JNIEnv* env, jobject, jstring name) {
  const char* text = name != nullptr ? env->GetStringUTFChars(name, nullptr) : nullptr;
  std::string report = pl::runtime::EngineImports::moduleDebug(text);
  if (text != nullptr) {
    env->ReleaseStringUTFChars(name, text);
  }
  return env->NewStringUTF(report.c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_importHookSelfTest(JNIEnv* env, jobject) {
  const auto probe = pl::runtime::EngineImports::selfTest();
  std::string line = probe.describe();
  char calls[48];
  std::snprintf(calls, sizeof(calls), " calls=%llu",
                static_cast<unsigned long long>(pl::runtime::EngineImports::selfTestCalls()));
  line += calls;
  return env->NewStringUTF(line.c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeHost_transpileShader(JNIEnv* env, jobject, jstring source,
                                                    jint stage) {
  const std::string text = toString(env, source);
  const int needed = rust_shader_transpile(text.c_str(), static_cast<uint32_t>(stage), nullptr, 0);
  if (needed <= 0) {
    return nullptr;
  }
  std::vector<char> buffer(static_cast<size_t>(needed));
  const int written = rust_shader_transpile(text.c_str(), static_cast<uint32_t>(stage), buffer.data(),
                                            static_cast<uint32_t>(buffer.size()));
  if (written <= 0) {
    return nullptr;
  }
  return env->NewStringUTF(buffer.data());
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_crc32(JNIEnv* env, jobject, jbyteArray data) {
  if (data == nullptr) {
    return 0;
  }
  const jsize length = env->GetArrayLength(data);
  std::vector<uint8_t> bytes(static_cast<size_t>(length));
  env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte*>(bytes.data()));
  return static_cast<jint>(rust_crc32(bytes.data(), bytes.size()));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_setVolume(JNIEnv*, jobject, jlong handle, jfloat volume) {
  RoadCommand command{};
  command.kind = 1;  // CMD_SET_VOLUME
  command.value = static_cast<uint64_t>(volume * 1000.0f);
  return road_host_push_command(sessionFromHandle(handle), &command);
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeHost_requestSelfTest(JNIEnv*, jobject, jlong handle) {
  RoadCommand command{};
  command.kind = 4;  // CMD_REQUEST_SELF_TEST
  return road_host_push_command(sessionFromHandle(handle), &command);
}

// ---------------------------------------------------------------------------
// Companion role
// ---------------------------------------------------------------------------

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeCompanion_attach(JNIEnv*, jobject, jint fd, jlong size, jint flags,
                                                jlong heapLimit) {
  if (!ensureCore() || g_companionAttach == nullptr) {
    return -100;
  }
  return g_companionAttach(fd, static_cast<size_t>(size), static_cast<uint32_t>(flags),
                           static_cast<uint64_t>(heapLimit));
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeCompanion_stop(JNIEnv*, jobject) {
  return road_companion_stop();
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeCompanion_isRunning(JNIEnv*, jobject) {
  return road_companion_is_running();
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_roadloader_core_NativeCompanion_selfTest(JNIEnv*, jobject) {
  return static_cast<jint>(road_companion_self_test());
}

extern "C" JNIEXPORT jstring JNICALL
Java_dev_roadloader_core_NativeCompanion_abiInfo(JNIEnv* env, jobject) {
  char buffer[128];
#if defined(__aarch64__)
  const char* abi = "arm64-v8a";
#elif defined(__arm__)
  const char* abi = "armeabi-v7a";
#else
  const char* abi = "unknown";
#endif
  std::snprintf(buffer, sizeof(buffer), "%s %d-bit pid=%d", abi,
                static_cast<int>(sizeof(void*) * 8), static_cast<int>(getpid()));
  return env->NewStringUTF(buffer);
}
