package dev.roadloader.core

/** Loads the JNI bridge once and remembers why it failed, if it did. */
object NativeLibrary {
    @Volatile
    private var loaded = false

    @Volatile
    private var failure: String? = null

    @Synchronized
    fun ensureLoaded(): Boolean {
        if (loaded) return true
        failure?.let { return false }
        return try {
            System.loadLibrary("roadloader32")
            loaded = true
            true
        } catch (error: Throwable) {
            failure = error.message ?: error.javaClass.simpleName
            false
        }
    }

    fun failureReason(): String? = failure
}
