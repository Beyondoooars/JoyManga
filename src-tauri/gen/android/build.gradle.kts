buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        // Tauri 2.12 要求 Gradle 8.13+；其 Android 库使用 compileSdk 36，需 AGP 8.10+。
        // Kotlin 2.2 用于读取该库的 Kotlin 2.1 元数据，并支持 Gradle 8.13。
        classpath("com.android.tools.build:gradle:8.10.1")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.10")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

tasks.register("clean").configure {
    delete("build")
}

