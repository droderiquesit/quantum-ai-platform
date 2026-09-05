/**
 * `/treasury/ledger`: the ledger's eligibility verdict on each user, and the
 * one decision an operator records with
 * `POST /ledger/users/{user}/eligibility`.
 *
 * The failures these tests prevent:
 *
 * * a user row that shows balances and mandates and says nothing about
 *   whether the next funding would be refused — asserted by the verdict on
 *   both an eligible row and a refused one, and by the refused row carrying
 *   the ledger's own token and its own sentence verbatim, because the
 *   sentence is what names the remedy;
 * * a decision recorded with no evidence — asserted by the confirm control
 *   staying unreachable until the document reference is named, and by the
 *   attestation the operator confirms naming the document, the date and the
 *   expiry in the first person;
 * * a decision that posts and leaves the row saying what it said before, or
 *   that names the session's person as the decider the platform recorded —
 *   asserted by the row re-rendering from the platform's answer, by the
 *   controls and the attestation naming no person, by the confirm step
 *   stating the deployment subject before the click, and by the result line
 *   saying the name is this console's knowledge and was not sent;
 * * a viewer offered a control the platform would refuse, with no reason —
 *   asserted by the disabled panel, the reason beside it, and nothing posted;
 * * a 400, a 403 or a 409 swallowed or paraphrased — asserted by each
 *   rendered as it came, with the 409 naming the remedy that is not a retry;
 * * a form on this page that mentions withdrawing, or a control that acts on
 *   the market — asserted on the panel and the confirm dialog.
 *
 * The bodies follow `backend/crates/apps/qip-api/ROUTES-LEDGER.md` and the
 * refusal sentences are `Ineligible::describe` in
 * `backend/crates/services/qip-capital/src/ledger/eligibility.rs`, character
 * for character. They are not captured from a running process: no deployment
 * has enrolled a user or served the decision route.
 *
 * The attribution the panel states is checked against the platform rather
 * than assumed: `upstreamHeaders` in `src/lib/server/upstream.ts` sends one
 * deployment bearer token and forwards nothing of the sealed session,
 * `SessionClaims` in `src/lib/server/identity.ts` says the claim set reaches
 * `qip-api` nowhere, `qip-api`'s `main.rs` builds every subject as
 * `format!("{}@env", role.as_str())`, and `routes.rs` hands that same
 * `principal.subject` to `Platform::decide_eligibility`. The record therefore
 * names a deployment. See the 2026-09-05 amendment to ADR 0041, which
 * established the identical gap on the venue registrations surface.
 *
 * The API is stubbed at the browser boundary, so the application code under
 * test is the code a deployment runs. What that cannot prove is the gateway:
 * `declaresWrite` in `src/lib/api/endpoints.ts` does not yet name this path,
 * so against a real gateway the POST is refused with 405 and
 * `x-qip-gateway: refused` before the credential is read. That refusal is
 * rendered — it is the `kind: "error"` arm — but it is not the platform
 * answering, and these tests do not claim the write reaches a platform.
 */
import { expect, test, type Page } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

/**
 * The subject `qip-api` builds for a credential holding the operator role —
 * a deployment, not a person. Named here because it is what the confirm step
 * must say, and it is a role name and a deployment word: no account.
 */
const PLATFORM_OPERATOR = "operator@env";

const WITHDRAWAL_REFUSED =
  "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision";

/** `Ineligible::describe` for `UnknownUser`, with the user substituted. */
const UNKNOWN_USER_REASON =
  "desk is not eligible (unknown_user): no operator has verified this user; an eligibility decision must be taken and recorded before capital is put to work";

/** `Ineligible::describe` for `Revoked`. */
const REVOKED_REASON =
  "alice is not eligible (revoked): the eligibility was revoked; a new decision by an operator is required";

const ELIGIBLE = {
  eligible: true,
  verified_at: "2026-01-15T09:00:00.000Z",
  can_invest: true,
  jurisdiction: "GB",
  expires_at: "2027-01-15T09:00:00.000Z",
  refused: null,
  reason: null,
} as const;

const UNKNOWN_USER = {
  eligible: false,
  verified_at: null,
  can_invest: null,
  jurisdiction: null,
  expires_at: null,
  refused: "unknown_user",
  reason: UNKNOWN_USER_REASON,
} as const;

