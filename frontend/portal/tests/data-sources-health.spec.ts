/**
 * `/data-sources/health`: per-source latency, freshness, quality and
 * provenance — the four facts a desk needs before trusting a feed, and the
 * four the platform serves none of.
 *
 * The failures these tests prevent:
 *
 * * a health screen that fills a column in. `GET /api/v1/data-sources/health`
 *   does not exist and `GET /data-sources` matches exactly one expression in
 *   `routes.rs` — `unavailable("sources", NO_DATA_FINDER)` — which serialises
 *   `{subject, available:false, reason}` and no other key. A page showing a
 *   latency here would be showing a number no code path in the platform can
 *   produce;
 * * a nearby fact standing in for a missing one. Edge-cell report age is not
 *   source freshness, and registration standing is not provenance. Both are
 *   real and both are about something else, and the page names them as
 *   different facts rather than putting them in the empty columns;
 * * a credential slot, a secret command or a venue URL being *rendered*.
 *   `GET /registrations` carries all three and this page renders none of them.
 *   A health screen is the one an operator screenshots into an incident
 *   thread, and the gateway already strips `QIP_API_BASE_URL` out of its own
 *   error bodies for exactly this reason;
 * * that finding being reported as more than it is. Everything in this file
 *   stubs at `page.route`, so nothing here exercises the gateway and nothing
 *   here can say what crossed the wire — and for a whole wave the page said the
 *   fields were withheld while the body it fetched carried every one of them.
 *   The wire half lives in `tests/wire.spec.ts`, against a real gateway;
 *   the assertion below is on the split claim, so a page that goes back to
 *   promising the transport fails here;
 * * the four states rendering alike. An empty catalogue, a platform nothing
 *   could reach, a credential a route refused, and a read still in flight are
 *   four different facts with four different remedies, and a console that
 *   showed the same grey panel for all of them would be telling an operator
 *   nothing during the incident it exists for.
 *
 * Bodies follow `backend/crates/apps/qip-api/ROUTES-REGISTRATIONS.md` and the
 * `unavailable` helper in `routes.rs`.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

/**
 * Two catalogued sources, carrying every field this page must not render:
 * a credential variable, the command that writes one, a venue URL and a
 * companion slot. If any of the four appears on the screen the assertions
 * below fail on the value itself, not on a class name.
 *
 * **Deliberately a superset of what `GET /registrations` now serves.** The
 * platform moved `secret_slot`, `secret_command`, the companion commands and
 * the `secret` on a registered standing onto `GET /registrations/slots` at
 * `Role::Operator`, so a real viewer body carries only the first four keys of
 * each row here. They are kept in this fixture on purpose: strip them and the
 * four "did not reach the browser" assertions below would be true of a body
 * that never held them, which is the test that passes forever and guards
 * nothing. Keeping them means this page is proven to render no slot even when
 * handed one — a stronger property than the route's current shape, and one
 * that survives the route changing again.
 *
 * What the wire actually carries is `tests/wire.spec.ts`'s business, against a
 * real gateway and a real upstream; a DOM stub cannot tell "not rendered" from
 * "not received" and must not be read as if it could.
 */
const REGISTRATIONS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  sources: [
    {
      source_id: "alpaca-daily-bars",
      requirement: "account",
      standing: {
        standing: "registered",
        operator: "operator@env",
        terms_read_at: "2025-10-09T08:50:00.000Z",
        secret: "QIP_ALPACA_API_SECRET_KEY",
      },
      terms: "https://alpaca.markets/terms",
      secret_slot: "QIP_ALPACA_API_SECRET_KEY",
      secret_command: "gcloud secrets versions add QIP_ALPACA_API_SECRET_KEY --data-file=-",
      companion_secret_slots: [
        {
          variable: "QIP_ALPACA_API_KEY_ID",
          secret_command: "gcloud secrets versions add QIP_ALPACA_API_KEY_ID --data-file=-",
        },
      ],
    },
    {
      source_id: "kalshi-markets",
      requirement: "account",
      standing: {
        standing: "pending",
        who_must_register: "qip-platform",
        reason: "no registration record exists for it, so it is refused",
      },
      terms: "https://kalshi.com/terms",
      secret_slot: null,
      secret_command: null,
      companion_secret_slots: [],
    },
  ],
} as const;

/** `GET /data-sources`, exactly as `unavailable("sources", NO_DATA_FINDER)` writes it. */
const DATA_SOURCES = {
  subject: "sources",
  available: false,
  reason:
    "no data-source registry is wired into this process. Discovery, approval and rejection are recorded by the data finder service, which this deployment does not run, so there is no source list to show — not an empty one.",
} as const;

