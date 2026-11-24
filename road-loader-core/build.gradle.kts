plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

// The Rust core and the C++ bridge are built for both ABIs here so every app
// module consumes the exact same artifacts.
val rustSources = rootProject.fileTree("rust") {
    include("**/*.rs", "**/Cargo.toml", "**/Cargo.lock")
    exclude("target/**", "**/target/**")
}
val rustStagedLibs = files(
    "src/main/jniLibs/armeabi-v7a/libroad_core_32.so",
    "src/main/jniLibs/arm64-v8a/libroad_core_32.so",
)

val cargoBuild = tasks.register<Exec>("cargoBuild") {
    description = "Cross-compiles libroad_core_32.so for both ABIs"
    group = "build"
    workingDir = rootProject.projectDir
    commandLine("bash", "scripts/build_rust.sh", "release")
    environment("PATH", "${System.getProperty("user.home")}/.cargo/bin:" + (System.getenv("PATH") ?: ""))
    environment("ANDROID_HOME", android.sdkDirectory.absolutePath)
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    inputs.files(rustSources, rootProject.file("scripts/build_rust.sh"))
        .withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.property("ndkVersion", android.ndkVersion)
    outputs.files(rustStagedLibs)
}

tasks.matching { it.name.startsWith("configureCMake") || it.name.startsWith("buildCMake") }
    .configureEach { dependsOn(cargoBuild) }
tasks.named("preBuild") { dependsOn(cargoBuild) }

android {
    namespace = "dev.roadloader.core"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        minSdk = 26
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a")
        }
        externalNativeBuild {
            cmake {
                arguments += listOf("-DANDROID_STL=c++_static")
                cppFlags += "-std=c++17"
            }
        }
    }

    externalNativeBuild {
        cmake {
            path = file("../road_loader_32/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    packaging {
        jniLibs {
            // The core library is dlopen'ed by name; keep it uncompressed and
            // directly extractable from the APK.
            useLegacyPackaging = false
        }
    }
}

dependencies {
    implementation("org.jetbrains.kotlin:kotlin-stdlib:1.9.25")
    testImplementation("junit:junit:4.13.2")
}
