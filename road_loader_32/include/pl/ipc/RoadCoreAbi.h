// C ABI exported by `libroad_core_32.so`.
//
// This header is the single contract between the Rust core, the JNI bridge and
// the game hooks. Every structure mirrors a `#[repr(C)]` Rust type; the sizes
// are checked at runtime through the `road_*_size()` functions and, where the
// size is target independent, at compile time.
#pragma once

#include <cstddef>
#include <cstdint>

extern "C" {

struct RoadFrameInfo {
  uint8_t* ptr;
  size_t len;
  uint64_t seq;
  uint32_t slot;
  uint32_t width;
  uint32_t height;
  uint32_t stride;
  uint64_t published_ns;
};

struct RoadStats {
  uint32_t status;
  uint32_t flags;
  uint32_t pid;
  uint32_t width;
  uint32_t height;
  uint32_t heartbeat_seq;
  uint32_t heartbeat_age_ms;
  uint64_t frame_count;
  uint64_t presented_count;
  uint32_t dropped_frames;
  uint32_t input_pushed;
  uint32_t input_consumed;
  uint32_t input_dropped;
  uint32_t cmd_pushed;
  uint32_t cmd_consumed;
  uint32_t cmd_dropped;
  uint32_t event_pushed;
  uint32_t event_consumed;
  uint32_t event_dropped;
  uint32_t last_input_x;
  uint32_t last_input_y;
  uint32_t last_input_kind;
  uint32_t last_input_seq;
  uint32_t audio_pushed;
  uint32_t audio_consumed;
  uint32_t audio_dropped;
  uint64_t heap_used;
  uint64_t heap_peak;
  uint64_t heap_limit;
  uint32_t render_us_avg;
  uint32_t render_us_max;
  uint32_t self_test;
  uint32_t crash_signal;
  uint64_t crash_addr;
  uint64_t crash_pc;
  uint64_t crash_lr;
  uint32_t asset_bytes;
  uint32_t asset_crc;
  uint32_t asset_status;
};

struct RoadAllocStats {
  uint64_t live_bytes;
  uint64_t peak_bytes;
  uint64_t limit_bytes;
  uint64_t failed_allocs;
  uint64_t rss_bytes;
};

struct RoadInputEvent {
  uint32_t seq;
  uint32_t kind;
  uint32_t pointer_id;
  uint32_t action;
  float x;
  float y;
  float pressure;
  float size;
  uint64_t timestamp_ns;
  uint32_t button;
  uint32_t key_code;
  uint32_t reserved[3];
};

struct RoadCommand {
  uint32_t kind;
  uint32_t flags;
  uint32_t seq;
  uint32_t length;
  uint64_t value;
  uint8_t payload[232];
};

struct RoadEvent {
  uint32_t kind;
  uint32_t flags;
  uint32_t seq;
  uint32_t length;
  uint64_t value;
  uint8_t payload[232];
};

struct RoadAudioBlock {
  uint32_t seq;
  uint32_t frames;
  uint32_t channels;
  uint32_t sample_rate;
  uint64_t timestamp_ns;
  uint32_t length;
  uint32_t flags;
  uint8_t data[4064];
};

/// Mesh vertex produced by the Rust mesher: 36 bytes.
struct RoadMeshVertex {
  float position[3];
  float normal[3];
  float uv[2];
  uint8_t ao;
  uint8_t pad[3];
};

uint32_t road_core_version();
uint32_t road_abi_version();
uint32_t road_stats_size();
uint32_t road_frame_info_size();
uint32_t road_header_size();
uint32_t road_input_event_size();
uint32_t road_command_size();
uint32_t road_event_size();
uint32_t road_audio_block_size();

void* road_host_create(uint32_t width, uint32_t height);
int road_host_fd(void* session);
int road_host_dup_fd(void* session);
uint64_t road_host_region_size(void* session);
int road_host_poll_frame(void* session, RoadFrameInfo* out);
uint8_t* road_host_frame_buffer(void* session, uint32_t slot, size_t* out_len);
void road_host_release_frame(void* session, uint32_t slot);
int road_host_push_input(void* session, const RoadInputEvent* event);
int road_host_push_command(void* session, const RoadCommand* command);
int road_host_pop_event(void* session, RoadEvent* out);
int road_host_pop_audio(void* session, RoadAudioBlock* out);
int road_host_stats(void* session, RoadStats* out);
void road_host_destroy(void* session);

/// Returns 0 on success, -1 on a bad descriptor, -2 on an ABI mismatch, -3 when a
/// companion is already attached, -4 when `size` exceeds
/// `MAX_REGION_BYTES` (128 MiB).
int road_companion_attach(int fd, size_t size, uint32_t flags, uint64_t heap_limit);
int road_companion_stop();
int road_companion_is_running();
uint32_t road_companion_self_test();
int road_companion_publish_rgba(const uint8_t* pixels, uint32_t width, uint32_t height,
                                uint32_t stride);
int road_companion_pop_input(RoadInputEvent* out);
void road_companion_set_input_sink(int enabled);

void road_set_heap_limit(uint64_t bytes);
int road_alloc_stats(RoadAllocStats* out);
uint32_t road_alloc_stats_size(void);

void* road_alloc(size_t size, size_t alignment);
void* road_calloc(size_t size, size_t alignment);
void road_free(void* pointer);
void* road_realloc(void* pointer, size_t size, size_t alignment);
size_t road_alloc_usable_size(void* pointer);

float rust_math_distance(float x1, float y1, float z1, float x2, float y2, float z2);
float rust_vec3_length(float x, float y, float z);
int rust_aabb_intersects(const float* min1, const float* max1, const float* min2,
                         const float* max2);
int rust_transform_point(const float* matrix, const float* point, float* out);
int rust_actor_move(const float* start, const float* half_extents, const float* velocity,
                    float dt, const float* solids, uint32_t solids_len, float* out_position,
                    uint32_t* out_flags);
int rust_mesh_build(const uint32_t* dims, const uint8_t* blocks, const uint8_t* opaque,
                    const float* origin, float scale, RoadMeshVertex* out, uint32_t out_capacity,
                    uint32_t use_simd);
int rust_shader_transpile(const char* source, uint32_t stage, char* out, uint32_t out_len);
uint32_t rust_audio_convert_i16(const int16_t* source, uint32_t sample_count, float* destination);
uint32_t rust_crc32(const uint8_t* data, size_t len);
uint32_t rust_asset_chunk_count(size_t len);

int road_patch_offset(const char* name, size_t address);
size_t road_patch_lookup(const char* name);
uint32_t road_patch_count();
void road_patch_clear();
}

static_assert(sizeof(RoadInputEvent) == 64, "RoadInputEvent must stay 64 bytes");
static_assert(sizeof(RoadCommand) == 256, "RoadCommand must stay 256 bytes");
static_assert(sizeof(RoadEvent) == 256, "RoadEvent must stay 256 bytes");
static_assert(sizeof(RoadAudioBlock) == 4096, "RoadAudioBlock must stay 4096 bytes");
static_assert(sizeof(RoadMeshVertex) == 36, "RoadMeshVertex must stay 36 bytes");
static_assert(sizeof(RoadStats) == 200, "RoadStats must match the Rust layout");
