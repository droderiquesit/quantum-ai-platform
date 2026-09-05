/**
 * The signup job's own tests: `node --test scripts/venue-signup/signup.test.mjs`.
 *
 * The browser tests drive the real Chromium against a mock signup page this
 * file serves on loopback; no venue is touched. The `gcloud` they write to is
 * a script that records only a SHA-256 of what it was given on stdin, so the
 * tests can prove the right value reached the right slot without a secret
 * ever landing on disk — the same property the job promises the operator.
 *
 * Each test names the failure it prevents. What the job refuses matters
 * more than what it fills.
 */
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  EXIT,
  approvalProblems,
  findGcloud,
  generatePassword,
  identityProblems,
  judge,
  loadApproval,
  loadIdentity,
  loadRecipe,
  main,
  perform,
  recipeProblems,
} from "./signup.mjs";
import { chromiumArguments, launchRefusal } from "./browser.mjs";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const scratch = mkdtempSync(join(tmpdir(), "venue-signup-test-"));
const gcloudDir = join(scratch, "bin");
const fakeGcloud = join(gcloudDir, "gcloud");
const identity = {
  legal_name: "Example Research Desk Ltd",
  contact_email: "desk@example.test",
  phone: "+44 20 7946 0000",
  address: "1 Example Street, London",
  country: "GB",
};
const TERMS = "https://venue.test/terms";
const posted = [];
let apiKeyShown = "";
let baseUrl = "";
let server;

function page(variant) {
  const extra = {
    clean: "",
    captcha: '<div class="g-recaptcha" data-sitekey="mock"></div>',
    tax: '<label for="tin">Tax identification number</label><input id="tin" name="tax_id">',
    extra: '<label for="ref">Referral code</label><input id="ref" name="referral_code">',
    verify: "",
  }[variant];
  return `<!doctype html><html><head><title>Sign up</title></head><body>
<h1>Open an account</h1>
<form method="post" action="/submit?variant=${variant}">
  <label for="name">Full name</label><input id="name" name="name">
  <label for="email">E-mail</label><input id="email" name="email" type="email">
  <label for="password">Password</label><input id="password" name="password" type="password">
  <label for="confirm">Confirm password</label><input id="confirm" name="confirm" type="password">
  ${extra}
  <label><input type="checkbox" name="terms"> I accept the terms</label>
  <button type="submit">Create account</button>
</form></body></html>`;
}

before(async () => {
  mkdirSync(gcloudDir, { recursive: true });
  writeFileSync(
    fakeGcloud,
    [
      "#!/bin/sh",
      '# A stand-in for gcloud: records the slot and a digest of stdin, never the value.',
      'if [ "$1 $2 $3" != "secrets versions add" ] || [ "$5" != "--data-file=-" ]; then echo "unexpected arguments: $*" >&2; exit 9; fi',
      'sha256sum | cut -d" " -f1 > "$(dirname "$0")/$4.sha256"',
    ].join("\n"),
  );
  chmodSync(fakeGcloud, 0o755);
  server = createServer((req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    if (req.method === "GET" && url.pathname === "/signup") {
      res.writeHead(200, { "content-type": "text/html" });
      res.end(page(url.searchParams.get("variant") ?? "clean"));
      return;
    }
    if (req.method === "POST" && url.pathname === "/submit") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        const form = Object.fromEntries(new URLSearchParams(body));
        posted.push(form);
        res.writeHead(200, { "content-type": "text/html" });
        if (url.searchParams.get("variant") === "verify") {
          res.end(
            '<html><body><h1>Check your e-mail</h1><p>We sent a code to your e-mail. Enter the code:</p>' +
              '<form><input name="code" autocomplete="one-time-code" inputmode="numeric"><button type="submit">Verify</button></form></body></html>',
          );
          return;
        }
        apiKeyShown = `MOCKKEY-${randomBytes(12).toString("hex")}`;
        res.end(`<html><body><h1 id="welcome">Welcome, ${form.name}</h1><p>Your key: <code id="api-key">${apiKeyShown}</code></p></body></html>`);
      });
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  baseUrl = `http://127.0.0.1:${server.address().port}`;
});

