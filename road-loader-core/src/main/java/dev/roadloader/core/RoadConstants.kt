package dev.roadloader.core

/** Values shared with the Rust core; keep in sync with `ipc/layout.rs`. */
object RoadConstants {
    const val ABI_VERSION = 1

    const val STATUS_EMPTY = 0
    const val STATUS_INITIALIZING = 1
    const val STATUS_RUNNING = 2
    const val STATUS_STOPPED = 3
    const val STATUS_CRASHED = 4
    const val STATUS_SAFE_MODE = 5

    const val FLAG_SAFE_MODE = 1 shl 0
    const val FLAG_RENDER_TEST_SCENE = 1 shl 1
    const val FLAG_SELF_TEST = 1 shl 2
    const val FLAG_HEADLESS = 1 shl 3
    const val FLAG_SHADERS = 1 shl 4

    const val INPUT_DOWN = 1
    const val INPUT_MOVE = 2
    const val INPUT_UP = 3
    const val INPUT_CANCEL = 4
    const val INPUT_KEY = 5
    const val INPUT_BUTTON = 6
    const val INPUT_STICK = 7

    const val CMD_SET_VOLUME = 1
    const val CMD_PATCH_OFFSET = 2
    const val CMD_SET_RENDER_DISTANCE = 3
    const val CMD_REQUEST_SELF_TEST = 4
    const val CMD_SHUTDOWN = 5
    const val CMD_ASSET_CHUNK = 6
    const val CMD_SET_SHADERS = 7
    const val CMD_SET_SAFE_MODE = 8

    /** Opens an asset stream: value = (asset id shl 32) or chunk count. */
    const val CMD_ASSET_BEGIN = 9

    /**
     * Ends an asset stream: value = (asset id shl 32) or expected bytes,
     * payload = 4-byte little-endian CRC32 of the whole asset.
     */
    const val CMD_ASSET_COMMIT = 10

    /** Payload bytes per asset chunk, matching `ASSET_CHUNK_DATA` in assets.rs. */
    const val ASSET_CHUNK_BYTES = 192

    const val ASSET_STATUS_NONE = 0
    const val ASSET_STATUS_PENDING = 1
    const val ASSET_STATUS_OK = 2
    const val ASSET_STATUS_EMPTY = 3
    const val ASSET_STATUS_RANGE = 4
    const val ASSET_STATUS_CHECKSUM = 5
    const val ASSET_STATUS_TOTAL = 6
    const val ASSET_STATUS_DUPLICATE = 7
    const val ASSET_STATUS_TOO_LARGE = 8
    const val ASSET_STATUS_CRC_MISMATCH = 9
    const val ASSET_STATUS_LENGTH_MISMATCH = 10
    const val ASSET_STATUS_NO_BEGIN = 11
    const val ASSET_STATUS_INCOMPLETE = 12
    const val ASSET_STATUS_BAD_TOTAL = 13

    const val FRAME_SLOT_COUNT = 3

    /** The only pixel format the companion publishes: 8-bit RGBA, 4 bytes/px. */
    const val FRAME_FORMAT_RGBA8888 = 1

    /**
     * Largest region the companion will map (matches `MAX_REGION_BYTES` in
     * `ipc/layout.rs`). A 3840x2160 region is about 95 MiB.
     */
    const val MAX_REGION_BYTES = 128L * 1024 * 1024

    // Indexes of the `LongArray` filled by `NativeHost.pollFrame`.
    const val FRAME_INFO_PTR = 0
    const val FRAME_INFO_SEQ = 1
    const val FRAME_INFO_SLOT = 2
    const val FRAME_INFO_WIDTH = 3
    const val FRAME_INFO_HEIGHT = 4
    const val FRAME_INFO_STRIDE = 5
    const val FRAME_INFO_LENGTH = 6
    const val FRAME_INFO_PUBLISHED_NS = 7
    const val FRAME_INFO_FIELDS = 8

    // Self-test bits reported by the core.
    const val TEST_MESH = 1 shl 0
    const val TEST_MATH = 1 shl 1
    const val TEST_PHYSICS = 1 shl 2
    const val TEST_ALLOC = 1 shl 3
    const val TEST_IPC = 1 shl 4
    const val TEST_ALL_OK = 1 shl 31

    // Indexes of `RoadSession.stats()`.
    const val STATS_STATUS = 0
    const val STATS_FLAGS = 1
    const val STATS_PID = 2
    const val STATS_WIDTH = 3
    const val STATS_HEIGHT = 4
    const val STATS_HEARTBEAT_SEQ = 5
    const val STATS_HEARTBEAT_AGE_MS = 6
    const val STATS_FRAME_COUNT = 7
    const val STATS_PRESENTED = 8
    const val STATS_DROPPED = 9
    const val STATS_INPUT_PUSHED = 10
    const val STATS_INPUT_CONSUMED = 11
    const val STATS_INPUT_DROPPED = 12
    const val STATS_CMD_PUSHED = 13
    const val STATS_CMD_CONSUMED = 14
    const val STATS_EVENT_PUSHED = 15
    const val STATS_EVENT_CONSUMED = 16
    const val STATS_HEAP_USED = 17
    const val STATS_HEAP_PEAK = 18
    const val STATS_HEAP_LIMIT = 19
    const val STATS_RENDER_AVG_US = 20
    const val STATS_RENDER_MAX_US = 21
    const val STATS_SELF_TEST = 22
    const val STATS_CRASH_SIGNAL = 23
    const val STATS_CRASH_ADDR = 24
    const val STATS_LAST_INPUT_SEQ = 25
    const val STATS_LAST_INPUT_X = 26
    const val STATS_LAST_INPUT_Y = 27
    const val STATS_AUDIO_PUSHED = 28
    const val STATS_AUDIO_CONSUMED = 29
    const val STATS_AUDIO_DROPPED = 30
    const val STATS_LAST_INPUT_KIND = 31
    const val STATS_ASSET_BYTES = 32
    const val STATS_ASSET_CRC = 33
    const val STATS_ASSET_STATUS = 34
    const val STATS_LENGTH = 35
}