const REVOKED = {
  eligible: false,
  verified_at: null,
  can_invest: null,
  jurisdiction: null,
  expires_at: null,
  refused: "revoked",
  reason: REVOKED_REASON,
} as const;

/** The verdict the platform answers once the desk has been granted. */
const DESK_GRANTED = {
  eligible: true,
  verified_at: "2026-03-02T00:00:00.000Z",
  can_invest: true,
  jurisdiction: "ZZ",
  expires_at: "2027-03-02T00:00:00.000Z",
  refused: null,
  reason: null,
} as const;

function mandate(jurisdiction: string) {
  return {
    capital: "1000000",
    currency: "USD",
    risk_tolerance: "1",
    liquidity_floor: "0",
    investable: "1000000",
    exploration_share: "0",
    jurisdiction,
    permitted_families: { any: true, families: [] },
  } as const;
}

function entitlements() {
  return [
    {
      family: "research-tests",
      role: "viewer",
      evaluated_at: "2026-03-01T08:53:20Z",
      can_view: { granted: true, reason: "holds a mandate" },
      can_invest: { granted: false, reason: "holds the viewer role, which does not invest" },
      can_withdraw: { granted: false, reason: WITHDRAWAL_REFUSED },
    },
  ] as const;
}

function alice(eligibility: unknown) {
  return {
    user_id: "alice",
    mandate: mandate("GB"),
    eligibility,
    balances: [],
    entitlements: entitlements(),
    entitlements_note: null,
  };
}

function desk(eligibility: unknown) {
  return {
    user_id: "desk",
    mandate: mandate("ZZ"),
    eligibility,
    balances: [],
    entitlements: entitlements(),
    entitlements_note: null,
  };
}

function ledgerBody(users: readonly unknown[]) {
  return {
    posture: "PAPER TRADING",
    served_at: "2026-03-01T08:53:20Z",
    evaluated_as_role: "viewer",
    products: ["research-tests"],
    fills_journalled: 0,
    users,
  };
}

const LEDGER_PATH = "/api/gateway/ledger/users";
const GATEWAY_LEDGER = "**/api/gateway/ledger/users**";

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

function json(body: unknown, status = 200) {
  return {
    status,
    headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
    body: JSON.stringify(body),
  };
}

/**
 * A platform whose list changes when the decision lands, so the page has to
 * render the platform's record rather than its own memory of the click.
 */
async function serveDecidableLedger(
  page: Page,
  initial: { readonly alice: unknown; readonly desk: unknown },
  answer: (user: string, body: unknown) => unknown,
  posted: unknown[],
): Promise<void> {
  const verdicts = { ...initial } as { alice: unknown; desk: unknown };
  await page.route(GATEWAY_LEDGER, async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() === "POST") {
      const user = path.slice(`${LEDGER_PATH}/`.length).replace("/eligibility", "");
      posted.push({ user, body: request.postDataJSON() });
      const verdict = answer(user, request.postDataJSON());
      if (user === "alice") verdicts.alice = verdict;
      if (user === "desk") verdicts.desk = verdict;
      const row = user === "alice" ? alice(verdicts.alice) : desk(verdicts.desk);
      await route.fulfill(json(row));
      return;
    }
    if (request.method() === "GET" && path === LEDGER_PATH) {
      await route.fulfill(json(ledgerBody([alice(verdicts.alice), desk(verdicts.desk)])));
      return;
    }
    await route.fallback();
  });
}

