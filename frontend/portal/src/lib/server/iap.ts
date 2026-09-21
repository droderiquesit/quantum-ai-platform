import { createPublicKey, verify as verifySignature, type KeyObject } from "node:crypto";

/**
 * Identity-Aware Proxy's asserted identity — the console's Google sign-in.
 *
 * **Why this and not an OAuth dance of our own.** ADR 0094 puts the portal
 * behind IAP at `portal.algorik.ai`. A request that reaches this process has
 * already been authenticated by Google against a Google account, and refused
 * if that account is not in the `roles/iap.httpsResourceAccessor` list. A
 * second "Sign in with Google" button inside the app would send a person who
 * has *just* proved a Google identity away to prove the same Google identity
 * again, to a different OAuth client, and would need a public client id
 * delivered to the browser — which ADR 0094 decision 2 deliberately arranged
 * for there not to be, by choosing the Google-managed OAuth client so the
 * deployment "mints, stores and rotates nothing". There is no client id to
 * ship because the decision was to have none.
 *
 * ADR 0013 predicted this exact outcome in its own reversal clause: "If token
 * verification is terminated elsewhere — an API gateway or IAP that validates
 * the token and forwards a trusted assertion, which is the standard Google
 * answer — then the platform never parses a JWT... This is the likely outcome
 * for the admin surface, which sits behind IAP by design."
 *
 * **So why is there a signature check here at all?** Because the clause above
 * describes the backend, and this process is not the thing IAP protects — it
 * is the thing *behind* the thing IAP protects. A header is only a claim about
 * where a request came from, and `x-goog-iap-jwt-assertion` is a header an
 * attacker sets as easily as any other. The layers that are supposed to make
 * that impossible (`INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER`, plus the invoker
 * grant to the IAP service agent alone) are exactly the layers that would be
 * quietly wrong if someone widened the ingress posture to make an error go
 * away — and then this file is all that stands between a `curl -H` and a
 * signed-in console. Verifying the assertion costs one cached JWKS fetch and
 * removes the entire class.
 *
 * This is not the dependency ADR 0013 admitted, and it is not an argument for
 * one. What is verified here is one algorithm (ES256), from one issuer, for
 * one audience, using `node:crypto`'s own ECDSA verifier — the same standing
 * this repository already gives `scrypt` and `createHmac` in `session.ts`.
 * Nothing here implements a cipher, a hash or a curve. The JWT pitfalls that
 * make ADR 0013's condition 1 bite are enumerated and closed one by one below,
 * where each is named:
 *
 * - `alg: none` and algorithm confusion — the verifier is chosen before the
 *   header is read, and a header naming anything but `ES256` is refused.
 * - An unverified `aud` — the audience must be configured, and must match
 *   exactly. A token minted by IAP for a *different* backend service is a
 *   valid Google-signed token, and accepting it lets anyone with access to
 *   any IAP resource in any project into this console.
 * - `iss` spoofing — pinned to IAP's one issuer string.
 * - Key rotation — keys are cached with a ceiling, and an unknown `kid`
 *   forces one throttled refetch rather than a refusal or a fetch storm.
 * - Parsing before verifying — the payload is decoded only after the
 *   signature over the encoded halves has been established.
 *
 * **What is deliberately not trusted:** `x-goog-authenticated-user-email` and
 * `x-goog-authenticated-user-id`. Those are unsigned conveniences; reading
 * them is reading whatever the last hop wrote. Only the signed assertion is
 * evidence, and nothing in this file looks at the convenience headers.
 *
 * **Nothing here logs the assertion, and no error message contains it.**
 */

/** The signed assertion IAP adds to every request it forwards. */
export const IAP_ASSERTION_HEADER = "x-goog-iap-jwt-assertion";

/** The only issuer an IAP assertion ever carries. */
const IAP_ISSUER = "https://cloud.google.com/iap";

/** The only algorithm IAP signs with, and the only one this file verifies. */
const IAP_ALGORITHM = "ES256";

/** Where Google publishes the IAP signing keys. */
const IAP_JWKS_URL = "https://www.gstatic.com/iap/verify/public_key-jwk";

