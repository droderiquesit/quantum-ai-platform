import { createHmac, randomBytes, scrypt, timingSafeEqual } from "node:crypto";
import { secretFromEnvironment } from "./secret";

/**
 * Server-side session and credential handling.
 *
 * Everything here is Node's standard library. ADR 0013 admits a dependency
 * only where getting it wrong is silent, the problem is adversarial, and a
 * maintained implementation exists — and it admits exactly one thing for
 * identity: verifying a third party's signed token. Sealing our own cookie and
 * hashing our own password are neither adversarial in that sense nor absent
 * from the platform we run on: `scrypt` and `createHmac` are in `node:crypto`,
 * maintained by the Node project, and using them is not hand-rolling
 * cryptography — writing scrypt would be.
 *
 * Three properties this file exists to hold:
 *
 * **The browser never holds a credential it could replay elsewhere.** The
 * cookie carries the session's own claims and a signature over them. It is
 * not a password, not a platform token, and not anything that authenticates
 * anywhere but here — the console's `viewer` credential for `qip-api` stays
 * server-side and is never derived from this cookie.
 *
 * **The cookie is the session; there is no server-side copy** (ADR 0019).
 * This reverses what this file used to do, and the reversal is the point. The
 * cookie held a random id and the session it named lived in a JSON file under
 * `/tmp` — which on Cloud Run is per-instance and in-memory. A user signed in
 * against one instance and was anonymous to the next, and everything was lost
 * on every scale event. A sealed claim set is verifiable by any instance
 * without any of them remembering anything.
 *
 * What that costs is revocation: a sealed cookie is honoured until it
 * expires, and signing out clears the browser's copy rather than invalidating
 * a copy taken from it. ADR 0019 argues why that is acceptable for a
 * twelve-hour `viewer` session on a paper-trading console, and names the
 * change that would make it unacceptable. A tampered cookie still fails the
 * signature check and reads as no session at all.
 *
 * **Comparisons are constant-time.** A signature or password check that
 * returns early on the first differing byte leaks, over enough attempts,
 * exactly how much of a guess was right. `timingSafeEqual` is used for both,
 * and the lengths are equalised first because it throws on a length mismatch —
 * which would itself be a length oracle.
 *
 * **Nothing here logs a secret.** No password, no session id, no signature
 * appears in an error message. An error says what to do next, not what was
 * presented.
 */

function scryptAsync(
  password: string,
  salt: Buffer,
  keyLength: number,
  options: { N: number; r: number; p: number; maxmem: number },
): Promise<Buffer> {
  // promisify() resolves to the overload without options, so wrap by hand.
  return new Promise((resolve, reject) => {
    scrypt(password, salt, keyLength, options, (error, derived) =>
      error ? reject(error) : resolve(derived),
    );
  });
}

/** How long a session lives before the user must sign in again. */
export const SESSION_TTL_MS = 12 * 60 * 60 * 1000;

/**
 * Cookie security is adaptive, and the downgrade is explicit.
 *
 * In production the session cookie is `__Host-` prefixed and `Secure`: bound
 * to this exact origin, never sent over plain HTTP. Local development runs on
 * `http://127.0.0.1`, where browsers refuse to *store* a Secure cookie at all
 * — the sign-in would succeed server-side and the browser would stay signed
 * out, which reads as a haunting rather than a bug. So a deployment may set
 * `ALGORIK_COOKIE_SECURE=false`, dropping Secure and the prefix together
 * (the prefix is invalid without Secure). The default is the strict form:
 * forgetting the variable yields the safe cookie, and the insecure form
 * exists only where someone wrote `false`.
 */
export function cookiesSecure(): boolean {
  return process.env.ALGORIK_COOKIE_SECURE !== "false";
}

export function sessionCookieName(): string {
  return cookiesSecure() ? "__Host-algorik_session" : "algorik_session";
}

export function csrfCookieName(): string {
  return cookiesSecure() ? "__Host-algorik_csrf" : "algorik_csrf";
}

export const CSRF_HEADER = "x-algorik-csrf";

/**
 * The key sessions are signed with.
 *
 * In production it must be supplied; a process that invented one would issue
 * sessions no other replica could verify, and every user would be signed out
 * whenever the load balancer moved them. In development a per-process key is
 * generated so a fresh checkout runs with no setup — and it is generated, not
 * hard-coded, so a default secret cannot escape into a deployment.
 */
