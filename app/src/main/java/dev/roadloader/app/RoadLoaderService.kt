package dev.roadloader.app

import dev.roadloader.core.CompanionServiceBase

/** In-APK companion process (`:road32`). */
class RoadLoaderService : CompanionServiceBase() {
    override val channelId = "road32-companion"
    override val notificationTitle = "Road Loader companion"
}
