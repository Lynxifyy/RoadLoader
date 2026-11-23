package dev.roadloader.core

import android.content.Context

/**
 * Remembers companion launch attempts so three failures in a row disable the
 * 32-bit path instead of looping on a crash (Phase 10).
 */
class CrashGuard(context: Context) {
    private val preferences =
        context.getSharedPreferences("road_loader_crash_guard", Context.MODE_PRIVATE)

    fun beginAttempt(): Int {
        val attempts = attempts() + 1
        preferences.edit().putInt(KEY_ATTEMPTS, attempts).apply()
        return attempts
    }

    fun markHealthy() {
        if (attempts() != 0) {
            preferences.edit().putInt(KEY_ATTEMPTS, 0).apply()
        }
    }

    fun attempts(): Int = preferences.getInt(KEY_ATTEMPTS, 0)

    fun isLocked(): Boolean = attempts() >= MAX_ATTEMPTS

    fun reset() {
        preferences.edit().clear().apply()
    }

    companion object {
        const val MAX_ATTEMPTS = 3
        private const val KEY_ATTEMPTS = "attempts"
    }
}
