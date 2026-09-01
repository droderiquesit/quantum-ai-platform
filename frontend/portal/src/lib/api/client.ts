/**
 * The browser's only route to the platform.
 *
 * Every call goes to this app's own gateway handler on the same origin, which
 * forwards it upstream with the deployment's credential attached. That keeps
 * the token out of the browser and means the backend needs no CORS policy to
 * be usable from here.
 *
 * The return type is a total account of what can come back — data, a stated
 * absence, a route that does not exist, a refusal, or an unreachable platform.
 * Callers switch on it; nothing in this module ever substitutes a value.
 */

import { isUnavailable, type Unavailable } from "./types";
import type * as T from "./types";

export const GATEWAY_PREFIX = "/api/gateway";

/** How the gateway classified the attempt, echoed in a response header. */
export type GatewayDisposition = "upstream" | "unreachable" | "timeout" | "misconfigured";

export const GATEWAY_HEADER = "x-qip-gateway";

export type ApiOutcome<D> =
  | { readonly kind: "ok"; readonly data: D }
  /** The platform answered, and the answer was "nothing here, because…". */
  | { readonly kind: "unavailable"; readonly subject: string; readonly reason: string }
  /** The platform has no such route. */
  | { readonly kind: "missing"; readonly endpoint: string; readonly status: number; readonly detail: string }
  /** The credential this deployment holds is not permitted to call it. */
  | { readonly kind: "denied"; readonly endpoint: string; readonly status: number; readonly detail: string }
  /** The gateway could not reach the platform at all. */
  | { readonly kind: "unreachable"; readonly endpoint: string; readonly detail: string }
  | {
      readonly kind: "error";
      readonly endpoint: string;
      readonly status: number | null;
      readonly detail: string;
    };

export interface ApiResponse<D> {
  readonly outcome: ApiOutcome<D>;
  /** Wall-clock time the answer landed, for freshness accounting. */
  readonly receivedAt: number;
  readonly latencyMs: number;
}

export function isOk<D>(outcome: ApiOutcome<D>): outcome is { kind: "ok"; data: D } {
  return outcome.kind === "ok";
}

/** A one-line description of a non-ok outcome, for a status strip. */
export function describeOutcome(outcome: ApiOutcome<unknown>): string {
  switch (outcome.kind) {
    case "ok":
      return "ok";
    case "unavailable":
      return `${outcome.subject}: not available`;
    case "missing":
      return `${outcome.endpoint} is not served by this platform (${outcome.status})`;
    case "denied":
      return `${outcome.endpoint} refused the console's credential (${outcome.status})`;
    case "unreachable":
      return `the platform is unreachable: ${outcome.detail}`;
    case "error":
      return outcome.status === null
        ? outcome.detail
        : `${outcome.endpoint} answered ${outcome.status}: ${outcome.detail}`;
  }
}

interface RequestOptions {
  readonly method?: "GET" | "POST" | "DELETE";
  readonly body?: unknown;
  readonly signal?: AbortSignal;
  readonly query?: Readonly<Record<string, string>>;
}

function detailFrom(body: unknown, fallback: string): string {
  if (typeof body === "object" && body !== null) {
    const record = body as Record<string, unknown>;
    for (const key of ["error", "detail", "message", "reason"]) {
      const value = record[key];
      if (typeof value === "string" && value.length > 0) return value;
    }
  }
  if (typeof body === "string" && body.length > 0) return body;
  return fallback;
}

export async function request<D>(
  path: string,
  options: RequestOptions = {},
): Promise<ApiResponse<D>> {
  const started = Date.now();
  const method = options.method ?? "GET";
  const search = options.query ? `?${new URLSearchParams(options.query).toString()}` : "";
  const url = `${GATEWAY_PREFIX}${path}${search}`;

  const finish = (outcome: ApiOutcome<D>): ApiResponse<D> => ({
    outcome,
    receivedAt: Date.now(),
    latencyMs: Date.now() - started,
  });

  let response: Response;
  try {
    const init: RequestInit = {
      method,
      cache: "no-store",
      headers: options.body === undefined ? undefined : { "content-type": "application/json" },
      ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
      ...(options.signal ? { signal: options.signal } : {}),
    };
    response = await fetch(url, init);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "the request could not be sent";
    return finish({ kind: "unreachable", endpoint: path, detail });
  }

  const disposition = (response.headers.get(GATEWAY_HEADER) ?? "upstream") as GatewayDisposition;

  let body: unknown = null;
  const text = await response.text();
  if (text.length > 0) {
    try {
      body = JSON.parse(text);
    } catch {
      body = text;
    }
  }

  if (disposition === "unreachable" || disposition === "timeout" || disposition === "misconfigured") {
    return finish({
      kind: "unreachable",
      endpoint: path,
      detail: detailFrom(body, `the gateway reported ${disposition}`),
    });
  }

  if (response.status === 404 || response.status === 501) {
    return finish({
      kind: "missing",
      endpoint: path,
      status: response.status,
      detail: detailFrom(body, "no such route"),
    });
  }

  if (response.status === 401 || response.status === 403) {
    return finish({
      kind: "denied",
      endpoint: path,
      status: response.status,
      detail: detailFrom(body, "the credential was refused"),
    });
  }

  if (!response.ok) {
    return finish({
      kind: "error",
      endpoint: path,
      status: response.status,
      detail: detailFrom(body, response.statusText || "the request failed"),
    });
  }

  if (isUnavailable(body)) {
    const absence = body as Unavailable;
    return finish({ kind: "unavailable", subject: absence.subject, reason: absence.reason });
  }

  return finish({ kind: "ok", data: body as D });
}

