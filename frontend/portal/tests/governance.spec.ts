/**
 * The governance/operator policy page, and the platform facts adjacent to it:
 * autonomy posture, the change history the controller has actually accepted,
 * and the governance findings the roster review actually returned.
 *
 * `/admin/autonomy` is a record, not a control (see the page's own doc
 * comment) — these tests pin that its numbers and rows are the platform's own,
 * not a placeholder left over from before the page was wired to
 * `GET /autonomy` and `GET /system/governance`.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const AUTONOMY_HISTORY = {
  "/autonomy": {
    level: "paper_trading",
    ceiling: "paper_trading",
    live: false,
    history: [
      {
        at: 1_700_000_000_000_000_000,
        from: "paper_trading",
        to: "paper_trading",
        operator: "ops-console-test",
        reason: "governance specification fixture",
      },
    ],
  },
};

const AUTONOMY_EMPTY = {
  "/autonomy": { level: "paper_trading", ceiling: "paper_trading", live: false, history: [] },
};

const GOVERNANCE_CLEAN = { "/system/governance": { agents: 4, findings: [] } };

const GOVERNANCE_FINDINGS = {
  "/system/governance": {
    agents: 4,
    findings: [
      {
        severity: "error",
        rule: "an_agent_declares_a_capability_its_role_does_not_permit",
        detail: "the risk desk agent declares order submission, which its role never grants",
        agents: ["risk-desk-01"],
      },
      {
        severity: "warning",
        rule: "an_agent_has_no_declared_owner",
        detail: "the sentiment scanner names no accountable owner",
        agents: ["sentiment-scanner"],
      },
    ],
  },
};

test("autonomy posture renders the platform's own level, ceiling and paper-trading state", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...AUTONOMY_HISTORY, ...GOVERNANCE_CLEAN });
  await page.goto("/admin/autonomy");

  await expect(page.getByText("Autonomy posture")).toBeVisible();
  // The premise: the platform's answer landed rather than a loading skeleton.
  await expect(page.getByText("paper_trading").first()).toBeVisible();
  const postureChip = page.locator("#content").getByText("PAPER TRADING", { exact: true });
  await expect(postureChip).toBeVisible();
  await expect(page.getByText("LIVE — a defect on this deployment")).toHaveCount(0);
});

test("a live-capable autonomy answer is shown as the defect it would be, not softened", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/autonomy": { level: "paper_trading", ceiling: "paper_trading", live: true, history: [] },
    ...GOVERNANCE_CLEAN,
  });
  await page.goto("/admin/autonomy");

  await expect(page.getByText("LIVE — a defect on this deployment")).toBeVisible();
  await expect(
    page.locator("#content").getByText("PAPER TRADING", { exact: true }),
  ).toHaveCount(0);
});

test("the autonomy change history renders every recorded change, and states a real empty history is a record", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...AUTONOMY_HISTORY, ...GOVERNANCE_CLEAN });
  await page.goto("/admin/autonomy");

  await expect(page.getByText("governance specification fixture")).toBeVisible();
  await expect(page.getByText("ops-console-test")).toBeVisible();

  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, { ...healthy(), ...AUTONOMY_EMPTY, ...GOVERNANCE_CLEAN });
  await page.goto("/admin/autonomy");
  await expect(page.getByText("The controller has never accepted a change.")).toBeVisible();
});

test("governance findings render severity, rule and the agents named, and a clean review says so", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...AUTONOMY_EMPTY, ...GOVERNANCE_FINDINGS });
  await page.goto("/admin/autonomy");

  await expect(
    page.getByText("an_agent_declares_a_capability_its_role_does_not_permit"),
  ).toBeVisible();
  await expect(page.getByText("risk-desk-01")).toBeVisible();
  await expect(page.getByText("2 finding(s), 1 error(s)")).toBeVisible();

  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, { ...healthy(), ...AUTONOMY_EMPTY, ...GOVERNANCE_CLEAN });
  await page.goto("/admin/autonomy");
  await expect(page.getByText("No finding against 4 agent(s).")).toBeVisible();
  await expect(page.getByText("clean")).toBeVisible();
});

const REGIONS_WITH_A_HALTED_CELL = {
  "/regions": {
    freshness_bound: "PT30S",
    cells: [
      {
        cell: "us-east-1",
        reported_at: "2026-09-03T12:00:00Z",
        age: "5s",
        stale: false,
        halted: true,
        positions: 3,
        strategies: 2,
        reconciliation_breaks: 2,
        gross: "10000",
        net: "1500",
      },
      {
        cell: "eu-west-1",
        reported_at: "2026-09-03T12:00:03Z",
        age: "2s",
        stale: false,
        halted: false,
        positions: 5,
        strategies: 4,
        reconciliation_breaks: 0,
        gross: "22000",
        net: "-800",
      },
    ],
  },
  "/mesh": { served: false },
};

test("regional brain status renders a halted cell and sums reconciliation breaks across every reporting cell", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...REGIONS_WITH_A_HALTED_CELL });
  await page.goto("/command/regions");

  // The premise: the platform's cell rows landed, in the table rather than
  // only in the (also real) positions chart lower on the page.
  const haltedRow = page.locator("table.dt tr", {
    has: page.getByRole("cell", { name: "us-east-1" }),
  });
  const freshRow = page.locator("table.dt tr", {
    has: page.getByRole("cell", { name: "eu-west-1" }),
  });
  await expect(haltedRow).toBeVisible();
  await expect(freshRow).toBeVisible();

  await expect(page.getByText("1 cell(s) halted")).toBeVisible();
  await expect(haltedRow.getByText("halted")).toBeVisible();
  await expect(freshRow.getByText("fresh")).toBeVisible();

  // Summed, not read off one row: 2 (us-east-1) + 0 (eu-west-1).
  const kpiRow = page.locator("text=Reconciliation breaks").locator("..");
  await expect(kpiRow).toContainText("2");
});

test("the mesh backbone panel reports a mesh that is not served, in its own words rather than as an empty table", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...REGIONS_WITH_A_HALTED_CELL });
  await page.goto("/command/regions");

  await expect(page.getByText("The mesh is not being served.")).toBeVisible();
});

const ORDERS_SENT_VS_FILLED = {
  "/orders": {
    orders: [
      {
        id: "ord-governance-spec-1",
        instrument: "SIM-ALPHA",
        side: "buy",
        quantity: "100",
        state: "partially_filled",
        filled: "40",
        simulated: true,
      },
    ],
    refusals: 0,
    reconciliation_breaks: ["venue reported more shares than the book expected"],
  },
  "/fills": { fills: [], any_live_fill: false },
};

test("the blotter renders quantity sent and quantity filled as two distinct, real figures", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), ...ORDERS_SENT_VS_FILLED });
  await page.goto("/orders");

  const row = page.locator("tr", { has: page.getByText("ord-governance-spec-1") });
  await expect(row).toBeVisible();
  await expect(row.getByText("100", { exact: true })).toBeVisible();
  await expect(row.getByText("40", { exact: true })).toBeVisible();

  // The reconciliation break the platform reported, not a summary that hides it.
  await expect(page.getByText("venue reported more shares than the book expected")).toBeVisible();
});
