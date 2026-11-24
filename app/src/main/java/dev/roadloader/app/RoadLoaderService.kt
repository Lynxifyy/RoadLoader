package dev.roadloader.app

import dev.roadloader.core.CompanionServiceBase

/**
 * In-APK companion process (`:road32`).
 *
 * A separate process, not a separate ABI: it runs at the ABI the APK was
 * installed with, so it is 64-bit on a 64-bit device. Only the armeabi-v7a
 * satellite is a genuine 32-bit engine there.
 */
class RoadLoaderService : CompanionServiceBase() {
    override val channelId = "road32-companion"
    override val notificationTitle = "Road Loader companion"
}
