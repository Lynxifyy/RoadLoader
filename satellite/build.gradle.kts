plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The satellite ships armeabi-v7a so a 32-bit device (or a 64-bit device with
// 32-bit support) runs it as a genuine 32-bit process. arm64-v8a is included so
// the APK still installs on 64-bit-only devices, where it exercises the same
// cross-app IPC path with the 64-bit build of the core.
android {
    namespace = "com.caramel.roadloader.satellite"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.caramel.roadloader.satellite"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "2.1.0-roadloader-rc1"
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

dependencies {
    implementation(project(":road-loader-core"))
}
