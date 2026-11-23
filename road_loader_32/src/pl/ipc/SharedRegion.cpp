#include "pl/ipc/SharedRegion.h"

#include "pl/Log.h"

namespace pl::ipc {

Status SharedRegion::checkAbi() {
  if (road_stats_size() != sizeof(RoadStats) ||
      road_input_event_size() != sizeof(RoadInputEvent) ||
      road_command_size() != sizeof(RoadCommand) || road_event_size() != sizeof(RoadEvent) ||
      road_audio_block_size() != sizeof(RoadAudioBlock)) {
    PL_LOGE("road core ABI mismatch: stats=%u/%zu input=%u/%zu command=%u/%zu event=%u/%zu audio=%u/%zu",
            road_stats_size(), sizeof(RoadStats), road_input_event_size(), sizeof(RoadInputEvent),
            road_command_size(), sizeof(RoadCommand), road_event_size(), sizeof(RoadEvent),
            road_audio_block_size(), sizeof(RoadAudioBlock));
    return Status::Unsupported;
  }
  if (road_frame_info_size() != sizeof(RoadFrameInfo)) {
    PL_LOGE("RoadFrameInfo mismatch: %u/%zu", road_frame_info_size(), sizeof(RoadFrameInfo));
    return Status::Unsupported;
  }
  return Status::Ok;
}

std::unique_ptr<SharedRegion> SharedRegion::create(uint32_t width, uint32_t height) {
  if (checkAbi() != Status::Ok) {
    return nullptr;
  }
  void* handle = road_host_create(width, height);
  if (handle == nullptr) {
    PL_LOGE("road_host_create failed");
    return nullptr;
  }
  return std::unique_ptr<SharedRegion>(new SharedRegion(handle));
}

SharedRegion::~SharedRegion() {
  if (handle_ != nullptr) {
    road_host_destroy(handle_);
    handle_ = nullptr;
  }
}

int SharedRegion::duplicateFd() const { return handle_ != nullptr ? road_host_fd(handle_) : -1; }

uint64_t SharedRegion::size() const {
  return handle_ != nullptr ? road_host_region_size(handle_) : 0;
}

bool SharedRegion::pollFrame(RoadFrameInfo* out) {
  return handle_ != nullptr && road_host_poll_frame(handle_, out) == 1;
}

void SharedRegion::releaseFrame(uint32_t slot) {
  if (handle_ != nullptr) {
    road_host_release_frame(handle_, slot);
  }
}

bool SharedRegion::pushInput(const RoadInputEvent& event) {
  return handle_ != nullptr && road_host_push_input(handle_, &event) == 0;
}

bool SharedRegion::pushCommand(uint32_t kind, uint64_t value) {
  if (handle_ == nullptr) {
    return false;
  }
  RoadCommand command{};
  command.kind = kind;
  command.value = value;
  return road_host_push_command(handle_, &command) == 0;
}

bool SharedRegion::stats(RoadStats* out) const {
  return handle_ != nullptr && road_host_stats(handle_, out) == 0;
}

}  // namespace pl::ipc
