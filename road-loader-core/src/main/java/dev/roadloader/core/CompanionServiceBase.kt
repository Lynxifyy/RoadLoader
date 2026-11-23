package dev.roadloader.core

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import android.os.ParcelFileDescriptor
import android.os.RemoteException

/**
 * Base class for the process that hosts the 32-bit engine.
 *
 * The descriptor travels over Binder rather than as an intent extra: Android 14
 * refuses to parcel file descriptors in intents, while Binder transactions
 * carry them natively.
 */
abstract class CompanionServiceBase : Service() {
    protected abstract val channelId: String
    protected abstract val notificationTitle: String

    private lateinit var worker: HandlerThread
    private lateinit var messenger: Messenger
    private var attached = false

    private val attachTimeout = Runnable {
        if (!attached) {
            stopSelf()
        }
    }

    override fun onCreate() {
        super.onCreate()
        worker = HandlerThread("road-companion-binder").apply { start() }
        messenger = Messenger(Handler(worker.looper) { message ->
            when (message.what) {
                MSG_ATTACH -> handleAttach(message)
                MSG_DETACH -> {
                    NativeCompanion.stop()
                    attached = false
                }
            }
            true
        })
    }

    override fun onBind(intent: Intent?): IBinder = messenger.binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        startForeground(NOTIFICATION_ID, buildNotification())
        if (!attached) {
            Handler(mainLooper).postDelayed(attachTimeout, ATTACH_TIMEOUT_MS)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        NativeCompanion.stop()
        worker.quitSafely()
        super.onDestroy()
    }

    private fun handleAttach(message: Message) {
        val data: Bundle = message.data
        val descriptor = data.getParcelable<ParcelFileDescriptor>(EXTRA_DESCRIPTOR)
        val result = if (descriptor == null) {
            ATTACH_BAD_ARGUMENT
        } else {
            val size = data.getLong(EXTRA_SIZE, 0)
            val flags = data.getInt(EXTRA_FLAGS, 0)
            val heapLimit = data.getLong(EXTRA_HEAP_LIMIT, 0)
            val status = NativeCompanion.attach(descriptor.fd, size, flags, heapLimit)
            descriptor.close()
            if (status == 0) {
                attached = true
                Handler(mainLooper).removeCallbacks(attachTimeout)
            }
            status
        }
        runCatching {
            message.replyTo?.send(Message.obtain(null, MSG_ATTACHED, result, 0))
        }.onFailure { error ->
            if (error is RemoteException) return@onFailure
        }
    }

    private fun createChannel() {
        val manager = getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(channelId) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    channelId,
                    notificationTitle,
                    NotificationManager.IMPORTANCE_LOW,
                ).apply {
                    description = "Keeps the 32-bit companion process alive"
                    setShowBadge(false)
                },
            )
        }
    }

    private fun buildNotification(): Notification =
        Notification.Builder(this, channelId)
            .setContentTitle(notificationTitle)
            .setContentText("Running in the background")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setOngoing(true)
            .build()

    companion object {
        const val MSG_ATTACH = 1
        const val MSG_ATTACHED = 2
        const val MSG_DETACH = 3

        const val ATTACH_OK = 0
        const val ATTACH_BAD_ARGUMENT = -1

        const val EXTRA_DESCRIPTOR = "descriptor"
        const val EXTRA_SIZE = "regionSize"
        const val EXTRA_FLAGS = "flags"
        const val EXTRA_HEAP_LIMIT = "heapLimit"

        private const val ATTACH_TIMEOUT_MS = 15_000L
        private const val NOTIFICATION_ID = 0x524F4144
    }
}
