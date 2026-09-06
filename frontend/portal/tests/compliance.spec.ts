/**
 * `/compliance`: the compliance facts this platform actually serves, and the
 * two it does not, said in the console's own vocabulary for a missing route.
 *
 * There is no `GET /compliance`. `qip-api/src/routes.rs` declares no such
 * pattern, `NOT_YET_SERVED.compliance` has recorded the absence since that
 * table existed, and this page renders the entry rather than paraphrasing it.
 * What it *does* render comes from five routes the platform serves:
 * `GET /registrations` (what each venue demands before it may be read, and who
 * registered), `GET /registrations/slots` (the deployment variable behind each,
 * at `Role::Operator` — the credential-slot column's own route since the
 * platform split the read by authority), `GET /system/governance`,
 * `GET /system` (the hash chain re-walked on the read) and `GET /autonomy`
 * (posture, ceiling, and every change with the operator who asked).
 *
 * The failures these tests prevent:
 *
 * * **a compliance screen that reports "nothing outstanding" when it failed to
 *   ask.** Of every surface in this console this is the one where an empty
 *   panel is most likely to be read as a clean bill. Four states — nothing has
 *   arrived yet, the catalogue answered and is empty, the platform was
 *   unreachable, and the credential was refused — get four different blocks,
 *   and each test asserts the other three are absent;
 * * **a standing inferred rather than read.** `pending`, `registered` and
 *   `keyless` are three arms of the platform's tagged enum and each renders as
 *   its own. A source whose requirement is `null` is not keyless — nobody
 *   declared what it needs — and the feed's gate refuses it either way, so a
 *   page that collapsed them would disagree with the gate;
 * * **integrity mistaken for attestation.** `chain_intact` proves no sealed
 *   record was edited. It is not a statement anybody signed, over a period,
 *   against an obligation — and the page says so beside it rather than letting
 *   the reader assume;
 * * **a credential value on screen.** The registration surface carries
 *   deployment variable *names*. No value exists in the process that serves
 *   them, and none may appear here;
 * * **a withheld credential slot rendered as an absent one.** The slot column
 *   reads an operator-role route this console's viewer credential is refused
 *   (ADR 0018). An em dash in that column means "the manifest names no
 *   variable"; a refusal shown as one would report a source as needing no key
 *   on the page whose job is being right about that;
 * * **a control.** Nothing on this page registers a venue, clears a finding,
 *   signs anything or submits an order, and it issues no non-GET request.
 *
 * Bodies follow `qip-api/src/registration_views.rs` and `ROUTES-REGISTRATIONS.md`.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const KEYLESS_ROW = {
  source_id: "frankfurter-rates",
  requirement: "keyless",
  standing: { standing: "keyless" },
  terms: "CC-BY-4.0",
} as const;

const REGISTERED_ROW = {
  source_id: "alpaca-daily-bars",
  requirement: "account",
  standing: {
    standing: "registered",
    operator: "operator@env",
    terms_read_at: "2025-10-08T11:00:00Z",
  },
  terms: "https://example.invalid/alpaca-terms",
} as const;

const PENDING_ROW = {
  source_id: "kalshi-markets",
  // The registry declared nothing. Not keyless: an unasked question.
  requirement: null,
  standing: {
    standing: "pending",
    who_must_register: "the platform's owner",
    reason:
      "kalshi-markets is refused: no registration record exists, so the platform's owner must read the venue's terms and record that they did",
  },
  terms: null,
} as const;

/** `GET /registrations`, `Role::Viewer`. No credential slot on this route. */
const REGISTRATIONS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  sources: [KEYLESS_ROW, REGISTERED_ROW, PENDING_ROW],
} as const;

/**
 * `GET /registrations/slots`, `Role::Operator`: the credential-slot column's
 * own route since the platform split the read by authority. A slot names where
 * a credential lives in this deployment's secret store, which is not a fact
 * about a venue, and it was being served to a viewer credential.
 */
const SLOTS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  sources: [
    { ...KEYLESS_ROW, secret_slot: null, secret_command: null, companion_secret_slots: [] },
    {
      ...REGISTERED_ROW,
      standing: { ...REGISTERED_ROW.standing, secret: "QIP_ALPACA_SECRET_KEY" },
      secret_slot: "QIP_ALPACA_SECRET_KEY",
      secret_command: "gcloud secrets versions add qip-alpaca-secret-key --data-file=-",
      companion_secret_slots: [
        {
          variable: "QIP_ALPACA_KEY_ID",
          secret_command: "gcloud secrets versions add qip-alpaca-key-id --data-file=-",
        },
      ],
    },
    {
      ...PENDING_ROW,
      secret_slot: "QIP_KALSHI_API_KEY",
      secret_command: "gcloud secrets versions add qip-kalshi-api-key --data-file=-",
      companion_secret_slots: [],
    },
  ],
} as const;