test("each user row carries the ledger's verdict, and a refused row carries the token and the reason it gave", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": ledgerBody([alice(ELIGIBLE), desk(UNKNOWN_USER)]),
  });
  await page.goto("/treasury/ledger");

  // The premise: the page rendered and both rows landed. Without it every
  // assertion below would hold just as well for a page that failed to render.
  await expect(page.getByRole("heading", { name: "Ledger" })).toBeVisible();
  await expect(page.getByTestId("ledger-user-count")).toHaveText("2");
  await expect(page.getByTestId("ledger-user")).toHaveCount(2);

  // The eligible row: the verdict and the terms an operator wrote.
  await expect(page.getByTestId("eligibility-alice")).toHaveAttribute("data-eligible", "true");
  await expect(page.getByTestId("eligibility-verdict-alice")).toHaveText("eligible");
  const terms = page.getByTestId("eligibility-terms-alice");
  await expect(terms).toContainText("2026-01-15");
  await expect(terms).toContainText("in GB");
  await expect(terms).toContainText("may have capital put to work");
  await expect(terms).toContainText("expires 2027-01-15");
  await expect(page.getByTestId("eligibility-reason-alice")).toHaveCount(0);

  // The refused row: the ledger's stable token, and its sentence verbatim.
  await expect(page.getByTestId("eligibility-desk")).toHaveAttribute("data-eligible", "false");
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await expect(page.getByTestId("eligibility-reason-desk")).toHaveText(UNKNOWN_USER_REASON);
  // Not "eligible": the refused row must not be readable as a granted one.
  await expect(page.getByTestId("eligibility-verdict-desk")).not.toHaveText("eligible");
  await expect(page.getByTestId("eligibility-terms-desk")).toHaveCount(0);

  // The declaration is still on the page, and still true of it.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");
  await expect(page.locator("#content")).toContainText("Nothing on this page can move capital.");

  // Nothing on the decision panel is about withdrawing, and nothing on it
  // acts on the market. The card's entitlement block says "can withdraw" —
  // that is the platform's refusal, and it is not on the form.
  const panel = page.getByTestId("eligibility-panel-desk");
  await expect(panel).toBeVisible();
  await expect(panel).not.toContainText(/withdraw/i);
  await expect(panel.getByRole("button", { name: /buy|sell|place|trade|order|transfer/i })).toHaveCount(0);

  // Rendering wrote nothing; the decision is behind two clicks.
  expect(writes).toEqual([]);
});

test("granting posts the attested terms and re-renders the row as eligible, naming no person as the decider the platform recorded", async ({
  page,
}) => {
  const posted: unknown[] = [];
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy() });
  await serveDecidableLedger(page, { alice: ELIGIBLE, desk: UNKNOWN_USER }, () => DESK_GRANTED, posted);

  await page.goto("/treasury/ledger");
  // The premise: the desk starts refused, so an "eligible" below is the
  // decision's doing and not the fixture's.
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");

  // The evidence is required before the decision can even be reviewed.
  const review = page.getByTestId("eligibility-review-desk");
  await expect(review).toBeDisabled();
  await expect(page.getByTestId("eligibility-document-required-desk")).toContainText(
    "a finding whose evidence nobody recorded is a check nobody can repeat",
  );

  await page.getByTestId("eligibility-verified-desk").fill("2026-03-02");
  await page.getByTestId("eligibility-expires-desk").fill("2027-03-02");
  await page.getByTestId("eligibility-can-invest-desk").check();
  // Prefilled from the mandate the platform answered, and left as it came.
  await expect(page.getByTestId("eligibility-jurisdiction-desk")).toHaveValue("ZZ");
  await page.getByTestId("eligibility-document-desk").fill("passport GBR-99, checked in person");

  await expect(review).toBeEnabled();
  // The control names no person. It once read "Decide eligibility as Dana
  // Ops", which named the console's signed-in user as the decider; the
  // platform records its own deployment credential's subject and always did,
  // so the copy named an identity nothing downstream holds (ADR 0041,
  // amendment of 2026-09-05).
  await expect(review).toHaveText("Decide eligibility");
  await expect(review).not.toContainText("Dana Ops");
  await review.click();

  const dialog = page.getByTestId("eligibility-dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).not.toContainText(/withdraw/i);
  // First person, and no name: the attestation opened "I, Dana Ops, verified"
  // and read as the attribution on the record, which does not exist.
  await expect(page.getByTestId("eligibility-attestation")).toHaveText(
    "I verified this user's identity on 2026-03-02 against passport GBR-99, checked in person; expires 2027-03-02",
  );
  await expect(page.getByTestId("eligibility-confirm")).toHaveText("Confirm decision");
  // The confirm step states what the platform will actually attribute, before
  // the click rather than after it: the console's own deployment credential,
  // named, and explicitly not the signed-in person. The premise is asserted
  // first — the paragraph exists and the session's name is on the page — so
  // this cannot pass by matching an empty locator.
  const attribution = page.getByTestId("eligibility-attribution");
  await expect(attribution).toBeVisible();
  await expect(page.getByTestId("ledger-session")).toContainText("Dana Ops");
  await expect(attribution).toContainText(PLATFORM_OPERATOR);
  await expect(attribution).toContainText("not Dana Ops");
  await page.getByTestId("eligibility-confirm").click();

  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("eligibility-desk")).toHaveAttribute("data-eligible", "true");
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("eligible");
  await expect(page.getByTestId("eligibility-terms-desk")).toContainText("expires 2027-03-02");

  // The name is this console's own knowledge of who was signed in, and it was
  // never sent: the record attributes the decision to the console's
  // credential subject. The line once read "Sent as Dana Ops", which claimed
  // a name had crossed the wire when no field on the body carries one.
  const result = page.getByTestId("eligibility-result-desk");
  await expect(result).toContainText("Decided while signed in as Dana Ops");
  await expect(result).not.toContainText("Sent as Dana Ops");
  await expect(result).toContainText("the platform was not told");
  await expect(result).toContainText("attributes the decision to this console");
  await expect(result).toContainText("eligible");

  // Alice was not touched by a decision about the desk.
  await expect(page.getByTestId("eligibility-verdict-alice")).toHaveText("eligible");
  // Exhaustive on purpose: the body is the whole body, so a key carrying the
  // signed-in name would fail here. The console sends no operator field, and
  // `EligibilityRequest::parse` in `qip-api`'s `ledger_views.rs` refuses any
  // key outside its `GRANTED_FIELDS`/`REVOKED_FIELDS` lists in any case.
  expect(posted).toEqual([
    {
      user: "desk",
      body: {
        decision: "granted",
        verified_at: "2026-03-02T00:00:00Z",
        can_invest: true,
        jurisdiction: "ZZ",
        expires_at: "2027-03-02T00:00:00Z",
        reason: "passport GBR-99, checked in person",
      },
    },
  ]);
});

