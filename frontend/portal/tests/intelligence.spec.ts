/**
 * The two intelligence pages that stopped illustrating and started reading.
 *
 * `/intelligence/news` is a client of `/stream/market` filtered to the
 * narrative topics, and `/intelligence/regimes` a client of `/stream/signals`
 * filtered to `regime.changed`. Each used to render a seeded illustration
 * under a SIMULATED DATA banner; each now renders what the stream delivers and
 * nothing else. The failures these tests prevent:
 *
 * * a page that shows a headline or a regime the platform never sent —
 *   asserted by the absence of the simulated banner and by a regime page that,
 *   with no `regime.changed`, names no regime;
 * * a filter that lets the wrong topic through, so a market tick reads as a
 *   news item or an opportunity as a regime change;
 * * a filtered-empty feed that reads as a silent stream — asserted by the
 *   premise line that counts what the stream did carry.
 *
 * The streams are stubbed at the browser boundary as complete SSE bodies in
 * the platform's own frame format (`id:` / `event:` / `data:` with the
 * envelope fields `stream`, `type`, `sequence`, `cursor`, `event_time`,
 * `ingest_time`, `correlation_id`, `payload`), so the parser and envelope
 * decoder under test are the ones a deployment runs.
 */
import { expect, test, type Page } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

interface Frame {
  readonly cursor: number;
  readonly type: string;
  readonly payload: Record<string, unknown>;
}

function sseBody(stream: string, frames: readonly Frame[]): string {
  return frames
    .map((frame, index) => {
      const data = JSON.stringify({
        stream,
        type: frame.type,
        sequence: index + 1,
        cursor: frame.cursor,
        event_time: "2026-09-04T15:45:08.370Z",
        ingest_time: "2026-09-04T15:45:08.371Z",
        correlation_id: `corr-${frame.cursor}`,
        payload: frame.payload,
      });
      return `id: ${frame.cursor}\nevent: ${frame.type}\ndata: ${data}\n\n`;
    })
    .join("");
}

async function serveStream(page: Page, channel: string, body: string): Promise<void> {
  await page.route(`**/api/stream/${channel}`, async (route) => {
    await route.fulfill({
      status: 200,
      headers: {
        "content-type": "text/event-stream; charset=utf-8",
        "cache-control": "no-store",
        "x-qip-gateway": "upstream",
      },
      body,
    });
  });
}

/** Distinctive, so no other panel can satisfy an assertion by accident. */
const HEADLINE = "Northwind guides deliveries higher after a debottlenecked launch window";

test("the news page renders a news item the market stream carried, with its own source and sentiment, and not the tick beside it", async ({
  page,
}) => {
  await servePlatform(page, healthy());
  await serveStream(
    page,
    "market",
    sseBody("market", [
      { cursor: 41, type: "market.tick", payload: { object_id: "eq-northwind", price: "101.25" } },
      {
        cursor: 42,
        type: "news.received",
        payload: {
          item_id: "news-42",
          headline: HEADLINE,
          body: "…",
          source: "newswire",
          published_at: "2026-09-04T15:40:00Z",
          entities: [],
          sentiment: { polarity: 0.42, confidence: 0.8, novelty: 0.3 },
        },
      },
    ]),
  );
  await page.goto("/intelligence/news");

  // Premise: the item the stub sent is on the screen, from the payload's own
  // fields — not a headline the page had lying around.
  const row = page.getByTestId("news-row");
  await expect(row).toContainText(HEADLINE);
  await expect(row).toContainText("newswire");
  await expect(row).toContainText("polarity 0.42");

  // The filter held: the tick that arrived on the same stream is not a news
  // row, and the counts say so.
  await expect(page.getByText("market.tick")).toHaveCount(0);

  // The illustration is gone, in both of its labels.
  await expect(page.getByTestId("simulated-banner")).toHaveCount(0);
  await expect(page.getByText("simulated data")).toHaveCount(0);
});

test("a market stream that carried events but no news is reported as a measured absence, not a silent feed", async ({
  page,
}) => {
  await servePlatform(page, healthy());
  await serveStream(
    page,
    "market",
    sseBody("market", [
      { cursor: 7, type: "market.tick", payload: { object_id: "eq-northwind" } },
      { cursor: 8, type: "market.quote", payload: { object_id: "eq-northwind" } },
    ]),
  );
  await page.goto("/intelligence/news");

  const premise = page.getByTestId("news-filter-premise");
  await expect(premise).toBeVisible();
  await expect(premise).toContainText("2 event(s)");
  await expect(page.getByTestId("news-row")).toHaveCount(0);
});

test("the regimes page names the regime of the newest regime.changed and no other, and names none when there was none", async ({
  page,
}) => {
  await servePlatform(page, healthy());
  await serveStream(
    page,
    "signals",
    sseBody("signals", [
      { cursor: 90, type: "regime.changed", payload: { from: "range_bound", to: "trending" } },
      { cursor: 91, type: "opportunity.detected", payload: { headline: "mean reversion on eq-northwind" } },
      { cursor: 92, type: "regime.changed", payload: { from: "trending", to: "stressed_selloff" } },
    ]),
  );
  await page.goto("/intelligence/regimes");

  const current = page.getByTestId("current-regime");
  await expect(current).toBeVisible();
  // The newest, not the first: a page that showed "trending" would be
  // reporting a regime the platform has since left.
  await expect(current).toContainText("stressed_selloff");
  await expect(current).not.toContainText("mean reversion");
  // Both changes are in the feed; the opportunity is not.
  await expect(page.getByText("opportunity.detected")).toHaveCount(0);
  await expect(page.getByTestId("regime-filter-premise")).toHaveCount(0);
  await expect(page.getByTestId("simulated-banner")).toHaveCount(0);
});

test("a signal stream with no regime change leaves the current regime unnamed rather than chosen", async ({
  page,
}) => {
  await servePlatform(page, healthy());
  await serveStream(
    page,
    "signals",
    sseBody("signals", [
      { cursor: 5, type: "opportunity.detected", payload: { headline: "something" } },
    ]),
  );
  await page.goto("/intelligence/regimes");

  await expect(page.getByTestId("current-regime")).toHaveCount(0);
  await expect(page.locator("[data-state-block='no-regime-change-recorded']")).toBeVisible();
  // The premise beside the conclusion: the feed was live and carried one
  // other event, so "no regime change" is a fact about the platform, not
  // about the socket.
  await expect(page.getByTestId("regime-filter-premise")).toContainText("1 signal event(s)");
  // None of the old illustration's regime names may appear anywhere.
  await expect(page.getByText(/range-bound|stressed \/ correlated selloff/)).toHaveCount(0);
});
