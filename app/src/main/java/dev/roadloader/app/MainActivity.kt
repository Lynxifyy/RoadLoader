package dev.roadloader.app

import android.app.Activity
import android.app.AlertDialog
import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.ParcelFileDescriptor
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import dev.roadloader.core.ApkInspector
import dev.roadloader.core.CompanionAttach
import dev.roadloader.core.CompanionServiceBase
import dev.roadloader.core.CrashGuard
import dev.roadloader.core.FrameRateCounter
import dev.roadloader.core.FrameSurfaceView
import dev.roadloader.core.NativeHost
import dev.roadloader.core.NativeLibrary
import dev.roadloader.core.RoadAssets
import dev.roadloader.core.RoadAudioBridge
import dev.roadloader.core.RoadConstants
import dev.roadloader.core.RoadMemory
import dev.roadloader.core.RoadSession
import dev.roadloader.core.RoadStats
import dev.roadloader.core.ShaderLab
import java.io.File
import java.io.FileOutputStream

class MainActivity : Activity() {
    private lateinit var surface: FrameSurfaceView
    private lateinit var statusView: TextView
    private lateinit var statsView: TextView
    private lateinit var overlay: TextView

    private val handler = Handler(Looper.getMainLooper())
    private val guard by lazy { CrashGuard(this) }
    private var session: RoadSession? = null
    private var audio: RoadAudioBridge? = null
    private var importedApk: File? = null
    private var running = false
    private var usingSatellite = false
    private var firstFrameSeen = false
    private var heartbeatLost = false
    private var shadersEnabled = true
    private var safeMode = false
    private var developerMode = false
    private var companionAbi: String? = null
    private var companionIs64Bit = false
    private var companionUid = 0
    private var presentedFps = 0
    private var presentedTotal = 0L
    private var frameAgeMs = 0
    private var engineFps = 0.0
    private val engineRate = FrameRateCounter()
    private var lastMemoryWarningAt = 0L
    private var lastEvent: String? = null
    private var pendingDescriptor: ParcelFileDescriptor? = null
    private var pendingSize = 0L
    private var pendingFlags = 0
    private var bound = false
    private var companionMessenger: Messenger? = null

