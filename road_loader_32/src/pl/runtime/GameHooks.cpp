#include "pl/runtime/GameHooks.h"

#include <algorithm>
#include <atomic>
#include <cstring>
#include <vector>

#include "pl/Log.h"

#if defined(PL_ENABLE_GL)
#include <GLES3/gl3.h>
#endif

namespace pl::runtime {
namespace {

std::atomic<void*> g_swapBuffersOriginal{nullptr};
std::atomic<void*> g_screenViewOriginal{nullptr};
std::atomic<void*> g_distanceOriginal{nullptr};
std::atomic<bool> g_inputSink{false};
std::atomic<uint64_t> g_framesCaptured{0};
std::atomic<uint64_t> g_meshCalls{0};
std::atomic<uint64_t> g_physicsCalls{0};
std::atomic<uint64_t> g_audioBlocks{0};
std::atomic<uint64_t> g_inputEvents{0};
std::atomic<uint64_t> g_meshFailures{0};
std::atomic<float> g_volume{1.0f};

#if defined(PL_ENABLE_GL)
/// Minimal GLES 3.0 post-processor: copies the back buffer into a texture and
/// draws it back through a fragment shader. This is the hook point where a
/// transpiled Java shader is injected (Phase 6).
class PostProcessor {
 public:
  bool ensureReady() {
    if (ready_) {
      return true;
    }
    if (failed_) {
      return false;
    }
    const char* vertexSource =
        "#version 300 es\n"
        "out vec2 vUv;\n"
        "void main() {\n"
        "  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));\n"
        "  vUv = p;\n"
        "  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);\n"
        "}\n";
    const char* fragmentSource =
        "#version 300 es\n"
        "precision highp float;\n"
        "in vec2 vUv;\n"
        "uniform sampler2D uTexture;\n"
        "uniform float uSaturation;\n"
        "out vec4 fragColor;\n"
        "void main() {\n"
        "  vec4 c = texture(uTexture, vUv);\n"
        "  float luma = dot(c.rgb, vec3(0.299, 0.587, 0.114));\n"
        "  c.rgb = mix(vec3(luma), c.rgb, uSaturation);\n"
        "  vec2 d = vUv - vec2(0.5);\n"
        "  c.rgb *= clamp(1.0 - dot(d, d) * 0.35, 0.75, 1.0);\n"
        "  fragColor = vec4(c.rgb, 1.0);\n"
        "}\n";
    const GLuint vertex = compile(GL_VERTEX_SHADER, vertexSource);
    const GLuint fragment = compile(GL_FRAGMENT_SHADER, fragmentSource);
    if (vertex == 0 || fragment == 0) {
      failed_ = true;
      return false;
    }
    program_ = glCreateProgram();
    glAttachShader(program_, vertex);
    glAttachShader(program_, fragment);
    glLinkProgram(program_);
    GLint linked = 0;
    glGetProgramiv(program_, GL_LINK_STATUS, &linked);
    glDeleteShader(vertex);
    glDeleteShader(fragment);
    if (linked == 0) {
      PL_LOGE("post-process program failed to link");
      glDeleteProgram(program_);
      program_ = 0;
      failed_ = true;
      return false;
    }
    glGenVertexArrays(1, &vao_);
    glGenTextures(1, &texture_);
    ready_ = true;
    PL_LOGI("BetterRenderDragon post-processor ready");
    return true;
  }

  void apply(int width, int height) {
    if (!enabled_ || width <= 0 || height <= 0 || !ensureReady()) {
      return;
    }
    glBindTexture(GL_TEXTURE_2D, texture_);
    glCopyTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 0, 0, width, height, 0);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glUseProgram(program_);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, texture_);
    glUniform1i(glGetUniformLocation(program_, "uTexture"), 0);
    glUniform1f(glGetUniformLocation(program_, "uSaturation"), saturation_);
    glBindVertexArray(vao_);
    glDisable(GL_DEPTH_TEST);
    glDisable(GL_BLEND);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    glBindVertexArray(0);
    glUseProgram(0);
  }

  void setEnabled(bool enabled) { enabled_ = enabled; }
  bool enabled() const { return enabled_; }

 private:
  static GLuint compile(GLenum type, const char* source) {
    const GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, nullptr);
    glCompileShader(shader);
    GLint compiled = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &compiled);
    if (compiled == 0) {
      char log[512] = {0};
      glGetShaderInfoLog(shader, sizeof(log) - 1, nullptr, log);
      PL_LOGE("shader compile failed: %s", log);
      glDeleteShader(shader);
      return 0;
    }
    return shader;
  }

  GLuint program_ = 0;
  GLuint vao_ = 0;
  GLuint texture_ = 0;
  float saturation_ = 1.12f;
  bool ready_ = false;
  bool failed_ = false;
  bool enabled_ = true;
};

