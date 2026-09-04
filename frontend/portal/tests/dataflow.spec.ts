/**
 * `/loop/dataflow`: the eight stages left to right, what flowed along each
 * edge, and where it went — every figure from a route, the stream, or the
 * cycle report the platform returned.
 *
 * The failures these tests prevent:
 *
 * * a flow drawn from memory — asserted by the stage boxes carrying the exact
 *   sentence `POST /cycle` answered, in the kernel's order, and by a refusal
 *   rendering the platform's reason word for word rather than a paraphrase;
 * * a page that goes quiet while the loop runs elsewhere — asserted by the
 *   cycle count following `/stream/health` and every route being re-read
 *   when it moves, not on a timer;
 * * a control that advances or trades — asserted by no non-GET request
 *   leaving the page at all.
 *
 * Every body below is copied from a `qip-api` process at `paper_trading` on
 * 2026-09-04 after three blind cycles, not hand-written; a plausible shape
 * that is not the real one tests the console against a platform that does
 * not exist.
 */
import { expect, test, type Page } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

// --- bodies copied from a running qip-api -----------------------------------

const CYCLE_3 = {
  cycle: 3,
  correlation_id: "01M1Q96BNKH9TYMAHC00DGR281",
  halted: false,
  traversed_every_stage: true,
  archived: 1,
  stages: [
    { stage: "sense", ran: true, produced: 0, detail: "no observations have been fed in; the platform is running blind", problems: [] },
    { stage: "understand", ran: true, produced: 0, detail: "world model holds 0 instrument(s), 0 entity(ies), 0 relationship(s), 0 causal claim(s), 0 readable feature value(s), 0 document(s)", problems: [] },
    { stage: "discover", ran: true, produced: 0, detail: "0 opportunity(ies) found, 0 queued, 0 suppressed this run", problems: [] },
    { stage: "reason", ran: true, produced: 0, detail: "nothing in the queue to reason about", problems: [] },
    { stage: "simulate", ran: true, produced: 0, detail: "0 observation(s) is too little history to simulate from", problems: [] },
    { stage: "decide", ran: true, produced: 0, detail: "no thesis cleared the action bar; nothing to propose", problems: [] },
    { stage: "act", ran: true, produced: 0, detail: "no approved proposal to release; risk monitor says continue", problems: [] },
    { stage: "learn", ran: true, produced: 0, detail: "no fills to attribute; nothing has resolved yet", problems: [] },
  ],
} as const;

const NO_ATTRIBUTION =
  "profit, loss and realised alpha are computed by the attributor inside the cycle and are not exposed by the platform; the book they would be measured against is behind the desk's capability gate. Showing a zero here would be the difference between a flat book and an unmeasured one.";

const MESH_NOT_SERVED =
  "this process serves no mesh. QIP_MESH_CELLS is not set, so no cell has an address to publish state deltas to or to poll capital from, and nothing here drains an inbox or dispatches an envelope. Cells pointed at this deployment are effectively partitioned: they keep trading inside the envelopes they already hold and stop when those expire.";

const NO_CELL_REPORTS =
  "no edge cell has reported to this process. The central plane's view of a cell comes from a CellReport the cell pushes; until one arrives there is no book, no utilisation and no latency to show. This is a silent feed, not a flat one.";

const NO_CALIBRATION =
  "no calibration has been computed. The LEARN stage grades a claim only once its horizon has passed and the platform's own series can settle it informatively; until one has, the platform has written down confidences and has not yet learned whether they held.";

const THREE_BLIND_CYCLES = {
  ...healthy({ cycles: 3, events: 4, archived: 4 }),
  "/system": {
    autonomy: "paper_trading",
    ceiling: "paper_trading",
    live: false,
    halted: false,
    halted_scopes: [],
    cycles: 3,
    events_logged: 4,
    chain_intact: true,
    chain_broken_at: null,
  },
  "/system/metrics": {
    cycles: 3,
    events_logged: 4,
    opportunities_queued: 0,
    proposals: 3,
    orders: 0,
    fills: 0,
    refusals: 0,
    live_fills: false,
  },
  "/opportunities": { opportunities: [] },
  "/proposals": {
    proposals: [
      { id: "prop-1", status: "draft", legs: 0, gross: 0, turnover: 0, rationale: "no thesis cleared the action bar this cycle" },
      { id: "prop-2", status: "draft", legs: 0, gross: 0, turnover: 0, rationale: "no thesis cleared the action bar this cycle" },
      { id: "prop-3", status: "draft", legs: 0, gross: 0, turnover: 0, rationale: "no thesis cleared the action bar this cycle" },
    ],
  },
  "/orders": { orders: [], refusals: 0, reconciliation_breaks: [] },
  "/fills": { fills: [], any_live_fill: false },
  "/portfolio": { proposals: 3, orders: 0, fills: 0, paper_only: true },
  "/pnl": { subject: "attribution", available: false, reason: NO_ATTRIBUTION },
  "/predictions": {
    as_of_cycle: 3,
    window: 1024,
    held: 0,
    open: 0,
    resolved: 0,
    instruments: {},
    calibration: { subject: "calibration", available: false, reason: NO_CALIBRATION },
  },
  "/mesh": { subject: "mesh", available: false, reason: MESH_NOT_SERVED },
  "/regions": { subject: "cells", available: false, reason: NO_CELL_REPORTS },
  "/cycle": CYCLE_3,
} as const;

