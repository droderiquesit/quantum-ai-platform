/**
 * What actually crosses the wire, asserted on the response rather than on the
 * screen.
 *
 * Every other suite here intercepts at the browser boundary with `page.route`,
 * which fulfils the request before the console's gateway ever runs. That is the
 * right tool for what a page draws and it is blind to what a page *receives* —
 * and blind in a way that shipped a false assurance: `/topology` said "an
 * upstream address never reaches a browser" and `/data-sources/health` said "no
 * credential, no variable name, no command, no venue URL, no internal address",
 * while `GET /mesh` handed every served cell's `address` to anything holding
 * the lowest role, `GET /system/status` handed over the same list again inside
 * its `mesh` block, and `GET /registrations` handed over each venue's slot name
 * and the one command that fills it. Both claims were true of the pixels. Both
 * were false of the transport, and a DOM assertion cannot tell those apart.
 *
 * So this file runs against the app instance that has a real upstream behind it
 * (`playwright.config.ts`, the `wire-chromium` project), reads the bodies the
 * gateway actually produced, and asserts:
 *
 * * the declared fields are gone, replaced by a visible marker, with the
 *   removal named in `x-qip-redacted` — on both routes that carry them, since
 *   redacting `/mesh` alone would have moved the leak rather than closed it;
 * * everything else crosses byte for byte, so "the gateway does not rewrite the
 *   platform's answers" stays a checkable statement rather than an aspiration;
 * * the fields the console does *not* remove still arrive, and the page that
 *   reads them says so. That assertion is deliberately the awkward way round:
 *   if a future change removes them at the gateway, this fails and the page's
 *   claim has to be rewritten to match, which is the outcome that was missing
 *   the first time.
 *
 * The registration half of that last point has since inverted, and the file is
 * more useful for it. The platform split `GET /registrations` by authority —
 * the credential slots moved to `GET /registrations/slots` at `Role::Operator`
 * — so the slot name and the `gcloud` line are now absent from the viewer's
 * body, and this suite asserts the absence on the response rather than the
 * screen. That is the same argument in the other direction: a DOM test cannot
 * tell "the console does not render it" from "the console was never sent it",
 * so only a wire test can say which of the two closed the gap. The stub
 * refuses `/registrations/slots` with the platform's own 403, because the
 * console's credential is the viewer token (ADR 0018) and a stub that served
 * it would be testing an entitlement this console does not have.
 *
 * The fixture bodies are in `tests/support/upstream-stub.mjs`.
 */
import { expect, test } from "@playwright/test";
import { REDACTED, WIRE_DISCLOSURES, WIRE_REDACTIONS } from "../src/lib/api/redaction";

/** The stub the app instance under test forwards to. */
const UPSTREAM = `http://127.0.0.1:${Number(process.env.PLAYWRIGHT_UPSTREAM_PORT ?? 3313)}`;

/** The address in the stub's mesh body. Nothing listens there; the string is the point. */
const CELL_ADDRESS = "10.4.7.2:8410";
const CELL_HOST = "10.4.7.2";
const CELL_PORT = "8410";

/** A venue slot the console renders elsewhere and therefore cannot remove here. */
const SECRET_SLOT = "QIP_ALPACA_API_SECRET_KEY";
const SECRET_COMMAND = "gcloud secrets versions add";

test("the cell address the platform serves is gone from the body the browser receives, and the header names what took it", async ({
  request,
}) => {
  // The premise, and it is the whole finding: the platform really does serve
  // it, to a caller with no role beyond viewer. Without this the assertion
  // below would pass against an upstream that never sent an address at all.
  const upstream = await request.get(`${UPSTREAM}/api/v1/mesh`);
  expect(upstream.status()).toBe(200);
  const served = await upstream.text();
  expect(served, "the upstream did not serve an address, so nothing below is a redaction").toContain(
    CELL_ADDRESS,
  );

  const answered = await request.get("/api/gateway/mesh");
  expect(answered.status()).toBe(200);
  const body = await answered.text();

  expect(body, "a cell's mesh address crossed the gateway").not.toContain(CELL_ADDRESS);
  expect(body, "the host half of a cell's mesh address crossed the gateway").not.toContain(CELL_HOST);
  expect(body, "the port half of a cell's mesh address crossed the gateway").not.toContain(CELL_PORT);

  // Replaced rather than deleted: a key that vanished would read as a platform
  // that stopped serving it, and this console exists to keep those apart.
  const parsed = JSON.parse(body) as { cells: { cell: string; address: string }[] };
  expect(parsed.cells).toHaveLength(1);
  expect(parsed.cells[0]!.address).toBe(REDACTED);
  // And the rest of the answer survived, so this is a redaction and not a
  // blanked body.
  expect(parsed.cells[0]!.cell).toBe("eu-west-1");

  // The removal is legible in the same network tab the value used to sit in.
  expect(answered.headers()["x-qip-redacted"]).toBe("cells[].address");
  expect(answered.headers()["x-qip-gateway"]).toBe("upstream");
});

