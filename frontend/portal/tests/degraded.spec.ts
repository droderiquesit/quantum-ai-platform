/**
 * What the console becomes when the platform is not there, and when it is
 * there but answering with something other than the platform.
 *
 * `tests/shell.spec.ts` already pins the clean absence: a gateway reporting
 * `unreachable`, every route still carrying `PAPER TRADING`, and `/portfolio`
 * saying so rather than showing an empty book. The two failures here are the
 * untidy ones, and both were reachable in a deployment whose `qip-api` will
 * not start:
 *
 *  1. A page that throws while rendering. React unmounts the tree, and with no
 *     `error.tsx` in the `(portal)` segment Next serves its production
 *     fallback — a blank document. The chrome goes with it, declaration
 *     included. `tests/support/platform.ts` records this happening once
 *     already, from a fixture whose shape was plausible and wrong.
 *  2. An answer that is not JSON. `QIP_API_BASE_URL` is the API's own Cloud
 *     Run URL, so a service with no healthy revision is answered for by Cloud
 *     Run's front end, in HTML. The client returned any non-empty string body
 *     as the panel's reason, so the page source became the reason, on every
 *     panel, re-fetched on every poll.
 *
 * Both are about the same property: the console must be legibly waiting on a
 * backend rather than legibly broken, because an operator who reads the second
 * goes looking for the wrong fault.
 */
import { expect, test } from "@playwright/test";
import {
  CLOUD_RUN_ERROR_TITLE,
  RISK_BODY,
  healthy,
  servePlatform,
  servePlatformAsHtmlErrorPage,
} from "./support/platform";

const AUTONOMY = {
  "/autonomy": {
    level: "paper_trading",
    ceiling: "paper_trading",
    live: false,
    history: [],
  },
};

/** The shape `GET /api/v1/system/governance` really returns. */
const GOVERNANCE_GOOD = { agents: 4, findings: [] };

/**
 * The same route with `findings` an object rather than an array.
 *
 * `risk/page.tsx` guards the table with `data.findings.length === 0`, which is
 * `undefined === 0` here, so rendering falls through to `data.findings.map`
 * and throws. Not a contrived shape: a subsystem that starts returning a
 * keyed object where it returned a list is an ordinary backend change, and the
 * console has no say in when one lands.
 */
const GOVERNANCE_WRONG_SHAPE = { agents: 4, findings: {} };

test("a page that throws keeps the shell, the declaration and a way out", async ({ page }) => {
  // The premise, asserted first and against the same route: with the real
  // shape the governance panel renders, so the failure below is caused by the
  // shape and not by /risk being broken in general. Without this, the test
  // would pass just as well against a page that never worked.
  await servePlatform(page, {
    ...healthy(),
    "/risk": RISK_BODY,
    ...AUTONOMY,
    "/system/governance": GOVERNANCE_GOOD,
  });
  await page.goto("/risk");
  await expect(page.getByText("The agent roster raises no governance finding.")).toBeVisible();
  await expect(page.getByTestId("portal-error")).toHaveCount(0);

  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, {
    ...healthy(),
    "/risk": RISK_BODY,
    ...AUTONOMY,
    "/system/governance": GOVERNANCE_WRONG_SHAPE,
  });
  await page.goto("/risk");

  // The page is replaced by a stated failure rather than by nothing.
  const failure = page.getByTestId("portal-error");
  await expect(failure).toBeVisible();
  await expect(failure).toContainText("This page stopped rendering.");

  // The chrome above the segment survived, so the operator still has the one
  // fact this console is obliged to state, and still has the navigation.
  const banner = page.getByTestId("paper-trading-banner");
  await expect(banner).toBeVisible();
  await expect(banner).toHaveText("PAPER TRADING");
  await expect(page.getByRole("link", { name: "Back to the dashboard" })).toBeVisible();

  // And it does not blame the platform for a fault in the console. The
  // distinction is the whole point: /risk answered fine here.
  await expect(failure).not.toContainText(/unreachable|not answering/i);
});

test("an HTML answer is described, not pasted into every panel", async ({ page }) => {
  await servePlatformAsHtmlErrorPage(page);
  await page.goto("/risk");

  const body = page.locator("body");

  // The premise: the console got that far and is reporting the status it was
  // given, so the absences below are about how it reports, not about it
  // having failed to report at all.
  await expect(body).toContainText("503");

  // What it says: the document described by its own title.
  await expect(body).toContainText(`the answer was an HTML page, not the platform's JSON`);
  await expect(body).toContainText(CLOUD_RUN_ERROR_TITLE);

  // What it must not say: the page's source. These are the parts of the
  // fixture that only appear on screen if the body was rendered verbatim —
  // the stylesheet, the tag, and the doctype. A reader seeing markup reads a
  // broken console.
  await expect(body).not.toContainText("font-family:Roboto");
  await expect(body).not.toContainText("<!doctype html>");
  await expect(body).not.toContainText("</body></html>");

  // The declaration is unconditional and is not assembled from upstream state,
  // so it survives an upstream that answers in a language the console does not
  // speak.
  await expect(page.getByTestId("paper-trading-banner")).toHaveText("PAPER TRADING");
});
