import * as SecureStore from "expo-secure-store";

// The server access token (STOCKWATCHER_API_TOKEN) lives only in the
// platform keystore: Keychain on iOS, Keystore-backed storage on Android.
// Never AsyncStorage, never a public build variable (docs/CONVENTIONS.md).

const ACCESS_TOKEN_KEY = "stockwatcher.accessToken";

const options: SecureStore.SecureStoreOptions = {
  // Readable once the device has been unlocked after boot, so a background
  // reconnect can authenticate; not synced to other devices.
  keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY,
};

export async function getAccessToken(): Promise<string | null> {
  return SecureStore.getItemAsync(ACCESS_TOKEN_KEY, options);
}

export async function setAccessToken(token: string): Promise<void> {
  await SecureStore.setItemAsync(ACCESS_TOKEN_KEY, token, options);
}

export async function clearAccessToken(): Promise<void> {
  await SecureStore.deleteItemAsync(ACCESS_TOKEN_KEY, options);
}
