/**
 * `/data-sources/registrations`: one card per source from `GET /registrations`,
 * and the one approval an operator records with
 * `POST /registrations/{source}/approve`.
 *
 * The failures these tests prevent:
 *
 * * a pending source with no way to record the registration a person made,
 *   or with the add-secret command missing — asserted by the enabled button
 *   naming the operator and the command rendered verbatim;
 * * an approval that posts and leaves the card saying "pending", or that
 *   shows the session's name where the platform recorded a different one —
 *   asserted by the card re-rendering as registered by the operator the
 *   *platform* answered (the credential's subject, which the body cannot
 *   name), and by the POST body carrying the terms and the variable name;
 * * a viewer offered a control the platform would refuse, with no reason —
 *   asserted by the disabled button and the reason beside it;
 * * a 400 from the platform swallowed or paraphrased — asserted by the
 *   refusal naming the field, verbatim, with the card still pending;
 * * a keyless source wearing an approve button, which would imply a
 *   registration nobody needs to make;
 * * a `secret_slot: null` or `requirement: null` read as "nothing is needed",
 *   which would report a source the platform is refusing as fine — asserted
 *   on the contract's own `kalshi-markets` row, which needs an account and
 *   still answers `secret_slot: null`;
 * * a refusal that echoes the value it refused, which would write a pasted
 *   key back onto the screen;
 * * the page without the paper-trading label, or with a control that could
 *   submit an order.
 *
 * The bodies are the examples in `backend/crates/apps/qip-api/ROUTES-REGISTRATIONS.md`
 * — `standing` tagged on its own `standing` key, `secret_command` beside
 * `secret_slot`, companions listed — with Alpaca moved back to pending so
 * there is something to approve, and one extra row for the `requirement: null`
 * arm the contract's table names. The refusal texts are the platform's own,
 * quoted from the Rust that produces them. No deployment has yet served the
 * route.
 */
import { expect, test, type Page } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const PENDING_REASON =
  "`alpaca-daily-bars` requires an account with the venue, opened in the operator's own name (requirement `account`) and no registration record exists for it, so it is refused. anonymous or automated registration is not a path this platform offers: it circumvents the venue's terms and identity checks, and a licence nobody read is one nobody can be held to";

const KEYLESS = {
  source_id: "coinbase-spot-ticker",
  requirement: "keyless",
  standing: { standing: "keyless" },
  terms: "coinbase-exchange-market-data-terms",
  secret_slot: null,
  secret_command: null,
  companion_secret_slots: [],
} as const;

const ALPACA_TERMS = "https://alpaca.markets/terms-and-conditions";
const ALPACA_SLOT = "QIP_ALPACA_API_SECRET_KEY";
const ALPACA_COMMAND = "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-";
const ALPACA_COMPANION = "QIP_ALPACA_API_KEY_ID";
const ALPACA_COMPANION_COMMAND = "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-";

const ALPACA_PENDING = {
  source_id: "alpaca-daily-bars",
  requirement: "account",
  standing: {
    standing: "pending",
    who_must_register: "qip-platform",
    reason: PENDING_REASON,
  },
  terms: ALPACA_TERMS,
  secret_slot: ALPACA_SLOT,
  secret_command: ALPACA_COMMAND,
  companion_secret_slots: [{ variable: ALPACA_COMPANION, secret_command: ALPACA_COMPANION_COMMAND }],
} as const;

/**
 * The contract's own `kalshi-markets` row, verbatim from
 * `ROUTES-REGISTRATIONS.md`: a source that needs an *account* and still
 * answers `secret_slot: null`, because the manifest the platform holds names
 * no variable. The two `null`s in this surface are not the same fact, and a
 * card that read this one as "keyless" would tell an operator no key is
 * needed for a source the platform is refusing for want of one.
 */
const KALSHI_REASON =
  "`kalshi-markets` requires an account with the venue, opened in the operator's own name (requirement `account`) and no registration record exists for it, so it is refused. The platform's owner must register with the venue under their own identity, read its terms, create the credential in the venue's dashboard, place it in Secret Manager as a `_FILE`-projected secret, and record the registration — see docs/operations/registering-a-venue.md. anonymous or automated registration is not a path this platform offers: it circumvents the venue's terms and identity checks, and a licence nobody read is one nobody can be held to";

