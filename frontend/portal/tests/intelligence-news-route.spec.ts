/**
 * `/intelligence/news` as a reader of `GET /news`.
 *
 * The failure these tests prevent has already happened: the page told its
 * reader "there is no `GET /api/v1/news`" while `routes.rs` served exactly
 * that route, answering `unavailable("news", NO_NARRATIVE_ADAPTER)`. A false
 * statement on the screen in the platform's disfavour is still a false
 * statement, and the paraphrase beside it was this console's memory of the
 * reason rather than the reason. So:
 *
 * * **The platform's own sentence is on the screen.** Asserted as the exact
 *   reason the stub returned, not a substring the page's own prose might
 *   also contain.
 * * **A body the route serves instead of an absence is rendered verbatim and
 *   labelled unread**, not through columns this console guessed — the route
 *   has no available shape today, and a page that invented one would be
 *   inventing news.
 *
 * The absence body is built to the contract `routes.rs` serialises through
 * `unavailable(subject, reason)` — `{subject, available: false, reason}` —
 * with the reason text from `qip-api/src/missing.rs`.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const NO_NARRATIVE_ADAPTER =
  "no narrative adapter is configured in this process. The kernel absorbs a news item only when an ingestion adapter hands it one, and this composition attaches none — the API's SENSE stage reads no vendor feed — so there is no headline, no entity and no sentiment to show, not a quiet tape.";

test("the news page renders the platform's own reason for serving no news, and no longer says the route does not exist", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/news": { subject: "news", available: false, reason: NO_NARRATIVE_ADAPTER },
  });
  await page.goto("/intelligence/news");

  // The premise: the route's answer landed and the panel rendered its arm.
  const reason = page.getByTestId("news-route-reason");
  await expect(reason).toBeVisible();

  // The property: the sentence is the platform's, whole.
  await expect(reason).toHaveText(NO_NARRATIVE_ADAPTER);
  await expect(page.getByTestId("news-route-body")).toHaveCount(0);

  // And the false claim is gone. Asserted on the panel that used to carry it,
  // which is rendered — a page-wide `not.toContainText` would pass on a page
  // that rendered nothing.
  const history = page.getByTestId("news-route-history");
  await expect(history).toContainText("is served but matched to one expression");
  await expect(history).not.toContainText("there is no");
});

test("a body the news route serves instead of an absence is rendered verbatim and labelled unread", async ({
  page,
}) => {
  // Not a shape any build of the handler emits — it has one arm and that arm
  // is the absence — so this guards the console against inventing a shape
  // once a second arm lands. The key is deliberately not a field a news item
  // would carry.
  await servePlatform(page, {
    ...healthy(),
    "/news": { "not-a-news-field": [{ marker: "sim-only" }] },
  });
  await page.goto("/intelligence/news");

  const body = page.getByTestId("news-route-body");
  await expect(body).toBeVisible();
  await expect(body).toContainText("not-a-news-field");
  await expect(page.locator("#content")).toContainText("not been taught its shape");
  await expect(page.getByTestId("news-route-reason")).toHaveCount(0);
});