/** Every call that leaves the process carries an explicit timeout. */
const JWKS_TIMEOUT_MS = 5_000;

/**
 * How long a fetched key set is reused.
 *
 * Long enough that the auth path is not a per-request dependency on gstatic,
 * short enough that a key withdrawn by Google stops being accepted the same
 * hour. Rotation is handled by the unknown-`kid` refetch below rather than by
 * this ceiling, which is the backstop for the other direction: a key that is
 * no longer published at all.
 */
const JWKS_TTL_MS = 60 * 60 * 1000;

/**
 * The floor between two refetches triggered by an unknown `kid`.
 *
 * Without it, a stream of assertions bearing a `kid` that will never resolve —
 * a forgery generator, or simply a misconfigured audience upstream — turns
 * every request into an outbound fetch, and the console takes gstatic down
 * with it or is rate-limited into refusing everyone. One refetch a minute is
 * ample for a rotation that happens on the order of weeks.
 */
const JWKS_REFETCH_FLOOR_MS = 60 * 1000;

/**
 * Clock skew tolerated on `exp`, `iat` and `nbf`.
 *
 * Cloud Run instances are NTP-synchronised, so this is small on purpose: a
 * generous window is a replay window, and the tokens in question live minutes.
 */
const CLOCK_SKEW_MS = 60 * 1000;

/**
 * The audience the assertion must name.
 *
 * `/projects/<project number>/global/backendServices/<backend service id>` —
 * the numeric id of the backend service `modules/iap-edge` creates, which is
 * knowable only after an apply. It is **not a secret**: it names a resource
 * and authenticates nobody, which is why it is a plain environment value and
 * not a mounted file. It is also not committed, because it is per-deployment
 * and no value of it is right for two environments.
 *
 * Absent, IAP mode is off entirely and this module grants nothing. That is the
 * fail-closed direction: a deployment that set the audience wrong refuses
 * everyone and is diagnosed in a minute, whereas one that treated "no
 * audience" as "accept any audience" would accept a token minted by IAP for
 * somebody else's project and look like it was working.
 */
const AUDIENCE_VARIABLE = "ALGORIK_IAP_AUDIENCE";

/**
 * A loopback JWKS endpoint, for the behavioural suite only.
 *
 * The suite has to mint assertions this process will accept, which means it
 * has to control the key set — and it runs against a production build, so a
 * `NODE_ENV` guard would not be available to it. The guard used instead is
 * that the override **must be a loopback address**: a deployment pointed at
 * `127.0.0.1` has no key set at all and therefore refuses every assertion,
 * so the worst a mis-set value can do here is close the door. An override
 * that could name an arbitrary host would be a way to hand the console's
 * trust root to whoever could set one environment variable, which is a much
 * shorter path to a forged session than forging an ECDSA signature.
 */
const JWKS_URL_VARIABLE = "ALGORIK_IAP_JWKS_URL";

/** The verified identity of whoever IAP let through. */
export interface IapIdentity {
  /** The `sub` claim: Google's stable id for the account. Never displayed. */
  readonly subject: string;
  /** The `email` claim, lowercased. */
  readonly email: string;
  /** The `hd` claim — the Workspace domain, when the account has one. */
  readonly hostedDomain: string | null;
  /** `exp`, in epoch milliseconds. The session derived from this never outlives it. */
  readonly expiresAtMs: number;
}

export function iapAudience(): string | null {
  const configured = process.env[AUDIENCE_VARIABLE]?.trim();
  return configured ? configured : null;
}

/** Whether this deployment sits behind IAP, as it has declared. */
export function iapConfigured(): boolean {
  return iapAudience() !== null;
}

/**
 * The key-set URL, honouring the loopback-only override. See
 * `JWKS_URL_VARIABLE` for why an arbitrary host is refused rather than
 * trusted.
 */