test("the same field on the route that embeds the mesh status whole, because redacting one of the two would only move it", async ({
  request,
}) => {
  const upstream = await request.get(`${UPSTREAM}/api/v1/system/status`);
  const served = await upstream.text();
  expect(
    served,
    "the upstream's system status carried no mesh address, so this proves nothing",
  ).toContain(CELL_ADDRESS);

  const answered = await request.get("/api/gateway/system/status");
  const body = await answered.text();
  expect(body, "a cell's mesh address crossed the gateway on /system/status").not.toContain(
    CELL_ADDRESS,
  );

  const parsed = JSON.parse(body) as {
    cycles: number;
    mesh: { cells: { address: string }[] };
  };
  expect(parsed.mesh.cells[0]!.address).toBe(REDACTED);
  // The fields nobody declared are untouched, on a body that was rebuilt.
  expect(parsed.cycles).toBe(3);
  expect(answered.headers()["x-qip-redacted"]).toBe("mesh.cells[].address");
});

test("a route with nothing declared crosses byte for byte, so the redaction did not become a rewrite", async ({
  request,
}) => {
  // `/system/metrics` has no declared field. The gateway must not so much as
  // reserialise it: a console that re-emitted every answer would change key
  // order and whitespace on routes nobody asked it to touch, and "passed
  // through unmodified" would stop being something anyone could check.
  const upstream = await request.get(`${UPSTREAM}/api/v1/system/metrics`);
  const served = await upstream.text();
  expect(served, "the upstream served an empty body, so equality below is trivial").toContain(
    "cycles",
  );
  // And it is served indented, so equality below can tell "forwarded" from
  // "parsed and written out again" — every other fixture is `JSON.stringify`
  // output, which a round trip reproduces exactly.
  expect(served, "the fixture is canonical JSON, so a rewrite would be invisible").toContain(
    '\n  "cycles"',
  );

  const answered = await request.get("/api/gateway/system/metrics");
  expect(await answered.text()).toBe(served);
  expect(answered.headers()["x-qip-redacted"]).toBeUndefined();

  // And every declared route is one this test's sibling covers, so a third
  // entry added to the table without a wire test is visible here.
  expect(WIRE_REDACTIONS.map((entry) => entry.route).sort()).toEqual(["/mesh", "/system/status"]);
});

test("a browser on /topology holds no cell address on any response it received", async ({ page }) => {
  // The operator's own path: open the page, screenshot it into an incident
  // thread. What matters is not the screen — the other suite covers that — but
  // what the tab holds behind it.
  const bodies: { url: string; body: string }[] = [];
  page.on("response", async (response) => {
    const url = new URL(response.url()).pathname;
    if (!url.startsWith("/api/gateway/")) return;
    try {
      bodies.push({ url, body: await response.text() });
    } catch {
      // A response whose body is no longer available is not evidence either
      // way; the premise below fails if that swallowed everything.
    }
  });

  await page.goto("/topology");
  await expect(page.getByRole("heading", { name: "Topology", exact: true })).toBeVisible();
  await expect(page.getByTestId("topology-node").first()).toBeVisible();

  // The premise: the reads that carry the field actually happened on this page.
  const read = bodies.map((entry) => entry.url);
  expect(read, "the page never read /mesh, so it held nothing to leak").toContain("/api/gateway/mesh");
  expect(read, "the page never read /system/status").toContain("/api/gateway/system/status");

  for (const entry of bodies) {
    expect(entry.body, `a cell's mesh address reached the browser on ${entry.url}`).not.toContain(
      CELL_HOST,
    );
  }
  // And it was there to be removed: the marker proves the mesh read landed
  // with cells in it rather than with an error the page rendered as absence.
  const mesh = bodies.find((entry) => entry.url === "/api/gateway/mesh");
  expect(mesh!.body).toContain(REDACTED);
});

