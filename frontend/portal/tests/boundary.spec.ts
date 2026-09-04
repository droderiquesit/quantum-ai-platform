/**
 * The paper-trading boundary, from the browser's side.
 *
 * `qip-api` serves no write route for an order and answers `POST
 * /api/v1/orders` with 405, and that is the right place for the boundary — one
 * the UI holds is one that moves when somebody edits the UI.
 *
 * The console's own obligation is narrower and absolute: it offers no control
 * that submits an order at all. It used to. `/order-entry` composed an
 * instrument, a side, a quantity and a price into a body and posted it, on the
 * stated expectation that "the day the route exists, this page starts working
 * without a change here" — which is the order-submitting control the rule
 * names, one backend commit away from being live. These tests pin its absence,
 * and pin that the halt control it sat beside is still asymmetric.
 */
import { expect, test } from "@playwright/test";
import { RISK_BODY, healthy, servePlatform } from "./support/platform";

const RISK = { "/risk": RISK_BODY };

test("no surface in this console composes or submits an order", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (request.method() !== "GET" && url.includes("/api/gateway/")) {
      writes.push(`${request.method()} ${new URL(url).pathname}`);
    }
  });

  await servePlatform(page, { ...healthy(), ...RISK });

  // The blotter is where an order ticket belongs and where the entry point to
  // one lived, so it is where a reintroduced one would appear first.
  await page.goto("/orders");
  // The premise. Without it the absences below would hold just as well for a
  // page that failed to render at all.
  await expect(page.getByText("Blotter summary")).toBeVisible();
  await expect(page.getByText("read-only")).toBeVisible();
  await expect(page.getByRole("link", { name: /new .*order/i })).toHaveCount(0);

  // Gone from the map, so the palette and the sidebar cannot reach it either.
  await expect(page.getByText("Paper order ticket")).toHaveCount(0);

  // And gone as a route, not merely unlinked: a page still built is a page a
  // typed URL still reaches.
  const direct = await page.goto("/order-entry");
  expect(direct?.status()).toBe(404);

  // Nothing the console did along the way was a write to the order surface.
  expect(writes.filter((write) => write.endsWith("/orders"))).toEqual([]);
});

test("the console can halt the platform and offers no way to clear a halt it did not cause", async ({
  page,
}) => {
  // Asymmetric on purpose: anything that notices trouble may stop the platform,
  // and only an operator with an identity may start it again. A console that
  // could clear its own halt would make the halt advisory.
  await servePlatform(page, { ...healthy(), ...RISK });
  await page.goto("/");

  await page.getByTestId("kill-switch-open").click();
  // The premise: the dialog is open and offering the halt path.
  await expect(page.getByTestId("kill-switch-confirm")).toBeVisible();
  // While the platform is not halted, no clear control exists at all.
  await expect(page.getByTestId("kill-switch-clear")).toHaveCount(0);

  // The confirm stays disabled until a reason is recorded: a halt with no
  // reason in the event log is a halt nobody can explain afterwards.
  await expect(page.getByTestId("kill-switch-confirm")).toBeDisabled();
  await page.getByTestId("kill-switch-reason").fill("boundary specification test");
  await expect(page.getByTestId("kill-switch-confirm")).toBeEnabled();
});

test("a halted platform is shown as halted rather than as quiet", async ({ page }) => {
  await servePlatform(page, {
    ...healthy({ halted: true }),
    "/risk": {
      ...RISK_BODY,
      kill_switch: {
        ...RISK_BODY.kill_switch,
        halted: true,
        reason: "venue reconciliation break",
      },
    },
  });
  await page.goto("/");
  await expect(page.locator("body")).toContainText(/halt/i);
});

test("the gateway refuses a write this console does not declare, before it reaches the platform", async ({
  request,
}) => {
  // The hole this closes was demonstrated, not imagined: a plain button on any
  // page could `fetch` the gateway with an order body and the gateway would
  // forward it upstream with the deployment credential attached. The refusal
  // is here rather than in `client.ts` because a guarantee held in the browser
  // bundle is one page edit wide.
  //
  // The premise: the gateway is up and answering, so the refusals below are
  // refusals and not a dead server.
  const declared = await request.post("/api/gateway/cycle");
  expect(declared.headers()["x-qip-gateway"]).toBe("unreachable");

  for (const [method, path] of [
    ["POST", "/api/gateway/orders"],
    ["POST", "/api/gateway/orders/submit"],
    ["POST", "/api/gateway/trade"],
    ["DELETE", "/api/gateway/orders"],
    ["POST", "/api/gateway/kill-switch/all"],
  ] as const) {
    const response = await request.fetch(path, { method });
    expect(response.status(), `${method} ${path} was not refused`).toBe(405);
    // The disposition, not just the status: a 405 from the platform and a
    // refusal by this gateway are different facts, and only one of them means
    // the credential never left this process.
    expect(response.headers()["x-qip-gateway"], `${method} ${path}`).toBe("refused");
  }
});

test("the gateway still forwards each of the three writes the console declares", async ({
  request,
}) => {
  // The half that distinguishes a working gate from one that refuses
  // everything. `QIP_API_BASE_URL` points at a port nothing listens on, so a
  // forwarded write reports the platform unreachable — which is proof it was
  // forwarded, and a refusal never is.
  for (const [method, path] of [
    ["POST", "/api/gateway/cycle"],
    ["POST", "/api/gateway/kill-switch"],
    ["DELETE", "/api/gateway/kill-switch"],
  ] as const) {
    const response = await request.fetch(path, { method });
    expect(response.headers()["x-qip-gateway"], `${method} ${path} was refused`).toBe(
      "unreachable",
    );
  }
});