after(() => {
  server?.close();
  rmSync(scratch, { recursive: true, force: true });
});

function recipe(variant, overrides = {}) {
  return {
    venue: "mock",
    source_id: "mock-feed",
    signup_url: `${baseUrl}/signup?variant=${variant}`,
    identity_verification_required: false,
    steps: [
      { selector: "#name", field: "legal_name" },
      { selector: "#email", field: "contact_email" },
      { selector: "#password", field: "password" },
      { selector: "#confirm", field: "password_confirm" },
      { selector: "input[name='terms']", field: "accept_terms", terms: TERMS },
    ],
    submit: "button[type='submit']",
    after_submit: "success",
    success: { selector: "#welcome", api_key_selector: "#api-key" },
    ...overrides,
  };
}

function approval(overrides = {}) {
  return {
    source_id: "mock-feed",
    operator: "d.roderiques",
    terms_read_at: new Date().toISOString(),
    terms: TERMS,
    secret_slots: { password: "mock-venue-password", api_key: "mock-venue-api-key" },
    ...overrides,
  };
}

const sha256 = (text) => createHash("sha256").update(text).digest("hex");
const digestFor = (slot) => readFileSync(join(gcloudDir, `${slot}.sha256`), "utf8").trim();

async function run(theRecipe, theApproval = approval()) {
  const lines = [];
  assert.deepEqual(recipeProblems(theRecipe, "mock"), [], "premise: the mock recipe is a valid recipe");
  assert.deepEqual(approvalProblems(theApproval, theRecipe), [], "premise: the approval covers the recipe");
  const result = await perform({
    recipe: theRecipe,
    identity,
    approval: theApproval,
    gcloud: fakeGcloud,
    scratchDir: join(scratch, "shots"),
    budgetMs: 20_000,
    log: (line) => lines.push(line),
  });
  return { result, lines };
}

// ---------------------------------------------------------------------------
// The browser tests
// ---------------------------------------------------------------------------

test("a clean form is filled from the identity file and both credentials reach their slots, and nothing else", async () => {
  posted.length = 0;
  const { result, lines } = await run(recipe("clean"));
  assert.equal(result.reason, null);
  assert.equal(result.code, EXIT.ok, JSON.stringify(result));
  assert.equal(result.outcome, "registered");
  assert.equal(result.screenshot, null, "a success is not screenshotted: the page shows the key");

  // What the venue received is the identity, typed, and a password the job made.
  assert.equal(posted.length, 1, "exactly one submission");
  const form = posted[0];
  assert.equal(form.name, identity.legal_name);
  assert.equal(form.email, identity.contact_email);
  assert.equal(form.terms, "on", "the approved terms box was ticked");
  assert.equal(form.password.length, 32);
  assert.equal(form.confirm, form.password);

  // What Secret Manager received is exactly what the venue got, by digest.
  assert.deepEqual(result.wrote, ["mock-venue-password", "mock-venue-api-key"]);
  assert.equal(digestFor("mock-venue-password"), sha256(form.password), "the password in the slot is the one the venue accepted");
  assert.equal(digestFor("mock-venue-api-key"), sha256(apiKeyShown), "the key in the slot is the one the page showed");

  // And neither value appears in anything the job said.
  const said = lines.join("\n") + JSON.stringify(result);
  assert.ok(!said.includes(form.password), "the password was printed");
  assert.ok(!said.includes(apiKeyShown), "the API key was printed");
});

test("a captcha on the form is a hand-back with a screenshot, before anything is typed or sent", async () => {
  posted.length = 0;
  const { result } = await run(recipe("captcha"));
  assert.equal(result.code, EXIT.hard_stop, JSON.stringify(result));
  assert.equal(result.outcome, "captcha");
  assert.match(result.reason, /captcha or bot challenge/);
  assert.ok(result.screenshot && existsSync(result.screenshot), `screenshot at ${result.screenshot}`);
  assert.ok(readFileSync(result.screenshot).subarray(1, 4).equals(Buffer.from("PNG")), "the screenshot is a PNG");
  assert.equal(posted.length, 0, "nothing was submitted");
  assert.deepEqual(result.wrote, [], "nothing was written: no account exists");
  assert.equal(result.submitted, false);
});