    private val attachReply = Messenger(
        Handler(Looper.getMainLooper()) { message ->
            if (message.what == CompanionServiceBase.MSG_ATTACHED) {
                companionIs64Bit = message.arg2 == 1
                companionAbi = message.data?.getString(CompanionServiceBase.EXTRA_ABI)
                companionUid = message.data?.getInt(CompanionServiceBase.EXTRA_CALLER_UID) ?: 0
                if (message.arg1 == CompanionServiceBase.ATTACH_OK) {
                    statusView.text = buildString {
                        append("Companion attached: ")
                        append(companionAbi ?: "abi unknown")
                        if (companionIs64Bit && !usingSatellite) {
                            append(
                                "\nThis is the same ABI as the UI. A real 32-bit engine " +
                                    "needs the armeabi-v7a satellite.",
                            )
                        }
                    }
                } else {
                    statusView.text = CompanionAttach.describe(message.arg1)
                }
            }
            true
        },
    )


    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            bound = true
            val descriptor = pendingDescriptor ?: return
            val messenger = binder?.let { Messenger(it) } ?: return
            companionMessenger = messenger
            val data = Bundle().apply {
                putParcelable(CompanionServiceBase.EXTRA_DESCRIPTOR, descriptor)
                putLong(CompanionServiceBase.EXTRA_SIZE, pendingSize)
                putInt(CompanionServiceBase.EXTRA_FLAGS, pendingFlags)
                putLong(CompanionServiceBase.EXTRA_HEAP_LIMIT, RoadMemory.DEFAULT_LIMIT_BYTES)
            }
            val message = Message.obtain(null, CompanionServiceBase.MSG_ATTACH).apply {
                this.data = data
                replyTo = attachReply
            }
            runCatching { messenger.send(message) }
            pendingDescriptor = null
            runCatching { descriptor.close() }
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            bound = false
            companionMessenger = null
        }
    }

    private val statsPoll = object : Runnable {
        override fun run() {
            pollStats()
            if (running) {
                handler.postDelayed(this, STATS_INTERVAL_MS)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        buildUi()
        requestNotificationPermission()
        refreshCoreStatus()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        stopCompanion(showToast = false)
        super.onDestroy()
    }

    // ------------------------------------------------------------------ UI

    private fun buildUi() {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.rgb(9, 12, 18))
            setPadding(24, 24, 24, 16)
        }

        root.addView(TextView(this).apply {
            text = "Road Loader"
            textSize = 22f
            setTextColor(Color.rgb(148, 233, 216))
        })

        statusView = TextView(this).apply {
            textSize = 12f
            setTextColor(Color.rgb(190, 198, 210))
            setPadding(0, 6, 0, 6)
        }
        root.addView(statusView)

        val surfaceHost = FrameLayout(this).apply {
            layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f)
            setBackgroundColor(Color.BLACK)
        }
        surface = FrameSurfaceView(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
            alpha = 0f
            onPresentStats = { stats ->
                presentedFps = stats.fps
                presentedTotal = stats.framesTotal
                frameAgeMs = stats.frameAgeMs
            }
            onFirstFrame = { onFirstFrame() }
        }
        surfaceHost.addView(surface)
        overlay = TextView(this).apply {
            textSize = 11f
            setTextColor(Color.rgb(255, 214, 120))
            setShadowLayer(4f, 0f, 0f, Color.BLACK)
            setPadding(16, 12, 16, 0)
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP or Gravity.START,
            )
            text = "idle"
        }
        surfaceHost.addView(overlay)
        root.addView(surfaceHost)

        statsView = TextView(this).apply {
            textSize = 11f
            setTextColor(Color.rgb(150, 158, 172))
            setPadding(0, 6, 0, 6)
        }
        root.addView(statsView)

        root.addView(
            buttonRow(
                button("Import APK") { pickApk() },
                button("Launch 32-bit") { launchCompanion(useSatellite = false) },
                button("Satellite") { launchCompanion(useSatellite = true) },
                button("Stop") { stopCompanion(showToast = true) },
            ),
        )
        root.addView(
            buttonRow(
                button("Self-test") { runSelfTest() },
                button("Shaders") { toggleShaders() },
                button("Safe mode") { toggleSafeMode() },
                button("Assets") { streamAssets() },
            ),
        )
        root.addView(
            buttonRow(
                button("Shader lab") { showShaderLab() },
                button("Dev mode") { toggleDeveloperMode() },
                button("Reset guard") {
                    guard.reset()
                    toast("Crash guard reset")
                    refreshCoreStatus()
                },
            ),
        )
        root.addView(
            buttonRow(
                button("Docs") { openDocs() },
                button("About") { showAbout() },
            ),
        )

        setContentView(root)
    }

    private fun buttonRow(vararg buttons: Button): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            buttons.forEach { child ->
                child.layoutParams =
                    LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
                child.textSize = 11f
                addView(child)
            }
        }

    private fun button(label: String, action: () -> Unit): Button =
        Button(this).apply {
            text = label
            setOnClickListener { action() }
        }

    private fun openDocs() {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(DOCS_URL))
        runCatching { startActivity(intent) }
            .onFailure { toast("No browser available") }
    }

    private fun showAbout() {
        val version = runCatching {
            packageManager.getPackageInfo(packageName, 0).versionName
        }.getOrNull() ?: "unknown"
        AlertDialog.Builder(this)
            .setTitle("Road Loader")
            .setMessage(
                "Version $version\n" +
                    "Primary ABI ${Build.SUPPORTED_ABIS.firstOrNull() ?: "unknown"}\n" +
                    "ABIs ${Build.SUPPORTED_ABIS.joinToString(", ")}\n" +
                    "Core ${NativeHost.coreStatus().lineSequence().firstOrNull()}\n\n" +
                    "Docs: $DOCS_URL",
            )
            .setPositiveButton("OK", null)
            .show()
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 7)
        }
    }

    // -------------------------------------------------------------- import

    private fun pickApk() {
        startActivityForResult(
            Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*"
            },
            REQUEST_IMPORT,
        )
    }

    @Deprecated("The framework callback is used to avoid an AndroidX dependency")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_IMPORT || resultCode != RESULT_OK) return
        data?.data?.let { copyAndInspect(it) }
    }

    private fun copyAndInspect(uri: Uri) {
        statusView.text = "Copying APK..."
        Thread {
            val target = File(filesDir, "imported.apk")
            val copied = runCatching {
                contentResolver.openInputStream(uri)?.use { input ->
                    FileOutputStream(target).use { output -> input.copyTo(output) }
                }
                target
            }.getOrNull()
            if (copied == null) {
                runOnUiThread { statusView.text = "Could not copy the selected file." }
                return@Thread
            }
            val report = ApkInspector.inspect(this, copied)
            importedApk = copied
            val assets = RoadAssets.listAssets(copied)
            runOnUiThread {
                statusView.text = buildString {
                    append(report.name)
                    report.packageName?.let { append("  ").append(it) }
                    report.versionName?.let { append(" v").append(it) }
                    append('\n').append(report.summary)
                    if (assets.isNotEmpty()) {
                        append("\nassets: ").append(assets.joinToString(", "))
                    }
                    append("\n\nThe demo renderer runs without the game; the APK is ")
                    append("inspected and staged for the hook layer.")
                }
            }
        }.start()
    }

    // -------------------------------------------------------------- launch

    private fun launchCompanion(useSatellite: Boolean) {
        if (running) {
            toast("Companion already running")
            return
        }
        if (guard.isLocked()) {
            AlertDialog.Builder(this)
                .setTitle("32-bit Engine Failure")
                .setMessage(
                    "The companion failed ${guard.attempts()} times in a row. Launch is " +
                        "blocked until the guard is reset or Safe Mode is used.",
                )
                .setPositiveButton("Safe Mode") { _, _ ->
                    safeMode = true
                    guard.reset()
                    launchCompanion(useSatellite)
                }
                .setNegativeButton("Reset guard") { _, _ -> guard.reset() }
                .setNeutralButton("Cancel", null)
                .show()
            return
        }
        if (useSatellite && !isSatelliteInstalled()) {
            AlertDialog.Builder(this)
                .setTitle("Satellite not installed")
                .setMessage(
                    "Install satellite/build/outputs/apk/release/satellite-release.apk " +
                        "(armeabi-v7a only) to run a real 32-bit engine process.\n\n" +
                        "If the device refuses to install it, it cannot execute 32-bit " +
                        "code at all, so no 32-bit engine is possible there.",
                )
                .setPositiveButton("OK", null)
                .show()
            return
        }

        val created = RoadSession.create(RENDER_WIDTH, RENDER_HEIGHT)
        if (created == null) {
            statusView.text = buildString {
                append("Could not create the shared region.\n")
                append(NativeLibrary.failureReason() ?: NativeHost.coreStatus())
            }
            return
        }
        session = created
        usingSatellite = useSatellite
        firstFrameSeen = false
        heartbeatLost = false
        running = true
        surface.session = created
        surface.alpha = 0f
        guard.beginAttempt()

        val flags = (if (shadersEnabled) RoadConstants.FLAG_SHADERS else 0) or
            (if (safeMode) RoadConstants.FLAG_SAFE_MODE else 0)
        val intent = if (useSatellite) {
            Intent().setComponent(ComponentName(SATELLITE_PACKAGE, SATELLITE_SERVICE))
        } else {
            Intent(this, RoadLoaderService::class.java)
        }
        pendingDescriptor = ParcelFileDescriptor.fromFd(created.duplicateFd())
        pendingSize = created.regionSize
        pendingFlags = flags
        val started = runCatching {
            startForegroundService(intent)
            bindService(intent, connection, 0)
        }.getOrElse { error ->
            statusView.text = "Failed to start companion: ${error.message}"
            running = false
            false
        }
        if (!started) {
            pendingDescriptor?.close()
            pendingDescriptor = null
            created.close()
            session = null
            surface.session = null
            return
        }

        val bridge = RoadAudioBridge(created)
        val audioStarted = bridge.start()
        audio = bridge
        statusView.text = if (useSatellite) {
            "Started the armeabi-v7a satellite service."
        } else {
            "Started the in-APK companion process (:road32)."
        }
        if (!audioStarted) {
            toast("Audio sink unavailable: ${bridge.error ?: "unknown"} (bridge drained)")
        }
        overlay.text = "waiting for first frame..."
        handler.post(statsPoll)
    }

    private fun onFirstFrame() {
        if (firstFrameSeen) return
        firstFrameSeen = true
        guard.markHealthy()
        surface.animate().alpha(1f).setDuration(350).start()
        statusView.text = if (usingSatellite) {
            "Streaming frames from the 32-bit satellite."
        } else {
            "Streaming frames from :road32."
        }
    }

    private fun stopCompanion(showToast: Boolean) {
        handler.removeCallbacks(statsPoll)
        audio?.stop()
        audio = null
        val active = session
        if (active != null) {
            active.pushCommand(RoadConstants.CMD_SHUTDOWN, 0)
            val intent = if (usingSatellite) {
                Intent().setComponent(ComponentName(SATELLITE_PACKAGE, SATELLITE_SERVICE))
            } else {
                Intent(this, RoadLoaderService::class.java)
            }
            if (bound) {
                runCatching { unbindService(connection) }
                bound = false
            }
            runCatching { stopService(intent) }
            active.close()
        }
        pendingDescriptor?.let { runCatching { it.close() } }
        pendingDescriptor = null
        companionMessenger = null
        session = null
        surface.session = null
        surface.alpha = 0f
        running = false
        presentedFps = 0
        presentedTotal = 0
        frameAgeMs = 0
        engineFps = 0.0
        engineRate.reset()
        companionAbi = null
        companionIs64Bit = false
        companionUid = 0
        if (showToast) toast("Companion stopped")
    }

    // --------------------------------------------------------------- stats

    private fun pollStats() {
        val active = session ?: return
        val stats = active.readStats()
        if (stats.status == RoadConstants.STATUS_CRASHED) {
            showCrash(stats)
            stopCompanion(showToast = false)
            return
        }
        if (stats.status == RoadConstants.STATUS_RUNNING &&
            stats.heartbeatAgeMs > RoadStats.HEARTBEAT_TIMEOUT_MS &&
            !heartbeatLost
        ) {
            heartbeatLost = true
            statusView.text = "32-bit engine stopped responding (heartbeat lost)."
            overlay.text = "companion dead"
            stopCompanion(showToast = false)
            return
        }
        if (stats.frameCount > 0 && !firstFrameSeen) {
            onFirstFrame()
        }
        // The companion's own frame counter is the only honest engine rate:
        // the GL loop draws at the display rate whether or not frames arrive.
        val rate = engineRate.sample(stats.frameCount, System.nanoTime())
        if (rate != null) {
            engineFps = rate
        }
        val memoryMessage = RoadMemory.applyPressurePolicy(active)
        if (memoryMessage != null && System.currentTimeMillis() - lastMemoryWarningAt > 15_000) {
            lastMemoryWarningAt = System.currentTimeMillis()
            toast(memoryMessage)
        }
        // The event ring has a single consumer: the UI drains it every poll so
        // the companion's IPC self-test always finds room.
        active.pollEvent()?.let { lastEvent = it }
        statsView.text = stats.describe()
        overlay.text = buildString {
            append("FPS ").append(engineFps.toInt())
            append(" (shown ").append(presentedFps).append(")")
            append(" | ").append(RoadStats.statusName(stats.status))
            append(" | heap ").append(stats.heapUsedMb()).append(" MB")
            append(" | hb ").append(stats.heartbeatAgeMs).append("ms")
            append(" | in ").append(stats.inputConsumed)
            append(" | ").append(
                when {
                    companionAbi == null -> "abi unknown"
                    // The status line is reused for errors, so the ABI note has
                    // to live somewhere that does not get overwritten.
                    companionIs64Bit && usingSatellite -> "64-bit (satellite)"
                    companionIs64Bit -> "64-bit (same ABI as UI)"
                    else -> "32-bit"
                },
            )
            if (firstFrameSeen && engineFps < 0.5) {
                // The last frame stays on screen, so say it is stale instead of
                // letting a frozen picture look like a running game.
                append(" | no frames")
            }
            if (developerMode) {
                append(" | age ").append(frameAgeMs).append("ms")
                append(" | shown total ").append(presentedTotal)
                companionAbi?.let { append(" | ").append(it) }
                if (companionUid != 0) append(" | uid ").append(companionUid)
                append("\nflags 0x").append(Integer.toHexString(stats.flags))
                append(" | last input ").append(stats.lastInputSeq)
                append(" | audio ").append(stats.audioPushed)
                append("/").append(audio?.blocksPlayed ?: 0)
                audio?.error?.let { append(" (").append(it).append(")") }
                append(" | render ").append(stats.renderAvgUs).append("us")
                lastEvent?.let { append("\nevent ").append(it) }
            }
        }
    }

    private fun showCrash(stats: RoadStats) {
        val signal = if (stats.crashSignal == 0L) "unknown" else "signal ${stats.crashSignal}"
        val address =
            if (stats.crashAddress == 0L) "unknown" else "0x${stats.crashAddress.toString(16)}"
        AlertDialog.Builder(this)
            .setTitle("32-bit Engine Failure")
            .setMessage("$signal at $address\n\n${stats.describe()}")
            .setPositiveButton("Safe Mode") { _, _ ->
                safeMode = true
                launchCompanion(usingSatellite)
            }
            .setNegativeButton("Close", null)
            .show()
    }

    // -------------------------------------------------------------- actions

    private fun runSelfTest() {
        val active = session ?: run {
            toast("Launch the companion first")
            return
        }
        active.requestSelfTest()
        handler.postDelayed({
            val mask = session?.readStats()?.selfTest ?: 0
            val bits = buildList {
                if (mask.toInt() and RoadConstants.TEST_MESH != 0) add("mesh")
                if (mask.toInt() and RoadConstants.TEST_MATH != 0) add("math")
                if (mask.toInt() and RoadConstants.TEST_PHYSICS != 0) add("physics")
                if (mask.toInt() and RoadConstants.TEST_ALLOC != 0) add("alloc")
                if (mask.toInt() and RoadConstants.TEST_IPC != 0) add("ipc")
            }
            val ok = mask.toInt() and RoadConstants.TEST_ALL_OK != 0
            AlertDialog.Builder(this)
                .setTitle(if (ok) "Self-test passed" else "Self-test failed")
                .setMessage(
                    "mask 0x${mask.toString(16)}\n" +
                        bits.joinToString(", ").ifEmpty { "no checks ran" },
                )
                .setPositiveButton("OK", null)
                .show()
        }, 700)
    }

    private fun toggleShaders() {
        shadersEnabled = !shadersEnabled
        session?.setShaders(shadersEnabled)
        toast("Shaders ${if (shadersEnabled) "enabled" else "disabled"}")
    }

    private fun toggleSafeMode() {
        safeMode = !safeMode
        if (safeMode) {
            session?.setSafeMode(true)
            toast("Safe Mode requested")
        } else {
            toast("Safe Mode can only be cleared by restarting the companion")
        }
    }

    private fun streamAssets() {
        val active = session ?: run {
            toast("Launch the companion first")
            return
        }
        val payload = RoadAssets.samplePayload(512 * 1024)
        Thread {
            val result = RoadAssets.preload(active, payload)
            runOnUiThread { statusView.text = result.describe() }
        }.start()
    }

    private fun showShaderLab() {
        val translated = ShaderLab.transpile(ShaderLab.SAMPLE_FRAGMENT, ShaderLab.STAGE_FRAGMENT)
        if (translated == null) {
            toast("Transpiler rejected the sample")
            return
        }
        AlertDialog.Builder(this)
            .setTitle("ES 3.0 output")
            .setMessage(translated.take(1200))
            .setPositiveButton("OK", null)
            .show()
    }

    private fun toggleDeveloperMode() {
        developerMode = !developerMode
        toast("Developer mode ${if (developerMode) "on" else "off"}")
    }

    // --------------------------------------------------------------- status

    private fun refreshCoreStatus() {
        val loaded = NativeLibrary.ensureLoaded()
        val guardState = "crash guard ${guard.attempts()}/${CrashGuard.MAX_ATTEMPTS}"
        val satellite = if (isSatelliteInstalled()) "satellite installed" else "satellite missing"
        statusView.text = if (loaded) {
            "${NativeHost.coreStatus()}\n${NativeHost.abiInfo()}\n$guardState | $satellite"
        } else {
            "Native library failed to load: ${NativeLibrary.failureReason()}"
        }
    }

    private fun isSatelliteInstalled(): Boolean =
        runCatching {
            packageManager.getPackageInfo(SATELLITE_PACKAGE, 0)
            true
        }.getOrDefault(false)

    private fun toast(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
    }

    companion object {
        private const val REQUEST_IMPORT = 1001
        private const val STATS_INTERVAL_MS = 400L
        private const val RENDER_WIDTH = 960
        private const val RENDER_HEIGHT = 540
        private const val SATELLITE_PACKAGE = "com.caramel.roadloader.satellite"
        private const val SATELLITE_SERVICE = "$SATELLITE_PACKAGE.SatelliteService"
        private const val DOCS_URL = "https://lynxifyy.github.io/RoadLoader/"
    }
}
