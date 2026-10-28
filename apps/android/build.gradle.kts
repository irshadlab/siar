/*
 * SIAR Android Messenger Application
 * Dual-licensed under GNU Affero General Public License v3 (AGPLv3) and
 * Commercial License & Enterprise Exemption Agreement (SIAR-CEEL-1.0).
 * Copyright (c) 2026 Irshad Lab <contact@irshadlab.org>
 */

// Plugin versions pinned here, applied per-module — standard Gradle
// convention-plugin pattern, not a choice specific to this project.
// Versions chosen as reasonably current stable releases as of this
// pass; not pinned against any actual Gradle sync in this sandbox
// (no Android SDK/Gradle wrapper exists here to run one — see this
// module's own top-level doc comment in `settings.gradle.kts`'s
// neighbor files for the wider pattern of what could and couldn't be
// verified this pass).
plugins {
    id("com.android.application") version "8.7.2" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
    // Kotlin 2.0+ moved Compose's IR lowering out of the Kotlin
    // compiler itself and into this dedicated plugin — declaring
    // `androidx.compose.*` dependencies without applying it compiles
    // the Kotlin source fine but fails at IR lowering the moment a
    // Compose runtime intrinsic like `remember` needs inlining (the
    // exact `BackendException`/`CompilationException` this fixes).
    // Version matches the Kotlin plugin version above — the Compose
    // compiler plugin is versioned in lockstep with Kotlin itself as
    // of this scheme, not an independently-chosen version.
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.21" apply false
}
