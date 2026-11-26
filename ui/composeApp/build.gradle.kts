import java.time.ZonedDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.compose.desktop.application.dsl.TargetFormat
import org.jetbrains.kotlin.gradle.tasks.KotlinCompilationTask

plugins {
    kotlin("multiplatform")
    kotlin("plugin.compose")
    id("org.jetbrains.compose")
    id("com.android.application")
}

val nativeScript = rootProject.projectDir.parentFile.resolve("tools/build/native.py")
val pythonCommand = providers.gradleProperty("paranoic.python").orElse(if (System.getProperty("os.name").startsWith("Windows")) "python" else "python3")
val generatedCoreBindings = layout.buildDirectory.dir("generated/uniffi")
val gitCommit = providers.exec {
    commandLine("git", "rev-parse", "--short=7", "HEAD")
    isIgnoreExitValue = true
}.standardOutput.asText.map { it.trim().ifEmpty { "nogit" } }.orElse("nogit")
val androidAbis = providers.gradleProperty("paranoic.androidAbis").orElse("arm64-v8a,x86_64")
// PDFium (B3): "fetch" = pinned, SHA-256-checked release (tools/build/native.py); "none" = offline build without PDF.
val pdfiumMode = providers.gradleProperty("paranoic.pdfium").orElse("fetch")
val generateCoreBindings by tasks.registering(Exec::class) {
    commandLine(pythonCommand.get(), nativeScript, "bindings")
    outputs.dir(generatedCoreBindings)
    // Cargo tracks Rust inputs. Always invoke it rather than caching stale ABI bindings.
    outputs.upToDateWhen { false }
}

kotlin {
    jvmToolchain(17)
    androidTarget { compilerOptions { jvmTarget.set(JvmTarget.JVM_17) } }
    jvm("desktop") { compilerOptions { jvmTarget.set(JvmTarget.JVM_17) } }
    sourceSets {
        commonMain.dependencies {
            implementation(compose.runtime)
            implementation(compose.foundation)
            implementation(compose.material3)
            implementation(compose.ui)
            // Material Symbols (Apache-2.0) for AppIcon; R8 strips unused vectors on Android.
            implementation(compose.materialIconsExtended)
            implementation(compose.components.resources)
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
        }
        getByName("desktopTest") {
            kotlin.srcDir("src/jvmTest/kotlin")
            dependencies { implementation(kotlin("test-junit")) }
        }
        getByName("androidUnitTest") {
            kotlin.srcDir("src/jvmTest/kotlin")
            dependencies { implementation(kotlin("test-junit")) }
        }
        getByName("androidInstrumentedTest").dependencies {
            implementation(kotlin("test-junit"))
            implementation("androidx.test:runner:1.7.0")
            implementation("androidx.test.ext:junit:1.3.0")
            implementation("org.jetbrains.compose.ui:ui-test-junit4:1.9.1")
        }
        androidMain.dependencies {
            implementation("androidx.activity:activity-compose:1.11.0")
            implementation("androidx.fragment:fragment-ktx:1.8.9")
            implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
            implementation("androidx.biometric:biometric:1.1.0")
            implementation("androidx.media3:media3-exoplayer:1.8.0")
            implementation("androidx.media3:media3-ui:1.8.0")
            implementation("net.java.dev.jna:jna:5.17.0@aar")
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
        }
        val desktopMain by getting {
            dependencies {
                implementation(compose.desktop.currentOs)
                implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.10.2")
                implementation("net.java.dev.jna:jna:5.17.0")
                implementation("net.java.dev.jna:jna-platform:5.17.0")
                val os = System.getProperty("os.name")
                val arm = System.getProperty("os.arch").contains("aarch64") || System.getProperty("os.arch").contains("arm64")
                val platform = when {
                    os.startsWith("Windows") -> "win"
                    os.startsWith("Mac") -> if (arm) "mac-aarch64" else "mac"
                    else -> if (arm) "linux-aarch64" else "linux"
                }
                listOf("base", "graphics", "swing", "media").forEach {
                    implementation("org.openjfx:javafx-$it:21.0.13:$platform")
                }
            }
        }
        // JVM-only bridge/UniFFI code is compiled by each leaf, never as common metadata.
        // Carry the producer dependency with the directory, including clean checkouts.
        val bindingsSources = files(generatedCoreBindings).builtBy(generateCoreBindings)
        getByName("androidMain").kotlin.srcDirs("src/jvmMain/kotlin", bindingsSources)
        desktopMain.kotlin.srcDirs("src/jvmMain/kotlin", bindingsSources)
        desktopMain.resources.srcDir(layout.buildDirectory.dir("native/resources"))
        desktopMain.resources.srcDir("native-libs/desktop")
    }
}

