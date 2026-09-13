plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val identifiedVersion = providers.environmentVariable("NODEHARBOR_VERSION").orElse("0.1.0-development").get()
val identifiedCommit = providers.environmentVariable("NODEHARBOR_COMMIT").orElse("development").get()
val signedRelease = providers.environmentVariable("NODEHARBOR_ANDROID_RELEASE").orElse("0").get() == "1"
val versionParts = identifiedVersion.split('.').map { it.toIntOrNull() }
val identifiedCode = if (versionParts.size == 3 && versionParts.all { it != null })
    versionParts[0]!!.toLong() * 100_000_000 + versionParts[1]!!.toLong() * 1_000_000 + versionParts[2]!! else 1L
require(identifiedCode in 1..2_100_000_000)
if (signedRelease) require(identifiedCommit.matches(Regex("[0-9a-f]{40}")) && !identifiedVersion.contains("development"))

android {
    namespace = "io.github.gu1p.nodeharbor"
    compileSdk = 37
    ndkVersion = "28.2.13676358"
    defaultConfig {
        applicationId = "io.github.gu1p.nodeharbor"
        minSdk = 33
        targetSdk = 37
        versionCode = identifiedCode.toInt()
        versionName = identifiedVersion
        buildConfigField("String", "SOURCE_COMMIT", "\"$identifiedCommit\"")
        buildConfigField("boolean", "DIRTY_SOURCE", providers.environmentVariable("NODEHARBOR_DIRTY_SOURCE").orElse("true").get())
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += "arm64-v8a" }
        externalNativeBuild { cmake { arguments += "-DANDROID_STL=c++_static" } }
    }
    buildFeatures { compose = true; buildConfig = true; aidl = true }
    if (signedRelease) {
        signingConfigs.create("distribution") {
            fun secret(name: String) = providers.environmentVariable(name).orNull
                ?: throw GradleException("Android release signing requires $name")
            storeFile = file(secret("NODEHARBOR_ANDROID_KEYSTORE"))
            storePassword = secret("NODEHARBOR_ANDROID_STORE_PASSWORD")
            keyAlias = secret("NODEHARBOR_ANDROID_KEY_ALIAS")
            keyPassword = secret("NODEHARBOR_ANDROID_KEY_PASSWORD")
        }
        buildTypes.getByName("release") { signingConfig = signingConfigs.getByName("distribution") }
        testBuildType = "release"
    }
    externalNativeBuild { cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" } }
    sourceSets.getByName("androidTest").assets.srcDir("build/boot-probe-assets")
    sourceSets.getByName("main").assets.srcDir("build/generated/worker-assets")
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions { unitTests.isReturnDefaultValues = false }
    lint {
        warningsAsErrors = true
        abortOnError = true
        // Supported versions are pinned for reproducible releases; update notices
        // are not correctness checks. This APK targets ARM64 phones, not ChromeOS.
        disable += setOf("AndroidGradlePluginVersion", "GradleDependency", "NewerVersionAvailable", "ChromeOsAbiSupport")
    }
}

val prepareWorkerAssets by tasks.registering(Exec::class) {
    workingDir(rootProject.projectDir.parentFile)
    commandLine("python3", "scripts/android_assets.py")
    inputs.files(fileTree("src/main/jniLibs"), fileTree("../runtime"), fileTree("../../guest"),
        file("../../scripts/android_assets.py"), file("../../scripts/android_runtime.py"))
    outputs.dir(layout.buildDirectory.dir("generated/worker-assets"))
}
tasks.named("preBuild") { dependsOn(prepareWorkerAssets) }

dependencies {
    val compose = platform("androidx.compose:compose-bom:2026.08.00")
    implementation(compose)
    androidTestImplementation(compose)
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250517")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    // Compose's transitive older Espresso reflects an API removed in Android 17.
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
}