const BODIES = {
  ...healthy(),
  "/registrations": REGISTRATIONS,
  "/data-sources": DATA_SOURCES,
} as const;

/** A platform that refuses the console's credential on one route and answers the rest. */
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

/** A platform that has not answered one route yet, so the page is genuinely mid-read. */
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

test("the four facts are each named absent, with the route that would carry them, and none is filled in", async ({
  page,
}) => {
  await servePlatform(page, BODIES);
  await page.goto("/data-sources/health");

  // The premise: the page rendered and its catalogue read landed, so the
  // absences below sit beside data rather than standing in for a dead page.
  await expect(page.getByRole("heading", { name: "Feed health", exact: true })).toBeVisible();
  await expect(page.getByTestId("feed-health-source-row")).toHaveCount(2);

  // The missing route, named through the console's own vocabulary.
  const missing = page.getByTestId("feed-health-missing");
  await expect(missing).toContainText("GET /api/v1/data-sources/health is missing");
  await expect(missing).toContainText("Nothing is shown in its place.");
  // And the check against the process, not against a memory of it.
  await expect(page.getByTestId("feed-health-verified")).toContainText(
    'unavailable("sources", NO_DATA_FINDER)',
  );
  await expect(page.getByTestId("feed-health-verified")).toContainText("no second arm");

  // Four rows, one per fact, each standing at "no field".
  await expect(page.getByTestId("feed-health-fact")).toHaveCount(4);
  for (const fact of ["latency", "freshness", "quality", "provenance"]) {
    const row = page.locator(`[data-testid="feed-health-fact"][data-fact="${fact}"]`);
    await expect(row, `no row for ${fact}`).toHaveCount(1);
    await expect(row).toContainText("GET /api/v1/data-sources/health");
    await expect(page.getByTestId(`feed-health-standing-${fact}`)).toHaveText("no field");
  }

  // The two nearby facts are named as different facts and are not in a column.
  const freshness = page.locator('[data-testid="feed-health-fact"][data-fact="freshness"]');
  await expect(freshness).toContainText("GET /regions");
  await expect(freshness).toContainText("not how old a source's data is");
  const provenance = page.locator('[data-testid="feed-health-fact"][data-fact="provenance"]');
  await expect(provenance).toContainText("licensing record about people, not a lineage record");
});