test("revoking re-renders the row as refused with the ledger's revoked token and its reason", async ({ page }) => {
  const posted: unknown[] = [];
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, { ...healthy() });
  await serveDecidableLedger(page, { alice: ELIGIBLE, desk: UNKNOWN_USER }, () => REVOKED, posted);

  await page.goto("/treasury/ledger");
  // The premise: alice starts eligible, so "refused" below is the revocation.
  await expect(page.getByTestId("eligibility-verdict-alice")).toHaveText("eligible");

  await page.getByTestId("eligibility-decision-alice").selectOption("revoked");
  // The verification terms belong to a grant and are gone from a revocation.
  await expect(page.getByTestId("eligibility-verified-alice")).toHaveCount(0);
  await expect(page.getByTestId("eligibility-jurisdiction-alice")).toHaveCount(0);
  await page.getByTestId("eligibility-document-alice").fill("case NOTE-2026-14, identity no longer evidenced");

  await page.getByTestId("eligibility-review-alice").click();
  // First person, no name — a revocation is attributed to the same deployment
  // subject a grant is.
  await expect(page.getByTestId("eligibility-attestation")).toContainText(
    "I revoke this user's eligibility on the record case NOTE-2026-14, identity no longer evidenced",
  );
  await expect(page.getByTestId("eligibility-attestation")).not.toContainText("Dana Ops");
  await expect(page.getByTestId("eligibility-attribution")).toContainText(PLATFORM_OPERATOR);
  await page.getByTestId("eligibility-confirm").click();

  await expect(page.getByTestId("eligibility-dialog")).toBeHidden();
  await expect(page.getByTestId("eligibility-alice")).toHaveAttribute("data-eligible", "false");
  await expect(page.getByTestId("eligibility-verdict-alice")).toHaveText("revoked");
  await expect(page.getByTestId("eligibility-reason-alice")).toHaveText(REVOKED_REASON);
  await expect(page.getByTestId("eligibility-terms-alice")).toHaveCount(0);

  expect(posted).toEqual([
    {
      user: "alice",
      body: { decision: "revoked", reason: "case NOTE-2026-14, identity no longer evidenced" },
    },
  ]);
});