PostProcessor& postProcessor() {
  static PostProcessor processor;
  return processor;
}

std::vector<uint8_t> g_captureBuffer;
std::vector<uint8_t> g_flipBuffer;

void captureAndPublish(int width, int height) {
  if (width <= 0 || height <= 0) {
    return;
  }
  const size_t rowBytes = static_cast<size_t>(width) * 4;
  const size_t total = rowBytes * static_cast<size_t>(height);
  if (g_captureBuffer.size() < total) {
    g_captureBuffer.resize(total);
    g_flipBuffer.resize(total);
  }
  glPixelStorei(GL_PACK_ALIGNMENT, 4);
  glReadPixels(0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, g_captureBuffer.data());
  // OpenGL's origin is bottom-left; the shared frame is top-left.
  for (int row = 0; row < height; ++row) {
    std::memcpy(g_flipBuffer.data() + static_cast<size_t>(row) * rowBytes,
                g_captureBuffer.data() + static_cast<size_t>(height - 1 - row) * rowBytes,
                rowBytes);
  }
  const int published = road_companion_publish_rgba(g_flipBuffer.data(), static_cast<uint32_t>(width),
                                                    static_cast<uint32_t>(height),
                                                    static_cast<uint32_t>(rowBytes));
  if (published == 0) {
    g_framesCaptured.fetch_add(1, std::memory_order_relaxed);
  }
}
#else
void captureAndPublish(int, int) {}
#endif

int currentViewportWidth() {
#if defined(PL_ENABLE_GL)
  GLint viewport[4] = {0, 0, 0, 0};
  glGetIntegerv(GL_VIEWPORT, viewport);
  return viewport[2];
#else
  return 0;
#endif
}

int currentViewportHeight() {
#if defined(PL_ENABLE_GL)
  GLint viewport[4] = {0, 0, 0, 0};
  glGetIntegerv(GL_VIEWPORT, viewport);
  return viewport[3];
#else
  return 0;
#endif
}

}  // namespace

extern "C" void plSwapBuffersThunk(void* platform) {
  const int width = currentViewportWidth();
  const int height = currentViewportHeight();
#if defined(PL_ENABLE_GL)
  postProcessor().apply(width, height);
#endif
  captureAndPublish(width, height);
  auto original = reinterpret_cast<void (*)(void*)>(g_swapBuffersOriginal.load());
  if (original != nullptr) {
    original(platform);
  }
}

extern "C" void plScreenViewSetupThunk(void* view) {
  g_inputSink.store(true, std::memory_order_relaxed);
  road_companion_set_input_sink(1);
  auto original = reinterpret_cast<void (*)(void*)>(g_screenViewOriginal.load());
  if (original != nullptr) {
    original(view);
  }
}

extern "C" float plVec3DistanceThunk(const float* a, const float* b) {
  g_physicsCalls.fetch_add(1, std::memory_order_relaxed);
  if (a == nullptr || b == nullptr) {
    auto original = reinterpret_cast<float (*)(const float*, const float*)>(
        g_distanceOriginal.load());
    return original != nullptr ? original(a, b) : 0.0f;
  }
  return GameHooks::distanceHook(a[0], a[1], a[2], b[0], b[1], b[2]);
}

size_t GameTargets::count() const {
  size_t total = 0;
  for (const uintptr_t target :
       {levelTick, actorRender, screenViewSetup, swapBuffers, processMouseEvent, chunkMesher,
        actorMove, vec3Distance, soundEnginePlay}) {
    total += target != 0 ? 1 : 0;
  }
  return total;
}

GameHooks& GameHooks::instance() {
  static GameHooks hooks;
  return hooks;
}

GameHooks::~GameHooks() { uninstallAll(); }

Status GameHooks::installOne(const char* name, uintptr_t target, void* replacement) {
  if (target == 0) {
    return Status::InvalidArgument;
  }
  Entry entry;
  entry.name = name;
  const Status status =
      entry.hook.install(reinterpret_cast<void*>(target), replacement, &entry.original);
  if (status != Status::Ok) {
    PL_LOGW("hook %s at 0x%zx not installed: %s", name, static_cast<size_t>(target),
            statusName(status));
    return status;
  }
  entries_.push_back(std::move(entry));
  return Status::Ok;
}

