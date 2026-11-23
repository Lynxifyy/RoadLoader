#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <csignal>
#include <vector>

#include "TestHarness.h"
#include "pl/ipc/RoadCoreAbi.h"
#include "pl/ipc/SharedRegion.h"

namespace {

constexpr uint32_t kWidth = 64;
constexpr uint32_t kHeight = 48;
// FLAG_HEADLESS: the companion keeps the heartbeat and input ring alive but
// leaves frame production to the caller, which makes the test deterministic.
constexpr uint32_t kHeadless = 1u << 3;

void fillPattern(std::vector<uint8_t>& pixels, uint8_t red, uint8_t green, uint8_t blue) {
  for (size_t index = 0; index + 3 < pixels.size(); index += 4) {
    pixels[index] = red;
    pixels[index + 1] = green;
    pixels[index + 2] = blue;
    pixels[index + 3] = 0xFF;
  }
}

}  // namespace

PL_TEST(shared_region_abi_matches) {
  PL_CHECK(pl::ipc::SharedRegion::checkAbi() == pl::Status::Ok);
  PL_CHECK_EQ(road_input_event_size(), uint32_t{64});
  PL_CHECK_EQ(road_command_size(), uint32_t{256});
  PL_CHECK_EQ(road_event_size(), uint32_t{256});
  PL_CHECK_EQ(road_audio_block_size(), uint32_t{4096});
  PL_CHECK_EQ(road_stats_size(), static_cast<uint32_t>(sizeof(RoadStats)));
}

PL_TEST(session_creation_reports_a_mapping) {
  auto region = pl::ipc::SharedRegion::create(kWidth, kHeight);
  PL_CHECK(region != nullptr);
  PL_CHECK(region->size() >= kWidth * kHeight * 4);
  const int fd = region->duplicateFd();
  PL_CHECK(fd >= 0);
  close(fd);
  RoadFrameInfo info{};
  PL_CHECK(!region->pollFrame(&info));
  RoadStats stats{};
  PL_CHECK(region->stats(&stats));
  PL_CHECK_EQ(stats.width, kWidth);
  PL_CHECK_EQ(stats.height, kHeight);
}

PL_TEST(cross_process_frame_and_input_path) {
  auto region = pl::ipc::SharedRegion::create(kWidth, kHeight);
  PL_CHECK(region != nullptr);
  const uint64_t size = region->size();
  const int fd = region->duplicateFd();
  PL_CHECK(fd >= 0);

  const pid_t child = fork();
  PL_CHECK(child >= 0);
  if (child == 0) {
    if (road_companion_attach(fd, size, kHeadless, 0) != 0) {
      _exit(10);
    }
    std::vector<uint8_t> pixels(static_cast<size_t>(kWidth) * kHeight * 4);
    fillPattern(pixels, 0x11, 0x22, 0x33);
    for (int frame = 0; frame < 100; ++frame) {
      road_companion_publish_rgba(pixels.data(), kWidth, kHeight, kWidth * 4);
      usleep(5000);
    }
    _exit(0);
  }
  close(fd);

  bool sawFrame = false;
  RoadFrameInfo info{};
  for (int attempt = 0; attempt < 400 && !sawFrame; ++attempt) {
    if (region->pollFrame(&info)) {
      if (info.width == kWidth && info.height == kHeight && info.ptr != nullptr &&
          info.ptr[0] == 0x11 && info.ptr[1] == 0x22 && info.ptr[2] == 0x33) {
        sawFrame = true;
      }
      region->releaseFrame(info.slot);
    }
    usleep(5000);
  }
  PL_CHECK(sawFrame);

  RoadInputEvent event{};
  event.kind = 1;  // INPUT_KIND_DOWN
  event.pointer_id = 7;
  event.x = 12.5f;
  event.y = 9.25f;
  event.timestamp_ns = 1;
  PL_CHECK(region->pushInput(event));

  bool consumed = false;
  RoadStats stats{};
  for (int attempt = 0; attempt < 400 && !consumed; ++attempt) {
    region->stats(&stats);
    if (stats.input_consumed >= 1) {
      consumed = true;
    }
    usleep(5000);
  }
  PL_CHECK(consumed);
  PL_CHECK(stats.heartbeat_seq > 0);
  PL_CHECK(stats.heartbeat_age_ms < 1000);

  int status = 0;
  waitpid(child, &status, 0);
  PL_CHECK(WIFEXITED(status));
  PL_CHECK_EQ(WEXITSTATUS(status), 0);
}

PL_TEST(heartbeat_stops_when_the_companion_dies) {
  auto region = pl::ipc::SharedRegion::create(kWidth, kHeight);
  PL_CHECK(region != nullptr);
  const uint64_t size = region->size();
  const int fd = region->duplicateFd();
  PL_CHECK(fd >= 0);

  const pid_t child = fork();
  PL_CHECK(child >= 0);
  if (child == 0) {
    if (road_companion_attach(fd, size, kHeadless, 0) != 0) {
      _exit(11);
    }
    for (;;) {
      pause();
    }
  }
  close(fd);

  RoadStats stats{};
  bool alive = false;
  for (int attempt = 0; attempt < 400 && !alive; ++attempt) {
    region->stats(&stats);
    alive = stats.heartbeat_seq > 0 && stats.heartbeat_age_ms < 500;
    usleep(5000);
  }
  PL_CHECK(alive);

  kill(child, SIGKILL);
  int status = 0;
  waitpid(child, &status, 0);
  usleep(350000);
  region->stats(&stats);
  // The UI treats anything above two seconds as dead; here we only prove the
  // counter stops advancing.
  PL_CHECK(stats.heartbeat_age_ms > 200);
}

PL_TEST(push_command_reaches_the_companion) {
  auto region = pl::ipc::SharedRegion::create(kWidth, kHeight);
  PL_CHECK(region != nullptr);
  const uint64_t size = region->size();
  const int fd = region->duplicateFd();
  PL_CHECK(fd >= 0);

  const pid_t child = fork();
  PL_CHECK(child >= 0);
  if (child == 0) {
    if (road_companion_attach(fd, size, kHeadless, 0) != 0) {
      _exit(12);
    }
    for (;;) {
      pause();
    }
  }
  close(fd);

  PL_CHECK(region->pushCommand(1 /* CMD_SET_VOLUME */, 500));
  bool consumed = false;
  RoadStats stats{};
  for (int attempt = 0; attempt < 400 && !consumed; ++attempt) {
    region->stats(&stats);
    consumed = stats.cmd_consumed >= 1;
    usleep(5000);
  }
  PL_CHECK(consumed);

  kill(child, SIGKILL);
  int status = 0;
  waitpid(child, &status, 0);
}
