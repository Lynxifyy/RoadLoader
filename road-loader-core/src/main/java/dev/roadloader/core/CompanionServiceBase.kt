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
import android.os.Binder
import android.os.Message
import android.os.Messenger
import android.os.ParcelFileDescriptor
import android.os.Process
import android.os.RemoteException
import android.util.Log

/**
 * Base class for the process that hosts the 32-bit engine.
 *
 * The descriptor travels over Binder rather than as an intent extra: Android 14
 * refuses to parcel file descriptors in intents, while Binder transactions
 * carry them natively.
 *
 * The service may be exported (the satellite is, so another app can drive it),
 * so the attach path trusts nothing: the region size is capped, the descriptor
 * is validated by the core, only one session is allowed, and the caller's uid is
 * logged. The caller already owns the region, so an attach grants it no read
 * access to engine memory.
 */
abstract class CompanionServiceBase : Service() {
    protected abstract val channelId: String
    protected abstract val notificationTitle: String

    private lateinit var worker: HandlerThread
    private lateinit var messenger: Messenger
    private var attached = false
    private var attachedUid = 0

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
                MSG_QUERY_ABI -> replyAbi(message)
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
        // Without a client the companion has nothing to render, so it must not
        // be restarted by the system after being killed.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        NativeCompanion.stop()
        worker.quitSafely()
        super.onDestroy()
    }

    private fun handleAttach(message: Message) {
        val data: Bundle = message.data
        val descriptor = data.getParcelable<ParcelFileDescriptor>(EXTRA_DESCRIPTOR)
        val callerUid = Binder.getCallingUid()
        val result = if (descriptor == null) {
            ATTACH_BAD_ARGUMENT
        } else {
            val size = data.getLong(EXTRA_SIZE, 0)
            val flags = data.getInt(EXTRA_FLAGS, 0)
            val heapLimit = data.getLong(EXTRA_HEAP_LIMIT, 0)
            when {
                !CompanionAttach.sizeAllowed(size) && size > 0 -> ATTACH_TOO_LARGE
                size <= 0 -> ATTACH_BAD_ARGUMENT
                else -> {
                    Log.i(TAG, "attach from uid $callerUid: $size bytes")
                    val status = try {
                        NativeCompanion.attach(descriptor.fd, size, flags, heapLimit)
                    } finally {
                        // Closing on every path keeps a failed attach from
                        // leaking the caller's descriptor into the companion.
                        descriptor.close()
                    }
                    if (status == 0) {
                        attached = true
                        attachedUid = callerUid
                        Handler(mainLooper).removeCallbacks(attachTimeout)
                        Log.i(TAG, "attached: ${abiReport()}")
                    } else {
                        Log.w(TAG, "attach from uid $callerUid refused: $status")
                    }
                    status
                }
            }
        }
        sendAbiReply(message, result)
    }

    private fun replyAbi(message: Message) {
        sendAbiReply(message, if (attached) ATTACH_OK else ATTACH_BAD_ARGUMENT)
    }

    private fun sendAbiReply(message: Message, result: Int) {
        val reply = Message.obtain(null, MSG_ATTACHED, result, if (Process.is64Bit()) 1 else 0)
        reply.data = Bundle().apply {
            putString(EXTRA_ABI, abiReport())
            putInt(EXTRA_CALLER_UID, attachedUid)
        }
        runCatching { message.replyTo?.send(reply) }
            .onFailure { error -> if (error !is RemoteException) Log.w(TAG, "reply failed: $error") }
    }

    /**
     * Describes the process this service runs in. `Process.is64Bit()` is the
     * authority: an APK that ships both ABIs runs its processes at the ABI the
     * device picked at install time, so a `:road32` process in a 64-bit install
     * is 64-bit even though the service is named for 32 bits.
     */
    private fun abiReport(): String {
        val bits = if (Process.is64Bit()) "64-bit" else "32-bit"
        val detail = runCatching { NativeCompanion.abiInfo() }.getOrDefault("native not loaded")
        return "$bits ($detail)"
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
        const val MSG_QUERY_ABI = 4

        const val ATTACH_OK = CompanionAttach.OK
        const val ATTACH_BAD_ARGUMENT = CompanionAttach.BAD_ARGUMENT
        const val ATTACH_BAD_ABI = CompanionAttach.BAD_ABI
        const val ATTACH_BUSY = CompanionAttach.BUSY
        const val ATTACH_TOO_LARGE = CompanionAttach.TOO_LARGE

        const val EXTRA_DESCRIPTOR = "descriptor"
        const val EXTRA_SIZE = "regionSize"
        const val EXTRA_FLAGS = "flags"
        const val EXTRA_HEAP_LIMIT = "heapLimit"
        const val EXTRA_ABI = "abi"
        const val EXTRA_CALLER_UID = "callerUid"

        private const val TAG = "CompanionService"
        private const val ATTACH_TIMEOUT_MS = 15_000L
        private const val NOTIFICATION_ID = 0x524F4144
    }
}