const AUTONOMY = {
  level: "paper_trading",
  ceiling: "paper_trading",
  live: false,
  history: [
    {
      at: 1728468800000000000,
      from: "observation",
      to: "paper_trading",
      operator: "operator@env",
      reason: "desk opened the session",
    },
  ],
} as const;

const SYSTEM = {
  autonomy: "paper_trading",
  ceiling: "paper_trading",
  live: false,
  halted: false,
  halted_scopes: [],
  cycles: 12,
  events_logged: 412,
  chain_intact: true,
  chain_broken_at: null,
} as const;

const GOVERNANCE = {
  agents: 9,
  findings: [
    {
      severity: "warning",
      rule: "manifest_declares_no_owner",
      detail: "two agents carry no owner in their manifest",
      agents: ["sentiment-analyst", "macro-analyst"],
    },
  ],
} as const;

function served(overrides: Record<string, unknown> = {}) {
  return {
    ...healthy(),
    "/registrations": REGISTRATIONS,
    "/registrations/slots": SLOTS,
    "/autonomy": AUTONOMY,
    "/system/governance": GOVERNANCE,
    "/system": SYSTEM,
    ...overrides,
  };
}

/** A platform that refuses the console's credential on every route. */
async function serveDenied(page: Page, detail: string): Promise<void> {
  await page.route(GATEWAY, async (route) => {
    await route.fulfill({
      status: 403,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: detail }),
    });
  });
}

/**
 * A platform that has been asked and has not yet answered.
 *
 * The one state no body can produce: the request is held open until the
 * returned function is called, so the loading block is on screen for as long
 * as the test needs rather than for a single frame between two others.
 */
async function serveStalled(page: Page): Promise<() => void> {
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(GATEWAY, async (route) => {
    await held;
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ subject: "stalled", available: false, reason: "released" }),
    });
  });
  return release;
}

test("the obligations table renders each standing as its own arm, with the terms cited and no credential value", async ({
  page,
}) => {
  await servePlatform(page, served());
  await page.goto("/compliance");

  // The premise: the page rendered and the table has all three sources.
  await expect(page.getByRole("heading", { name: "Compliance", exact: true })).toBeVisible();
  await expect(page.getByTestId("compliance-obligation-row")).toHaveCount(3);
  await expect(page.getByTestId("compliance-source-count")).toHaveText("3");
  await expect(page.getByTestId("compliance-pending-count")).toHaveText("1");

  const keyless = page.locator('[data-testid="compliance-obligation-row"][data-source="frankfurter-rates"]');
  await expect(keyless).toHaveAttribute("data-standing", "keyless");
  await expect(keyless).toContainText("keyless — no registration required");
  await expect(keyless).toContainText("CC-BY-4.0");

  const registered = page.locator('[data-testid="compliance-obligation-row"][data-source="alpaca-daily-bars"]');
  await expect(registered).toHaveAttribute("data-standing", "registered");
  await expect(registered).toContainText("operator@env");
  await expect(registered).toContainText("https://example.invalid/alpaca-terms");
  // Variable names, both of them. Never a value — none exists to show.
  await expect(registered).toContainText("QIP_ALPACA_SECRET_KEY");
  await expect(registered).toContainText("QIP_ALPACA_KEY_ID");

  // The unasked question is pending, not keyless, and says who must act.
  const pending = page.locator('[data-testid="compliance-obligation-row"][data-source="kalshi-markets"]');
  await expect(pending).toHaveAttribute("data-standing", "pending");
  await expect(pending).toHaveAttribute("data-alert", "true");
  await expect(pending.getByTestId("compliance-requirement")).toHaveText("none declared");
  await expect(pending).toContainText("the platform's owner must register");
  await expect(pending.getByTestId("compliance-terms")).toHaveText("none cited");

  // The body's own posture literal, and the label beside the autonomy panel.
  await expect(page.getByTestId("compliance-body-posture")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("compliance-paper-label")).toHaveText("PAPER TRADING");
});

