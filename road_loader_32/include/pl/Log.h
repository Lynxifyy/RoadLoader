// Road Loader 32-bit companion: logging helpers.
#pragma once

#include <cstdarg>
#include <cstdio>

#if defined(__ANDROID__)
#include <android/log.h>
#define PL_LOG(level, tag, ...) __android_log_print(level, tag, __VA_ARGS__)
#else
#define PL_LOG(level, tag, ...)                        \
  do {                                                 \
    std::fprintf(stderr, "[%s] ", tag);                \
    std::fprintf(stderr, __VA_ARGS__);                 \
    std::fprintf(stderr, "\n");                        \
  } while (0)
#endif

#define PL_TAG "RoadLoader32"
#define PL_LOGI(...) PL_LOG(ANDROID_LOG_INFO, PL_TAG, __VA_ARGS__)
#define PL_LOGW(...) PL_LOG(ANDROID_LOG_WARN, PL_TAG, __VA_ARGS__)
#define PL_LOGE(...) PL_LOG(ANDROID_LOG_ERROR, PL_TAG, __VA_ARGS__)
#define PL_LOGD(...) PL_LOG(ANDROID_LOG_DEBUG, PL_TAG, __VA_ARGS__)
