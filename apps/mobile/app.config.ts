import { existsSync } from "node:fs";
import type { ConfigContext, ExpoConfig } from "expo/config";

// Names and ids are fixed in docs/CONVENTIONS.md ("Names, ports, URLs").
// The EAS project was created by the owner and linked with `eas init --id`;
// changing the id or owner here points builds and updates at another project.
const EAS_PROJECT_ID = "b43c471a-3d47-4904-bd31-a7f5797ee45a";
const APP_ID = "io.wavystyle.stockwatcher";

// Dominant background from src/theme.ts, repeated here because app config
// runs in Node at build time and cannot import app code.
const BACKGROUND = "#0f1216";

// Android push needs Firebase Cloud Messaging: without google-services.json in the build,
// getExpoPushTokenAsync throws and no phone can register (found 2026-10-22, when no Runners or
// burst push had ever been delivered). The file stays out of git: on EAS it is the file-type
// environment variable GOOGLE_SERVICES_JSON, locally ./google-services.json. The FCM v1
// service-account key that lets Expo send is a separate EAS credential, never in the repo.
const GOOGLE_SERVICES_JSON =
  process.env.GOOGLE_SERVICES_JSON ??
  (existsSync("./google-services.json") ? "./google-services.json" : undefined);

export default ({ config }: ConfigContext): ExpoConfig => ({
  ...config,
  name: "stockwatcher",
  slug: "stockwatcher",
  owner: "creativecoder87",
  // The store version. EAS keeps versionCode/buildNumber remotely
  // (eas.json appVersionSource: "remote"), so they are not set here.
  version: "0.1.0",
  scheme: "stockwatcher",
  platforms: ["android", "ios"],
  orientation: "portrait",
  icon: "./assets/icon.png",
  // Dark-first, like the web app; a light theme is not built yet.
  userInterfaceStyle: "dark",
  backgroundColor: BACKGROUND,

  // Fingerprint: an OTA update only reaches binaries whose native layer
  // hashes the same, so a JS-only push never lands on an incompatible APK.
  // release.yml compares this hash with the latest preview build to decide
  // whether a new APK is needed.
  runtimeVersion: { policy: "fingerprint" },
  updates: {
    url: `https://u.expo.dev/${EAS_PROJECT_ID}`,
    checkAutomatically: "ON_LOAD",
    // Never block launch waiting for the network: start on the cached
    // bundle and apply a fresh download on the next start (or on
    // "Restart to update", see src/updates/).
    fallbackToCacheTimeout: 0,
  },

  ios: {
    bundleIdentifier: APP_ID,
    supportsTablet: true,
    // SDK 56+ reads the deployment target here (expo-build-properties'
    // ios.deploymentTarget is deprecated). 16.4 is expo-modules-core's floor
    // in SDK 58.
    deploymentTarget: "16.4",
    infoPlist: {
      ITSAppUsesNonExemptEncryption: false,
    },
  },
  android: {
    package: APP_ID,
    ...(GOOGLE_SERVICES_JSON ? { googleServicesFile: GOOGLE_SERVICES_JSON } : {}),
    adaptiveIcon: {
      backgroundColor: BACKGROUND,
      foregroundImage: "./assets/android-icon-foreground.png",
      backgroundImage: "./assets/android-icon-background.png",
      monochromeImage: "./assets/android-icon-monochrome.png",
    },
    // Off until the first pushed screen exists. Turning it on means the
    // system back preview must play the same motion spec as the in-app
    // back (CLAUDE.md "Screen transitions and motion", rule 4), and the
    // transitions library has no built-in integration for that yet.
    predictiveBackGestureEnabled: false,
  },

  plugins: [
    [
      "expo-build-properties",
      {
        // RN 0.88's own defaults (react-native/gradle/libs.versions.toml),
        // stated explicitly so a later SDK bump shows up as a reviewed diff.
        // Kotlin is left to the SDK: restating it only creates a pin that
        // every upgrade has to chase.
        android: {
          minSdkVersion: 24,
          compileSdkVersion: 37,
          targetSdkVersion: 36,
        },
      },
    ],
    [
      "expo-splash-screen",
      {
        image: "./assets/splash-icon.png",
        imageWidth: 160,
        resizeMode: "contain",
        backgroundColor: BACKGROUND,
      },
    ],
    [
      "expo-font",
      {
        // Bundled into the binary (Geist, SIL OFL 1.1): no runtime fetch
        // and no load delay. The same family as the web app.
        fonts: [
          "./node_modules/@expo-google-fonts/geist/400Regular/Geist_400Regular.ttf",
          "./node_modules/@expo-google-fonts/geist/600SemiBold/Geist_600SemiBold.ttf",
        ],
      },
    ],
    [
      "expo-notifications",
      {
        // Android draws this as a white silhouette in the status bar.
        icon: "./assets/android-icon-monochrome.png",
        color: "#8b9cff",
        defaultChannel: "alerts",
      },
    ],
    [
      "expo-secure-store",
      {
        // The access token must not be restored onto another device from
        // an Android cloud backup.
        configureAndroidBackup: true,
      },
    ],
    "expo-localization",
    "expo-dev-client",
    // Native builds one module at a time: Skia's C++ ran the EAS worker out of memory otherwise.
    "./plugins/withLowMemoryNativeBuild",
  ],

  extra: {
    eas: { projectId: EAS_PROJECT_ID },
  },
});
