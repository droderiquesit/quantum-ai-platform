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
export interface Upstream {
  readonly baseUrl: string;
  readonly token: string | null;
  readonly timeoutMs: number;
}

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
  const token = process.env.QIP_API_TOKEN?.trim();
  const timeout = Number(process.env.QIP_API_TIMEOUT_MS ?? 10_000);
  return {
    baseUrl: parsed.origin + parsed.pathname.replace(/\/$/, ""),
    token: token && token.length > 0 ? token : null,
    timeoutMs: Number.isFinite(timeout) && timeout > 0 ? timeout : 10_000,
  };
}

/** The platform's versioned prefix. Versioned as a whole, so it is one string. */
export const API_VERSION_PREFIX = "/api/v1";

export function upstreamHeaders(target: Upstream, extra?: HeadersInit): Headers {
  const headers = new Headers(extra);
  if (target.token) headers.set("authorization", `Bearer ${target.token}`);
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
