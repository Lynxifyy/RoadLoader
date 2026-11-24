package dev.roadloader.core

import android.content.Context
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.zip.ZipFile

/** Reads the parts of a Minecraft APK that decide whether 32-bit can run it. */
object ApkInspector {
    data class Report(
        val name: String,
        val packageName: String?,
        val versionName: String?,
        val abi: String?,
        val elf: String?,
        val compatible: Boolean,
        val summary: String,
        /** Staged copy of the 32-bit engine, when the APK has one. */
        val engine: File? = null,
        val engineSha256: String? = null,
    )

    private val nativeLibNames = listOf("libminecraftpe.so", "libminecraftpe32.so")

    private fun sha256(file: File): String? = runCatching {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(1 shl 16)
            while (true) {
                val read = input.read(buffer)
                if (read <= 0) break
                digest.update(buffer, 0, read)
            }
        }
        digest.digest().joinToString("") { "%02x".format(it) }
    }.getOrNull()

    /** Where an imported APK's engine is kept between launches. */
    fun stagedEngine(context: Context): File =
        File(File(context.filesDir, "engine"), "libminecraftpe.so")

    fun inspect(context: Context, apk: File): Report {
        val archiveInfo = context.packageManager.getPackageArchiveInfo(apk.absolutePath, 0)
        var packageName = archiveInfo?.packageName
        var versionName = archiveInfo?.versionName

        var abi: String? = null
        var elf: String? = null
        var summary = ""
        var compatible = false
        var stagedEngine: File? = null
        var stagedSha: String? = null
        try {
            ZipFile(apk).use { zip ->
                val entries = zip.entries().toList()
                val candidates = entries.filter { entry ->
                    entry.name.startsWith("lib/") && nativeLibNames.any { entry.name.endsWith(it) }
                }
                val v7a = candidates.firstOrNull { it.name.startsWith("lib/armeabi-v7a/") }
                val v8a = candidates.firstOrNull { it.name.startsWith("lib/arm64-v8a/") }
                abi = when {
                    v7a != null && v8a != null -> "armeabi-v7a + arm64-v8a"
                    v7a != null -> "armeabi-v7a"
                    v8a != null -> "arm64-v8a"
                    else -> null
                }
                val target = v7a ?: candidates.firstOrNull()
                if (target == null) {
                    summary = if (entries.any { it.name.startsWith("lib/") }) {
                        "No libminecraftpe.so found: this APK does not contain the game engine."
                    } else {
                        "No native libraries in this APK; a split base APK keeps them in its ABI split."
                    }
                } else {
                    // The engine is kept, not deleted: it is what the hook layer
                    // reads, and a 98 MB copy is worth keeping rather than
                    // repeating on every inspection.
                    val staged = stagedEngine(context)
                    staged.parentFile?.mkdirs()
                    if (staged.length() != target.size) {
                        zip.getInputStream(target).use { input ->
                            FileOutputStream(staged).use { output -> input.copyTo(output) }
                        }
                    }
                    stagedEngine = staged
                    stagedSha = sha256(staged)
                    elf = NativeHost.probeElf(staged.absolutePath)
                    val isArm32 = elf?.contains("class=32") == true && elf?.contains("machine=arm") == true
                    compatible = v7a != null && isArm32
                    summary = buildString {
                        append("native: ").append(target.name)
                        append(" ").append(target.size / (1024 * 1024)).append(" MB\n")
                        append("sha256: ").append(stagedSha?.take(16) ?: "unreadable").append('\n')
                        append("abi: ").append(abi ?: "none").append('\n')
                        append("elf: ").append(elf ?: "unreadable").append('\n')
                        append(
                            if (compatible) {
                                "Compatible: 32-bit ARM engine found."
                            } else if (v7a == null) {
                                "Not compatible: no armeabi-v7a library in this APK."
                            } else {
                                "Not compatible: the armeabi-v7a library is not a 32-bit ARM ELF."
                            },
                        )
                    }
                }
            }
        } catch (error: Throwable) {
            summary = "Could not read APK: ${error.message ?: error.javaClass.simpleName}"
        }
        if (packageName == null) packageName = archiveInfo?.packageName
        if (versionName == null) versionName = archiveInfo?.versionName
        return Report(
            name = apk.name,
            packageName = packageName,
            versionName = versionName,
            abi = abi,
            elf = elf,
            compatible = compatible,
            summary = summary.ifEmpty { "No native library information." },
            engine = stagedEngine,
            engineSha256 = stagedSha,
        )
    }
}