let developmentKey: Buffer | null = null;

/**
 * The credential sessions are signed with.
 *
 * Named once, because it is resolved through the `_FILE` contract in
 * `./secret` and both halves of that contract have to name the same variable
 * for the "both set" refusal to mean anything.
 */
const SESSION_SECRET_VARIABLE = "ALGORIK_SESSION_SECRET";

/**
 * The session secret the deployment configured, or `null` if it set none.
 *
 * Resolved through `./secret` rather than read from `process.env` directly,
 * so `ALGORIK_SESSION_SECRET_FILE` — the mounted file
 * `scripts/deploy-frontends.sh` projects — is honoured. This file used to read
 * the variable itself, and a deployment that mounted the secret as a file
 * would have found no key: in production a refusal to start, and in
 * development a per-process key no other replica could verify, silently.
 *
 * A line break inside the value is refused. The secret is seeded as one
 * base64 line with the newline stripped; a break in the middle means the file
 * holds more than one thing — a second version pasted under the first, a PEM
 * — and an HMAC over all of it keys every cookie on bytes nobody chose, which
 * another replica whose file differs by one line cannot verify. Trailing
 * whitespace is already stripped by `./secret`, so `echo`'s newline is not
 * what this catches.
 */
export function configuredSessionSecret(): string | null {
  const configured = secretFromEnvironment(SESSION_SECRET_VARIABLE);
  if (configured !== null && /[\r\n]/u.test(configured)) {
    throw new Error(
      `${SESSION_SECRET_VARIABLE} contains a line break. A signing key is one line; a value ` +
        `with a break in it is a file holding more than one thing, and sessions signed over ` +
        `all of it would verify nowhere else.`,
    );
  }
  return configured;
}

function signingKey(): Buffer {
  const configured = configuredSessionSecret();
  if (configured) {
    if (configured.length < 32) {
      throw new Error(
        "ALGORIK_SESSION_SECRET is shorter than 32 characters. A short signing key is a guessable one.",
      );
    }
    return Buffer.from(configured, "utf8");
  }
  if (process.env.NODE_ENV === "production") {
    throw new Error(
      "ALGORIK_SESSION_SECRET must be set in production. Refusing to sign sessions with a key this process invented, which no other replica could verify.",
    );
  }
  developmentKey ??= randomBytes(32);
  return developmentKey;
}

/** Equal-length constant-time comparison over UTF-8 strings. */
function constantTimeEquals(left: string, right: string): boolean {
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  // timingSafeEqual throws on unequal lengths, which would leak length. Hash
  // both to a fixed width first so the comparison is always the same shape.
  const ha = createHmac("sha256", signingKey()).update(a).digest();
  const hb = createHmac("sha256", signingKey()).update(b).digest();
  return timingSafeEqual(ha, hb);
}

/**
 * `<base64url(claims)>.<signature>` — the cookie's whole contents.
 *
 * The signature covers the encoded payload rather than the decoded object, so
 * verification never parses anything before it has established the bytes are
 * ours. A verifier that parsed first would be running a JSON parser on
 * attacker-controlled input as its outermost operation.
 */
export function sealClaims(claims: unknown): string {
  const payload = Buffer.from(JSON.stringify(claims), "utf8").toString("base64url");
  const signature = createHmac("sha256", signingKey()).update(payload).digest("base64url");
  return `${payload}.${signature}`;
}

/**
 * The claims, or null if the cookie was absent, malformed or tampered.
 *
 * Null for every failure, deliberately: a caller that could tell "no cookie"
 * from "bad signature" would eventually report the difference to someone, and
 * the difference is only interesting to whoever is forging one.
 */
export function unsealClaims<T>(cookieValue: string | undefined): T | null {
  if (!cookieValue) return null;
  const separator = cookieValue.lastIndexOf(".");
  if (separator <= 0) return null;
  const payload = cookieValue.slice(0, separator);
  const presented = cookieValue.slice(separator + 1);
  const expected = createHmac("sha256", signingKey()).update(payload).digest("base64url");
  if (!constantTimeEquals(presented, expected)) return null;
  try {
    return JSON.parse(Buffer.from(payload, "base64url").toString("utf8")) as T;
  } catch {
    return null;
  }
}

/**
 * A handle for one sign-in.
 *
 * Nothing looks this up any more — it exists so a session can be named in a
 * log line or a device list without naming the user, and so two sign-ins by
 * one person are distinguishable.
 */
