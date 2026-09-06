/**
 * `/topology`: the service dependency graph, assembled in the browser from
 * four routes that are served, because the platform serves no fifth one that
 * holds the graph.
 *
 * The failures these tests prevent:
 *
 * * a console asserting a fact the platform does not hold. `GET
 *   /api/v1/topology` is not in `routes.rs`; the picture on this page is a
 *   join this console performed at read time, it cannot be replayed from the
 *   event log, and the page has to say so rather than presenting a graph as
 *   though something computed it. Asserted on the missing-endpoint block and
 *   on the assembly statement, and on every node carrying the route that
 *   evidenced it;
 * * an internal address reaching the browser. `GET /mesh` carries
 *   `cells[].address` — a cell's base URL on the mesh transport, configured
 *   through `QIP_MESH_PEER`. The stub below puts a real-looking one in the
 *   body, and the assertion is on that string appearing nowhere in the page,
 *   including in title attributes. The gateway strips `QIP_API_BASE_URL` out
 *   of its own error bodies for the same reason, and a topology screen is the
 *   most likely place to undo it;
 * * the other binaries drawn as healthy, or drawn at all. This console talks
 *   to one process and no route lists the rest, so the fast brain, the deep
 *   brain and the execution node are absent and stated absent — a greyed-out
 *   node would claim a measurement nobody took;
 * * the four states rendering alike. A read in flight, a platform that
 *   answered and described one node, a platform nothing could reach, and a
 *   credential a route refused are four facts with four remedies;
 * * a control. A topology screen invites restart, drain and isolate, and no
 *   route exists for any of them.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

/** `GET /system`, field for field as `system()` in `routes.rs` writes it. */
const SYSTEM = {
  autonomy: "paper_trading",
  ceiling: "paper_trading",
  live: false,
  halted: false,
  halted_scopes: [],
  cycles: 12,
  events_logged: 12,
  chain_intact: true,
  chain_broken_at: null,
} as const;

/**
 * The address in here is the point of it. It is the one field on this surface
 * that must not reach a browser, so the fixture carries a plausible one and
 * the test asserts on the string.
 */
const CELL_ADDRESS = "http://10.4.7.2:8410";

const MESH = {
  served: true,
  cells_served: 1,
  deltas_absorbed: 12,
  envelopes_dispatched: 3,
  inbox_depth: 0,
  cells: [{ cell: "eu-west-1", address: CELL_ADDRESS, spool_pending: 0, circuit: "closed" }],
  standings: [
    {
      cell: "eu-west-1",
      region: "eu-west",
      sequence: 4,
      halted: false,
      strategies: 2,
      reconciliation_breaks: 0,
      reconciliation_breaks_omitted: 0,
    },
  ],
  last_undecodable: null,
} as const;

const REGIONS = {
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
    },
  ],
} as const;

const AGENTS = {
  agents: [
    {
      id: "risk-sentinel",
      name: "Risk Sentinel",
      role: "guardian",
      owner: "risk-desk",
      purpose: "watch exposure against the configured limit set",
      capabilities: ["risk_read"],
    },
  ],
} as const;

const BODIES = {
  ...healthy(),
  "/system": SYSTEM,
  "/mesh": MESH,
  "/regions": REGIONS,
  "/agents": AGENTS,
} as const;

/** Every input answered, and each answered with nothing in it. */
const EMPTY_BODIES = {
  ...healthy(),
  "/system": SYSTEM,
  "/mesh": { served: false },
  "/regions": { freshness_bound: "45s", cells: [] },
  "/agents": { agents: [] },
} as const;

async function serveDenied(page: Page, path: string, bodies: Record<string, unknown>) {
  await page.route(GATEWAY, async (route) => {
    const url = new URL(route.request().url()).pathname.replace("/api/gateway", "");
    if (url === path) {
      await route.fulfill({
        status: 403,
        headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
        body: JSON.stringify({ error: "this route requires the viewer role" }),
      });
      return;
    }
    const key = Object.keys(bodies).find((k) => url === k || url.endsWith(k));
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify(
        key === undefined
          ? { subject: url.replace(/^\//, ""), available: false, reason: `no stub for ${url}` }
          : bodies[key],
      ),
    });
  });
}

