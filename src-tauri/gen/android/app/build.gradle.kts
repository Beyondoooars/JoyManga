import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val tauriConfFile = file("../../../tauri.conf.json")
val fallbackVersion = tauriProperties.getProperty("tauri.android.versionName", "0.0.0")
val parsedVersion: String = if (tauriConfFile.exists()) {
    val content = tauriConfFile.readText()
    val match = Regex("\"version\"\\s*:\\s*\"([^\"]+)\"").find(content)
    match?.groupValues?.get(1) ?: fallbackVersion
} else {
    fallbackVersion
}

val appVersionName: String = System.getenv("APP_VERSION_NAME") ?: parsedVersion

// 自动计算 versionCode: major * 10000 + minor * 100 + patch
val defaultVersionCode: Int = try {
    val clean = appVersionName.split("-")[0]
    val parts = clean.split(".")
    val major = parts.getOrNull(0)?.toIntOrNull() ?: 0
    val minor = parts.getOrNull(1)?.toIntOrNull() ?: 0
    val patch = parts.getOrNull(2)?.toIntOrNull() ?: 0
    major * 10000 + minor * 100 + patch
} catch (e: Exception) {
    202
}

val propVersionCode = tauriProperties.getProperty("tauri.android.versionCode")?.toIntOrNull() ?: 0
val appVersionCode: Int = System.getenv("APP_VERSION_CODE")?.toIntOrNull()
    ?: maxOf(propVersionCode, defaultVersionCode)

android {
    compileSdk = 34
    namespace = "com.joymanga.app"
    defaultConfig {
        // release 包默认禁止明文 HTTP（应用只访问 HTTPS 接口）；debug 包在下方单独放开，
        // 便于本地调试抓包。用 manifestPlaceholders 让 manifest 里的取值随构建类型变化。
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.joymanga.app"
        minSdk = 24
        targetSdk = 34
        versionCode = appVersionCode
        versionName = appVersionName
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }
    signingConfigs {
        create("release") {
            val keystoreFile = file("release.keystore")
            if (keystoreFile.exists()) {
                storeFile = keystoreFile
                storePassword = System.getenv("KEYSTORE_PASSWORD")
                keyAlias = System.getenv("KEY_ALIAS")
                keyPassword = System.getenv("KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

gradle.taskGraph.whenReady {
    if (allTasks.any { it.name.contains("Release") && (it.name.startsWith("assemble") || it.name.startsWith("package")) }) {
        require(file("release.keystore").isFile) { "Release signing keystore is required" }
        require(listOf("KEYSTORE_PASSWORD", "KEY_ALIAS", "KEY_PASSWORD").all { !System.getenv(it).isNullOrBlank() }) { "Release signing credentials are required" }
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.6.1")
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("com.google.android.material:material:1.8.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")