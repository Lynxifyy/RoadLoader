// Small status enum shared by the native modules.
#pragma once

#include <cstdint>

namespace pl {

enum class Status : int32_t {
  Ok = 0,
  InvalidArgument = -1,
  Unsupported = -2,
  NotFound = -3,
  OutOfMemory = -4,
  PermissionDenied = -5,
  AlreadyExists = -6,
  IoError = -7,
};

inline const char* statusName(Status status) {
  switch (status) {
    case Status::Ok:
      return "ok";
    case Status::InvalidArgument:
      return "invalid-argument";
    case Status::Unsupported:
      return "unsupported";
    case Status::NotFound:
      return "not-found";
    case Status::OutOfMemory:
      return "out-of-memory";
    case Status::PermissionDenied:
      return "permission-denied";
    case Status::AlreadyExists:
      return "already-exists";
    case Status::IoError:
      return "io-error";
  }
  return "unknown";
}

}  // namespace pl
