// Keeps the Android native build inside an EAS free-tier worker's memory.
//
// Two preview builds (25fb1377, e22d6ab4) died in :shopify_react-native-skia:buildCMake with the
// compiler "Killed": Gradle ran Skia's, Reanimated's and expo-modules-core's C++ builds at the
// same time (org.gradle.parallel=true, no worker cap), each with its own parallel ninja, beside a
// 2 GB Gradle heap. Building one native module at a time leaves the compiler the memory it needs;
// the build takes a little longer, which is far cheaper than a failed 80-minute build.
const { withGradleProperties } = require("expo/config-plugins");

const PROPERTIES = {
  "org.gradle.parallel": "false",
  "org.gradle.workers.max": "1",
  // Enough for Gradle itself; the rest of the worker's memory is left to the C++ compilers.
  "org.gradle.jvmargs": "-Xmx2048m -XX:MaxMetaspaceSize=512m",
  "kotlin.daemon.jvmargs": "-Xmx1024m",
};

module.exports = function withLowMemoryNativeBuild(config) {
  return withGradleProperties(config, (cfg) => {
    for (const [key, value] of Object.entries(PROPERTIES)) {
      const existing = cfg.modResults.find((p) => p.type === "property" && p.key === key);
      if (existing) existing.value = value;
      else cfg.modResults.push({ type: "property", key, value });
    }
    return cfg;
  });
};