test("a tax-id field is a hand-back naming the field, before anything is sent", async () => {
  posted.length = 0;
  const { result } = await run(recipe("tax"));
  assert.equal(result.code, EXIT.hard_stop, JSON.stringify(result));
  assert.equal(result.outcome, "identity_or_tax_field");
  assert.match(result.reason, /name="tax_id"/);
  assert.match(result.reason, /Tax identification number/);
  assert.equal(posted.length, 0);
  assert.deepEqual(result.wrote, []);
});

test("a field the recipe does not list is a hand-back naming it, not a guess at what to type", async () => {
  posted.length = 0;
  const { result } = await run(recipe("extra"));
  assert.equal(result.code, EXIT.hard_stop, JSON.stringify(result));
  assert.equal(result.outcome, "unexpected_field");
  assert.match(result.reason, /name="referral_code"/);
  assert.match(result.reason, /1 field\(s\) the recipe does not list/);
  assert.equal(posted.length, 0);
  assert.deepEqual(result.wrote, []);
});

test("a verification-code prompt after submit is handed back with the password already in its slot", async () => {
  // Alpaca's recipe declares this path: the account is created, the code
  // is the operator's. Losing the password here would leave an account
  // nobody can enter.
  posted.length = 0;
  rmSync(join(gcloudDir, "mock-venue-password.sha256"), { force: true });
  const { result } = await run(recipe("verify", { after_submit: "email_verification", success: undefined }), approval({ secret_slots: { password: "mock-venue-password" } }));
  assert.equal(result.code, EXIT.hard_stop, JSON.stringify(result));
  assert.equal(result.outcome, "verification_code");
  assert.match(result.reason, /verification or second-factor code/);
  assert.match(result.reason, /The recipe declares this step/);
  assert.equal(result.submitted, true);
  assert.equal(posted.length, 1);
  assert.deepEqual(result.wrote, ["mock-venue-password"]);
  assert.equal(digestFor("mock-venue-password"), sha256(posted[0].password));
  assert.ok(existsSync(result.screenshot));
});

// ---------------------------------------------------------------------------
// Refusals that need no browser
// ---------------------------------------------------------------------------

test("a missing, blank, or expired approval is refused, and a fresh one is accepted", () => {
  const r = recipe("clean");
  // Premise: a fresh approval loads.
  const fresh = loadApproval("/fresh.json", r, () => JSON.stringify(approval()));
  assert.equal(fresh.ok, true, JSON.stringify(fresh));

  const missing = loadApproval("/nowhere.json", r, () => {
    throw new Error("ENOENT");
  });
  assert.equal(missing.ok, false);
  assert.equal(missing.code, EXIT.approval_refused);
  assert.match(missing.reason, /cannot be read/);

  const blank = loadApproval("/blank.json", r, () => "  \n");
  assert.equal(blank.code, EXIT.approval_refused);
  assert.match(blank.reason, /blank/);

  const absent = loadApproval(null, r, () => "");
  assert.equal(absent.code, EXIT.usage);

  const now = Date.now();
  const stale = approvalProblems(approval({ terms_read_at: new Date(now - 25 * 3_600_000).toISOString() }), r, now);
  assert.ok(stale.some((p) => p.includes("25 hours old")), JSON.stringify(stale));
  const nearlyStale = approvalProblems(approval({ terms_read_at: new Date(now - 23 * 3_600_000).toISOString() }), r, now);
  assert.deepEqual(nearlyStale, [], "23 hours is inside the window");

  const nobody = approvalProblems(approval({ operator: "  " }), r, now);
  assert.ok(nobody.some((p) => p.includes("'operator' is blank")), JSON.stringify(nobody));
  const otherTerms = approvalProblems(approval({ terms: "https://venue.test/other-terms" }), r, now);
  assert.ok(otherTerms.some((p) => p.includes("ticks no box the operator did not accept")), JSON.stringify(otherTerms));
  const otherSource = approvalProblems(approval({ source_id: "other-feed" }), r, now);
  assert.ok(otherSource.some((p) => p.includes("'source_id' is 'other-feed'")), JSON.stringify(otherSource));
});