test("a viewer's decision panel is disabled with the reason, and nothing is posted", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/gateway/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await serveSession(page, ["viewer"]);
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": ledgerBody([alice(ELIGIBLE), desk(UNKNOWN_USER)]),
  });
  await page.goto("/treasury/ledger");

  // The premise: the session landed and it is a viewer.
  await expect(page.getByTestId("ledger-session")).toContainText("roles: viewer");
  await expect(page.getByTestId("eligibility-panel-desk")).toHaveAttribute("data-allowed", "false");
  await expect(page.getByTestId("eligibility-review-desk")).toBeDisabled();
  await expect(page.getByTestId("eligibility-document-desk")).toBeDisabled();
  await expect(page.getByTestId("eligibility-decision-desk")).toBeDisabled();

  const reason = page.getByTestId("eligibility-permission-desk");
  await expect(reason).toContainText("your session holds the viewer role and not operator");
  await expect(reason).toContainText("needs the operator role");

  // A disabled control is not a control: force the click and nothing opens.
  await page.getByTestId("eligibility-review-desk").click({ force: true });
  await expect(page.getByTestId("eligibility-dialog")).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a 400 from the platform is rendered naming the field, and the row keeps the verdict it had", async ({ page }) => {
  const REFUSAL =
    "`verified_at` is not an RFC 3339 timestamp: the field was empty, and an eligibility whose verification instant nobody stated is a finding nobody can date";
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": ledgerBody([alice(ELIGIBLE), desk(UNKNOWN_USER)]),
  });
  await page.route(GATEWAY_LEDGER, async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill(json({ error: REFUSAL }, 400));
  });

  await page.goto("/treasury/ledger");
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await page.getByTestId("eligibility-document-desk").fill("passport GBR-99, checked in person");
  await page.getByTestId("eligibility-review-desk").click();
  await page.getByTestId("eligibility-confirm").click();

  const refusal = page.getByTestId("eligibility-refusal");
  await expect(refusal).toBeVisible();
  await expect(refusal).toContainText("answered 400");
  await expect(refusal).toContainText(REFUSAL);
  // The dialog stays open with the field to correct, and the verdict is the
  // platform's still: a refused decision changed nothing.
  await expect(page.getByTestId("eligibility-dialog")).toBeVisible();
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await expect(page.getByTestId("eligibility-result-desk")).toHaveCount(0);
});

test("a 403 from the platform is rendered as the credential refused, in the platform's words", async ({ page }) => {
  const DENIED =
    "POST /ledger/users/desk/eligibility requires the operator role; this credential holds viewer";
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": ledgerBody([alice(ELIGIBLE), desk(UNKNOWN_USER)]),
  });
  await page.route(GATEWAY_LEDGER, async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill(json({ error: DENIED }, 403));
  });

  await page.goto("/treasury/ledger");
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await page.getByTestId("eligibility-document-desk").fill("passport GBR-99, checked in person");
  await page.getByTestId("eligibility-review-desk").click();
  await page.getByTestId("eligibility-confirm").click();

  const refusal = page.getByTestId("eligibility-refusal");
  await expect(refusal).toContainText("refused this console's credential (403)");
  await expect(refusal).toContainText(DENIED);
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
});

test("a 409 tells the operator to sign in again rather than reading as something to retry", async ({ page }) => {
  const STALE =
    "the operator identity was verified 22 minutes ago; an eligibility decision accepts one verified within 15 minutes";
  await serveSession(page, ["viewer", "operator"]);
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": ledgerBody([alice(ELIGIBLE), desk(UNKNOWN_USER)]),
  });
  await page.route(GATEWAY_LEDGER, async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill(json({ error: STALE }, 409));
  });

  await page.goto("/treasury/ledger");
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await page.getByTestId("eligibility-document-desk").fill("passport GBR-99, checked in person");
  await page.getByTestId("eligibility-review-desk").click();
  await page.getByTestId("eligibility-confirm").click();

  const refusal = page.getByTestId("eligibility-refusal");
  await expect(refusal).toContainText("your session's credential is older than 15 minutes; sign in again");
  await expect(refusal).toContainText(STALE);
  // Not a retry, and not a state change: the verdict is what it was.
  await expect(page.getByTestId("eligibility-verdict-desk")).toHaveText("unknown_user");
  await expect(page.getByTestId("eligibility-result-desk")).toHaveCount(0);
});