test("the catalogue gives every source an empty health record and leaks no credential, command or venue URL", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, BODIES);
  await page.goto("/data-sources/health");

  // The premise: both catalogued sources are on the screen by name, so the
  // absences below are absences within rows that exist.
  const rows = page.getByTestId("feed-health-source-row");
  await expect(rows).toHaveCount(2);
  await expect(page.locator('[data-testid="feed-health-source-row"][data-source="alpaca-daily-bars"]')).toHaveCount(1);
  await expect(page.locator('[data-testid="feed-health-source-row"][data-source="kalshi-markets"]')).toHaveCount(1);

  // Four empty cells per row, and "no record" rather than a blank that would
  // read as a value nobody typed.
  await expect(page.getByTestId("feed-health-cell-latency")).toHaveCount(2);
  await expect(page.getByTestId("feed-health-cell-provenance")).toHaveCount(2);
  await expect(page.getByTestId("feed-health-cell-latency").first()).toHaveText("no record");

  // The redaction, asserted on the values themselves. Each of these is in the
  // body the page was served, so a page that rendered the row wholesale would
  // fail here on the string rather than on a heading.
  const content = page.locator("#content");
  await expect(content, "a credential variable name reached the browser").not.toContainText(
    "QIP_ALPACA_API_SECRET_KEY",
  );
  await expect(content, "a companion credential variable reached the browser").not.toContainText(
    "QIP_ALPACA_API_KEY_ID",
  );
  await expect(content, "the command that writes a secret reached the browser").not.toContainText(
    "gcloud secrets versions add",
  );
  await expect(content, "a venue URL reached the browser").not.toContainText("alpaca.markets");
  await expect(content, "a venue URL reached the browser").not.toContainText("kalshi.com");
  // And no anchor anywhere on the page points off-site.
  await expect(content.locator('a[href^="http"]')).toHaveCount(0);

  // The page says so, rather than only doing it — and says it about the page
  // rather than about the transport. The previous headline stopped at "no
  // internal address", which reads as an assurance that none of it reached the
  // browser; the read behind this very screen carries all four to anything
  // holding the lowest role. Both halves are asserted so neither can be
  // dropped back to the comfortable one.
  const redaction = page.getByTestId("feed-health-redaction");
  await expect(redaction).toContainText(
    "No credential, no variable name, no command, no venue URL and no internal address is rendered on this page.",
  );
  await expect(redaction).toContainText(
    "Not rendering a field is not the same as not receiving it",
  );
  const disclosure = page.locator('[data-testid="feed-health-disclosure-row"][data-route="/registrations"]');
  await expect(disclosure, "the page no longer names what its own read carried").toHaveCount(1);
  await expect(disclosure).toHaveAttribute("data-role", "viewer");
  // `terms` and not `secret_slot`. The row used to name four fields, and three
  // of them left the viewer's body when the platform split the route: naming
  // them now would tell an operator their browser holds material it has not
  // been sent since. The remaining one is real — a venue URL is in the body
  // behind this screen and is not on it — so this assertion still guards
  // something rather than restating an empty list.
  await expect(disclosure).toContainText("terms");
  await expect(disclosure, "the row still claims the viewer's body carries a credential slot").not.toContainText(
    "secret_slot",
  );
  await expect(disclosure).toContainText("Platform-side fix:");

  // The registry route's own answer, in the platform's words. `client.ts`
  // classifies `{subject, available:false, reason}` as a stated absence before
  // a page sees it, so this is the platform saying there is no source list —
  // not this console inferring one from an empty body.
  const registry = page.getByTestId("feed-health-registry");
  await expect(registry).toContainText("The platform serves no sources in this deployment.");
  await expect(registry).toContainText("no data-source registry is wired into this process");

  // No control, and nothing this page did was a write.
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  await expect(
    content.getByRole("button", {
      name: /^(submit|buy|sell|order|approve|register|trade|execute)/i,
    }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a read in flight, an empty catalogue, an unreachable platform and a refused credential are four different blocks", async ({
  page,
}) => {
  // Premise: the same page with a catalogue in it renders rows, so each state
  // below is an absence of data and not a page that never worked.
  await servePlatform(page, BODIES);
  await page.goto("/data-sources/health");
  await expect(page.getByTestId("feed-health-source-row")).toHaveCount(2);
  await expect(page.getByTestId("feed-health-none")).toHaveCount(0);

  // 1. Loading: the read is genuinely in flight, and the panel says so with a
  //    busy skeleton rather than an empty table.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await serveSlow(page, "/registrations", { ...BODIES }, 6_000);
  await page.goto("/data-sources/health");
  const catalogue = page.getByTestId("feed-health-catalogue");
  await expect(catalogue.locator('[aria-busy="true"]')).toBeVisible();
  await expect(page.getByTestId("feed-health-none")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 2. An observed empty catalogue: the read succeeded and found nothing.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, { ...BODIES, "/registrations": { ...REGISTRATIONS, sources: [] } });
  await page.goto("/data-sources/health");
  const none = page.getByTestId("feed-health-none");
  await expect(none).toBeVisible();
  await expect(none).toContainText("The platform catalogues no source at all.");
  await expect(none).toContainText("a read that succeeded and found nothing");
  await expect(page.getByTestId("feed-health-source-row")).toHaveCount(0);
  await expect(catalogue.locator('[aria-busy="true"]')).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 3. A platform nothing could reach: its own block, in its own colour.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatformUnreachable(page);
  await page.goto("/data-sources/health");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("feed-health-none")).toHaveCount(0);
  await expect(page.getByTestId("feed-health-source-row")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // 4. A credential the route refuses. `/registrations` requires viewer, and a
  //    session below it meets this rather than an empty catalogue.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await serveDenied(page, "/registrations", { ...BODIES });
  await page.goto("/data-sources/health");
  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/registrations");
  await expect(denied).toContainText("this route requires the viewer role");
  await expect(page.getByTestId("feed-health-none")).toHaveCount(0);
  await expect(page.getByTestId("feed-health-source-row")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);

  // The declaration survives every one of the four.
  await expect(page.getByTestId("feed-health-paper-label")).toHaveText("PAPER TRADING");
});

test("the surface is reachable from the console's own map and declares the posture it renders", async ({
  page,
}) => {
  await servePlatform(page, BODIES);
  await page.goto("/data-sources");

  // The premise: the sidebar rendered and carries the section this page is in.
  const sidebar = page.getByTestId("sidebar");
  await expect(sidebar.locator('a[href="/data-sources"]')).toHaveCount(1);

  const link = sidebar.locator('a[href="/data-sources/health"]');
  await expect(link, "feed health is not reachable from the navigation").toHaveCount(1);
  await link.click();
  await expect(page.getByRole("heading", { name: "Feed health", exact: true })).toBeVisible();

  // The posture, both as this console's own statement and as the body reported
  // it — a page showing a posture without the label is the defect `/risk`
  // shipped once.
  await expect(page.getByTestId("feed-health-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("feed-health-body-posture")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("feed-health-declaration")).toContainText(
    "nothing here can submit an order",
  );
});
