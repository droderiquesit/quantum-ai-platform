/**
 * Where the platform lives and how this process talks to it.
 *
 * Resolved per request rather than at module load so a container can be
 * restarted with a new target without rebuilding the app, and so the test
 * harness can point the gateway at a port nothing listens on.
 *
 * This module is imported only by route handlers, which never run in the
 * browser: the credential it reads must not cross that line.
 */
import { identityToken } from "./google-credentials";
import { secretFromEnvironment } from "./secret";

export interface Upstream {
  readonly baseUrl: string;
  readonly token: string | null;
  readonly timeoutMs: number;
  /**
   * The Cloud Run audience this console must prove an identity to, or null.
   *
   * See `AUDIENCE_VARIABLE` below. Null means the platform is not an
   * IAM-protected Cloud Run service — a local `qip-api`, or the suites' stub
   * — and no identity token is minted or sent.
   */
  readonly identityAudience: string | null;
}

/**
 * The credential the platform expects.
 *
 * Named here rather than inline because it is resolved through the `_FILE`
 * contract in `./secret`, and both halves of that contract have to name the
 * same variable for the "both set" refusal to mean anything.
 */
const TOKEN_VARIABLE = "QIP_API_TOKEN";

/**
 * The Cloud Run audience, when the platform is one.
 *
 * `qip-api` runs with `INGRESS_TRAFFIC_INTERNAL_ONLY` and requires
 * authentication, so reaching it needs a Google **identity** token whose `aud`
 * is the service's own URL. It is not a secret — it is a URL — so it is a
 * plain environment value rather than a mounted file, and it is not committed
 * because no value of it is right for two environments.
 *
 * **Absent means no identity token is sent, and that is deliberate rather than
 * lax.** Two things are true at once: a local `qip-api` and the suites' stub
 * have no IAM in front of them and would reject nothing, and a deployment that
 * does need one gets a loud refusal rather than a quiet anonymous call,
 * because minting throws when the metadata server is not there. The dangerous
 * middle — set but unusable — is the case that now fails closed; the previous
 * behaviour had no such variable at all, so *every* call was the quiet
 * anonymous one.
 */
const AUDIENCE_VARIABLE = "QIP_API_AUDIENCE";

/**
 * Where the identity token rides.
 *
 * **Not `authorization`.** Cloud Run forwards `authorization` to the container
 * untouched, so putting the Google token there would overwrite the platform's
 * own bearer credential and `qip-api` would answer 401 — trading one
 * authentication failure for another. `x-serverless-authorization` is the
 * header Cloud Run's front end consumes for its IAM check and strips before
 * the container sees it, which is exactly the separation needed here: Google
 * decides whether this console may call the service, and the platform decides,
 * from its own token, what this console may read.
 */
const IDENTITY_HEADER = "x-serverless-authorization";

export class UpstreamNotConfigured extends Error {}

export function upstream(): Upstream {
  const raw = process.env.QIP_API_BASE_URL?.trim();
  if (!raw) {
    throw new UpstreamNotConfigured(
      "QIP_API_BASE_URL is not set, so this console has no platform to read. " +
        "Set it to the base URL of the qip-api process, for example http://127.0.0.1:8080.",
    );
  }
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw new UpstreamNotConfigured(`QIP_API_BASE_URL is not a URL: ${raw}`);
  }
  // Resolved through the `_FILE` indirection the Secret Manager CSI driver and
  // Cloud Run's secret volumes both project, so the token is a mounted file
  // rather than a line in `/proc/<pid>/environ`. `secretFromEnvironment`
  // throws on a configuration that cannot be resolved — both sources set, or a
  // named file that is missing or empty — and that throw is deliberate: the
  // platform answers 401 to an unauthenticated call, so a console that
  // silently continued without its credential would report the platform
  // unreachable when the fault is entirely its own.
  const token = secretFromEnvironment(TOKEN_VARIABLE);
  const timeout = Number(process.env.QIP_API_TIMEOUT_MS ?? 10_000);
  const audience = process.env[AUDIENCE_VARIABLE]?.trim();
  return {
    baseUrl: parsed.origin + parsed.pathname.replace(/\/$/, ""),
    token,
    timeoutMs: Number.isFinite(timeout) && timeout > 0 ? timeout : 10_000,
    identityAudience: audience ? audience : null,
  };
}

/** The platform's versioned prefix. Versioned as a whole, so it is one string. */
export const API_VERSION_PREFIX = "/api/v1";

/**
 * The headers a call to the platform carries.
 *
 * Two credentials, on two headers, answering two different questions — see
 * `IDENTITY_HEADER` for why they cannot share one. Async because the identity
 * token comes from the metadata server; the access-token cache in
 * `google-credentials.ts` keeps that off the per-request path.
 *
 * A failure to mint propagates. The alternative — catching it and sending the
 * request anyway — produces a 403 from Cloud Run's front end that says nothing
 * about this console's own misconfiguration, and that is precisely the
 * three-step diagnosis ADR 0094 asked to be made a one-step one.
 */
export async function upstreamHeaders(target: Upstream, extra?: HeadersInit): Promise<Headers> {
  const headers = new Headers(extra);
  if (target.token) headers.set("authorization", `Bearer ${target.token}`);
  if (target.identityAudience) {
    headers.set(IDENTITY_HEADER, `Bearer ${await identityToken(target.identityAudience)}`);
  }
  return headers;
}

/** Path segments the gateway refuses to forward. */
const UNROUTABLE = /[^A-Za-z0-9._~-]/;

/**
 * Join a caller-supplied path onto the versioned prefix, refusing anything that
 * could escape it. The platform normalises paths itself and rejects traversal,
 * but a gateway that forwards `..` upstream has delegated its own access
 * control to the thing it is fronting.
 */
export function resolveUpstreamPath(segments: readonly string[]): string {
  if (segments.length === 0) {
    throw new UpstreamNotConfigured("the gateway was called with no path");
  }
  for (const segment of segments) {
    if (segment === "" || segment === "." || segment === ".." || UNROUTABLE.test(segment)) {
      throw new UpstreamNotConfigured(
        `the path segment ${JSON.stringify(segment)} is not routable`,
      );
    }
  }
  return `${API_VERSION_PREFIX}/${segments.join("/")}`;
}
