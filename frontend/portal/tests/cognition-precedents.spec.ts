/**
 * `/cognition/precedents`: the precedent REASON recorded beside each
 * hypothesis, read from `GET /cognition/precedents` and rendered field for
 * field.
 *
 * The failures these tests prevent:
 *
 * * a field dropped because this console did not anticipate it — asserted by
 *   a key the page has no knowledge of appearing with its value verbatim;
 * * a nested structure flattened to a JSON string — asserted by `nearest[]`
 *   drawn as a table and `digest` as a nested list, with `null` agreement
 *   rendered as absent beside a zero count, because "a share of nothing is
 *   not zero agreement, it is no evidence";
 * * columns taken from the first row alone — asserted by `realised_move_bps`
 *   and `agreed` present as columns when only the resolved episode carries
 *   them, and the unresolved row showing them as absent;
 * * the leading field buried — asserted on the DOM order: `similarity` is the
 *   first column of the table, wherever the platform put it in the object;
 * * an empty recall rendered as nothing — asserted by the empty state saying
 *   so and no precedent card drawn beside it;
 * * a page in the cognition section without the paper-trading declaration, or
 *   with a control that could recall, store or submit anything.
 *
 * The first record is the example in
 * `backend/crates/apps/qip-api/ROUTES-COGNITION.md` verbatim — what a
 * deployment answers after one cycle with an empty memory. The second is
 * built to the same contract's stated shape for `nearest[]` with one resolved
 * and one unresolved episode, and carries one field the contract does not
 * name; no deployment has recalled a precedent, so it cannot be captured.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const CONTRACT_EXAMPLE = {
  hypothesis_id: "hyp-3f2a",
  cycle: 2,
  confidence: 0.64,
  examined: 0,
  memory_size: 1,
  nearest: [],
  digest: { nearest: 0, resolved: 0, agreeing: 0, agreement: null },
} as const;

const WITH_NEAREST = {
  hypothesis_id: "hyp-7c10",
  cycle: 3,
  confidence: 0.71,
  examined: 4,
  memory_size: 5,
  nearest: [
    {
      episode_id: "ep-0001",
      instrument: "FICT-AAA",
      at: "2025-10-09T08:00:00Z",
      known_at: "2025-10-09T08:05:00Z",
      similarity: 0.9312,
      claim: "up",
      decision: "acted",
      realised_move_bps: 42,
      agreed: true,
    },
    {
      episode_id: "ep-0002",
      instrument: "FICT-BBB",
      at: "2025-10-09T08:10:00Z",
      known_at: "2025-10-09T08:15:00Z",
      similarity: 0.8104,
      claim: "down",
      decision: "declined",
    },
  ],
  digest: { nearest: 2, resolved: 1, agreeing: 1, agreement: 1 },
  regime: "range-bound",
} as const;

const PRECEDENTS = { precedents: [CONTRACT_EXAMPLE, WITH_NEAREST] } as const;

test("the precedents page renders every field as it came, nested structures as structures, and holds no control that acts", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/cognition/precedents": PRECEDENTS });
  await page.goto("/cognition/precedents");

  // The premise: the page rendered and the route's answer landed on it.
  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Precedents" })).toBeVisible();
  await expect(page.getByTestId("precedent-count")).toHaveText("2");
  await expect(page.getByTestId("precedent")).toHaveCount(2);

  await expect(page.getByTestId("cognition-paper-label")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can act.");

  // The contract's own example: every top-level key in the order it came,
  // the empty list said in words, the digest nested with null agreement
  // rendered as absent beside its zero counts.
  const first = page.getByTestId("precedent").nth(0);
  const firstFields = await first
    .getByTestId("precedent-field")
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-field")));
  expect(firstFields).toEqual(["hypothesis_id", "cycle", "confidence", "examined", "memory_size", "nearest", "digest"]);
  await expect(first.locator('[data-testid="precedent-field"][data-field="hypothesis_id"]')).toContainText("hyp-3f2a");
  await expect(first.locator('[data-testid="precedent-field"][data-field="nearest"]')).toHaveAttribute("data-shape", "empty-list");
  await expect(first.locator('[data-testid="precedent-field"][data-field="nearest"]')).toContainText("an empty list");
  await expect(first.locator('[data-testid="precedent-field"][data-field="digest"]')).toHaveAttribute("data-shape", "record");
  const digest = first.locator('[data-testid="precedent-field"][data-field="digest"]');
  await expect(digest.locator('[data-testid="precedent-field-nested"][data-field="agreeing"]')).toContainText("0");
  await expect(digest.locator('[data-testid="precedent-field-nested"][data-field="agreement"]')).toContainText("—");
  await expect(digest.locator('[data-testid="precedent-field-nested"][data-field="agreement"]')).not.toContainText("0");
  await expect(first).not.toContainText("{");

  // The record with recalled episodes: `nearest[]` is a table, `similarity`
  // its first column, the union of keys its columns, and the unresolved row
  // absent where it carries nothing. The field the page never heard of is
  // there with its value.
  const second = page.getByTestId("precedent").nth(1);
  await expect(second.locator('[data-testid="precedent-field"][data-field="nearest"]')).toHaveAttribute("data-shape", "table");
  const table = second.getByTestId("precedent-table");
  await expect(table).toHaveCount(1);
  const columns = await table.locator("thead th").evaluateAll((cells) => cells.map((cell) => cell.textContent?.trim()));
  expect(columns).toEqual([
    "similarity",
    "episode_id",
    "instrument",
    "at",
    "known_at",
    "claim",
    "decision",
    "realised_move_bps",
    "agreed",
  ]);
  const rows = table.getByTestId("precedent-table-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).locator('[data-column="similarity"]')).toHaveText("0.9312");
  await expect(rows.nth(0).locator('[data-column="realised_move_bps"]')).toHaveText("42");
  await expect(rows.nth(0).locator('[data-column="agreed"]')).toHaveText("true");
  await expect(rows.nth(1).locator('[data-column="similarity"]')).toHaveText("0.8104");
  await expect(rows.nth(1).locator('[data-column="realised_move_bps"]')).toHaveText("—");
  await expect(rows.nth(1).locator('[data-column="agreed"]')).toHaveText("—");
  await expect(second.locator('[data-testid="precedent-field"][data-field="regime"]')).toContainText("range-bound");
  await expect(second.locator('[data-testid="precedent-field"][data-field="digest"] [data-field="agreement"]')).toContainText("1");
  await expect(page.getByTestId("precedents-empty")).toHaveCount(0);

  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  const formsOutsideDialog = await page
    .locator("form")
    .evaluateAll((forms) => forms.filter((form) => form.closest("dialog") === null).length);
  expect(formsOutsideDialog).toBe(0);
  await expect(
    content.getByRole("button", { name: /^(recall|store|forget|evict|submit|buy|sell|trade)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("an empty recall says the memory recalled nothing rather than rendering nothing", async ({ page }) => {
  await servePlatform(page, { ...healthy(), "/cognition/precedents": { precedents: [] } });
  await page.goto("/cognition/precedents");

  await expect(page.getByTestId("precedent-count")).toHaveText("0");
  await expect(page.getByTestId("precedents-empty")).toBeVisible();
  await expect(page.getByTestId("precedents-empty")).toContainText("recalled no precedent");
  await expect(page.getByTestId("precedent")).toHaveCount(0);
  await expect(page.getByTestId("cognition-paper-label")).toHaveText("PAPER TRADING");
});
