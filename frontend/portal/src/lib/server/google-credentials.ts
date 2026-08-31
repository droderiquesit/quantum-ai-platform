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

/** Discard the cached token. Exists for tests; nothing in a request path calls it. */
export function forgetAccessToken(): void {
  cached = null;
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