const KALSHI_PENDING = {
  source_id: "kalshi-markets",
  requirement: "account",
  standing: { standing: "pending", who_must_register: "qip-platform", reason: KALSHI_REASON },
  terms: "https://kalshi.com/terms",
  secret_slot: null,
  secret_command: null,
  companion_secret_slots: [],
} as const;

/**
 * The other arm of the requirement column: `null`, which the contract's table
 * says is "not a keyless source" but an unasked question, and whose standing
 * is therefore pending. A card that rendered a missing requirement as nothing
 * at all would look like a source with no obligations.
 */
const UNDECLARED = {
  source_id: "frankfurter-rates",
  requirement: null,
  standing: {
    standing: "pending",
    who_must_register: "qip-platform",
    reason: "`frankfurter-rates` has no registration requirement declared, so it is refused as unknown",
  },
  terms: null,
  secret_slot: null,
  secret_command: null,
  companion_secret_slots: [],
} as const;

/** The operator the platform records: its credential's subject, not the session's display name. */
const PLATFORM_OPERATOR = "operator@env";

const ALPACA_REGISTERED_STANDING = {
  standing: "registered",
  operator: PLATFORM_OPERATOR,
  terms_read_at: "2025-10-09T08:53:20.000Z",
  secret: ALPACA_SLOT,
} as const;

/** `ApprovalView`: the standing, not the whole source. */
const APPROVAL = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20.000Z",
  source_id: "alpaca-daily-bars",
  standing: ALPACA_REGISTERED_STANDING,
} as const;

const SERVED_AT = "2025-10-09T08:53:20.000Z";

const REGISTRATIONS = {
  posture: "PAPER TRADING",
  served_at: SERVED_AT,
  sources: [KEYLESS, ALPACA_PENDING, KALSHI_PENDING, UNDECLARED],
} as const;

const APPROVE_PATH = "/api/gateway/registrations/alpaca-daily-bars/approve";

/** The session as `/api/auth/session` projects it, with the roles under test. */
async function serveSession(page: Page, roles: readonly string[], displayName = "Dana Ops"): Promise<void> {
  await page.route("**/api/auth/session", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        status: "authenticated",
        session: {
          user: { email: "dana@example.test", displayName, accountType: "institutional", emailVerified: true, roles },
          expiresAt: Date.now() + 3_600_000,
          authenticatedAt: Date.now(),
        },
      }),
    });
  });
}