test("an identity file carrying a key-shaped or tax-id-shaped value is refused without echoing it", () => {
  // Premise: the clean identity passes.
  assert.deepEqual(identityProblems(identity), []);
  const cases = [
    ["address", `AKIA${"A".repeat(16)}`, "AWS access key id"],
    ["legal_name", "Example 123-45-6789 Ltd", "US SSN"],
    ["phone", "12-3456789", "US EIN"],
    ["address", `token ${"a".repeat(40)}`, "key-shaped run"],
  ];
  for (const [key, value, shape] of cases) {
    const problems = identityProblems({ ...identity, [key]: value });
    const hit = problems.find((p) => p.startsWith(`'${key}' looks like`));
    assert.ok(hit && hit.includes(shape), `${key}=${shape}: ${JSON.stringify(problems)}`);
    assert.ok(!problems.join("\n").includes(value), "the refusal echoed the value");
  }
  // A sixth field is refused by name whatever it holds: the job does not
  // trust itself to leave it out of a form.
  const extra = identityProblems({ ...identity, tax_id: "x" });
  assert.ok(extra.some((p) => p.includes("'tax_id' is not one of the five fields")), JSON.stringify(extra));
  // Through the file loader, the refusal has the same shape and the exit code is the identity's.
  const loaded = loadIdentity({ COMPANY_IDENTITY_FILE: "/id.json" }, () => JSON.stringify({ ...identity, address: `AKIA${"B".repeat(16)}` }));
  assert.equal(loaded.ok, false);
  assert.equal(loaded.code, EXIT.identity_refused);
  assert.ok(!loaded.reason.includes("AKIAB"));
  const unset = loadIdentity({}, () => "");
  assert.equal(unset.code, EXIT.identity_refused);
});

test("the kalshi recipe refuses before a browser is launched and says a person must open the account", async () => {
  const loaded = loadRecipe("kalshi");
  assert.equal(loaded.ok, true, loaded.reason);
  assert.equal(loaded.recipe.identity_verification_required, true);
  let launched = false;
  const result = await perform({
    recipe: loaded.recipe,
    identity,
    approval: approval({ source_id: "kalshi-markets" }),
    gcloud: fakeGcloud,
    launchImpl: async () => {
      launched = true;
      throw new Error("a browser was launched");
    },
    log: () => {},
  });
  assert.equal(launched, false, "no browser");
  assert.equal(result.code, EXIT.hard_stop);
  assert.equal(result.outcome, "identity_verification_required");
  assert.match(result.reason, /must be opened by a person/);
  assert.match(result.reason, /No browser was opened/);

  // The command line reaches the same refusal without reading an identity
  // or an approval: nothing in them changes the answer.
  const out = [];
  const code = await main(["--venue", "kalshi", "--approval", "/nowhere.json"], { VENUE_SIGNUP_CHROMIUM: "/nonexistent/chromium", PATH: gcloudDir }, {
    log: (l) => out.push(l),
    out: (l) => out.push(l),
  });
  assert.equal(code, EXIT.hard_stop);
  assert.ok(out.some((l) => l.includes("identity_verification_required")), JSON.stringify(out));
});

test("without gcloud on PATH the job refuses before the venue is touched", async () => {
  assert.equal(findGcloud(gcloudDir), fakeGcloud, "premise: the fake is found when it is on PATH");
  assert.equal(findGcloud("/nonexistent/bin"), null);
  let launched = false;
  const result = await perform({
    recipe: recipe("clean"),
    identity,
    approval: approval(),
    gcloud: null,
    launchImpl: async () => {
      launched = true;
      throw new Error("a browser was launched");
    },
    log: () => {},
  });
  assert.equal(launched, false);
  assert.equal(result.code, EXIT.prerequisite_missing);
  assert.match(result.reason, /gcloud is not on PATH/);
});

