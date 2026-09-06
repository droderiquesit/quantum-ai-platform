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
  predictions: { method: "GET", path: "/predictions", role: "viewer", summary: "recorded claims per instrument and their calibration" },
  correlation: { method: "GET", path: "/correlation", role: "viewer", summary: "pearson correlation of returns over the tape, or why not" },
  backtests: { method: "GET", path: "/backtests", role: "viewer", summary: "holdout evidence, gate findings and bands from the ledger" },
  regimes: { method: "GET", path: "/regimes", role: "viewer", summary: "why no regime view is served, and the declared stream topic" },
  registrations: { method: "GET", path: "/registrations", role: "viewer", summary: "what each venue demands before it is read, and who has registered" },
  cycle: { method: "POST", path: "/cycle", role: "analyst", summary: "run one cycle of the intelligence loop" },
  killSwitchTrip: { method: "POST", path: "/kill-switch", role: "operator", summary: "halt the platform" },
  killSwitchClear: { method: "DELETE", path: "/kill-switch", role: "operator", summary: "clear a halt" },
  /**
   * The fourth write, and the only one with a path parameter. It records
   * that a named operator registered with a venue under terms they read; it
   * names no instrument, side or quantity, and the platform behind it creates
   * no account — a registration nobody made cannot be approved into existence.
   */
  registrationApprove: {
    method: "POST",
    path: "/registrations/{source_id}/approve",
    role: "operator",
    summary: "record that the operator registered with a venue under terms they read",
  },
  /**
   * The fifth write, and it was missing from this table for a whole wave.
   *
   * `EligibilityPanel` has called `POST /ledger/users/{user}/eligibility`
   * since the panel landed, `ledger_views.rs` serves it, and the backend's
   * own boundary suite pins it as the fifth mutating route
   * (`every_mutating_route_is_one_of_five_and_each_raises_a_typed_intent`).
   * The gateway, meanwhile, refuses any non-GET this table does not declare,
   * so every decision a person made in a real deployment came back 405.
   *
   * Nothing caught it because the seven eligibility specs `page.route`-mock
   * the gateway, which is the one component that would have refused — a test
   * that stubs the thing under test. `the_gateway_declares_every_write_the_console_can_make`
   * in `tests/registrations-gateway.spec.ts` now derives the check from this
   * table rather than from a list someone must remember to extend.
   *
   * It records an operator's decision about whether one investor may be
   * funded. It names no instrument, side or quantity, and it cannot move
   * capital: `fund_user` is refused until a standing grant exists, so this
   * route can only ever lift a refusal a person took responsibility for.
   */
  ledgerEligibilityDecide: {
    method: "POST",
    path: "/ledger/users/{user}/eligibility",
    role: "operator",
    summary: "record an operator's decision on whether one investor may be funded",
  },
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
  /**
   * The note here said `/data-sources` carries no health fields "even when the
   * data finder is wired in", which reads as a route with a wiring branch that
   * happens not to be taken. There is no branch. `routes.rs` matches
   * `(Method::Get, "/data-sources")` to one expression and returns
   * `unavailable("sources", NO_DATA_FINDER)` unconditionally, so no
   * composition of this process answers anything else — a fact stronger than
   * the one the note claimed, stated imprecisely in the platform's favour.
   */
  dataSourceHealth: {
    method: "GET",
    path: "/api/v1/data-sources/health",
    needed_for: "per-source latency, freshness, quality and provenance",
    note:
      "GET /data-sources answers an availability record and nothing else: routes.rs matches it to one expression, unavailable(\"sources\", NO_DATA_FINDER), with no second arm, so no build or composition of this process serves a health or provenance field on that route.",
  },
  compliance: {
    method: "GET",
    path: "/api/v1/compliance",
    needed_for: "compliance obligations and attestations",
    note: "GET /risk covers exposure and the kill switch; there is no compliance surface.",
  },
  /**
   * The attestation half of the entry above, separated because the two are
   * missing for different reasons and a page that named only "compliance"
   * invited the reading that the hash chain covers both.
   *
   * `GET /system` re-walks the hash chain on every read and answers
   * `chain_intact`, which proves no sealed record was edited. That is not an
   * attestation: nobody signed it, it covers no period, and it names no
   * obligation it was produced against. The `/compliance` page renders this
   * beside the chain result precisely so a reader does not take a live
   * integrity check for a countersigned statement.
   */
  complianceAttestations: {
    method: "GET",
    path: "/api/v1/compliance/attestations",
    needed_for: "the attestations a named person signed, what each covered, and when it lapses",
    note:
      "GET /system re-verifies the event log's hash chain on every read, which proves records were not edited after sealing. No route serves a statement anybody signed, over a stated period, against a named obligation — and integrity is not attestation.",
  },
  /**
   * Why the risk surface can say a halt is on but not how the desk got here.
   *
   * `GET /risk` answers the kill switch's *current* trip — halted, the scopes,
   * who tripped it, the reason — and a count of clearances. A count is not a
   * record: it cannot say when each halt began, how long it ran, who cleared
   * it or on what basis, so a desk reconstructing an incident from this
   * console alone cannot. The event log holds the facts; no HTTP route
   * projects them.
   */
  killSwitchHistory: {
    method: "GET",
    path: "/api/v1/kill-switch/history",
    needed_for: "each halt and clearance with its operator, instant, scope and basis",
    note:
      "GET /risk answers the current trip and a count of clearances only. A count cannot say when a halt began, how long it ran, or who cleared it, so the halt record an incident review needs is not reachable from this console.",
  },
  topology: {
    method: "GET",
    path: "/api/v1/topology",
    needed_for: "service dependency graph",
    note:
      "assembled here from /system, /mesh, /regions and /agents; the platform serves no single topology document.",
  },
  /**
   * The route that would make `/treasury/accounts` the reader's own account
   * rather than one an operator picked.
   *
   * Written down here, in the table every page renders from, because it was
   * previously stated only in a source comment and in a handoff document —
   * neither of which an operator looking at the screen can read. The console
   * shows one account because that is all it can honestly show; the reason is
   * a missing route, and a missing route is a thing this console has a way of
   * saying.
   */
  accountForSession: {
    method: "GET",
    path: "/api/v1/account/me",
    needed_for: "an account bound to the person signed in, instead of one an operator selected",
    note:
      "this console authenticates to the platform with one deployment credential, so every browser session arrives as the same subject and the platform has no ledger account to resolve it to. Binding one needs a keyed assertion the API verifies — an architecture decision, not a page.",
  },
  /**
   * The single-account read. `GET /ledger/users` answers every account and
   * requires `analyst`; there is no `GET /ledger/users/{user}`, which is why
   * the account page filters a list it had to fetch whole, and why an id the
   * ledger does not hold is this console's finding rather than the platform's
   * 404.
   */
  accountByUser: {
    method: "GET",
    path: "/api/v1/ledger/users/{user}",
    needed_for: "one account without reading every account",
    note:
      "GET /ledger/users lists them all, so a page about one account is served a body carrying the rest. A 404 for an id the ledger does not hold would also be the platform's answer rather than this console's inference from a list.",
  },
} as const;

