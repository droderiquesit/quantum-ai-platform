/**
 * The console's own Google credential, from the metadata server.
 *
 * Application Default Credentials, reduced to the one form this process is
 * ever deployed with: a Cloud Run service account, resolved at runtime from
 * the instance metadata server. There is no key file, there is no key file
 * path, and there is no branch here that would read one — the standing rule is
 * Workload Identity Federation only, and a resolver that *could* fall back to
 * a downloaded key is how a downloaded key ends up in a repository.
 *
 * This is not the Identity Platform browser API key. That key selects a
 * project and authenticates nobody; it is public by design. This token
 * authenticates the console *as itself*, and it is what the administrative
 * half of `identity-platform.ts` presents when it reads or writes an account's
 * custom claims.
 *
 * The token is cached until shortly before it expires. Not for speed: the
 * metadata server is rate-limited per instance, and a console under load that
 * fetched a token per request would start failing to authenticate for reasons
 * that look nothing like an authentication problem.
 *
 * Nothing here logs the token, and no error message contains it.
 */

const METADATA_HOST = "http://metadata.google.internal";
const TOKEN_PATH = "/computeMetadata/v1/instance/service-accounts/default/token";
const IDENTITY_PATH = "/computeMetadata/v1/instance/service-accounts/default/identity";

/** Every call that leaves the process carries an explicit timeout. */
const TIMEOUT_MS = 5_000;

/**
 * Refresh this long before expiry.
 *
 * A token used at the instant it expires is a token the far end rejects, and
 * the resulting 401 is indistinguishable from a missing IAM grant — which is
 * the wrong thing to spend an afternoon on.
 */
const REFRESH_MARGIN_MS = 60_000;

export class MetadataUnavailable extends Error {}

interface CachedToken {
  readonly token: string;
  readonly expiresAtMs: number;
}

let cached: CachedToken | null = null;

/**
 * Identity tokens, one per audience.
 *
 * Keyed by audience because an identity token *is* its audience: one minted
 * for `qip-api` proves nothing to anything else, and a single-slot cache
 * shared between two audiences would hand each caller the other's token and
 * produce a 401 that looks like a missing IAM grant.
 */
const identityCache = new Map<string, CachedToken>();

/** Discard the cached tokens. Exists for tests; nothing in a request path calls it. */
export function forgetAccessToken(): void {
  cached = null;
  identityCache.clear();
}

/**
 * An OAuth access token for this process's service account.
 *
 * Throws when the metadata server is not reachable, which off Cloud Run it
 * never is. That is deliberate: the caller is an administrative path that
 * cannot do its job without a credential, and returning null would let it
 * continue and fail later somewhere less obvious.
 */
export async function accessToken(now: number = Date.now()): Promise<string> {
  if (cached && cached.expiresAtMs - REFRESH_MARGIN_MS > now) return cached.token;

  let response: Response;
  try {
    response = await fetch(`${METADATA_HOST}${TOKEN_PATH}`, {
      // The header is what distinguishes a real metadata request from a
      // browser that was tricked into making one; the server refuses without
      // it.
      headers: { "metadata-flavor": "Google" },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (cause) {
    throw new MetadataUnavailable(
      "the instance metadata server did not answer, so this process has no Google " +
        "credential of its own. Off Cloud Run that is expected: run with no " +
        "ALGORIK_IDENTITY_PROJECT_ID to use the development identity provider. " +
        `(${cause instanceof Error ? cause.message : "unknown error"})`,
    );
  }

  if (!response.ok) {
    throw new MetadataUnavailable(
      `the metadata server refused a token with HTTP ${response.status}. The service ` +
        "account may have no scope for it.",
    );
  }

  const payload = (await response.json().catch(() => null)) as {
    access_token?: string;
    expires_in?: number;
  } | null;

  const token = payload?.access_token;
  if (!token) {
    throw new MetadataUnavailable(
      "the metadata server answered without an access_token field",
    );
  }

  // expires_in is seconds. A missing or absurd value is treated as one
  // minute rather than trusted: caching a token past its life produces
  // intermittent 401s, and re-fetching too often is merely wasteful.
  const lifetimeSeconds =
    typeof payload.expires_in === "number" && payload.expires_in > 0 ? payload.expires_in : 60;
  cached = { token, expiresAtMs: now + lifetimeSeconds * 1000 };
  return token;
}

/**
 * How long an identity token is reused.
 *
 * The metadata server mints these with an hour's life and does not report the
 * expiry alongside the token — the response is the bare JWT. Rather than
 * decode a token this process is only carrying, it is held for a fixed
 * interval well inside that hour. Fifty minutes would be cutting it fine on a
 * request that then takes a while; forty-five is not.
 */
const IDENTITY_TTL_MS = 45 * 60 * 1000;

/**
 * A Google **identity** token for `audience`, from the metadata server.
 *
 * This is the credential Cloud Run's IAM check wants, and the console had none
 * — which is the bug ADR 0094 names among its costs: "The portal's gateway
 * still cannot authenticate to `qip-api`. It sends the platform bearer token
 * and never a Google ID token... so the invoker grant ADR 0018 made is not yet
 * exercised by anything." An *access* token, which `accessToken` above mints,
 * is not a substitute: Cloud Run's front end validates an ID token whose `aud`
 * is the service URL, and refuses an access token with a 401 that reads
 * exactly like a missing grant — which is a long afternoon spent on the IAM
 * console for a fault that is in this file.
 *
 * `format=full` is asked for because it includes the instance claims a Cloud
 * Run front end expects; the default omits them.
 *
 * Throws rather than returning null, for the same reason `accessToken` does:
 * the caller cannot do its job without this, and a caller that continued would
 * send an unauthenticated request and report the platform unreachable when the
 * fault is entirely its own.
 *
 * Nothing here logs the token, and no error message contains it.
 */
export async function identityToken(audience: string, now: number = Date.now()): Promise<string> {
  const held = identityCache.get(audience);
  if (held && held.expiresAtMs - REFRESH_MARGIN_MS > now) return held.token;

  const url = `${METADATA_HOST}${IDENTITY_PATH}?audience=${encodeURIComponent(audience)}&format=full`;
  let response: Response;
  try {
    response = await fetch(url, {
      headers: { "metadata-flavor": "Google" },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (cause) {
    throw new MetadataUnavailable(
      "the instance metadata server did not answer, so this console cannot prove its " +
        "identity to the platform. Off Cloud Run that is expected: leave " +
        "QIP_API_AUDIENCE unset when the platform is not an authenticated Cloud Run " +
        `service. (${cause instanceof Error ? cause.message : "unknown error"})`,
    );
  }

  if (!response.ok) {
    throw new MetadataUnavailable(
      `the metadata server refused an identity token with HTTP ${response.status}. Check that ` +
        "QIP_API_AUDIENCE is the platform's Cloud Run URL and nothing else.",
    );
  }

  // The body is the bare JWT, not JSON. A response that is not one — an error
  // page from something in the way, say — is refused here rather than carried
  // upstream as though it were a credential.
  const token = (await response.text()).trim();
  if (token.split(".").length !== 3) {
    throw new MetadataUnavailable(
      "the metadata server answered with something that is not an identity token",
    );
  }
  identityCache.set(audience, { token, expiresAtMs: now + IDENTITY_TTL_MS });
  return token;
}
