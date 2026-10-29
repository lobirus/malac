import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "io.malac.app"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.malac.app"
        minSdk = 26
        targetSdk = 37
        versionCode = 11
        versionName = "1.2.5"
    }

    // Release builds are signed when the environment names a keystore, as
    // the release workflow does from the repository's secrets.
    val keystore = providers.environmentVariable("MALAC_KEYSTORE").orNull
    if (keystore != null) {
        signingConfigs.create("release") {
            storeFile = file(keystore)
            storeType = "PKCS12"
            storePassword = providers.environmentVariable("MALAC_KEYSTORE_PASSWORD").orNull
            keyAlias = providers.environmentVariable("MALAC_KEY_ALIAS").orNull ?: "malac"
            keyPassword = storePassword
        }
    }

    buildTypes {
        debug {
            // Installs next to the release app instead of clashing with it.
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
        }
        release {
            signingConfig = signingConfigs.findByName("release")
            // R8 stays off until a shrunk build has been tried on devices.
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_17
    }
}

dependencies {
    implementation(project(":core"))

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons.core)
    debugImplementation(libs.compose.ui.tooling)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.work.runtime)
    implementation(libs.androidx.datastore.preferences)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.play.services.code.scanner)
    // Push without Google services: a distributor app such as ntfy wakes malac.
    implementation(libs.unifiedpush.connector)
    // The scanner brings Fragment 1.1, which loses activity results such as
    // the notification permission's; 1.3 and later deliver them.
    implementation(libs.androidx.fragment)
}