Status GameHooks::install(const GameTargets& targets, const GameOptions& options) {
  std::lock_guard<std::mutex> lock(mutex_);
  options_ = options;
  if (!entries_.empty()) {
    return Status::AlreadyExists;
  }
  if (options.safeMode) {
    PL_LOGI("safe mode: no engine hooks installed");
    return Status::Ok;
  }
#if defined(PL_ENABLE_GL)
  postProcessor().setEnabled(options.enableShaders);
#endif
  size_t installed = 0;
  if (targets.swapBuffers != 0) {
    if (installOne("AppPlatform::swapBuffers", targets.swapBuffers,
                   reinterpret_cast<void*>(&plSwapBuffersThunk)) == Status::Ok) {
      g_swapBuffersOriginal.store(entries_.back().original);
      ++installed;
    }
  }
  if (targets.vec3Distance != 0) {
    if (installOne("Vec3::distance", targets.vec3Distance,
                   reinterpret_cast<void*>(&plVec3DistanceThunk)) == Status::Ok) {
      g_distanceOriginal.store(entries_.back().original);
      ++installed;
    }
  }
  if (targets.screenViewSetup != 0) {
    if (installOne("ScreenView::setup", targets.screenViewSetup,
                   reinterpret_cast<void*>(&plScreenViewSetupThunk)) == Status::Ok) {
      g_screenViewOriginal.store(entries_.back().original);
      ++installed;
    }
  }
  // The remaining targets need a per-version adapter (chunk layout, entity
  // layout, audio buffer descriptor). They are reported so the caller can see
  // what is still unresolved instead of silently installing a wrong ABI.
  for (const auto& [name, target] :
       std::initializer_list<std::pair<const char*, uintptr_t>>{
           {"Level::tick", targets.levelTick},
           {"Actor::render", targets.actorRender},
           {"AppPlatform::processMouseEvent", targets.processMouseEvent},
           {"ChunkMesher::buildMesh", targets.chunkMesher},
           {"Actor::move", targets.actorMove},
           {"SoundEngine::play", targets.soundEnginePlay}}) {
    if (target != 0) {
      PL_LOGW("%s found at 0x%zx but needs a per-version adapter; not hooked", name,
              static_cast<size_t>(target));
    }
  }
  PL_LOGI("installed %zu hook(s)", installed);
  return installed > 0 ? Status::Ok : Status::NotFound;
}

void GameHooks::uninstallAll() {
  std::lock_guard<std::mutex> lock(mutex_);
  for (auto& entry : entries_) {
    entry.hook.uninstall();
  }
  entries_.clear();
  g_swapBuffersOriginal.store(nullptr);
  g_screenViewOriginal.store(nullptr);
  g_distanceOriginal.store(nullptr);
  if (g_inputSink.exchange(false)) {
    road_companion_set_input_sink(0);
  }
}

size_t GameHooks::activeCount() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return entries_.size();
}

bool GameHooks::isInstalled(const char* name) const {
  std::lock_guard<std::mutex> lock(mutex_);
  return std::any_of(entries_.begin(), entries_.end(),
                     [name](const Entry& entry) { return entry.name == name; });
}

HookStats GameHooks::stats() const {
  HookStats snapshot;
  snapshot.framesCaptured = g_framesCaptured.load(std::memory_order_relaxed);
  snapshot.meshCalls = g_meshCalls.load(std::memory_order_relaxed);
  snapshot.physicsCalls = g_physicsCalls.load(std::memory_order_relaxed);
  snapshot.audioBlocks = g_audioBlocks.load(std::memory_order_relaxed);
  snapshot.inputEvents = g_inputEvents.load(std::memory_order_relaxed);
  snapshot.meshFailures = g_meshFailures.load(std::memory_order_relaxed);
  return snapshot;
}

void GameHooks::setVolume(float volume) {
  g_volume.store(std::clamp(volume, 0.0f, 1.0f), std::memory_order_relaxed);
}

float GameHooks::distanceHook(float x1, float y1, float z1, float x2, float y2, float z2) {
  return rust_math_distance(x1, y1, z1, x2, y2, z2);
}

int GameHooks::meshHook(const uint32_t* dims, const uint8_t* blocks, const uint8_t* opaque,
                        const float* origin, float scale, RoadMeshVertex* out, uint32_t capacity) {
  g_meshCalls.fetch_add(1, std::memory_order_relaxed);
  if (dims == nullptr || blocks == nullptr || opaque == nullptr || out == nullptr) {
    g_meshFailures.fetch_add(1, std::memory_order_relaxed);
    return -1;
  }
  const int vertices = rust_mesh_build(dims, blocks, opaque, origin, scale, out, capacity, 1);
  if (vertices < 0) {
    g_meshFailures.fetch_add(1, std::memory_order_relaxed);
  }
  return vertices;
}

}  // namespace pl::runtime
