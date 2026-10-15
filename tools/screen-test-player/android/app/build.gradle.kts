plugins { id("com.android.application") }
android {
    namespace = "com.nodeforge.screentest"
    compileSdk = 36
    defaultConfig { applicationId = "com.nodeforge.screentest"; minSdk = 35; targetSdk = 36; versionCode = 1; versionName = "1.0" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
