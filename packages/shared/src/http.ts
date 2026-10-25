export class UnauthorizedError extends Error {
  constructor() {
    super("The server rejected the access token.");
    this.name = "UnauthorizedError";
  }
}

/**
 * The Rust server's origin, for a page the server does not serve. The web app is served by it, so
 * this stays "" and every `/api` path is same-origin. The desktop app's page is its own local
 * origin (`http://tauri.localhost`): its relative `/api` requests reached the bundle instead, the
 * probe got HTML and the app fell back to the fixture (2026-10-25: the installed desktop had never
 * shown server data). main.tsx names the server there.
 */
let serverOrigin = "";

export function setServerOrigin(origin: string): void {
  serverOrigin = origin.replace(/\/+$/, "");
}

/** `path` (starting with "/") on the server: unchanged on the web, absolute in the desktop app. */
export function serverUrl(path: string): string {
  return serverOrigin + path;
}

/**
 * fetch() with `Authorization: Bearer <token>` when a token is present.
 * A 401 throws UnauthorizedError so callers can route to sign-in in one
 * place; every other status is returned for the caller to handle.
 */
export async function authenticatedFetch(
  input: string | URL,
  token: string | null | undefined,
  init: RequestInit = {},
): Promise<Response> {
  const headers = new Headers(init.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const res = await fetch(input, { ...init, headers });
  if (res.status === 401) throw new UnauthorizedError();
  return res;
}