android {
    namespace = "io.github.stratayun.paranoic"
    compileSdk = 36
    defaultConfig {
        applicationId = "io.github.stratayun.paranoic"
        minSdk = 26
        targetSdk = 36
        // Every build is identifiable on the phone (Diagnostics → "App:"):
        // commit and UTC build time; versionCode grows so installs never look like a downgrade.
        val built = ZonedDateTime.now(ZoneOffset.UTC)
        versionCode = ((built.toEpochSecond() - 1_767_225_600L) / 60).toInt().coerceAtLeast(1)
        versionName = "2.1 · ${gitCommit.get()} · ${built.format(DateTimeFormatter.ofPattern("dd.MM HH:mm"))} UTC"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += androidAbis.get().split(",") }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("native/android"))
    sourceSets["main"].jniLibs.srcDir("native-libs/android")
    // License notices of the bundled PDFium (tools/build/native.py android).
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("native/android-assets"))
    // Extract native libs at install: the PDF worker (libparanoic_pdf_worker.so)
    // must exist as an executable file in nativeLibraryDir.
    packaging { jniLibs { useLegacyPackaging = true } }
    // One project key for every APK (owner 26.11), so each build installs over the last.
    // The key never enters the repository: CI decodes it from GitHub Secrets, local
    // builds point PARANOIC_KEYSTORE at the owner's copy. Without it: one-off debug key.
    val keystore = providers.environmentVariable("PARANOIC_KEYSTORE").orNull?.takeIf { it.isNotBlank() && file(it).isFile }
    if (keystore != null) {
        val password = providers.environmentVariable("PARANOIC_KEYSTORE_PASSWORD").get()
        val project = signingConfigs.create("paranoic") {
            storeFile = file(keystore); storeType = "PKCS12"; storePassword = password
            keyAlias = providers.environmentVariable("PARANOIC_KEY_ALIAS").getOrElse("paranoic"); keyPassword = password
        }
        buildTypes.getByName("debug").signingConfig = project
        buildTypes.getByName("release").signingConfig = project
    }
}

dependencies {
    // Empty test Activity supplied by Compose; tests never launch the real Core UI.
    add("debugImplementation", "androidx.compose.ui:ui-test-manifest:1.9.3")
}
val prepareDesktopCore by tasks.registering(Exec::class) {
    commandLine(pythonCommand.get(), nativeScript, "desktop", "--pdfium", pdfiumMode.get())
}
val prepareAndroidCore by tasks.registering(Exec::class) {
    commandLine(pythonCommand.get(), nativeScript, "android", "--abis", androidAbis.get(), "--pdfium", pdfiumMode.get())
}
prepareDesktopCore { mustRunAfter(generateCoreBindings) }
prepareAndroidCore { mustRunAfter(generateCoreBindings) }
tasks.withType<KotlinCompilationTask<*>>().configureEach { dependsOn(generateCoreBindings) }
tasks.matching { it.name == "desktopProcessResources" }.configureEach { dependsOn(prepareDesktopCore) }
// The source-directory merge snapshots jniLibs before merge*NativeLibs.
// On a clean checkout the Rust outputs must exist before either task runs.
tasks.matching { it.name.startsWith("merge") &&
    (it.name.endsWith("JniLibFolders") || it.name.endsWith("NativeLibs"))
}.configureEach { dependsOn(prepareAndroidCore) }
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("Assets") }.configureEach { dependsOn(prepareAndroidCore) }

compose.resources {
    packageOfResClass = "io.github.stratayun.paranoic.resources"
}

compose.desktop {
    application {
        mainClass = "io.github.stratayun.paranoic.platform.MainKt"
        nativeDistributions {
            targetFormats(TargetFormat.Msi)
            packageName = "Paranoic"
            packageVersion = "2.1.0"
            // From :composeApp:suggestRuntimeModules; jdk.unsupported is needed by Skiko and JNA,
            // without it the packaged app exits before its window (first Windows run, 25.11).
            modules("java.desktop", "java.instrument", "java.prefs", "jdk.httpserver", "jdk.jfr", "jdk.unsupported", "jdk.unsupported.desktop")
            // PDF worker executable (tools/build/native.py desktop); found only via compose.application.resources.dir.
            appResourcesRootDir.set(layout.buildDirectory.dir("native/app-resources"))
            // -Pparanoic.winConsole: a console launcher that prints any startup failure,
            // including one before the app's own code (diagnostic build only).
            windows { console = providers.gradleProperty("paranoic.winConsole").isPresent }
        }
    }
}

