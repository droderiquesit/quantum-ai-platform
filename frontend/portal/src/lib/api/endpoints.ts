/**
 * The platform endpoints this console reads, and the ones it needs that the
 * platform does not serve yet.
 *
 * Keeping both in one table means a page never has to guess why a panel is
 * empty: either the route answered, or it is listed here as absent and the page
 * says so by name. Nothing in the UI invents a number to fill the gap.
 */

export interface EndpointSpec {
  readonly method: "GET" | "POST" | "DELETE";
  /** Path under `/api/v1`. */
  readonly path: string;
  /** Least role the platform requires. */
  readonly role: "monitor" | "viewer" | "analyst" | "operator";
  readonly summary: string;
}

/** Routes declared by `backend/crates/apps/qip-api/src/routes.rs`. */
export const REST: Record<string, EndpointSpec> = {
  health: { method: "GET", path: "/health", role: "monitor", summary: "liveness and halt state" },
  systemStatus: { method: "GET", path: "/system/status", role: "viewer", summary: "autonomy, kill switch, cycle count" },
  systemMetrics: { method: "GET", path: "/system/metrics", role: "monitor", summary: "counters and gauges" },
  governance: { method: "GET", path: "/system/governance", role: "viewer", summary: "agent roster governance findings" },
  mesh: { method: "GET", path: "/mesh", role: "viewer", summary: "mesh backbone counters" },
  portfolio: { method: "GET", path: "/portfolio", role: "viewer", summary: "book counts and paper-only flag" },
  opportunities: { method: "GET", path: "/opportunities", role: "viewer", summary: "the opportunity queue" },
  proposals: { method: "GET", path: "/proposals", role: "viewer", summary: "proposals and their status" },
  orders: { method: "GET", path: "/orders", role: "viewer", summary: "orders, refusals, reconciliation breaks" },
  fills: { method: "GET", path: "/fills", role: "viewer", summary: "every fill and whether it was simulated" },
  agents: { method: "GET", path: "/agents", role: "viewer", summary: "the agent roster and manifests" },
  autonomy: { method: "GET", path: "/autonomy", role: "viewer", summary: "autonomy level, ceiling and history" },
  system: { method: "GET", path: "/system", role: "viewer", summary: "autonomy, halt, cycles, event-chain integrity" },
  regions: { method: "GET", path: "/regions", role: "viewer", summary: "edge cells, their books and report age" },
  markets: { method: "GET", path: "/markets", role: "viewer", summary: "market state (desk capability gated)" },
  assets: { method: "GET", path: "/assets", role: "viewer", summary: "reference universe (desk capability gated)" },
  arbitrage: { method: "GET", path: "/arbitrage", role: "viewer", summary: "active arbitrage paths" },
  strategies: { method: "GET", path: "/strategies", role: "viewer", summary: "strategies and ladder stage" },
  models: { method: "GET", path: "/models", role: "viewer", summary: "observed model spend" },
  capital: { method: "GET", path: "/capital", role: "viewer", summary: "bounds, envelopes, outstanding recalls" },
  risk: { method: "GET", path: "/risk", role: "viewer", summary: "exposure, concentration, kill switch" },
  pnl: { method: "GET", path: "/pnl", role: "viewer", summary: "profit, loss, realised against expected alpha" },
  dataSources: { method: "GET", path: "/data-sources", role: "viewer", summary: "data sources with health and licensing" },
  training: { method: "GET", path: "/training", role: "viewer", summary: "training runs and status" },
  quantum: { method: "GET", path: "/quantum", role: "viewer", summary: "quantum jobs and classical baseline" },
  cycle: { method: "POST", path: "/cycle", role: "analyst", summary: "run one cycle of the intelligence loop" },
  killSwitchTrip: { method: "POST", path: "/kill-switch", role: "operator", summary: "halt the platform" },
  killSwitchClear: { method: "DELETE", path: "/kill-switch", role: "operator", summary: "clear a halt" },
} as const;

/** Server-sent event channels under `/api/v1/stream`. */
export const STREAM_CHANNELS = ["market", "signals", "orders", "positions", "health"] as const;
export type StreamChannel = (typeof STREAM_CHANNELS)[number];

/**
 * Endpoints a page here needs that the platform does not serve.
 *
 * A page that depends on one of these renders the entry verbatim rather than a
 * placeholder chart, so what is missing is legible from the screen.
 */
export interface MissingEndpoint {
  readonly method: string;
  readonly path: string;
  readonly needed_for: string;
  readonly note: string;
}

export const NOT_YET_SERVED: Record<string, MissingEndpoint> = {
  positions: {
    method: "GET",
    path: "/api/v1/positions",
    needed_for: "position-level portfolio detail",
    note:
      "GET /portfolio returns counts and the paper-only flag; position rows sit behind the desk's capability gate.",
  },
  cash: {
    method: "GET",
    path: "/api/v1/cash",
    needed_for: "cash and settlement balances",
    note: "no cash ledger is exposed; /capital reports allocation bounds, not balances.",
  },
  dataSourceHealth: {
    method: "GET",
    path: "/api/v1/data-sources/health",
    needed_for: "per-source latency, freshness, quality and provenance",
    note:
      "GET /data-sources answers with an availability record only; it carries no health or provenance fields even when the data finder is wired in.",
  },
  compliance: {
    method: "GET",
    path: "/api/v1/compliance",
    needed_for: "compliance obligations and attestations",
    note: "GET /risk covers exposure and the kill switch; there is no compliance surface.",
  },
  topology: {
    method: "GET",
    path: "/api/v1/topology",
    needed_for: "service dependency graph",
    note:
      "assembled here from /system, /mesh, /regions and /agents; the platform serves no single topology document.",
  },
} as const;