async function serveSlow(page: Page, path: string, bodies: Record<string, unknown>, delayMs: number) {
  await page.route(GATEWAY, async (route) => {
    const url = new URL(route.request().url()).pathname.replace("/api/gateway", "");
    if (url === path) await new Promise((resolve) => setTimeout(resolve, delayMs));
    const key = Object.keys(bodies).find((k) => url === k || url.endsWith(k));
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify(
        key === undefined
          ? { subject: url.replace(/^\//, ""), available: false, reason: `no stub for ${url}` }
          : bodies[key],
      ),
    });
  });
}

test("the page says the platform serves no topology document and that this graph is a join it performed", async ({
  page,
}) => {
  await servePlatform(page, BODIES);
  await page.goto("/topology");

  // The premise: the graph rendered, so the disclaimers below sit beside a
  // picture rather than replacing one.
  await expect(page.getByRole("heading", { name: "Topology", exact: true })).toBeVisible();
  await expect(page.getByTestId("topology-node").first()).toBeVisible();

  const missing = page.getByTestId("topology-missing");
  await expect(missing).toContainText("GET /api/v1/topology is missing");
  await expect(missing).toContainText("the platform serves no single topology document");

  const assembly = page.getByTestId("topology-assembly");
  await expect(assembly).toContainText("assembled in this browser from four routes");
  await expect(assembly).toContainText("not a document the platform holds");
  await expect(assembly).toContainText("nothing can replay this picture");
  for (const route of ["GET /system", "GET /mesh", "GET /regions", "GET /agents"]) {
    await expect(assembly, `${route} is not named as an input`).toContainText(route);
  }

  // The one inference a graph invites, refused in words on the page.
  await expect(page.getByTestId("topology-graph")).toContainText("It is not a network path");
});

test("every node and edge names the route that evidenced it, and no address, host or port is rendered", async ({
  page,
}) => {
  await servePlatform(page, BODIES);
  await page.goto("/topology");

  // The premise: four kinds of node are on the screen, from four reads.
  await expect(page.locator('[data-testid="topology-node"][data-kind="cell"]')).toHaveCount(1);
  await expect(page.locator('[data-testid="topology-node"][data-kind="mesh"]')).toHaveCount(1);
  await expect(page.locator('[data-testid="topology-node"][data-kind="centre"]')).toHaveCount(1);
  await expect(page.locator('[data-testid="topology-node"][data-kind="agent"]')).toHaveCount(1);

  // Every node carries its evidence, and it is one of the four reads.
  const nodes = page.getByTestId("topology-node");
  const count = await nodes.count();
  expect(count, "no node to check, so the loop below would prove nothing").toBe(4);
  for (let index = 0; index < count; index += 1) {
    const evidence = await nodes.nth(index).getAttribute("data-evidence");
    expect(["GET /system", "GET /mesh", "GET /regions", "GET /agents"]).toContain(evidence);
    await expect(nodes.nth(index)).toContainText(evidence ?? "");
  }

  // The edges, and the evidence beside each.
  const edges = page.getByTestId("topology-edge");
  await expect(edges).toHaveCount(2);
  await expect(edges.first()).toContainText("evidenced by GET /regions");
  await expect(page.locator('[data-testid="topology-edge"][data-from="mesh"][data-to="centre"]')).toContainText(
    "evidenced by GET /mesh",
  );

  // The centre carries the platform's own fields, so the graph is a report.
  const centre = page.locator('[data-testid="topology-node"][data-kind="centre"]');
  await expect(centre).toContainText("autonomy paper_trading");
  await expect(centre).toContainText("not live-capable");
  await expect(centre).toContainText("event chain intact");

  // THE REDACTION. The address is in the body this page was served and it is
  // nowhere in the page — not in text, not in a title, not in any attribute.
  const html = await page.locator("#content").innerHTML();
  expect(html, "a cell's mesh address reached the browser").not.toContain(CELL_ADDRESS);
  expect(html, "the host part of a cell's mesh address reached the browser").not.toContain("10.4.7.2");
  expect(html, "the port part of a cell's mesh address reached the browser").not.toContain("8410");

  // And the page says it withholds it, rather than only withholding it.
  await expect(page.getByTestId("topology-withheld")).toContainText("cells[].address");
  await expect(page.getByTestId("topology-withheld")).toContainText("QIP_MESH_PEER");

  // The processes this console cannot see are named absent, not drawn down.
  await expect(page.getByTestId("topology-out-of-view")).toContainText("fast brain");
  await expect(page.getByTestId("topology-out-of-view")).toContainText("They are drawn nowhere.");
  await expect(page.locator('[data-testid="topology-node"][data-node="qip-fastbrain"]')).toHaveCount(0);

  // No control, on the page that most invites one.
  const content = page.locator("#content");
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(restart|drain|isolate|failover|halt|submit|order)/i }),
  ).toHaveCount(0);
});

