import { NextResponse, type NextRequest } from "next/server";
import { STREAM_CHANNELS, type StreamChannel } from "@/lib/api/endpoints";
import { authRequired } from "@/lib/server/auth-gate";
import { sessionFrom } from "@/lib/server/auth-http";
import { API_VERSION_PREFIX, upstream, upstreamHeaders, type Upstream } from "@/lib/server/upstream";

/**
 * The server-sent event gateway.
 *
 * `/api/stream/<channel>` is piped from `<QIP_API_BASE_URL>/api/v1/stream/
 * <channel>` byte for byte. `Last-Event-ID` is forwarded so a reconnecting
 * client resumes where it stopped rather than restarting the sequence, which is
 * the whole reason the platform stamps one.
 *
 * Nothing is buffered or transformed on the way through: an event that is late
 * because a proxy held it is indistinguishable, downstream, from an event the
 * platform sent late, and a stale-data indicator that cannot tell those apart
 * is worthless.
 */

export const runtime = "nodejs";
export const dynamic = "force-dynamic";
export const fetchCache = "force-no-store";

interface RouteContext {
  params: Promise<{ channel: string }>;
}

function isChannel(value: string): value is StreamChannel {
  return (STREAM_CHANNELS as readonly string[]).includes(value);
}

export async function GET(request: NextRequest, context: RouteContext): Promise<Response> {
  // The same session boundary the REST gateway holds. This handler used to
  // have none: with the gate on, `/api/gateway/orders` answered 401 to an
  // anonymous caller while `/api/stream/orders` — the same data over time,
  // forwarded with the same bearer token — answered it in full. A stream is
  // read-only, so no CSRF pair is asked for; the session is.
  if (authRequired() && !sessionFrom(request)) {
    return NextResponse.json(
      { error: "sign in to use this console", gateway: "unauthenticated" },
      { status: 401, headers: { "x-qip-gateway": "upstream", "cache-control": "no-store" } },
    );
  }
  const { channel } = await context.params;
  if (!isChannel(channel)) {
    return NextResponse.json(
      {
        error: `there is no stream channel named ${JSON.stringify(channel)}`,
        channels: STREAM_CHANNELS,
      },
      { status: 404, headers: { "x-qip-gateway": "misconfigured", "cache-control": "no-store" } },
    );
  }

  let target: Upstream;
  try {
    target = upstream();
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "the gateway is not configured";
    return NextResponse.json(
      { error: detail, gateway: "misconfigured" },
      { status: 500, headers: { "x-qip-gateway": "misconfigured", "cache-control": "no-store" } },
    );
  }

  const url = `${target.baseUrl}${API_VERSION_PREFIX}/stream/${channel}`;
  const headers = upstreamHeaders(target, {
    accept: "text/event-stream",
    "cache-control": "no-store",
  });
  const lastEventId = request.headers.get("last-event-id");
  if (lastEventId) headers.set("last-event-id", lastEventId);

  try {
    const response = await fetch(url, {
      headers,
      signal: request.signal,
      cache: "no-store",
      redirect: "manual",
    });

    if (!response.ok || !response.body) {
      const text = await response.text().catch(() => "");
      return NextResponse.json(
        {
          error: `${url} answered ${response.status}`,
          gateway: "upstream",
          status: response.status,
          body: text.slice(0, 512),
        },
        {
          status: response.status === 404 ? 404 : 502,
          headers: { "x-qip-gateway": "upstream", "cache-control": "no-store" },
        },
      );
    }

    return new Response(response.body, {
      status: 200,
      headers: {
        "content-type": "text/event-stream; charset=utf-8",
        "cache-control": "no-store, no-transform",
        connection: "keep-alive",
        // Nginx and friends buffer text/event-stream by default, which turns a
        // live feed into a batch delivery.
        "x-accel-buffering": "no",
        "x-qip-gateway": "upstream",
      },
    });
  } catch (cause) {
    if (request.signal.aborted) {
      // The reader went away. Not an error, and nothing left to answer.
      return new Response(null, { status: 499, headers: { "x-qip-gateway": "upstream" } });
    }
    const detail = cause instanceof Error ? cause.message : "unknown error";
    return NextResponse.json(
      { error: `${url} could not be reached: ${detail}`, gateway: "unreachable", upstream: url },
      { status: 502, headers: { "x-qip-gateway": "unreachable", "cache-control": "no-store" } },
    );
  }
}