function jwksUrl(): string {
  const override = process.env[JWKS_URL_VARIABLE]?.trim();
  if (!override) return IAP_JWKS_URL;
  let parsed: URL;
  try {
    parsed = new URL(override);
  } catch {
    return IAP_JWKS_URL;
  }
  const loopback = parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost" || parsed.hostname === "[::1]";
  return loopback ? parsed.toString() : IAP_JWKS_URL;
}

interface KeySet {
  readonly keys: ReadonlyMap<string, KeyObject>;
  readonly fetchedAtMs: number;
}

let keySet: KeySet | null = null;
let lastFetchAttemptMs = 0;

/** Discard the cached key set. Exists for tests; nothing in a request path calls it. */
export function forgetIapKeys(): void {
  keySet = null;
  lastFetchAttemptMs = 0;
}

interface PublishedKey {
  kid?: unknown;
  kty?: unknown;
  crv?: unknown;
  alg?: unknown;
}

/**
 * Fetch and parse the published key set.
 *
 * A key whose `kty`/`crv`/`alg` is not the P-256 triple IAP uses is dropped
 * rather than imported: `createPublicKey` would happily build an RSA key from
 * an RSA JWK, and an RSA key in this map is a key some future caller could
 * verify an RS256 token against. The map holds only keys this file's one
 * verifier can use.
 */
async function fetchKeys(nowMs: number): Promise<KeySet | null> {
  lastFetchAttemptMs = nowMs;
  let response: Response;
  try {
    response = await fetch(jwksUrl(), { signal: AbortSignal.timeout(JWKS_TIMEOUT_MS) });
  } catch {
    return null;
  }
  if (!response.ok) return null;
  const payload = (await response.json().catch(() => null)) as { keys?: unknown } | null;
  const published = Array.isArray(payload?.keys) ? (payload.keys as PublishedKey[]) : [];
  const keys = new Map<string, KeyObject>();
  for (const key of published) {
    const kid = typeof key.kid === "string" ? key.kid : null;
    if (!kid) continue;
    if (key.kty !== "EC" || key.crv !== "P-256") continue;
    if (key.alg !== undefined && key.alg !== IAP_ALGORITHM) continue;
    try {
      keys.set(kid, createPublicKey({ key: key as never, format: "jwk" }));
    } catch {
      // A malformed entry is skipped, not fatal: one bad key in a published
      // set must not stop the others from verifying anybody.
      continue;
    }
  }
  if (keys.size === 0) return null;
  return { keys, fetchedAtMs: nowMs };
}

/**
 * The public key for a `kid`, refetching once if it is unknown.
 *
 * Rotation is the reason for the refetch and the throttle is the reason it is
 * safe: a genuinely new key resolves on its first presentation, and a `kid`
 * that will never resolve costs one fetch a minute rather than one a request.
 */
async function keyFor(kid: string, nowMs: number): Promise<KeyObject | null> {
  if (!keySet || nowMs - keySet.fetchedAtMs > JWKS_TTL_MS) {
    keySet = (await fetchKeys(nowMs)) ?? keySet;
  }
  const known = keySet?.keys.get(kid);
  if (known) return known;
  if (nowMs - lastFetchAttemptMs < JWKS_REFETCH_FLOOR_MS) return null;
  keySet = (await fetchKeys(nowMs)) ?? keySet;
  return keySet?.keys.get(kid) ?? null;
}

function decodeSegment(segment: string): unknown {
  // Base64url, strictly: `Buffer.from(_, "base64url")` ignores what it cannot
  // read, so a segment that is not base64url at all decodes to something
  // rather than failing. Re-encoding and comparing is what makes the check
  // real — a token whose payload has trailing junk must not verify as the
  // payload without it.
  const decoded = Buffer.from(segment, "base64url");
  if (decoded.toString("base64url") !== segment) return null;
  try {
    return JSON.parse(decoded.toString("utf8")) as unknown;
  } catch {
    return null;
  }
}

