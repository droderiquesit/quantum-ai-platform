/**
 * `/data-sources`: the registration standing of every catalogued source, on the
 * page an operator opens to ask why a feed is quiet.
 *
 * The failures these tests prevent:
 *
 * * a source the platform refuses to read until somebody registers, shown on
 *   the feed catalogue as merely quiet — asserted by the pending badge naming
 *   the owner who must register, in the row for that source;
 * * a registered source that hides who registered it, which is the whole point
 *   of the record — asserted by the badge carrying the operator the *platform*
 *   answered;
 * * a keyless source dressed as pending, or the reverse: they are opposite
 *   facts about whether an account is needed at all;
 * * a badge that goes nowhere, so the standing is a dead end rather than a way
 *   to the page that explains it — asserted by the href and by the navigation;
 * * a control on the index that registers something. The approval is a record
 *   of a fact a person made true and lives on one page; a second one here would
 *   be a second write for the same fact;
 * * an unreachable platform rendered as an empty catalogue, which reads as
 *   "no source needs an account" when it means "nobody answered";
 * * the index without the posture the platform declared.
 *
 * The bodies are the example in
 * `backend/crates/apps/qip-api/ROUTES-REGISTRATIONS.md` — the three standings,
 * tagged on their own `standing` key, in catalogue order. No deployment has yet
 * served the route.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

/** The refusal the registry itself writes, quoted by the contract verbatim. */
const KALSHI_REASON =
  "`kalshi-markets` requires an account with the venue, opened in the operator's own name (requirement `account`) and no registration record exists for it, so it is refused. The platform's owner must register with the venue under their own identity, read its terms, create the credential in the venue's dashboard, place it in Secret Manager as a `_FILE`-projected secret, and record the registration — see docs/operations/registering-a-venue.md. anonymous or automated registration is not a path this platform offers: it circumvents the venue's terms and identity checks, and a licence nobody read is one nobody can be held to";

/** The operator the platform recorded: its credential's subject. */
const PLATFORM_OPERATOR = "operator@env";

const KALSHI_PENDING = {
  source_id: "kalshi-markets",
  requirement: "account",
  standing: {
    standing: "pending",
    who_must_register: "qip-platform",
    reason: KALSHI_REASON,
  },
  terms: "https://kalshi.com/terms",
  secret_slot: null,
  secret_command: null,
  companion_secret_slots: [],
} as const;

const ALPACA_REGISTERED = {
  source_id: "alpaca-daily-bars",
  requirement: "account",
  standing: {
    standing: "registered",
    operator: PLATFORM_OPERATOR,
    terms_read_at: "2025-10-09T08:50:00.000Z",
    secret: "QIP_ALPACA_API_SECRET_KEY",
  },
  terms: "https://alpaca.markets/terms-and-conditions",
  secret_slot: "QIP_ALPACA_API_SECRET_KEY",
  secret_command: "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-",
  companion_secret_slots: [
    {
      variable: "QIP_ALPACA_API_KEY_ID",
      secret_command: "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-",
    },
  ],
} as const;

const COINBASE_KEYLESS = {
  source_id: "coinbase-spot-ticker",
  requirement: "keyless",
  standing: { standing: "keyless" },
  terms: "coinbase-exchange-market-data-terms",
  secret_slot: null,
  secret_command: null,
  companion_secret_slots: [],
} as const;

const REGISTRATIONS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20.000Z",
  sources: [KALSHI_PENDING, ALPACA_REGISTERED, COINBASE_KEYLESS],
} as const;

const REGISTRATIONS_HREF = "/data-sources/registrations";