test("a pending source renders the approve button naming the operator, the terms to read, the secret slot and the add-secret command", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources/registrations");

  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Venue registrations" })).toBeVisible();
  // The premise: the route's answer landed and the session was read.
  await expect(page.getByTestId("registrations-session")).toContainText("signed in as Dana Ops");
  const card = page.getByTestId("registration-alpaca-daily-bars");
  await expect(card).toHaveAttribute("data-standing", "pending");
  await expect(page.getByTestId("registration-standing-alpaca-daily-bars")).toHaveText("pending");
  await expect(page.getByTestId("registration-pending-alpaca-daily-bars")).toContainText("qip-platform");
  await expect(page.getByTestId("registration-pending-alpaca-daily-bars")).toContainText(PENDING_REASON);
  await expect(card).toContainText("account");

  const button = page.getByTestId("registration-approve-alpaca-daily-bars");
  await expect(button).toBeVisible();
  await expect(button).toBeEnabled();
  await expect(button).toHaveText("Approve registration as Dana Ops");
  await expect(page.getByTestId("registration-approve-reason-alpaca-daily-bars")).toHaveCount(0);

  const terms = page.getByTestId("registration-terms-alpaca-daily-bars");
  await expect(terms.getByRole("link", { name: ALPACA_TERMS })).toHaveAttribute("href", ALPACA_TERMS);
  await expect(terms).toContainText("read these yourself");
  await expect(page.getByTestId("registration-secret-slot-alpaca-daily-bars")).toHaveText(ALPACA_SLOT);
  await expect(page.getByTestId("registration-command-alpaca-daily-bars")).toHaveText(ALPACA_COMMAND);
  // Exact, because the companion's copy control carries this label as a prefix.
  await expect(card.getByRole("button", { name: "Copy the add-secret command for alpaca-daily-bars", exact: true })).toBeVisible();
  // The companion the manifest also reads, with its own command.
  await expect(page.getByTestId("registration-companion-slot-alpaca-daily-bars")).toHaveText(ALPACA_COMPANION);
  await expect(page.getByTestId(`registration-command-alpaca-daily-bars-${ALPACA_COMPANION}`)).toHaveText(
    ALPACA_COMPANION_COMMAND,
  );

  await expect(page.getByTestId("registrations-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("registrations-body-posture")).toHaveText("PAPER TRADING");
  // The body's own `served_at`, which is a different fact from the freshness
  // chip beside it: one is when the platform says it answered, the other is
  // when this browser received it.
  await expect(page.getByTestId("registrations-served-at")).toHaveText("served 2025-10-09 08:53:20.000");
  await expect(content).toContainText("submits an order");
  // No order control. Anchored on the verb so the approve control is not
  // matched by accident and an order verb is not missed.
  await expect(content.getByRole("button", { name: /^(buy|sell|place|submit|trade|order)/i })).toHaveCount(0);
  await expect(content.locator("form")).toHaveCount(0);
  // Rendering wrote nothing; the only write this page has is behind two clicks.
  expect(writes).toEqual([]);
});

test("approving posts the terms and the variable name, and the card re-renders as registered by the operator the platform answered", async ({
  page,
}) => {
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });

  // A stateful platform: the POST flips what the GET answers, so the page
  // has to show the platform's record rather than its own memory of the click.
  let registered = false;
  const posted: unknown[] = [];
  await page.route("**/api/gateway/registrations**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() === "POST" && path === APPROVE_PATH) {
      posted.push(request.postDataJSON());
      registered = true;
      await route.fulfill({
        status: 200,
        headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
        body: JSON.stringify(APPROVAL),
      });
      return;
    }
    if (request.method() === "GET" && path === "/api/gateway/registrations") {
      await route.fulfill({
        status: 200,
        headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
        body: JSON.stringify({
          ...REGISTRATIONS,
          sources: [KEYLESS, registered ? { ...ALPACA_PENDING, standing: ALPACA_REGISTERED_STANDING } : ALPACA_PENDING],
        }),
      });
      return;
    }
    await route.fallback();
  });

  await page.goto("/data-sources/registrations");
  const button = page.getByTestId("registration-approve-alpaca-daily-bars");
  await expect(button).toBeEnabled();
  await button.click();

  const dialog = page.getByTestId("registration-dialog");
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId("registration-statement")).toHaveText(
    `I have read ${ALPACA_TERMS} and I register this venue under the company's own identity; the platform will not create the account anonymously.`,
  );
  await expect(page.getByTestId("registration-terms")).toHaveValue(ALPACA_TERMS);
  await expect(page.getByTestId("registration-secret")).toHaveValue(ALPACA_SLOT);
  await expect(page.getByTestId("registration-confirm")).toHaveText("Confirm as Dana Ops");
  await page.getByTestId("registration-confirm").click();

  await expect(dialog).toBeHidden();
  const card = page.getByTestId("registration-alpaca-daily-bars");
  await expect(card).toHaveAttribute("data-standing", "registered");
  await expect(page.getByTestId("registration-standing-alpaca-daily-bars")).toHaveText(`registered by ${PLATFORM_OPERATOR}`);
  const record = page.getByTestId("registration-record-alpaca-daily-bars");
  await expect(record).toContainText(PLATFORM_OPERATOR);
  await expect(record).toContainText(ALPACA_SLOT);
  // The session's name is on the button, never on the record: the platform
  // took the operator from its credential and the page shows what it said.
  await expect(record).not.toContainText("Dana Ops");
  await expect(page.getByTestId("registration-approve-alpaca-daily-bars")).toHaveCount(0);
  // The rest of the card is still the list's: the slot and command did not
  // vanish because the approval answered only a standing.
  await expect(page.getByTestId("registration-command-alpaca-daily-bars")).toHaveText(ALPACA_COMMAND);

  expect(posted).toEqual([{ terms: ALPACA_TERMS, secret: ALPACA_SLOT }]);
});