test("the credential-slot column states the refusal when the operator route is denied, and never as an em dash", async ({
  page,
}) => {
  // The deployed case: `GET /registrations/slots` is `Role::Operator` and the
  // console holds the viewer token (ADR 0018), so the column's own read is
  // refused while the obligations table itself renders fine.
  //
  // The failure this prevents is narrow and bad: this column already uses an
  // em dash for "the manifest names no credential variable", so a refusal that
  // fell through to the same glyph would report a source as needing no key.
  // On the compliance surface that is the one mistake that matters.
  const slotReads: number[] = [];
  await servePlatform(page, served());
  await page.route("**/api/gateway/registrations/slots", async (route) => {
    slotReads.push(403);
    await route.fulfill({
      status: 403,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: "this operation requires the operator role" }),
    });
  });
  await page.goto("/compliance");

  // Premise: the table rendered from the viewer's list, and the second read
  // really happened and was really refused.
  await expect(page.getByTestId("compliance-obligation-row")).toHaveCount(3);
  expect(slotReads, "the page never read /registrations/slots").toEqual([403]);

  const cells = page.getByTestId("compliance-slot");
  await expect(cells).toHaveCount(3);
  for (const cell of await cells.all()) {
    await expect(cell).toHaveAttribute("data-slot-access", "refused");
    await expect(cell).toContainText("withheld");
    await expect(cell).toContainText("operator role");
    await expect(cell).not.toHaveText("—");
  }

  // No slot name reaches the screen from anywhere else, and the page does not
  // fall back to a value it was not served.
  const content = page.locator("#content");
  await expect(content).not.toContainText("QIP_ALPACA_SECRET_KEY");
  await expect(content).not.toContainText("QIP_KALSHI_API_KEY");
  await expect(page.getByTestId("compliance-paper-label")).toHaveText("PAPER TRADING");
});

test("the page reports posture, autonomy and its operator-attributed change record as the platform served them", async ({
  page,
}) => {
  await servePlatform(page, served());
  await page.goto("/compliance");

  // Premise: the autonomy panel landed rather than showing a dash.
  await expect(page.getByTestId("compliance-autonomy-level")).toHaveText("paper_trading");
  await expect(page.getByTestId("compliance-autonomy-ceiling")).toHaveText("paper_trading");
  await expect(page.getByTestId("compliance-autonomy-live")).toHaveText("no");
  await expect(page.getByTestId("compliance-autonomy-changes")).toHaveText("1");

  const row = page.getByTestId("compliance-autonomy-row");
  await expect(row).toHaveCount(1);
  await expect(row).toContainText("operator@env");
  await expect(row).toContainText("desk opened the session");
  await expect(row).toContainText("observation");

  // Governance is the platform's review, rendered as it came.
  await expect(page.getByTestId("compliance-governance-agents")).toHaveText("9");
  await expect(page.getByTestId("compliance-governance-row")).toHaveCount(1);
  await expect(page.getByTestId("compliance-governance-row")).toContainText(
    "manifest_declares_no_owner",
  );
});

test("the chain result is shown as a live integrity check and never as an attestation, and both missing routes are named", async ({
  page,
}) => {
  await servePlatform(page, served());
  await page.goto("/compliance");

  // Premise: the audit panel read /system rather than rendering a dash.
  await expect(page.getByTestId("compliance-events")).toHaveText("412");
  await expect(page.getByTestId("compliance-chain")).toHaveText("yes");
  await expect(page.getByTestId("compliance-audit")).toContainText(
    "re-walked on this read, not a stored attestation",
  );
  await expect(page.getByTestId("compliance-chain-broken")).toHaveCount(0);

  const absences = page.getByTestId("compliance-absences");
  await expect(absences.locator("[data-state-block=endpoint-missing]")).toHaveCount(2);
  await expect(absences).toContainText("GET /api/v1/compliance is missing");
  await expect(absences).toContainText("GET /api/v1/compliance/attestations is missing");
  await expect(absences).toContainText("integrity is not attestation");
  await expect(absences).toContainText(
    "nothing here was signed by a person, covers a stated period, or was produced against a named obligation",
  );
});

