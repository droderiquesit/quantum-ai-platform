/**
 * A real HTTP platform, for the one suite that cannot use a stub.
 *
 * Every other specification intercepts at the browser boundary with
 * `page.route`, which is the right tool — but Playwright fulfils those requests
 * before the service worker ever sees them. A test of what the worker caches
 * would therefore pass whatever the worker did, which is worse than no test.
 *
 * So this process answers over a socket instead: the console's gateway handler
 * really forwards to it, the browser really goes to the network, and the worker
 * really gets a `fetch` event to make a decision about.
 *
 * Node's standard library only. This is test scaffolding, not a dependency.
 */
import { createServer } from "node:http";

const PORT = Number(process.env.PORT ?? 3313);

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
    mesh: { served: false },
  },
  "/api/v1/system/metrics": {
    cycles: 3,
    events_logged: 12,
    opportunities_queued: 0,
    proposals: 0,
    orders: 0,
    fills: 0,
    refusals: 0,
    live_fills: false,
  },
  "/api/v1/portfolio": { proposals: 0, orders: 0, fills: 0, paper_only: true },
  "/api/v1/opportunities": { opportunities: [] },
};

createServer((request, response) => {
  const path = new URL(request.url ?? "/", "http://localhost").pathname;
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