test("the four reads report themselves, so a hole in the graph is legible as a hole", async ({
  page,
}) => {
  // `/agents` is left to the catch-all, which answers a stated absence exactly
  // as `qip-api` does for a subsystem that is not composed in. The graph must
  // still draw the other three and must not read as "there are no agents".
  await servePlatform(page, { ...healthy(), "/system": SYSTEM, "/mesh": MESH, "/regions": REGIONS });
  await page.goto("/topology");

  // The premise: the graph rendered from the three reads that answered.
  await expect(page.locator('[data-testid="topology-node"][data-kind="cell"]')).toHaveCount(1);

  const rows = page.getByTestId("topology-evidence-row");
  await expect(rows).toHaveCount(4);
  await expect(page.locator('[data-testid="topology-evidence-row"][data-route="/system"]')).toHaveAttribute(
    "data-outcome",
    "ok",
  );
  const agentsRow = page.locator('[data-testid="topology-evidence-row"][data-route="/agents"]');
  await expect(agentsRow).toHaveAttribute("data-outcome", "unavailable");
  await expect(agentsRow).toContainText("not available");

  // And the tier says unknown rather than empty.
  const agentTier = page.locator('[data-testid="topology-tier"][data-evidence="GET /agents"]');
  await expect(agentTier).toContainText("this tier is unknown rather than empty");
  await expect(agentTier).not.toContainText("listed no agent");
});

test("a read in flight, a platform describing one node, an unreachable platform and a refused credential are four different blocks", async ({
  page,
}) => {
  // Premise: the same page with a full platform behind it draws four nodes, so
  // each state below is an absence and not a page that never worked.
  await servePlatform(page, BODIES);
  await page.goto("/topology");
  await expect(page.getByTestId("topology-node")).toHaveCount(4);
  await expect(page.getByTestId("topology-empty")).toHaveCount(0);

  // 1. Loading: the spine read is genuinely in flight.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await serveSlow(page, "/system", { ...BODIES }, 6_000);
  await page.goto("/topology");
  const graph = page.getByTestId("topology-graph");
  await expect(graph.locator('[aria-busy="true"]')).toBeVisible();
  await expect(page.getByTestId("topology-empty")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 2. Observed empty: all four reads landed and described one node.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, EMPTY_BODIES);
  await page.goto("/topology");
  const empty = page.getByTestId("topology-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("The platform described no topology beyond the process that answered.");
  await expect(empty).toContainText("a platform this console reached");
  await expect(page.getByTestId("topology-node")).toHaveCount(0);
  await expect(graph.locator('[aria-busy="true"]')).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 3. A platform nothing could reach.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatformUnreachable(page);
  await page.goto("/topology");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("topology-empty")).toHaveCount(0);
  await expect(page.getByTestId("topology-node")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 4. A credential the spine route refuses.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await serveDenied(page, "/system", { ...BODIES });
  await page.goto("/topology");
  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/system");
  await expect(denied).toContainText("this route requires the viewer role");
  await expect(page.getByTestId("topology-empty")).toHaveCount(0);
  await expect(page.getByTestId("topology-node")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);

  // The declaration survives every one of the four.
  await expect(page.getByTestId("topology-paper-label")).toHaveText("PAPER TRADING");
});

test("the surface is reachable from the console's own map and labels the posture it renders", async ({
  page,
}) => {
  await servePlatform(page, BODIES);
  await page.goto("/operations/mesh");

  const sidebar = page.getByTestId("sidebar");
  // The premise: the sidebar rendered and carries the section this page is in.
  await expect(sidebar.locator('a[href="/operations/mesh"]')).toHaveCount(1);

  const link = sidebar.locator('a[href="/topology"]');
  await expect(link, "the topology surface is not reachable from the navigation").toHaveCount(1);
  await link.click();
  await expect(page.getByRole("heading", { name: "Topology", exact: true })).toBeVisible();

  // The page renders a posture — the centre node carries the autonomy level —
  // so it carries the label. A posture without one is the defect `/risk`
  // shipped once, and this page shows autonomy in two places.
  const content = page.locator("#content");
  await expect(content).toContainText("paper_trading");
  await expect(page.getByTestId("topology-autonomy")).toHaveText("autonomy paper_trading");
  await expect(page.getByTestId("topology-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("topology-declaration")).toContainText(
    "nothing on this page can submit an order",
  );
});