test("a viewer's approve button is disabled with the reason, and nothing is posted", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/gateway/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await serveSession(page, ["viewer"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources/registrations");

  // The premise: the session landed and it is a viewer.
  await expect(page.getByTestId("registrations-session")).toContainText("roles: viewer");
  const button = page.getByTestId("registration-approve-alpaca-daily-bars");
  await expect(button).toBeVisible();
  await expect(button).toBeDisabled();
  const reason = page.getByTestId("registration-approve-reason-alpaca-daily-bars");
  await expect(reason).toContainText("your session holds the viewer role and not operator");
  await expect(reason).toContainText("needs the operator role");

  // A disabled control is not a control: force the click and nothing opens.
  await button.click({ force: true });
  await expect(page.getByTestId("registration-dialog")).toHaveCount(0);
  expect(writes).toEqual([]);
});

/**
 * The platform's own refusal for a pasted key, verbatim from
 * `SecretRef::validate`
 * (`backend/crates/services/qip-market-ingestion/src/connector/manifest.rs`),
 * for the 27-character lowercase-leading value typed below. Verbatim and not
 * paraphrased because the property under test is that the page repeats the
 * platform's words rather than inventing gentler ones — a rewritten refusal
 * is a refusal an operator cannot search for.
 */
const PASTED = "sk_test_not_a_variable_name";
const SHAPE_REFUSAL =
  "the secret reference (27 characters) has a lowercase letter at position 1, so it is not a deployment variable name. " +
  "A manifest names the variable the credential is read from and never carries the credential itself; a name starts with A-Z " +
  "and continues in A-Z, 0-9 and _, so a pasted key cannot be written here. The value is not repeated in this message in case it is one";

test("a 400 from the platform is rendered naming the field, and the card stays pending", async ({ page }) => {
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  const REFUSAL = SHAPE_REFUSAL;
  await page.route("**/api/gateway/registrations/**", async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 400,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: REFUSAL }),
    });
  });

  await page.goto("/data-sources/registrations");
  await page.getByTestId("registration-approve-alpaca-daily-bars").click();
  await expect(page.getByTestId("registration-dialog")).toBeVisible();
  await page.getByTestId("registration-secret").fill(PASTED);
  await page.getByTestId("registration-confirm").click();

  const refusal = page.getByTestId("registration-refusal");
  await expect(refusal).toBeVisible();
  await expect(refusal).toContainText("answered 400");
  await expect(refusal).toContainText(REFUSAL);
  // The refusal does not repeat what was refused. The platform's rule is that
  // a message about a possibly-pasted key never carries the key; a console
  // that helpfully echoed "you typed X" would write it back to the screen and
  // into whichever ticket the line is copied into. The input still holds it,
  // because that is the field the operator has to correct.
  await expect(refusal).not.toContainText(PASTED);
  await expect(page.getByTestId("registration-secret")).toHaveValue(PASTED);
  // The dialog stays open with the field to correct; the card is still pending.
  await expect(page.getByTestId("registration-dialog")).toBeVisible();
  await expect(page.getByTestId("registration-alpaca-daily-bars")).toHaveAttribute("data-standing", "pending");
});

test("a 403 from the platform is rendered as the credential refused, in the platform's words", async ({ page }) => {
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  const DENIED = "POST /registrations/alpaca-daily-bars/approve requires the operator role; this credential holds viewer";
  await page.route("**/api/gateway/registrations/**", async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 403,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: DENIED }),
    });
  });

  await page.goto("/data-sources/registrations");
  await page.getByTestId("registration-approve-alpaca-daily-bars").click();
  await page.getByTestId("registration-confirm").click();

  const refusal = page.getByTestId("registration-refusal");
  await expect(refusal).toContainText("refused this console's credential (403)");
  await expect(refusal).toContainText(DENIED);
  await expect(page.getByTestId("registration-alpaca-daily-bars")).toHaveAttribute("data-standing", "pending");
});

test("a 409 from the platform tells the operator to sign in again, with the platform's reason", async ({ page }) => {
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  const STALE = "the operator identity was verified 22 minutes ago; an approval accepts one verified within 15 minutes";
  await page.route("**/api/gateway/registrations/**", async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 409,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: STALE }),
    });
  });

  await page.goto("/data-sources/registrations");
  await page.getByTestId("registration-approve-alpaca-daily-bars").click();
  await page.getByTestId("registration-confirm").click();

  const refusal = page.getByTestId("registration-refusal");
  await expect(refusal).toContainText("your session's credential is older than 15 minutes; sign in again");
  await expect(refusal).toContainText(STALE);
  await expect(page.getByTestId("registration-alpaca-daily-bars")).toHaveAttribute("data-standing", "pending");
});

