# Running the phone app on the Android emulator (Windows)

Expo Go cannot run the app (SDK 58 plus native modules: Skia, Gesture Handler, Reanimated), so local
testing needs a **development build**. Building one from the normal checkout fails on Windows because
Bun's isolated `node_modules` layout plus CMake's mirrored object paths overflow MAX_PATH, even with
`LongPathsEnabled=1` (`subst` does not help: the tools resolve the real path). The working recipe
(2026-10-17, used again 2026-10-18) builds from a short-path clone with a hoisted install.

## One-time build

1. Clone into a short path and install hoisted:
   ```sh
   git clone H:\wavystack\stockwatcher C:\sw
   cd C:\sw && bun install --linker hoisted
   ```
   If that clone was installed isolated before, delete every `apps/*/node_modules` and
   `packages/*/node_modules` first, or stale links break `expo`.
2. Junction `C:\sw\apps\mobile\node_modules\@expo-google-fonts` → `C:\sw\node_modules\@expo-google-fonts`
   (`app.config.ts` references the bundled fonts through `./node_modules/...`).
3. `cd apps/mobile && bunx expo prebuild --platform android --no-install --clean`, then add
   `externalNativeBuild { cmake { buildStagingDirectory = file("C:/x") } }` inside `android {` in
   `android/app/build.gradle` (the codegen CMake path still overflows otherwise).
4. With `ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk`:
   `gradlew app:assembleDebug -PreactNativeArchitectures=x86_64`, then `adb install -r` the APK.

A new build is only needed when native code changes (a new native module or a config plugin change);
JavaScript changes load from Metro.

## Each session

1. Start the emulator from Android Studio (Pixel AVD). Launching it from Git Bash segfaults.
2. Bring `C:\sw` to the branch under test (`git fetch H:\wavystack\stockwatcher <branch>:<branch>`,
   switch, `bun install --linker hoisted`).
3. Start Metro with `--lan`, not `--localhost` (which binds IPv6 `[::1]` only, so `adb reverse` gets
   an empty reply):
   ```sh
   cd C:\sw\apps\mobile
   EXPO_PUBLIC_SERVER_URL=https://stockwatcher.wavystyle.io bunx expo start --dev-client --lan --port 8081
   ```
   Without `EXPO_PUBLIC_SERVER_URL` a dev build talks to `http://10.0.2.2:8790` (the host's `bun dev`).
4. `adb reverse tcp:8081 tcp:8081`, then open
   `stockwatcher://expo-development-client/?url=http%3A%2F%2F127.0.0.1%3A8081`.
5. Edit in `H:\wavystack\stockwatcher` and copy changed files into `C:\sw` to hot-reload; commit only
   in `H:`. Style changes sometimes need an app restart (`adb shell am force-stop
   io.wavystyle.stockwatcher`, then step 4 again).

## Known emulator traps

- Android's monospace font lacks ↑↓→▲▼✕, so chart-native draws those as shapes.
- `expo start` rewrites `apps/mobile/tsconfig.json` (drops its comments). Never commit that from `C:\sw`.
- Under memory pressure the emulator can stop presenting frames and dropping input while the app
  keeps running (screenshots show a stale frame; `uiautomator dump` says it cannot get an idle state).
  Restart the app or the emulator before trusting what the screen shows.
- Close every headless browser you start for web checks; dozens of leaked ones starve the host.
