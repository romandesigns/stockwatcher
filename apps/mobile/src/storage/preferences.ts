import AsyncStorage from "@react-native-async-storage/async-storage";

// Non-secret preferences only (layout choices, last-viewed symbol...).
// Anything that grants access belongs in ./secure.ts instead.

const PREFIX = "stockwatcher.pref.";

export async function getPreference<T>(key: string): Promise<T | null> {
  const raw = await AsyncStorage.getItem(PREFIX + key);
  if (raw === null) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    // A value written by an older app version in another shape: treat it
    // as unset rather than crashing the screen that reads it.
    return null;
  }
}

export async function setPreference<T>(key: string, value: T): Promise<void> {
  await AsyncStorage.setItem(PREFIX + key, JSON.stringify(value));
}
