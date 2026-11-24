plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// armeabi-v7a only, deliberately. An APK that ships both ABIs is installed at
// the device's preferred ABI, so on a 64-bit device every process in it is
// 64-bit no matter what the service is called. A 32-bit-only APK is the only
// way to get a genuine 32-bit process on such a device; it will not install on
// 64-bit-only hardware, which is correct because that hardware cannot execute
// 32-bit code at all.
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
            abiFilters += "armeabi-v7a"
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
