import { NextResponse, type NextRequest } from "next/server";
import { requireCsrf, sessionIdFrom } from "@/lib/server/auth-http";
import { identityStore } from "@/lib/server/identity-store";
import {
  resolveUpstreamPath,
  upstream,
  upstreamHeaders,
  type Upstream,
} from "@/lib/server/upstream";

/**
 * The REST gateway.
 *
 * `/api/gateway/<rest>` becomes `<QIP_API_BASE_URL>/api/v1/<rest>`, with the
 * deployment's bearer token attached here rather than in the browser. The
 * upstream status and body are passed through untouched: a console that
 * rewrote a 404 into an empty list would be lying about the platform.
 *
 * `x-qip-gateway` states who produced the response, so the client can tell "the
 * platform said no such route" from "this process could not reach the platform".
 */

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

interface RouteContext {
  params: Promise<{ path: string[] }>;
}

/**
 * The session boundary, when authentication is required.
 *
 * The middleware's redirect is comfort; this is the check that counts. The
 * cookie's signature is verified and the session looked up server-side, so a
 * forged or revoked cookie reads nothing — and mutating calls additionally
 * need the CSRF pair, because a browser can be made to *send* cookies
 * cross-site but not to read the token that must be echoed in a header.
 */
function refuseUnauthenticated(request: NextRequest): NextResponse | null {
  if (process.env.ALGORIK_AUTH_REQUIRED !== "true") return null;
  const sessionId = sessionIdFrom(request);
  if (!sessionId || !identityStore.session(sessionId)) {
    return NextResponse.json(
      { error: "sign in to use this console", gateway: "unauthenticated" },
      { status: 401, headers: { "x-qip-gateway": "upstream", "cache-control": "no-store" } },
    );
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    const refused = requireCsrf(request);
    if (refused) return refused;
  }
  return null;
}

async function forward(request: NextRequest, context: RouteContext): Promise<Response> {
  const refused = refuseUnauthenticated(request);
  if (refused) return refused;

  let target: Upstream;
  let path: string;
  try {
    target = upstream();
    const { path: segments } = await context.params;
    path = resolveUpstreamPath(segments);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "the gateway is not configured";
    return NextResponse.json(
      { error: detail, gateway: "misconfigured" },
      { status: 500, headers: { "x-qip-gateway": "misconfigured", "cache-control": "no-store" } },
    );
  }

  const incoming = new URL(request.url);
  const url = `${target.baseUrl}${path}${incoming.search}`;

  const headers = upstreamHeaders(target, { accept: "application/json" });
  const contentType = request.headers.get("content-type");
  if (contentType) headers.set("content-type", contentType);

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), target.timeoutMs);

  try {
    const body =
      request.method === "GET" || request.method === "HEAD" ? undefined : await request.text();
    const response = await fetch(url, {
      method: request.method,
      headers,
      ...(body === undefined || body.length === 0 ? {} : { body }),
      signal: controller.signal,
      cache: "no-store",
      redirect: "manual",
    });
    const text = await response.text();
    return new Response(text, {
      status: response.status,
      headers: {
        "content-type": response.headers.get("content-type") ?? "application/json",
        "cache-control": "no-store",
        "x-qip-gateway": "upstream",
        "x-qip-upstream-url": url,
      },
    });
  } catch (cause) {
    const aborted = controller.signal.aborted;
    const detail = aborted
      ? `the platform did not answer ${url} within ${target.timeoutMs}ms`
      : `${url} could not be reached: ${cause instanceof Error ? cause.message : "unknown error"}`;
    return NextResponse.json(
      { error: detail, gateway: aborted ? "timeout" : "unreachable", upstream: url },
      {
        status: aborted ? 504 : 502,
        headers: {
          "x-qip-gateway": aborted ? "timeout" : "unreachable",
          "cache-control": "no-store",
        },
      },
    );
  } finally {
    clearTimeout(timer);
  }
}

export const GET = forward;
export const POST = forward;
export const DELETE = forward;