test("a broken hash chain is raised as its own alarm and says no statement can be made over the period", async ({
  page,
}) => {
  // Premise first: the same page with an intact chain raises nothing, so the
  // block below is a response to the body and not always on screen.
  await servePlatform(page, served());
  await page.goto("/compliance");
  await expect(page.getByTestId("compliance-chain")).toHaveText("yes");
  await expect(page.getByTestId("compliance-chain-broken")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  await servePlatform(
    page,
    served({ "/system": { ...SYSTEM, chain_intact: false, chain_broken_at: 87 } }),
  );
  await page.goto("/compliance");
  await expect(page.getByTestId("compliance-chain")).toHaveText("NO");
  const broken = page.getByTestId("compliance-chain-broken");
  await expect(broken).toBeVisible();
  await expect(broken).toContainText("breaks at record 87");
  await expect(broken).toContainText("no compliance statement can be made over a period");
});

test("nothing has arrived yet, an empty catalogue, an unreachable platform and a refused credential are four different blocks", async ({
  page,
}) => {
  // 1. Nothing has arrived yet.
  const release = await serveStalled(page);
  await page.goto("/compliance");
  const loading = page.locator('[aria-busy="true"]').first();
  await expect(loading).toBeVisible();
  await expect(loading).toContainText("loading");
  await expect(page.getByTestId("compliance-obligations-empty")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  release();
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // The premise for the three below: the same page with a catalogue in it
  // renders the table, so each absence is an absence of data.
  await servePlatform(page, served());
  await page.goto("/compliance");
  await expect(page.getByTestId("compliance-obligations")).toBeVisible();
  await expect(page.getByTestId("compliance-obligations-empty")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 2. The route answered and the catalogue is empty.
  await servePlatform(page, served({ "/registrations": { ...REGISTRATIONS, sources: [] } }));
  await page.goto("/compliance");
  const empty = page.getByTestId("compliance-obligations-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("The catalogue holds no source.");
  await expect(empty).toContainText("a read that succeeded and found nothing");
  await expect(page.getByTestId("compliance-obligations")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 3. A platform nothing could reach.
  await servePlatformUnreachable(page);
  await page.goto("/compliance");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("compliance-obligations-empty")).toHaveCount(0);
  await expect(page.getByTestId("compliance-obligations")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 4. A credential the routes refuse. `/registrations` requires the viewer
  //    role and the portal grants viewer on self-registration, so a console
  //    credential short of it meets this rather than a clean compliance page.
  await serveDenied(page, "this route requires the viewer role");
  await page.goto("/compliance");
  // Scoped to the obligations panel's own refusal rather than the first on the
  // page: every panel here reads a different route, so all four report the
  // refusal, and `.first()` would assert about whichever panel happens to sit
  // highest — a test that would pass if the obligations panel rendered nothing
  // at all.
  const denied = page.locator("[data-state-block=refused]", {
    hasText: "/api/v1/registrations",
  });
  await expect(denied).toHaveCount(1);
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/registrations");
  await expect(denied).toContainText("this route requires the viewer role");
  await expect(page.getByTestId("compliance-obligations-empty")).toHaveCount(0);
  await expect(page.getByTestId("compliance-obligations")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);

  // The declaration survives every one of them.
  await expect(page.getByTestId("compliance-paper-label")).toHaveText("PAPER TRADING");
});

test("the compliance page has no control at all and issues no write", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, served());
  await page.goto("/compliance");

  // Premise: the page drew, so "no control" is about a rendered page.
  await expect(page.getByTestId("compliance-obligations")).toBeVisible();

  const content = page.locator("#content");
  // Nothing that could compose or carry a decision.
  await expect(content.locator("form, input, select, textarea")).toHaveCount(0);
  await expect(
    content.getByRole("button", {
      name: /^(buy|sell|submit|send|place|order|trade|execute|approve|sign|register|attest)/i,
    }),
  ).toHaveCount(0);

  // Every button on the page is one panel's refresh — a control over a read.
  // Asserted as "all of them are that" rather than "none of them is a write",
  // because a new control would have to be added to this list to pass.
  const buttons = content.locator("button");
  const count = await buttons.count();
  expect(count, "the page has no buttons at all, so this check guards nothing").toBeGreaterThan(0);
  const labels = await buttons.evaluateAll((nodes) =>
    nodes.map((node) => node.getAttribute("aria-label") ?? node.textContent ?? ""),
  );
  expect(labels.filter((label) => !label.startsWith("Refresh "))).toEqual([]);

  expect(writes).toEqual([]);
});

test("the compliance page is reachable from the console's own navigation and names the routes it reads", async ({
  page,
}) => {
  await servePlatform(page, served());
  await page.goto("/risk");

  // The premise: the sidebar rendered and carries the risk section.
  const sidebar = page.getByTestId("sidebar");
  await expect(sidebar.locator('a[href="/risk"]')).toHaveCount(1);

  const link = sidebar.locator('a[href="/compliance"]');
  await expect(link, "the compliance page is not reachable from the navigation").toHaveCount(1);
  await link.click();
  await expect(page.getByRole("heading", { name: "Compliance", exact: true })).toBeVisible();
  const declaration = page.getByTestId("compliance-declaration");
  await expect(declaration).toContainText("GET /registrations");
  await expect(declaration).toContainText("GET /system/governance");
  await expect(declaration).toContainText("GET /autonomy");
  await expect(declaration).toContainText("no control here");
});