/** The kernel's order. A page that rendered these alphabetically would pass a set check. */
const STAGES = ["sense", "understand", "discover", "reason", "simulate", "decide", "act", "learn"] as const;

function healthFrame(cursor: number, cycles: number): string {
  const data = JSON.stringify({
    stream: "health",
    type: "health.changed",
    sequence: 1,
    cursor,
    event_time: "2026-09-04T17:20:00.000Z",
    ingest_time: "2026-09-04T17:20:00.001Z",
    correlation_id: `health-${cursor}`,
    payload: {
      status: "ok",
      halted: false,
      halted_scopes: [],
      autonomy: "paper_trading",
      ceiling: "paper_trading",
      live_capable: false,
      reconciliation_breaks: 0,
      cycles,
      events_logged: cycles + 1,
      chain_intact: true,
      cells_reporting: 0,
      cells_stale: 0,
    },
  });
  return `id: ${cursor}\nevent: health.changed\ndata: ${data}\n\n`;
}

const METRICS_AFTER_CYCLE_13 = {
  ...THREE_BLIND_CYCLES["/system/metrics"],
  cycles: 13,
  proposals: 13,
};

/**
 * A platform whose loop advances while the dataflow page is open.
 *
 * The health stream reports cycle 12 on its first connection and cycle 13 on
 * every later one; each body ends, so the hook reconnects and resumes — the
 * platform closes every connection at its lifetime bound, and this is the
 * same path. Later connections replay cursor 2, which the reader
 * de-duplicates, so the count settles at 13 and stays there.
 *
 * Two orderings are pinned, because the first version of this test counted
 * metrics requests from the whole session and was satisfied by the loop
 * page's own poll before the dataflow page existed:
 *
 * * the second stream connection is not served until the dataflow page has
 *   read `/system/metrics` once on mount, so the mount read cannot be the
 *   re-read;
 * * `/system/metrics` answers the three-cycle body until that second
 *   connection has been served and the cycle-13 body from then on, so a
 *   page that re-reads on the change shows 13 proposals and a page that
 *   waits for its 15-second poll does not.
 *
 * Requests are attributed to a page by the URL of the frame that made them,
 * which Playwright tracks across the client-side navigation from the loop
 * page. Not by the Referer header: at interception time the browser has not
 * yet attached it, and a gate on it waited forever.
 */
async function serveAdvancingPlatform(page: Page): Promise<{
  connections: () => number;
  dataflowMetricsReads: () => number;
}> {
  let connections = 0;
  let servedCycle13 = false;
  let dataflowMetricsReads = 0;
  await page.route("**/api/gateway/system/metrics", async (route) => {
    if (new URL(route.request().frame().url()).pathname === "/loop/dataflow") dataflowMetricsReads += 1;
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify(servedCycle13 ? METRICS_AFTER_CYCLE_13 : THREE_BLIND_CYCLES["/system/metrics"]),
    });
  });
  await page.route("**/api/stream/health", async (route) => {
    connections += 1;
    if (connections > 1) {
      while (dataflowMetricsReads < 1) await new Promise((resolve) => setTimeout(resolve, 50));
      servedCycle13 = true;
    }
    await route.fulfill({
      status: 200,
      headers: {
        "content-type": "text/event-stream; charset=utf-8",
        "cache-control": "no-store",
        "x-qip-gateway": "upstream",
      },
      body: connections === 1 ? healthFrame(1, 12) : healthFrame(2, 13),
    });
  });
  return { connections: () => connections, dataflowMetricsReads: () => dataflowMetricsReads };
}

/** A health stream that never answers, so nothing on the page moves by itself. */
async function serveSilentHealthStream(page: Page): Promise<void> {
  await page.route("**/api/stream/health", async (route) => {
    await route.fulfill({
      status: 200,
      headers: { "content-type": "text/event-stream; charset=utf-8", "x-qip-gateway": "upstream" },
      body: ": heartbeat\n\n",
    });
  });
}

