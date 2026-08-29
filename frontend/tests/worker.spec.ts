/**
 * What the offline layer keeps, and the one thing it must never keep.
 *
 * A worker that cached a platform answer would let this console draw a
 * position, a limit or a halt state from hours ago and render it identically to
 * one fetched a second ago. That is the precise failure the rest of the console
 * is built to make impossible — every panel states the age of what it shows —
 * and a cache underneath it would defeat all of it at once.
 *
 * This suite runs against its own app instance in front of a real upstream
 * (see `playwright.config.ts`). Nothing here is intercepted: Playwright fulfils
 * a routed request before the service worker receives a `fetch` event, so a
 * worker test written against the usual stubs passes no matter what the worker
 * does. Here the request really crosses a socket and the worker really decides.
 */
import { expect, test } from "@playwright/test";

/** Give the worker time to install, claim, and serve a second load. */
async function withActiveWorker(page: import("@playwright/test").Page) {
  await page.goto("/");
  const active = await page.evaluate(async () => {
    if (!("serviceWorker" in navigator)) return false;
    const registration = await navigator.serviceWorker.ready;
    return registration.active !== null;
  });
  expect(active, "no service worker activated, so nothing below proves anything").toBe(true);
  // The first load registers; the second is the one the worker controls.
  await page.reload();
  await page.waitForTimeout(2500);
  const controlled = await page.evaluate(() => navigator.serviceWorker.controller !== null);
  expect(controlled, "the worker activated but never took control of the page").toBe(true);
}

/** Every path held in any cache this origin owns. */
async function cachedPaths(page: import("@playwright/test").Page): Promise<string[]> {
  return page.evaluate(async () => {
    const held: string[] = [];
    for (const name of await caches.keys()) {
      const cache = await caches.open(name);
      for (const request of await cache.keys()) held.push(new URL(request.url).pathname);
    }
    return held;
  });
}

test("the worker caches the shell, so the assertions below are about a worker that is working", async ({
  page,
}) => {
  // The premise for the prohibition. "Nothing under /api/ was cached" is
  // trivially true of a worker that cached nothing at all, so what it does
  // cache has to be established first and established positively.
  await withActiveWorker(page);
  const held = await cachedPaths(page);
  expect(held, "the worker cached no offline page").toContain("/offline");
  expect(
    held.filter((path) => path.startsWith("/_next/static/")).length,
    "the worker cached no build asset, so it is not serving anything offline",
  ).toBeGreaterThan(0);
});

test("the worker never stores a platform answer", async ({ page }) => {
  await withActiveWorker(page);

  // The premise: the console really did read the platform on this page, over
  // the network, through a worker that really saw the request. Without this a
  // clean result could mean the reads never happened.
  const readThePlatform = await page.evaluate(async () => {
    const response = await fetch("/api/gateway/system/metrics", { cache: "no-store" });
    return response.ok;
  });
  expect(readThePlatform, "the console could not read the platform at all").toBe(true);

  const held = await cachedPaths(page);
  const platform = held.filter((path) => path === "/api" || path.startsWith("/api/"));
  expect(
    platform,
    `the worker stored a platform answer, which would be served offline as though current: ${platform.join(", ")}`,
  ).toEqual([]);
});

/*
 * Not tested here: the navigate handler's fallback to the offline page.
 *
 * Playwright's offline emulation does not reach a service worker's own
 * `fetch`, so the worker succeeds against the real server and the assertion
 * passes without the fallback ever running. That is a test that would report a
 * working fallback whatever the code did, so it is absent rather than green.
 * What the offline page itself renders is covered in `mobile.spec.ts`.
 */
