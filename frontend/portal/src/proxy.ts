import { NextResponse, type NextRequest } from "next/server";

/**
 * The navigation gate, when authentication is required. (Next 16 calls this
 * file convention "proxy"; it is the edge request filter, nothing more.)
 *
 * Two deliberate limits, both worth stating:
 *
 * **On unless ALGORIK_AUTH_REQUIRED=false.** The open console — a kiosk on a
 * desk, the suites that predate accounts — is something a deployment writes
 * down; absence, a typo, or any other value is the closed gate. The rule is
 * `authRequired()` in `lib/server/auth-gate.ts`, inlined here because this
 * file runs at the edge and imports nothing from `lib/server`. The flag is
 * read per request, so the same build serves both.
 *
 * **This is UX, not the security boundary.** Edge middleware has no
 * `node:crypto`, so it checks cookie *presence* and redirects — a comfort for
 * humans who land somewhere they cannot use. The boundary that matters is in
 * the Node-runtime handlers: the gateway verifies the cookie's signature and
 * the session's existence before it forwards anything, so a forged cookie
 * gets past this redirect and then reads nothing at all.
 */

const PUBLIC_PREFIXES = [
  "/welcome", "/platform", "/technology", "/security", "/institutional",
  "/developers", "/company", "/contact", "/legal",
  "/sign-in", "/sign-up", "/verify-email", "/forgot-password", "/reset-password",
  "/session-expired", "/access-denied", "/account-locked", "/agreements",
  "/api/auth",
  "/offline",
  "/brand", "/icons", "/manifest.webmanifest", "/sw.js", "/icon.svg", "/favicon.ico",
];

// Mirrors sessionCookieName() in lib/server/session.ts, inlined because this
// file runs at the edge and must not import a module that touches node:crypto.
function sessionCookieName(): string {
  return process.env.ALGORIK_COOKIE_SECURE !== "false"
    ? "__Host-algorik_session"
    : "algorik_session";
}

export function proxy(request: NextRequest): NextResponse {
  // Mirrors authRequired() in lib/server/auth-gate.ts: closed unless opted out.
  if (process.env.ALGORIK_AUTH_REQUIRED === "false") return NextResponse.next();

  const { pathname } = request.nextUrl;
  if (PUBLIC_PREFIXES.some((prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`))) {
    return NextResponse.next();
  }

  // API paths are never redirected. A machine calling the gateway without a
  // session must receive 401 from the handler, not a 200 HTML sign-in page a
  // redirect-following client mistakes for success — which is exactly how a
  // "revoked session still reads the platform" false-positive was minted
  // during this slice's own testing. The Node-runtime handlers hold the real
  // boundary; this file only decides redirect-versus-pass-through.
  if (pathname.startsWith("/api/")) {
    return NextResponse.next();
  }

  const hasSessionCookie = Boolean(request.cookies.get(sessionCookieName())?.value);
  if (hasSessionCookie) return NextResponse.next();

  // Behind ADR 0094's IAP front door a person arrives already authenticated
  // and with no cookie at all, so the cookie check above would bounce them to
  // a sign-in page they have no business seeing and cannot usefully complete.
  // Presence is all this file may check: the edge runtime has no `node:crypto`
  // and could not fetch a key set synchronously if it did, and the signature
  // is what the assertion is worth. That is not a hole, for the same reason
  // the cookie check above is not one — this file decides redirect versus
  // pass-through, and every Node handler behind it verifies the assertion
  // before it returns anything. A request that sets this header without a
  // valid signature gets past the redirect and then reads nothing.
  if (request.headers.has("x-goog-iap-jwt-assertion")) return NextResponse.next();

  // The signed-out root goes to the front door, not to a login wall: a person
  // typing the bare domain has expressed no intent to sign in yet.
  if (pathname === "/") {
    return NextResponse.redirect(new URL("/welcome", request.url));
  }
  const destination = new URL("/sign-in", request.url);
  destination.searchParams.set("next", pathname + request.nextUrl.search);
  return NextResponse.redirect(destination);
}

export const config = {
  // Everything except Next's own assets; the allowlist above does the rest.
  matcher: ["/((?!_next/static|_next/image).*)"],
};
