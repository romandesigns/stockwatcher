import { Platform } from "react-native";

// Where the phone finds the Rust server. EXPO_PUBLIC_SERVER_URL wins (a
// physical phone on the LAN needs the PC's address, e.g.
// http://192.168.1.20:8790). Otherwise a dev build talks to the local dev
// stack (the Android emulator reaches the host's localhost as 10.0.2.2) and
// a release build to production. The URL is not a secret; the access token
// is, and it lives only in the keystore (src/storage/secure.ts).
const DEFAULT_URL = __DEV__ ? (Platform.OS === "android" ? "http://10.0.2.2:8790" : "http://localhost:8790") : "https://stockwatcher.wavystyle.io";

export const SERVER_URL = (process.env.EXPO_PUBLIC_SERVER_URL || DEFAULT_URL).replace(/\/+$/, "");

export const wsUrl = (base = SERVER_URL) => `${base.replace(/^http/, "ws")}/ws`;