function claimString(claims: Record<string, unknown>, name: string): string | null {
  const value = claims[name];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function claimSeconds(claims: Record<string, unknown>, name: string): number | null {
  const value = claims[name];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/**
 * The identity an assertion proves, or null.
 *
 * Null for every failure, deliberately, and for the same reason `unsealClaims`
 * returns null for every failure: a caller able to tell "expired" from "bad
 * signature" from "wrong audience" would eventually report the difference to
 * someone, and the difference is only useful to whoever is forging one.
 */
export async function verifyIapAssertion(
  assertion: string | null | undefined,
  nowMs: number = Date.now(),
): Promise<IapIdentity | null> {
  const audience = iapAudience();
  if (!audience) return null;
  if (!assertion) return null;

  const parts = assertion.split(".");
  if (parts.length !== 3) return null;
  const [encodedHeader, encodedPayload, encodedSignature] = parts as [string, string, string];
  if (!encodedHeader || !encodedPayload || !encodedSignature) return null;

  const header = decodeSegment(encodedHeader);
  if (!header || typeof header !== "object") return null;
  const headerFields = header as Record<string, unknown>;

  // The algorithm is checked against the one this file implements; it never
  // *selects* an implementation. This is the `alg: none` and the
  // RS256-for-HS256 confusion closed in one line: there is one verifier, and a
  // token that does not claim to be for it is refused before any key is read.
  if (headerFields.alg !== IAP_ALGORITHM) return null;
  if (headerFields.typ !== undefined && headerFields.typ !== "JWT") return null;
  const kid = claimString(headerFields, "kid");
  if (!kid) return null;

  const key = await keyFor(kid, nowMs);
  if (!key) return null;

  const signature = Buffer.from(encodedSignature, "base64url");
  // A JWS carries the two 32-byte ECDSA integers concatenated, not the DER
  // sequence `crypto.verify` assumes by default. Without `ieee-p1363` every
  // genuine assertion fails, which reads as an IAP outage rather than a bug
  // here; with the wrong length it would fail closed anyway.
  if (signature.length !== 64) return null;
  const signedBytes = Buffer.from(`${encodedHeader}.${encodedPayload}`, "utf8");
  let signatureValid: boolean;
  try {
    signatureValid = verifySignature(
      "sha256",
      signedBytes,
      { key, dsaEncoding: "ieee-p1363" },
      signature,
    );
  } catch {
    return null;
  }
  if (!signatureValid) return null;

  // Only now is anything parsed as a claim set. Everything above ran on bytes.
  const payload = decodeSegment(encodedPayload);
  if (!payload || typeof payload !== "object") return null;
  const claims = payload as Record<string, unknown>;

  if (claims.iss !== IAP_ISSUER) return null;
  // Exact equality, not a prefix or a substring. A backend service id is a
  // decimal number, and `startsWith` on an audience whose last component is
  // `12` is true of the one ending `123`.
  if (claims.aud !== audience) return null;

  const expiresAtSeconds = claimSeconds(claims, "exp");
  if (expiresAtSeconds === null) return null;
  const expiresAtMs = expiresAtSeconds * 1000;
  if (expiresAtMs + CLOCK_SKEW_MS <= nowMs) return null;

  const issuedAtSeconds = claimSeconds(claims, "iat");
  if (issuedAtSeconds !== null && issuedAtSeconds * 1000 - CLOCK_SKEW_MS > nowMs) return null;
  const notBeforeSeconds = claimSeconds(claims, "nbf");
  if (notBeforeSeconds !== null && notBeforeSeconds * 1000 - CLOCK_SKEW_MS > nowMs) return null;

  const subject = claimString(claims, "sub");
  const email = claimString(claims, "email");
  // Both are required. An assertion with no `email` is one IAP issued for a
  // service account or a programmatic caller; this console is a surface for a
  // person, and an account it cannot name is one it cannot attribute a read
  // to afterwards.
  if (!subject || !email) return null;

  return {
    subject,
    email: email.toLowerCase(),
    hostedDomain: claimString(claims, "hd"),
    expiresAtMs,
  };
}

/** The verified identity on a request, or null. Reads only the signed header. */
export async function iapIdentityFrom(
  headers: Headers,
  nowMs: number = Date.now(),
): Promise<IapIdentity | null> {
  return verifyIapAssertion(headers.get(IAP_ASSERTION_HEADER), nowMs);
}