test("a keyless source shows no approve button and no secret slot", async ({ page }) => {
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources/registrations");

  // The premise: the operator could approve, so an absent button is the
  // source's doing and not the session's.
  await expect(page.getByTestId("registration-approve-alpaca-daily-bars")).toBeEnabled();

  const card = page.getByTestId("registration-coinbase-spot-ticker");
  await expect(card).toHaveAttribute("data-standing", "keyless");
  await expect(page.getByTestId("registration-standing-coinbase-spot-ticker")).toHaveText("keyless");
  await expect(card).toContainText("no registration is needed");
  await expect(page.getByTestId("registration-terms-coinbase-spot-ticker")).toHaveText("coinbase-exchange-market-data-terms");
  await expect(page.getByTestId("registration-approve-coinbase-spot-ticker")).toHaveCount(0);
  await expect(card.getByRole("button", { name: /^approve/i })).toHaveCount(0);
  await expect(page.getByTestId("registration-secret-slot-coinbase-spot-ticker")).toHaveCount(0);
  await expect(page.getByTestId("registration-command-coinbase-spot-ticker")).toHaveCount(0);
  await expect(card).toContainText("a keyless source reads no credential");
});

test("a pending source with no secret slot is not shown as keyless, and an undeclared requirement is stated as unasked", async ({
  page,
}) => {
  // The two `null`s the contract distinguishes and a card could conflate.
  // `secret_slot: null` on `kalshi-markets` means the manifest names no
  // variable *yet*; `requirement: null` means nobody asked the venue what it
  // demands. Reading either as "nothing is needed" is the failure — it would
  // tell an operator a refused source is fine, on a page whose whole purpose
  // is naming what a person still has to do.
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources/registrations");

  // The premise: the keyless card is on the same screen and does say keyless,
  // so the wording below is this card's and not a page that renders one string
  // everywhere.
  await expect(page.getByTestId("registration-no-slot-coinbase-spot-ticker")).toHaveText(
    "none; a keyless source reads no credential",
  );

  const kalshi = page.getByTestId("registration-kalshi-markets");
  await expect(kalshi).toHaveAttribute("data-standing", "pending");
  await expect(page.getByTestId("registration-pending-kalshi-markets")).toContainText(
    "anonymous or automated registration is not a path this platform offers",
  );
  const noSlot = page.getByTestId("registration-no-slot-kalshi-markets");
  await expect(noSlot).toContainText("the manifest the platform holds for this source names no credential variable");
  await expect(noSlot).not.toContainText("keyless");
  // It still has the approval to record — the missing slot is not a missing obligation.
  await expect(page.getByTestId("registration-approve-kalshi-markets")).toBeEnabled();
  await expect(page.getByTestId("registration-command-kalshi-markets")).toHaveCount(0);

  const undeclared = page.getByTestId("registration-frankfurter-rates");
  await expect(undeclared).toHaveAttribute("data-standing", "pending");
  await expect(undeclared).toContainText("not declared");
  await expect(undeclared).toContainText("refuses as unknown");
  await expect(undeclared).toContainText("the platform declares no terms reference for this source");

  // The dialog for a source with no slot prefills nothing rather than
  // inventing a variable name: the platform refuses a blank one and says so,
  // and a guess written into the record would be a record of the wrong name.
  await page.getByTestId("registration-approve-kalshi-markets").click();
  await expect(page.getByTestId("registration-secret")).toHaveValue("");
  await expect(page.getByTestId("registration-terms")).toHaveValue("https://kalshi.com/terms");
});

test("with no one signed in the approve button is disabled and says a named operator is needed", async ({ page }) => {
  // The open-console instance answers `/api/auth/session` unauthenticated
  // itself; nothing is stubbed for it here.
  await servePlatform(page, { ...healthy(), "/registrations": REGISTRATIONS });
  await page.goto("/data-sources/registrations");

  await expect(page.getByTestId("registrations-session")).toContainText("no one is signed in");
  await expect(page.getByTestId("registration-approve-alpaca-daily-bars")).toBeDisabled();
  await expect(page.getByTestId("registration-approve-reason-alpaca-daily-bars")).toContainText(
    "no named operator to register as",
  );
});
