plugins {
    `kotlin-dsl`
}

gradlePlugin {
    plugins {
        create("pluginsForCoolKids") {
            id = "rust"
            implementationClass = "RustPlugin"
        }
    }
}

repositories {
    google()
    mavenCentral()
}

dependencies {
    compileOnly(gradleApi())
    // 与根 build.gradle.kts 的 AGP 版本保持一致。
    implementation("com.android.tools.build:gradle:8.10.1")
}

