package com.caramel.roadloader.satellite

import dev.roadloader.core.CompanionServiceBase

/** Runs the 32-bit engine in an armeabi-v7a-only application. */
class SatelliteService : CompanionServiceBase() {
    override val channelId = "satellite-companion"
    override val notificationTitle = "Road Loader 32-bit engine"
}