test("the eight stages are drawn left to right in the kernel's order, and every refusal is the platform's own words", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, THREE_BLIND_CYCLES);
  await serveSilentHealthStream(page);
  await page.goto("/loop/dataflow");

  // The premise: the page is the dataflow page and the platform's answers
  // landed on it. Without the second half, a page that rendered nothing but
  // its heading would satisfy the order check below with eight empty boxes.
  await expect(page.getByRole("heading", { name: "Dataflow" })).toBeVisible();
  await expect(page.getByTestId("dataflow-no-report")).toBeVisible();
  const content = page.locator("#content");
  await expect(content).toContainText("4 event(s) in memory");

  // The order, not the set. Read from the DOM in document order.
  const names = await page
    .locator('[data-testid^="dataflow-stage-"]:not([data-testid^="dataflow-stage-detail-"])')
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-testid")));
  expect(names).toEqual(STAGES.map((stage) => `dataflow-stage-${stage}`));
  for (const stage of STAGES) {
    await expect(page.getByTestId(`dataflow-stage-${stage}`)).toContainText("no report");
  }

  // Posture on the page, from /system/status, with the label beside it.
  await expect(content).toContainText("PAPER TRADING");

  // Four refusals, verbatim. A paraphrase would pass a looser check and
  // still tell an operator something the platform did not say.
  await expect(content).toContainText(NO_ATTRIBUTION);
  await expect(content).toContainText(MESH_NOT_SERVED);
  await expect(content).toContainText(NO_CELL_REPORTS);
  await expect(content).toContainText(NO_CALIBRATION);

  // The counts on the edges are the routes', not the report's.
  await expect(content).toContainText("3 proposal(s)");
  await expect(content).toContainText("every fill simulated");

  // Nothing on this page writes. The one control that advances the loop is
  // a link to the page that has it.
  await expect(page.getByTestId("dataflow-loop-link")).toHaveAttribute("href", "/loop");
  expect(writes).toEqual([]);
});

test("the stage detail is the sentence POST /cycle answered, carried from the loop page's run", async ({
  page,
}) => {
  await servePlatform(page, THREE_BLIND_CYCLES);
  await serveSilentHealthStream(page);

  // The premise: a cycle was run from the control that exists for it, and
  // the loop page shows the report it got.
  await page.goto("/loop");
  await page.getByTestId("run-cycle").click();
  await expect(page.locator("#content")).toContainText("cycle 3 ·");

  await page.getByTestId("loop-dataflow-link").click();
  await expect(page).toHaveURL(/\/loop\/dataflow$/);
  await expect(page.getByTestId("dataflow-report-cycle")).toHaveText("#3");
  await expect(page.getByTestId("dataflow-no-report")).toHaveCount(0);

  for (const stage of CYCLE_3.stages) {
    await expect(
      page.getByTestId(`dataflow-stage-detail-${stage.stage}`),
      `stage ${stage.stage} does not carry the route's detail`,
    ).toHaveText(stage.detail);
    await expect(page.getByTestId(`dataflow-stage-${stage.stage}`)).toContainText("0 produced");
  }

  // The report and the platform agree on which cycle is latest, so no drift
  // warning — the premise for the drift half of the next test.
  await expect(page.getByTestId("dataflow-drift")).toHaveCount(0);
});

test("the cycle count follows the health stream, and every route is re-read when it moves", async ({
  page,
}) => {
  await servePlatform(page, THREE_BLIND_CYCLES);
  const stream = await serveAdvancingPlatform(page);

  // The premise: a report from cycle 3 is held, so the drift warning has
  // something to compare the stream against.
  await page.goto("/loop");
  await page.getByTestId("run-cycle").click();
  await expect(page.locator("#content")).toContainText("cycle 3 ·");
  await page.getByTestId("loop-dataflow-link").click();

  // First connection: 12. The reconnect resumes and delivers 13.
  await expect(page.getByTestId("dataflow-stream-cycles")).toHaveText("13");
  expect(stream.connections()).toBeGreaterThanOrEqual(2);

  // The re-read fired on the change, and it was a real request from this
  // page, not a re-render: the metrics route was asked by the dataflow page
  // at least twice — once on mount, once after the stream moved — and the
  // page shows the body the second request got. The poll is fifteen seconds,
  // so a page that only polled would still be showing 3 here.
  await expect(page.getByTestId("dataflow-refetched")).not.toHaveText("—");
  await expect.poll(() => stream.dataflowMetricsReads()).toBeGreaterThanOrEqual(2);
  await expect(page.getByTestId("dataflow-stage-decide")).toContainText("13 proposal(s)");

  // The platform is at 13 and the report is cycle 3, and the page says so
  // rather than captioning cycle 3's sentences as cycle 13's.
  const drift = page.getByTestId("dataflow-drift");
  await expect(drift).toBeVisible();
  await expect(drift).toContainText("The platform reports 13 cycle(s)");
  await expect(drift).toContainText("from cycle 3");
});
