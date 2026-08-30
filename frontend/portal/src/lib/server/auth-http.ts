import { NextResponse, type NextRequest } from "next/server";
import {
  CSRF_HEADER,
  csrfAccepted,
  csrfCookieName,
  csrfCookieOptions,
  sessionCookieName,
  sessionCookieOptions,
  SESSION_TTL_MS,
  sealSession,
  unsealSession,
} from "./session";

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

/** The signed-in session id, or null. Signature-checked, never trusted raw. */
export function sessionIdFrom(request: NextRequest): string | null {
  return unsealSession(request.cookies.get(sessionCookieName())?.value);
}

export function attachSession(response: NextResponse, sessionId: string): void {
  response.cookies.set(sessionCookieName(), sealSession(sessionId), sessionCookieOptions(SESSION_TTL_MS));
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
