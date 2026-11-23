#include <cstring>
#include <string>
#include <vector>

#include "TestHarness.h"
#include "pl/ipc/RoadCoreAbi.h"

PL_TEST(core_abi_versions_are_reported) {
  PL_CHECK_EQ(road_core_version() >> 16, uint32_t{2});
  PL_CHECK_EQ(road_abi_version(), uint32_t{1});
  PL_CHECK(road_header_size() > 0);
  PL_CHECK_EQ(road_frame_info_size(), static_cast<uint32_t>(sizeof(RoadFrameInfo)));
  PL_CHECK_EQ(road_alloc_stats_size(), static_cast<uint32_t>(sizeof(RoadAllocStats)));
  PL_CHECK_EQ(road_stats_size(), static_cast<uint32_t>(sizeof(RoadStats)));
}

PL_TEST(shader_transpiler_rewrites_fragment_shader) {
  const char* source = "uniform sampler2D uTex;\nvarying vec2 vUv;\nvoid main(){ gl_FragColor = texture2D(uTex, vUv); }";
  const int needed = rust_shader_transpile(source, 1, nullptr, 0);
  PL_CHECK(needed > 0);
  std::vector<char> buffer(static_cast<size_t>(needed));
  const int written =
      rust_shader_transpile(source, 1, buffer.data(), static_cast<uint32_t>(buffer.size()));
  PL_CHECK_EQ(written, needed);
  const std::string text(buffer.data());
  PL_CHECK(text.find("#version 300 es") != std::string::npos);
  PL_CHECK(text.find("layout(location = 0) out vec4 pc_fragColor;") != std::string::npos);
  PL_CHECK(text.find("texture(uTex, vUv)") != std::string::npos);
}

PL_TEST(shader_transpiler_rejects_unsupported_input) {
  PL_CHECK_EQ(rust_shader_transpile("#include <x>\nvoid main(){}", 1, nullptr, 0), -3);
  PL_CHECK_EQ(rust_shader_transpile("uniform float x;", 1, nullptr, 0), -4);
}

PL_TEST(shader_transpiler_truncates_safely) {
  const char* source = "void main(){ gl_FragColor = vec4(1.0); }";
  char small[8] = {0};
  const int result = rust_shader_transpile(source, 1, small, sizeof(small));
  PL_CHECK(result < 0);
  PL_CHECK_EQ(small[sizeof(small) - 1], char{0});
}

PL_TEST(mesh_c_abi_matches_the_reference_shape) {
  uint32_t dims[3] = {16, 16, 16};
  std::vector<uint8_t> blocks(16 * 16 * 16, 0);
  blocks[0] = 1;
  uint8_t opaque[256] = {0};
  for (size_t index = 1; index < 256; ++index) {
    opaque[index] = 1;
  }
  std::vector<RoadMeshVertex> vertices(1024);
  const int count = rust_mesh_build(dims, blocks.data(), opaque, nullptr, 1.0f, vertices.data(),
                                    static_cast<uint32_t>(vertices.size()), 1);
  PL_CHECK_EQ(count, 36);
  // The first quad is a unit face at the origin.
  bool sawUnitNormal = false;
  for (int index = 0; index < count; ++index) {
    const auto& vertex = vertices[static_cast<size_t>(index)];
    const float length = vertex.normal[0] * vertex.normal[0] +
                         vertex.normal[1] * vertex.normal[1] +
                         vertex.normal[2] * vertex.normal[2];
    if (length > 0.99f && length < 1.01f) {
      sawUnitNormal = true;
    }
  }
  PL_CHECK(sawUnitNormal);
}

PL_TEST(actor_move_c_abi_lands_on_the_floor) {
  const float start[3] = {0.0f, 2.0f, 0.0f};
  const float half[3] = {0.3f, 0.9f, 0.3f};
  const float velocity[3] = {0.0f, -10.0f, 0.0f};
  const float solids[6] = {-4.0f, -1.0f, -4.0f, 4.0f, 0.0f, 4.0f};
  float position[3] = {0.0f, 0.0f, 0.0f};
  uint32_t flags = 0;
  PL_CHECK_EQ(rust_actor_move(start, half, velocity, 0.5f, solids, 1, position, &flags), 0);
  PL_CHECK((flags & 1u) != 0);
  PL_CHECK(position[1] < 1.0f);
}

PL_TEST(audio_conversion_and_crc_helpers) {
  const int16_t input[4] = {0, 16384, -16384, -32768};
  float output[4] = {0, 0, 0, 0};
  PL_CHECK_EQ(rust_audio_convert_i16(input, 4, output), uint32_t{4});
  PL_CHECK_EQ(output[1], 0.5f);
  PL_CHECK_EQ(output[3], -1.0f);

  const uint8_t data[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
  PL_CHECK_EQ(rust_crc32(data, sizeof(data)), uint32_t{0xCBF43926u});
  PL_CHECK_EQ(rust_asset_chunk_count(0), uint32_t{0});
  PL_CHECK_EQ(rust_asset_chunk_count(192), uint32_t{1});
  PL_CHECK_EQ(rust_asset_chunk_count(193), uint32_t{2});
}