test("the venue slot is gone from the viewer's body because the platform moved it, and the page names what the read still carries rather than the four fields it used to", async ({
  page,
  request,
}) => {
  // This test used to assert the opposite, and it was right to. `GET
  // /registrations` was `Role::Viewer` and carried `secret_slot`,
  // `secret_command` and each companion command, the credential-lifecycle
  // pages rendered them, and stripping them at the gateway would have broken
  // those pages while keeping nothing from a browser that can call the route
  // itself. So the honest position then was that they arrive, and the page had
  // to say so.
  //
  // The platform has since split the route by authority, which is the fix that
  // position asked for: the slots moved to `GET /registrations/slots` at
  // `Role::Operator`. So the assertion inverts, and it has to be made *here*,
  // on the response body, for the same reason the original did — a DOM test
  // cannot tell "the console does not render it" from "the console was never
  // sent it", and that gap is exactly what let the mesh address ship.
  const answered = await request.get("/api/gateway/registrations");
  expect(answered.status()).toBe(200);
  const body = await answered.text();

  // The premise: the viewer's body really is this route's answer and really
  // has rows in it. Without this the two absences below would pass against an
  // empty body, a 404, or a gateway that answered nothing at all.
  const parsed = JSON.parse(body) as {
    sources: { source_id: string; terms: string; secret_slot?: unknown }[];
  };
  expect(parsed.sources, "the viewer's body carried no sources, so nothing below is a narrowing").toHaveLength(1);
  expect(parsed.sources[0]!.source_id).toBe("alpaca-daily-bars");

  expect(body, "a credential slot is still on the viewer's body").not.toContain(SECRET_SLOT);
  expect(body, "the Secret Manager write command is still on the viewer's body").not.toContain(
    SECRET_COMMAND,
  );
  expect(parsed.sources[0]!.secret_slot).toBeUndefined();
  // And the narrowing is the platform's, not this gateway's: a console that
  // had started stripping the field would be making a claim rather than
  // reporting one, and `x-qip-redacted` is how those two are told apart.
  expect(answered.headers()["x-qip-redacted"]).toBeUndefined();

  // The field that does still arrive, and that the health page is accountable
  // for naming.
  expect(parsed.sources[0]!.terms).toBe("https://alpaca.markets/terms-and-conditions");

  // The operator route the slots moved to, refused to this console's
  // credential — which is what a deployment answers it (ADR 0018: the console
  // authenticates as viewer and holds no other platform credential).
  const slots = await request.get("/api/gateway/registrations/slots");
  expect(slots.status(), "the slots route was not refused, so the console holds more than viewer").toBe(403);
  expect(await slots.text()).toContain("operator role");

  await page.goto("/data-sources/health");
  const row = page.locator('[data-testid="feed-health-disclosure-row"][data-route="/registrations"]');
  await expect(row, "the page does not name what its own read carried").toHaveCount(1);
  await expect(row).toHaveAttribute("data-role", "viewer");
  // Derived from the table rather than transcribed, so a field that leaves the
  // wire and the table leaves this assertion with it.
  for (const field of WIRE_DISCLOSURES[0]!.fields) {
    await expect(row, `${field} is not named as something this browser received`).toContainText(
      field,
    );
  }
  // The table is the narrowed one. Asserted by value and not only by
  // iteration: a loop over an empty list passes, and a loop over the old four
  // would pass against a page that still claimed the browser holds slots it
  // has not been sent since the split.
  expect(WIRE_DISCLOSURES[0]!.fields).toEqual(["terms"]);
  await expect(row, "the page still claims the viewer's body carries a slot").not.toContainText(
    "secret_slot",
  );

  // The pixel half still holds: naming a field is not printing a value.
  const content = page.locator("#content");
  await expect(content, "a credential variable name was rendered").not.toContainText(SECRET_SLOT);
  await expect(content, "the command that writes a secret was rendered").not.toContainText(
    SECRET_COMMAND,
  );
});