/** The typed surface pages call. One function per route the console reads. */
export const platform = {
  health: (signal?: AbortSignal) => request<T.Health>("/health", withSignal(signal)),
  systemStatus: (signal?: AbortSignal) => request<T.SystemStatus>("/system/status", withSignal(signal)),
  systemMetrics: (signal?: AbortSignal) => request<T.SystemMetrics>("/system/metrics", withSignal(signal)),
  governance: (signal?: AbortSignal) => request<T.Governance>("/system/governance", withSignal(signal)),
  mesh: (signal?: AbortSignal) => request<T.MeshStatus>("/mesh", withSignal(signal)),
  portfolio: (signal?: AbortSignal) => request<T.Portfolio>("/portfolio", withSignal(signal)),
  opportunities: (signal?: AbortSignal) => request<T.Opportunities>("/opportunities", withSignal(signal)),
  proposals: (signal?: AbortSignal) => request<T.Proposals>("/proposals", withSignal(signal)),
  orders: (signal?: AbortSignal) => request<T.Orders>("/orders", withSignal(signal)),
  fills: (signal?: AbortSignal) => request<T.Fills>("/fills", withSignal(signal)),
  agents: (signal?: AbortSignal) => request<T.Agents>("/agents", withSignal(signal)),
  autonomy: (signal?: AbortSignal) => request<T.Autonomy>("/autonomy", withSignal(signal)),
  system: (signal?: AbortSignal) => request<T.SystemView>("/system", withSignal(signal)),
  regions: (signal?: AbortSignal) => request<T.Regions>("/regions", withSignal(signal)),
  markets: (signal?: AbortSignal) => request<unknown>("/markets", withSignal(signal)),
  assets: (signal?: AbortSignal) => request<unknown>("/assets", withSignal(signal)),
  arbitrage: (signal?: AbortSignal) => request<unknown>("/arbitrage", withSignal(signal)),
  strategies: (signal?: AbortSignal) => request<T.Strategies>("/strategies", withSignal(signal)),
  models: (signal?: AbortSignal) => request<T.Models>("/models", withSignal(signal)),
  capital: (signal?: AbortSignal) => request<T.Capital>("/capital", withSignal(signal)),
  risk: (signal?: AbortSignal) => request<T.Risk>("/risk", withSignal(signal)),
  pnl: (signal?: AbortSignal) => request<unknown>("/pnl", withSignal(signal)),
  dataSources: (signal?: AbortSignal) => request<unknown>("/data-sources", withSignal(signal)),
  training: (signal?: AbortSignal) => request<unknown>("/training", withSignal(signal)),
  quantum: (signal?: AbortSignal) => request<T.Quantum>("/quantum", withSignal(signal)),

  /**
   * The route table the process is serving right now.
   *
   * Read live rather than assumed from `endpoints.ts`: a console that lists
   * routes from a constant will keep listing one the platform stopped serving.
   */
  openapi: (signal?: AbortSignal) =>
    request<T.OpenApiDocument>("/openapi.json", withSignal(signal)),

  /**
   * The three writes this console performs, and the only ones it may.
   *
   * Each is an operational control over the loop — advance it, stop it, start
   * it again — and none of them names an instrument, a side or a quantity.
   * There is deliberately no fourth: a function here that posted an order body
   * would be an order-submitting control whatever the platform answered to it,
   * and `POST /api/v1/orders` returning 405 today is the platform's fact, not
   * this console's guarantee. See `tests/boundary.spec.ts`.
   */
  runCycle: () => request<T.CycleReport>("/cycle", { method: "POST" }),
  tripKillSwitch: (reason: string) =>
    request<T.KillSwitchResponse>("/kill-switch", { method: "POST", query: { reason } }),
  clearKillSwitch: () => request<T.KillSwitchResponse>("/kill-switch", { method: "DELETE" }),
} as const;

function withSignal(signal: AbortSignal | undefined): RequestOptions {
  return signal ? { signal } : {};
}