/**
 * Whether this console declares a write on that method and that path.
 *
 * The gateway asks before it forwards. Until it did, the "three writes and no
 * fourth" rule lived in `client.ts` — in the browser bundle, where any page
 * could reach past it with a bare `fetch` and the gateway would attach the
 * deployment credential to whatever came through. `/order-entry` was that page
 * once: it composed an instrument, a side, a quantity and a price and posted
 * them, waiting for the platform to grow the route. A write this table does
 * not name now never reaches the platform, and never reaches it wearing the
 * platform's credential.
 *
 * The match is on the whole path, not a prefix: `/kill-switch/all` is a
 * different route from `/kill-switch` and is not declared. A `{parameter}`
 * segment in a declared path matches exactly one non-empty segment, so
 * `/registrations/alpaca-daily-bars/approve` is declared and
 * `/registrations/approve`, `/registrations/a/b/approve` and
 * `/registrations/a/approve/now` are not — a template that matched a prefix
 * would declare every route under it, including ones the platform grows
 * later without this console being asked.
 *
 * GET is not asked about. A read cannot submit an order, and a gateway that
 * refused an unlisted read would make this console lie about a route the
 * platform had started serving.
 */
export function declaresWrite(method: string, path: string): boolean {
  return Object.values(REST).some(
    (spec) => spec.method !== "GET" && spec.method === method && pathMatches(spec.path, path),
  );
}

/**
 * The declared writes, as a sentence, derived from the table rather than
 * transcribed from it.
 *
 * The gateway's refusal used to spell the four writes out by hand and say
 * "adding a fifth is an edit to the route table". A fifth was added to the
 * platform and the panel, nobody edited the table, and the refusal went on
 * confidently naming four — so the message that was supposed to tell an
 * operator what to do instead was itself the stale thing. Deriving it means
 * the sentence cannot disagree with the allowlist it describes.
 */
export function describeWrites(): string {
  const writes = Object.values(REST)
    .filter((spec) => spec.method !== "GET")
    .map((spec) => `${spec.method} ${spec.path}`);
  if (writes.length === 0) return "none";
  if (writes.length === 1) return writes[0] ?? "none";
  return `${writes.slice(0, -1).join(", ")} and ${writes[writes.length - 1]}`;
}

function pathMatches(template: string, path: string): boolean {
  const wanted = template.split("/");
  const given = path.split("/");
  if (wanted.length !== given.length) return false;
  return wanted.every((segment, index) => {
    const actual = given[index] ?? "";
    if (segment.startsWith("{") && segment.endsWith("}")) return actual.length > 0;
    return segment === actual;
  });
}