export function newSessionId(): string {
  return randomBytes(32).toString("base64url");
}

export function newCsrfToken(): string {
  return randomBytes(32).toString("base64url");
}

/**
 * Password hashing with scrypt.
 *
 * Parameters chosen for a login path: N=2^15 costs roughly 100ms here, which
 * is tolerable for a person signing in and expensive for someone working
 * through a stolen list. The salt is per-password and stored beside the hash,
 * because a shared salt lets one rainbow table cover every account.
 */
const SCRYPT_COST = 2 ** 15;
const SCRYPT_BLOCK = 8;
const SCRYPT_PARALLEL = 1;
const KEY_LENGTH = 64;

export async function hashPassword(password: string): Promise<string> {
  const salt = randomBytes(16);
  const derived = await scryptAsync(password, salt, KEY_LENGTH, {
    N: SCRYPT_COST,
    r: SCRYPT_BLOCK,
    p: SCRYPT_PARALLEL,
    // Node's default maxmem is too small for N=2^15; state it rather than
    // letting the call throw at runtime on the first real sign-up.
    maxmem: 128 * SCRYPT_COST * SCRYPT_BLOCK * 2,
  });
  return `scrypt$${SCRYPT_COST}$${SCRYPT_BLOCK}$${SCRYPT_PARALLEL}$${salt.toString("base64url")}$${derived.toString("base64url")}`;
}

export async function verifyPassword(password: string, stored: string): Promise<boolean> {
  const parts = stored.split("$");
  if (parts.length !== 6 || parts[0] !== "scrypt") return false;
  const cost = Number(parts[1]);
  const block = Number(parts[2]);
  const parallel = Number(parts[3]);
  const salt = Buffer.from(parts[4] ?? "", "base64url");
  const expected = Buffer.from(parts[5] ?? "", "base64url");
  if (!Number.isFinite(cost) || !Number.isFinite(block) || !Number.isFinite(parallel)) return false;
  const derived = await scryptAsync(password, salt, expected.length, {
    N: cost,
    r: block,
    p: parallel,
    maxmem: 128 * cost * block * 2,
  });
  return derived.length === expected.length && timingSafeEqual(derived, expected);
}

/** Cookie attributes. `__Host-` requires Secure and Path=/ and forbids Domain. */
export function sessionCookieOptions(maxAgeMs: number) {
  return {
    httpOnly: true,
    // Not readable from JavaScript, so an injected script cannot exfiltrate it.
    sameSite: "lax" as const,
    // Lax rather than Strict: Strict drops the cookie on the first request of
    // a redirect back from an identity provider, which reads to a user as
    // "signing in did nothing".
    secure: cookiesSecure(),
    path: "/",
    maxAge: Math.floor(maxAgeMs / 1000),
  };
}

/**
 * The CSRF cookie, deliberately readable by JavaScript.
 *
 * Double-submit: the browser reads this and echoes it in a header, and an
 * attacker's page can cause the cookie to be *sent* but cannot read it to
 * construct the header. That is the whole mechanism, and it is why this one
 * cookie is not `httpOnly`.
 */
export function csrfCookieOptions() {
  return {
    httpOnly: false,
    sameSite: "lax" as const,
    secure: cookiesSecure(),
    path: "/",
    maxAge: Math.floor(SESSION_TTL_MS / 1000),
  };
}

/**
 * Whether a mutating request may proceed.
 *
 * Two independent checks, because either alone has a known gap: the origin
 * header is absent on some legitimate requests, and a double-submit token is
 * defeated by an attacker who can write cookies for the site.
 */
export function csrfAccepted(request: Request, cookieToken: string | undefined): boolean {
  const header = request.headers.get(CSRF_HEADER);
  if (!header || !cookieToken) return false;
  if (!constantTimeEquals(header, cookieToken)) return false;

  const origin = request.headers.get("origin");
  if (origin) {
    // Compared against the Host header, not request.url: the framework
    // normalises its own URL (127.0.0.1 becomes localhost behind next
    // start), and a check that compares the browser's truth against the
    // framework's rewrite refuses every legitimate same-origin POST. The
    // Host header is what the client actually connected to, and a cross-site
    // attacker controls neither it nor Origin.
    const host = request.headers.get("host");
    try {
      if (!host || new URL(origin).host !== host) return false;
    } catch {
      return false;
    }
  }
  return true;
}
