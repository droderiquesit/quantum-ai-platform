/**
 * A real HTTP platform, for the two suites that cannot use a stub.
 *
 * Most specifications intercept at the browser boundary with `page.route`,
 * which is the right tool for what a page renders — but it fulfils a request
 * before the service worker ever sees it, and it never runs the gateway at all.
 * A test of what the worker caches, or of what the gateway removes from a body,
 * would therefore pass whatever those components did, which is worse than no
 * test: the console shipped two pages claiming an upstream address never
 * reached a browser, and every assertion behind that claim was a DOM assertion
 * against a stub the gateway had never touched.
 *
 * So this process answers over a socket instead: the console's gateway handler
 * really forwards to it, the browser really goes to the network, the worker
 * really gets a `fetch` event to make a decision about, and `wire.spec.ts` can
 * read what actually came back.
 *
 * Node's standard library only. This is test scaffolding, not a dependency.
 */
import { createServer } from "node:http";

const PORT = Number(process.env.PORT ?? 3313);

/**
 * A cell's base URL on the mesh transport, shaped like the real thing.
 *
 * `wire.spec.ts` asserts this string leaves this process and does not arrive at
 * a browser. It is a fixture: nothing listens here.
 */
const CELL_ADDRESS = "10.4.7.2:8410";

/** `MeshStatus` as `qip-api/src/mesh.rs` serialises it, cells and all. */
const MESH = {
  served: true,
  cells_served: 1,
  deltas_absorbed: 12,
  envelopes_dispatched: 3,
  inbox_depth: 0,
  cells: [{ cell: "eu-west-1", address: CELL_ADDRESS, spool_pending: 0, circuit: "closed" }],
  standings: [],
  last_undecodable: null,
};

/** Enough of the platform's surface for a page to render and poll. */
const BODIES = {
  "/api/v1/health": {
    status: "ok",
    halted: false,
    autonomy: "paper_trading",
    live_capable: false,
    reconciliation_breaks: 0,
  },
  "/api/v1/system/status": {
    autonomy: "paper_trading",
    configured_autonomy: "paper_trading",
    ceiling: "paper_trading",
    live_capable: false,
    halted: false,
    halted_scopes: [],
    cycles: 3,
    events: 12,
    archived: 12,
    // The embedded copy, and the reason the redaction is declared twice:
    // `SystemStatus` carries the mesh status whole, so every cell address is
    // on a route nobody thinks of as the mesh route.
    mesh: MESH,
  },
  "/api/v1/mesh": MESH,
  "/api/v1/system": {
    autonomy: "paper_trading",
    ceiling: "paper_trading",
    live: false,
    halted: false,
    halted_scopes: [],
    cycles: 3,
    events_logged: 12,
    chain_intact: true,
    chain_broken_at: null,
  },
  "/api/v1/regions": {
    freshness_bound: "45s",
    cells: [
      {
        cell: "eu-west-1",
        reported_at: "2025-10-09T08:53:20Z",
        age: "3s",
        stale: false,
        halted: false,
        positions: 4,
        strategies: 2,
        reconciliation_breaks: 0,
        gross: "0",
        net: "0",
      },
    ],
  },
  "/api/v1/agents": { agents: [] },
  // Carried in full, exactly as `registration_views.rs` serves it to
  // `Role::Viewer`. `wire.spec.ts` asserts these fields *do* arrive: the
  // console renders them on the credential-lifecycle pages, so it cannot
  // remove them, and the page that reads this route says so instead of
  // claiming otherwise.
  "/api/v1/registrations": {
    posture: "PAPER TRADING",
    served_at: "2025-10-09T08:53:20Z",
    sources: [
      {
        source_id: "alpaca-daily-bars",
        requirement: "account",
        standing: { standing: "keyless" },
        terms: "https://alpaca.markets/terms-and-conditions",
        secret_slot: "QIP_ALPACA_API_SECRET_KEY",
        secret_command: "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-",
        companion_secret_slots: [
          {
            variable: "QIP_ALPACA_API_KEY_ID",
            secret_command: "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-",
          },
        ],
      },
    ],
  },
  // `/api/v1/system/metrics` is deliberately not here: it is served from RAW
  // below, as bytes, so that "the gateway forwarded this untouched" is a claim
  // a test can distinguish from "the gateway parsed it and wrote it out again".
  "/api/v1/portfolio": { proposals: 0, orders: 0, fills: 0, paper_only: true },
  "/api/v1/opportunities": { opportunities: [] },
};

/**
 * One route served as bytes rather than as an object.
 *
 * The gateway is supposed to forward a body it has nothing declared for
 * untouched, and "untouched" has to be distinguishable from "parsed and
 * re-emitted" or the test guards nothing: every other body here is produced by
 * `JSON.stringify`, so a round trip through the gateway would reproduce it
 * exactly and a gateway that reserialised everything would pass. This one is
 * indented, so a rewrite shows up as a diff.
 */
const RAW = {
  "/api/v1/system/metrics": `{
  "cycles": 3,
  "events_logged": 12,
  "opportunities_queued": 0,
  "proposals": 0,
  "orders": 0,
  "fills": 0,
  "refusals": 0,
  "live_fills": false
}
`,
};

createServer((request, response) => {
  const path = new URL(request.url ?? "/", "http://localhost").pathname;
  const raw = RAW[path];
  if (raw !== undefined) {
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
    response.end(raw);
    return;
  }
  const body = BODIES[path] ?? {
    subject: path.replace("/api/v1/", ""),
    available: false,
    reason: "this stub does not model that subject",
  };
  response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
  response.end(JSON.stringify(body));
}).listen(PORT, "127.0.0.1", () => {
  // Playwright waits for this port to accept a connection before starting.
  console.log(`upstream stub on 127.0.0.1:${PORT}`);
});
