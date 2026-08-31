import { NextResponse, type NextRequest } from "next/server";
import {
  CSRF_HEADER,
  csrfAccepted,
  csrfCookieName,
  csrfCookieOptions,
  sessionCookieName,
  sessionCookieOptions,
  sealClaims,
  unsealClaims,
} from "./session";
import type { SessionClaims } from "./identity";

/**
 * The shared plumbing of the auth routes, so each route is only its own rule.
 *
 * Every response sets `cache-control: no-store`: an authentication answer
 * cached by any intermediary is a session leak with a delay on it.
 */

export const noStore = { "cache-control": "no-store" } as const;

/** Body reader with a hard size cap. An auth body has no business being big. */
export async function readJson<T>(request: NextRequest, limitBytes = 16 * 1024): Promise<T | null> {
  const raw = await request.text();
  if (raw.length === 0 || raw.length > limitBytes) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

/** Refusal shared by every mutating route when the CSRF pair is absent/wrong. */
export function csrfRefusal(): NextResponse {
  return NextResponse.json(
    {
      ok: false,
      failure: {
        code: "not_permitted",
        message: `The request is missing its ${CSRF_HEADER} header. Reload the page and try again.`,
      },
    },
    { status: 403, headers: noStore },
  );
}

export function requireCsrf(request: NextRequest): NextResponse | null {
  const cookie = request.cookies.get(csrfCookieName())?.value;
  return csrfAccepted(request, cookie) ? null : csrfRefusal();
}

/**
 * The signed-in session, or null. Signature-checked, never trusted raw.
 *
 * Expiry is enforced here rather than left to the cookie's `maxAge`. A browser
 * that ignores `maxAge`, or a copy of the cookie replayed by something that is
 * not a browser at all, would otherwise present a session this process had no
 * reason to stop honouring. The claim is the authority; the cookie attribute
 * is a courtesy to the browser.
 */
export function sessionFrom(request: NextRequest): SessionClaims | null {
  const claims = unsealClaims<SessionClaims>(request.cookies.get(sessionCookieName())?.value);
  if (!claims) return null;
  if (typeof claims.expiresAt !== "number" || claims.expiresAt <= Date.now()) return null;
  return claims;
}

export function attachSession(response: NextResponse, claims: SessionClaims): void {
  response.cookies.set(
    sessionCookieName(),
    sealClaims(claims),
    // The cookie's lifetime is the claim's, not a constant. The two drifting
    // apart would leave either a cookie the browser discards while the server
    // would still honour it, or one the browser keeps sending after it stopped
    // meaning anything.
    sessionCookieOptions(Math.max(0, claims.expiresAt - Date.now())),
  );
}

export function clearSession(response: NextResponse): void {
  response.cookies.set(sessionCookieName(), "", { ...sessionCookieOptions(0), maxAge: 0 });
}

export function attachCsrf(response: NextResponse, token: string): void {
  response.cookies.set(csrfCookieName(), token, csrfCookieOptions());
}

/** Coarse device description for the session list. Never the raw user agent. */
export function deviceFrom(request: NextRequest): string {
  const agent = request.headers.get("user-agent") ?? "";
  const browser =
    /firefox/i.test(agent) ? "Firefox" :
    /edg/i.test(agent) ? "Edge" :
    /chrome|crios/i.test(agent) ? "Chrome" :
    /safari/i.test(agent) ? "Safari" : "browser";
  const platform =
    /android/i.test(agent) ? "Android" :
    /iphone|ipad|ios/i.test(agent) ? "iOS" :
    /mac/i.test(agent) ? "macOS" :
    /windows/i.test(agent) ? "Windows" :
    /linux/i.test(agent) ? "Linux" : "device";
  return `${browser} on ${platform}`;
}
