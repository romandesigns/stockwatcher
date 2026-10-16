export class UnauthorizedError extends Error {
  constructor() {
    super("The server rejected the access token.");
    this.name = "UnauthorizedError";
  }
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
