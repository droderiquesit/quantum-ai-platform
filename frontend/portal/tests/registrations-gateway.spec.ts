/**
 * The gateway's declaration of the registration write.
 *
 * `declaresWrite` matches a whole path, and the approval is the first
 * declared write with a parameter in it. The failure this pins is a template
 * that matched a prefix, which would declare every write under
 * `/registrations/` — including ones the platform grows later without this
 * console being asked — or one that matched nothing, which would refuse the
 * page's only control while the page rendered it as if it worked.
 *
 * The base instance points `QIP_API_BASE_URL` at a port nothing listens on,
 * so a forwarded write reports the platform unreachable — which is proof it
 * was forwarded — and a refusal never does.
 */
import { expect, test } from "@playwright/test";

test("the gateway forwards the approval for exactly one source segment", async ({ request }) => {
  const forwarded = await request.post("/api/gateway/registrations/alpaca-daily-bars/approve", {
    data: { terms: "https://example.test/terms", secret: "QIP_EXAMPLE_KEY" },
  });
  expect(forwarded.headers()["x-qip-gateway"]).toBe("unreachable");
});

test("the gateway refuses every other shape under /registrations before reading the credential", async ({
  request,
}) => {
  for (const [method, path] of [
    ["POST", "/api/gateway/registrations"],
    ["POST", "/api/gateway/registrations/approve"],
    ["POST", "/api/gateway/registrations/a/b/approve"],
    ["POST", "/api/gateway/registrations/a/approve/now"],
    ["POST", "/api/gateway/registrations/a/revoke"],
    ["DELETE", "/api/gateway/registrations/a/approve"],
  ] as const) {
    const response = await request.fetch(path, { method });
    expect(response.status(), `${method} ${path} was not refused`).toBe(405);
    expect(response.headers()["x-qip-gateway"], `${method} ${path}`).toBe("refused");
  }
});
