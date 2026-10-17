// The server access token (STOCKWATCHER_API_TOKEN), entered once on the sign-in card and kept in this
// browser's localStorage so a reload stays signed in. It is never put in a URL or a build variable, and
// it is sent only to this origin's /api and /ws. Clearing it (an unauthorized answer) shows the card again.

const KEY = "sw-access-token";

export function getToken(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export function setToken(token: string): void {
  try {
    localStorage.setItem(KEY, token);
  } catch {
    // Private mode: the token lasts only for this page; the feed already holds it.
  }
}

export function clearToken(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // Nothing stored.
  }
}
