/*
 * SIAR Android Messenger Application (Jetpack Compose UI)
 * Dual-licensed under GNU Affero General Public License v3 (AGPLv3) and
 * Commercial License & Enterprise Exemption Agreement (SIAR-CEEL-1.0).
 * Copyright (c) 2026 Irshad Lab <contact@irshadlab.org>
 */

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.siar.messenger"
    // API 26 (Android 8.0) floor — Wi-Fi Aware (`android.net.wifi.aware`,
    // next.md §18-20) isn't available before API 26 at all, and this
    // app has no fallback path for a device that lacks it (Wi-Fi Direct/
    // BLE/Bluetooth Classic still work down-level, but next.md's own
    // §18 treats Aware as a first-class plane, not optional). A lower
    // floor would mean silently disabling a whole transport on older
    // devices rather than declaring the real requirement up front.
    compileSdk = 35

    defaultConfig {
        applicationId = "com.siar.messenger"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
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
    // Required for AGP to set up Compose resource/manifest merging at
    // all — the Compose compiler plugin above fixes IR lowering, this
    // is the separate, also-required flag that tells AGP itself this
    // module uses Compose. Omitting this is a different failure mode
    // than the one reported (a `buildFeatures` /Compose-related error
    // at a different task), not something the reported stack trace
    // pointed at directly, but a real, standard requirement worth
    // fixing preventively rather than waiting for it to surface as its
    // own separate error report.
    buildFeatures {
        compose = true
    }

    // The four existing Rust JNI crates (`siar-transport-wifi-direct`,
    // `siar-transport-wifi-aware`, `siar-transport-ble-android`,
    // `siar-transport-bluetooth-classic`) plus this app's own
    // `siar-android-connectivity` (`../rust-jni-glue/`) and
    // `siar-android-messaging` (`../messaging-jni/`) glue crates each
    // build their own `cdylib` — `cargo build --workspace --target
    // <android-abi>` (via `cargo-ndk`, not set up as a Gradle task here;
    // see this module's own top-level note on what this pass could and
    // couldn't actually run) produces one `.so` per crate per ABI. This
    // app expects them pre-built and copied into the standard
    // `jniLibs/<abi>/` layout below — genuinely real Android/Gradle
    // convention, not invented for this project, but the copy step
    // itself is a build-pipeline task this pass doesn't add (no NDK, no
    // cargo-ndk, no way to actually run either in this sandbox to get
    // that task right rather than guessed).
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
}