test("every catalogued source wears its registration standing, and a pending one names who must register", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources");

  const panel = page.getByTestId("data-sources-registrations");
  // The premise: the catalogue landed and every source in it has a row. Without
  // this, the standings below could all be absent from a panel that never
  // rendered and the text assertions would have nothing to disagree with.
  await expect(panel.getByRole("heading", { name: "Registration standing per catalogued source" })).toBeVisible();
  await expect(panel.locator("tbody tr")).toHaveCount(3);

  const pending = page.getByTestId("data-sources-standing-kalshi-markets");
  await expect(pending).toHaveText("pending — qip-platform must register");
  await expect(pending).toHaveAttribute("data-standing", "pending");
  // The registry's own refusal is what the badge carries as its explanation;
  // the page paraphrases none of it.
  await expect(pending).toHaveAttribute("title", KALSHI_REASON);

  const registered = page.getByTestId("data-sources-standing-alpaca-daily-bars");
  await expect(registered).toHaveText(`registered by ${PLATFORM_OPERATOR}`);
  await expect(registered).toHaveAttribute("data-standing", "registered");

  const keyless = page.getByTestId("data-sources-standing-coinbase-spot-ticker");
  await expect(keyless).toHaveText("keyless");
  await expect(keyless).toHaveAttribute("data-standing", "keyless");

  // The requirement beside the standing, from the platform's own table: a
  // keyless source and a pending one differ in what the venue demands, not only
  // in how the row is coloured.
  const kalshiRow = panel.locator('tbody tr[data-standing="pending"]');
  await expect(kalshiRow).toContainText("kalshi-markets");
  await expect(kalshiRow).toContainText("account");
});

test("the standing badge is a link to the venue registrations page, and the index registers nothing itself", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources");

  const badge = page.getByTestId("data-sources-standing-kalshi-markets");
  await expect(badge).toHaveAttribute("href", REGISTRATIONS_HREF);
  await expect(page.getByTestId("data-sources-standing-alpaca-daily-bars")).toHaveAttribute(
    "href",
    REGISTRATIONS_HREF,
  );
  await expect(page.getByTestId("data-sources-standing-coinbase-spot-ticker")).toHaveAttribute(
    "href",
    REGISTRATIONS_HREF,
  );

  // Read-only here: nothing on the index approves, and the one control that
  // records a registration is on the page the badge leads to.
  const content = page.locator("#content");
  await expect(content.getByRole("button", { name: /approve/i })).toHaveCount(0);
  await expect(content.locator("form")).toHaveCount(0);

  // The href is only half of it — a link that does not navigate is a decoration.
  await badge.click();
  await expect(page).toHaveURL(new RegExp(`${REGISTRATIONS_HREF}$`));
  await expect(page.getByRole("heading", { name: "Venue registrations" })).toBeVisible();

  // Rendering the badges wrote nothing: the standing is read, never asserted.
  expect(writes, "the data sources index wrote to the platform").toEqual([]);
});

test("the registration panel carries the posture the platform answered, beside the console's own label", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources");

  // The body's own literal, rendered as it came rather than assumed by the page.
  await expect(page.getByTestId("data-sources-registrations-posture")).toHaveText("PAPER TRADING");
  // And the declaration the chrome carries on every route, which is the one an
  // operator sees before any body has landed.
  await expect(page.getByTestId("paper-trading-banner")).toBeVisible();
});

test("an empty catalogue says the platform listed nothing rather than showing a source as keyless", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/registrations": { posture: "PAPER TRADING", served_at: "2025-10-09T08:53:20.000Z", sources: [] },
  });
  await page.goto("/data-sources");

  const panel = page.getByTestId("data-sources-registrations");
  await expect(panel).toContainText("The platform catalogues no source's registration requirement.");
  // An empty catalogue is not a keyless one. The badge for a source that was
  // never listed must not exist at all.
  await expect(page.getByTestId("data-sources-standing-coinbase-spot-ticker")).toHaveCount(0);
});

test("a platform nobody can reach is reported as unreachable, not as a catalogue with no sources", async ({
  page,
}) => {
  await servePlatformUnreachable(page);
  await page.goto("/data-sources");

  const panel = page.getByTestId("data-sources-registrations");
  await expect(panel).toContainText("The platform could not be reached.");
  // The opposite fact, which must not appear: "no source needs an account" and
  // "nobody answered" would look alike if this panel fell back to its empty state.
  await expect(panel).not.toContainText("catalogues no source");
  await expect(page.getByTestId("data-sources-standing-kalshi-markets")).toHaveCount(0);
});
