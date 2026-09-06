/**
 * Every write the console can make is one the gateway declares.
 *
 * This exists because one was not, for a whole wave. `EligibilityPanel` has
 * called `POST /ledger/users/{user}/eligibility` since it landed, the platform
 * has served it, and the backend's own boundary suite pins it as the fifth
 * mutating route — but nobody added it to the `REST` table, and the gateway
 * refuses any non-GET that table does not declare. So every eligibility
 * decision an operator made in a real deployment came back 405 from this
 * console's own gateway, and the panel rendered the refusal as if the platform
 * had produced it.
 *
 * Nothing caught it. The seven eligibility specs `page.route`-mock the
 * gateway, which is the one component that would have refused — a test that
 * stubs the thing under test. These tests use Playwright's `request` fixture
 * instead, so the real route handler runs.
 *
 * The discriminator is the same one `registrations-gateway.spec.ts` uses: the
 * base instance points `QIP_API_BASE_URL` at a port nothing listens on, so a
 * write that was *forwarded* reports the platform unreachable — which is proof
 * it got past the allowlist — and a write that was *refused* never reaches the
 * network at all. `unreachable` means declared; `refused` means not.
 *
 * The first test is derived from `REST` rather than from a list written out
 * here, because a list written out here is the same kind of artefact that
 * failed: something a person must remember to extend when they add a sixth
 * write, and which says nothing when they forget.
 */
import { expect, test } from "@playwright/test";
import { REST } from "../src/lib/api/endpoints";

/** A concrete path for a template, substituting one segment per parameter. */
function concrete(path: string): string {
  return path
    .split("/")
    .map((segment) =>
      segment.startsWith("{") && segment.endsWith("}") ? "sample-segment" : segment,
    )
    .join("/");
}

test("the gateway forwards every write the route table declares", async ({ request }) => {
  const writes = Object.entries(REST).filter(([, spec]) => spec.method !== "GET");

  // Premise first. A filter that matched nothing would make every assertion
  // below vacuous, and this test would pass loudest on the day the table was
  // emptied. Five is what the platform's own boundary suite pins.
  expect(writes.length, "the route table declares no writes at all").toBeGreaterThanOrEqual(5);

  for (const [name, spec] of writes) {
    const path = `/api/gateway${concrete(spec.path)}`;
    const response = await request.fetch(path, {
      method: spec.method,
      data: {},
    });
    expect(
      response.headers()["x-qip-gateway"],
      `${name} (${spec.method} ${spec.path}) was refused by the console's own gateway; ` +
        "it is in the route table, so it must be forwarded",
    ).toBe("unreachable");
  }
});

test("the eligibility decision reaches the platform rather than this console's 405", async ({
  request,
}) => {
  // The specific regression, pinned by name as well as by the loop above, so
  // that deleting the table entry fails a test that says what was lost.
  const response = await request.post("/api/gateway/ledger/users/alice/eligibility", {
    data: { decision: "granted", verified_at: "2026-01-01T00:00:00Z", reason: "checked" },
  });
  expect(
    response.headers()["x-qip-gateway"],
    "the eligibility decision was refused by the gateway rather than forwarded",
  ).toBe("unreachable");
  expect(response.status(), "a refused write answers 405 and never reaches the platform").not.toBe(
    405,
  );
});

test("a write the table does not declare is still refused, and the refusal names the real set", async ({
  request,
}) => {
  // The other half. A gateway that forwarded everything would pass both tests
  // above, so this one proves the allowlist still refuses — and that the
  // refusal's own sentence is derived rather than transcribed, which is how it
  // came to name four writes while the console could make five.
  const response = await request.post("/api/gateway/ledger/users/alice/withdraw", { data: {} });
  expect(response.status()).toBe(405);
  expect(response.headers()["x-qip-gateway"]).toBe("refused");

  const body: { error?: string } = await response.json();
  const declared = Object.values(REST).filter((spec) => spec.method !== "GET");
  for (const spec of declared) {
    expect(
      body.error,
      `the refusal does not name ${spec.method} ${spec.path}, so its sentence and the ` +
        "allowlist it describes have drifted apart again",
    ).toContain(`${spec.method} ${spec.path}`);
  }
});