test("the committed recipes are valid, alpaca declares e-mail verification as its stop, and its terms box needs the approval to name the same terms", () => {
  const alpaca = loadRecipe("alpaca");
  assert.equal(alpaca.ok, true, alpaca.reason);
  assert.equal(alpaca.recipe.after_submit, "email_verification");
  assert.equal(alpaca.recipe.identity_verification_required, false);
  const termsStep = alpaca.recipe.steps.find((s) => s.field === "accept_terms");
  assert.ok(termsStep, "alpaca's recipe ticks a terms box");
  const problems = approvalProblems(
    { source_id: "alpaca-daily-bars", operator: "d.roderiques", terms_read_at: new Date().toISOString(), terms: "https://elsewhere.test/terms", secret_slots: { password: "alpaca-password" } },
    alpaca.recipe,
  );
  assert.ok(problems.some((p) => p.includes("ticks no box the operator did not accept")), JSON.stringify(problems));

  // A recipe that names an https venue is accepted; plaintext to anywhere
  // but loopback is not.
  assert.ok(recipeProblems({ ...recipe("clean"), signup_url: "http://venue.test/signup" }, "mock").some((p) => p.includes("plaintext")));
  assert.ok(recipeProblems({ ...recipe("clean"), stray: 1 }, "mock").some((p) => p.includes("'stray'")));
});

test("the judge stops on a consent box the recipe does not cover and on one-time-code fields, and passes a form it fully knows", () => {
  const r = recipe("clean");
  const known = { tag: "input", type: "text", name: "name", id: "name", autocomplete: "", inputmode: "", placeholder: "", label: "Full name", knownAs: ["#name"] };
  assert.equal(judge({ captcha: [], text: "Open an account", fields: [known] }, r), null);
  const consent = { ...known, type: "checkbox", name: "marketing", id: "", label: "Send me offers", knownAs: [] };
  assert.equal(judge({ captcha: [], text: "", fields: [known, consent] }, r)?.kind, "unapproved_consent");
  const otp = { ...known, name: "code", id: "", autocomplete: "one-time-code", label: "", knownAs: ["#name"] };
  assert.equal(judge({ captcha: [], text: "", fields: [otp] }, r)?.kind, "verification_code");
  const dob = { ...known, type: "date", name: "birthday", label: "", knownAs: ["#name"] };
  assert.equal(judge({ captcha: [], text: "", fields: [dob] }, r)?.kind, "identity_or_tax_field");
  const upload = { ...known, type: "file", name: "document", label: "", knownAs: ["#name"] };
  assert.equal(judge({ captcha: [], text: "", fields: [upload] }, r)?.kind, "identity_or_tax_field");
  assert.equal(judge({ captcha: [], text: "Please verify you are human to continue", fields: [known] }, r)?.kind, "captcha");
});

test("the browser honours the proxy from the environment and refuses to run with TLS verification off", () => {
  const args = chromiumArguments({ env: { HTTPS_PROXY: "http://proxy.test:3128", NO_PROXY: "localhost" }, profileDir: "/p", uid: 1000 });
  assert.ok(args.includes("--proxy-server=http://proxy.test:3128"), JSON.stringify(args));
  assert.ok(args.includes("--proxy-bypass-list=localhost"));
  assert.ok(!args.includes("--no-sandbox"), "not root, so the sandbox stays");
  assert.ok(!args.some((a) => a.includes("ignore-certificate-errors")), "certificate errors are never ignored");
  assert.equal(launchRefusal({}), null);
  assert.match(launchRefusal({ NODE_TLS_REJECT_UNAUTHORIZED: "0" }), /disables TLS verification/);
});

test("a generated password is long, drawn from every class, and different every time", () => {
  const a = generatePassword();
  const b = generatePassword();
  assert.equal(a.length, 32);
  assert.notEqual(a, b);
  assert.match(a, /[A-Z]/);
  assert.match(a, /[a-z]/);
  assert.match(a, /[0-9]/);
  assert.match(a, /[!#$%&*+\-=?@^_~]/);
});
