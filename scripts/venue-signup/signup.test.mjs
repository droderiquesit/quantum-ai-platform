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
import { inflateSync } from "node:zlib";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  EXIT,
  ambiguousSelectors,
  approvalProblems,
  findGcloud,
  generatePassword,
  identityProblems,
  judge,
  loadApproval,
  loadIdentity,
  loadRecipe,
  main,
  originRefusal,
  perform,
  recipeProblems,
  writeSecret,
} from "./signup.mjs";
import { childEnvironment, chromiumArguments, launch, launchRefusal } from "./browser.mjs";

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
const PROJECT = "mock-venue-project";
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
    // Two elements answering to '#name'. The DOM permits it and venues ship
    // it; a recipe selector that matches both says which element it means to
    // nobody.
    twins: '<label for="name">Trading name</label><input id="name" name="trading_name">',
    // A second consent box beside the approved one. If a recipe's terms
    // selector were broad enough to match it, this box would be ticked
    // without anyone having read what it says.
    consent: '<label><input type="checkbox" name="marketing" class="consent"> Send me offers</label>',
    // The password rendered as text rather than dots: what a venue's
    // show-password control produces, and what makes a screenshot legible.
    visible: "",
  }[variant];
  // The 'visible' variant has no submit control, so the run reaches its
  // hand-back with every field filled — the moment a screenshot would
  // otherwise carry the password.
  const passwordType = variant === "visible" ? "text" : "password";
  const submit = variant === "visible" ? "" : '<button type="submit">Create account</button>';
  const termsClass = variant === "consent" ? ' class="consent"' : "";
  return `<!doctype html><html><head><title>Sign up</title></head><body>
<h1>Open an account</h1>
<form method="post" action="/submit?variant=${variant}">
  <label for="name">Full name</label><input id="name" name="name">
  <label for="email">E-mail</label><input id="email" name="email" type="email">
  <label for="password">Password</label><input id="password" name="password" type="${passwordType}">
  <label for="confirm">Confirm password</label><input id="confirm" name="confirm" type="${passwordType}">
  ${extra}
  <label><input type="checkbox" name="terms"${termsClass}> I accept the terms</label>
  ${submit}
</form></body></html>`;
}

before(async () => {
  mkdirSync(gcloudDir, { recursive: true });
  writeFileSync(
    fakeGcloud,
    [
      "#!/bin/sh",
      '# A stand-in for gcloud: records the slot and a digest of stdin, never the value.',
      '# It refuses the arguments it is not given, so a write that stopped naming',
      '# --project — and would land in whatever the ambient config points at —',
      '# fails every test that writes a secret rather than passing quietly.',
      'if [ "$1 $2 $3" != "secrets versions add" ] || [ "$5" != "--data-file=-" ]; then echo "unexpected arguments: $*" >&2; exit 9; fi',
      `if [ "$6" != "--project=${PROJECT}" ]; then echo "no project named: $*" >&2; exit 9; fi`,
      'if [ -n "$7" ]; then echo "unexpected trailing arguments: $*" >&2; exit 9; fi',
      'sha256sum | cut -d" " -f1 > "$(dirname "$0")/$4.sha256"',
    ].join("\n"),
  );
  chmodSync(fakeGcloud, 0o755);
  server = createServer((req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    // The venue sends the browser to another origin. 'localhost' and
    // '127.0.0.1' are one machine and two origins, which is exactly the
    // distinction the job has to make.
    if (req.method === "GET" && url.pathname === "/signup" && url.searchParams.get("variant") === "offsite") {
      res.writeHead(302, { location: `http://localhost:${server.address().port}/signup?variant=clean` }).end();
      return;
    }
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
    project: PROJECT,
    secret_slots: { password: "mock-venue-password", api_key: "mock-venue-api-key" },
    ...overrides,
  };
}

/**
 * The number of dark pixels in a PNG: how much ink the page drew.
 *
 * The screenshot test needs to assert on what the captured artefact shows,
 * not on whether the job called something. Byte equality cannot do it — two
 * captures of an unchanged page differ, because the viewport settles a few
 * pixels either way and the encoder follows — so the PNG is decoded here with
 * `node:zlib` (no dependency is added for this, and none may be) and its dark
 * pixels counted. Thirty-two characters of password drawn into a field is
 * ~1,500 of them; a field the value was removed from draws none.
 */
function ink(png) {
  let at = 8;
  let width = 0;
  let height = 0;
  let depth = 0;
  let colour = 0;
  let interlace = 0;
  const parts = [];
  while (at + 8 <= png.length) {
    const length = png.readUInt32BE(at);
    const type = png.toString("ascii", at + 4, at + 8);
    const body = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      depth = body[8];
      colour = body[9];
      interlace = body[12];
    } else if (type === "IDAT") parts.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  assert.ok(depth === 8 && [2, 6].includes(colour) && interlace === 0, `unexpected PNG shape: depth ${depth}, colour type ${colour}, interlace ${interlace}`);
  const channels = colour === 6 ? 4 : 3;
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const image = Buffer.alloc(height * stride);
  let dark = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
    const row = image.subarray(y * stride, (y + 1) * stride);
    const prior = y === 0 ? Buffer.alloc(stride) : image.subarray((y - 1) * stride, y * stride);
    for (let x = 0; x < stride; x += 1) {
      const a = x >= channels ? row[x - channels] : 0;
      const b = prior[x];
      const c = x >= channels ? prior[x - channels] : 0;
      const v = line[x];
      let value;
      if (filter === 0) value = v;
      else if (filter === 1) value = v + a;
      else if (filter === 2) value = v + b;
      else if (filter === 3) value = v + ((a + b) >> 1);
      else {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        value = v + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c);
      }
      row[x] = value & 0xff;
    }
    for (let x = 0; x < width; x += 1) {
      const luminance = 0.299 * row[x * channels] + 0.587 * row[x * channels + 1] + 0.114 * row[x * channels + 2];
      if (luminance < 200) dark += 1;
    }
  }
  return dark;
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

test("a recipe selector that matches two elements is a hand-back, and a broad consent selector cannot tick a box nobody read", async () => {
  // The stops this job exists for are evaluated against what the recipe says
  // it knows. A selector matching several elements says it knows all of them,
  // which would leave the two residue stops — a consent box the approval does
  // not cover, and a field nobody reviewed — with nothing to stop on. A hard
  // stop a recipe can pre-empt is not a hard stop.
  posted.length = 0;
  const twins = await run(recipe("twins"));
  assert.equal(twins.result.code, EXIT.hard_stop, JSON.stringify(twins.result));
  assert.equal(twins.result.outcome, "ambiguous_selector");
  assert.match(twins.result.reason, /'#name'/);
  assert.equal(posted.length, 0, "nothing was submitted");
  assert.deepEqual(twins.result.wrote, []);

  // The same, reached the other way: a terms selector broad enough to cover
  // the marketing box beside it. It is refused rather than used, so the
  // second box is never ticked.
  posted.length = 0;
  const broadTerms = recipe("consent", {
    steps: [
      { selector: "#name", field: "legal_name" },
      { selector: "#email", field: "contact_email" },
      { selector: "#password", field: "password" },
      { selector: "#confirm", field: "password_confirm" },
      { selector: "[class='consent']", field: "accept_terms", terms: TERMS },
    ],
  });
  const consent = await run(broadTerms);
  assert.equal(consent.result.outcome, "ambiguous_selector", JSON.stringify(consent.result));
  assert.match(consent.result.reason, /\[class='consent'\]/);
  assert.equal(posted.length, 0, "a form with an unread consent box was submitted");

  // And the static half: a selector naming a kind of element rather than one.
  const bare = recipeProblems({ ...recipe("clean"), steps: [{ selector: "input", field: "legal_name" }] }, "mock");
  assert.ok(bare.some((p) => p.includes("'input' is not anchored")), JSON.stringify(bare));
  const list = recipeProblems({ ...recipe("clean"), submit: "#a, #b" }, "mock");
  assert.ok(list.some((p) => p.includes("is a selector list")), JSON.stringify(list));
  assert.equal(recipeProblems(recipe("clean"), "mock").length, 0, "premise: an anchored recipe is still accepted");

  // The same rule held a second time inside the judge, so that the residue
  // stops cannot be emptied by a broad selector however the job reaches them.
  // The terms step's own selector vouches for the marketing box here; being
  // ambiguous, it vouches for neither.
  const terms = broadTerms.steps.find((s) => s.field === "accept_terms").selector;
  const box = { tag: "input", type: "checkbox", name: "marketing", id: "", autocomplete: "", inputmode: "", placeholder: "", label: "Send me offers", knownAs: [terms] };
  assert.equal(judge({ captcha: [], text: "", fields: [box], ambiguous: [] }, broadTerms), null, "premise: while it vouches, the box is not stopped on");
  assert.equal(judge({ captcha: [], text: "", fields: [box], ambiguous: [terms] }, broadTerms)?.kind, "unapproved_consent");
  const stray = { ...box, type: "text", name: "referral" };
  assert.equal(judge({ captcha: [], text: "", fields: [stray], ambiguous: [terms] }, broadTerms)?.kind, "unexpected_field");
  assert.deepEqual(ambiguousSelectors({ ambiguous: [terms] }, broadTerms), [terms], "the multiplicity gate names the step's selector");
  assert.deepEqual(ambiguousSelectors({ ambiguous: ["#nothing-of-ours"] }, broadTerms), [], "a selector the recipe does not use is not the recipe's problem");
});

test("a redirect to another origin is a hand-back before a single field is filled", async () => {
  // Page.navigate reports success for a redirect, so where the browser ended
  // up is the only evidence of where the typing would go. localhost and
  // 127.0.0.1 are one machine and two origins.
  posted.length = 0;
  const { result } = await run(recipe("offsite"));
  assert.equal(result.code, EXIT.hard_stop, JSON.stringify(result));
  assert.equal(result.outcome, "origin_changed");
  assert.match(result.reason, /http:\/\/localhost:\d+/);
  assert.match(result.reason, /a redirect to another origin is a hand-back/);
  assert.equal(posted.length, 0, "the company's identity was typed into a page nobody reviewed");
  assert.deepEqual(result.wrote, []);

  const r = recipe("clean");
  assert.equal(originRefusal(r.signup_url, r), null, "premise: the recipe's own page is accepted");
  assert.equal(originRefusal(`${baseUrl}/somewhere-else`, r), null, "a different path on the same origin is the same origin");
  // An http downgrade of the same host is a different origin, and a page with
  // no origin at all is one nobody can compare.
  assert.match(originRefusal("http://venue.test/signup", { ...r, signup_url: "https://venue.test/signup" }), /the page is at http:\/\/venue\.test but the mock recipe names https:\/\/venue\.test/);
  assert.match(originRefusal("about:blank", r), /no origin to compare/);
  assert.match(originRefusal("", r), /cannot be established/);
});

test("the hand-back screenshot does not carry the password that was typed into the form", async () => {
  // The job screenshots when it hands back, and after the fill loop the form
  // holds a password this job generated. A capture of that page is the
  // credential, written to a directory whose whole point is that credentials
  // do not go there. The 'visible' variant renders the password as text, as a
  // venue's show-password control does, and has no submit control, so the run
  // hands back with every field filled.
  const shot = async () => {
    const { result } = await run(recipe("visible", { submit: "button[type='submit']", after_submit: "email_verification", success: undefined }), approval({ secret_slots: { password: "mock-venue-password" } }));
    assert.equal(result.outcome, "venue_failed", JSON.stringify(result));
    assert.ok(result.screenshot && existsSync(result.screenshot), `screenshot at ${result.screenshot}`);
    const png = readFileSync(result.screenshot);
    assert.ok(png.subarray(1, 4).equals(Buffer.from("PNG")), "the artefact is a PNG");
    return ink(png);
  };

  // Two references from the same page, captured the same way: what it looks
  // like with the passwords typed in, and what it looks like with the fields
  // empty. A PNG's byte length is not stable across captures — the viewport
  // settles a few pixels either way — so the measure is the artefact's dark
  // pixels, which count the characters actually drawn.
  const browser = await launch({ env: process.env });
  let inkFilled;
  let inkEmpty;
  try {
    await browser.navigate(`${baseUrl}/signup?variant=visible`, 20_000);
    const capture = async (value, name) => {
      await browser.evaluate(
        `(() => {
          document.querySelector("#name").value = ${JSON.stringify(identity.legal_name)};
          document.querySelector("#email").value = ${JSON.stringify(identity.contact_email)};
          const box = document.querySelector("input[name='terms']"); if (!box.checked) box.click();
          for (const id of ["#password", "#confirm"]) document.querySelector(id).value = ${JSON.stringify(value)};
          document.activeElement.blur();
          return "ok";
        })()`,
      );
      return ink(readFileSync(await browser.screenshot(join(scratch, name))));
    };
    inkFilled = await capture(`Aa1!${"q".repeat(28)}`, "reference-filled.png");
    inkEmpty = await capture("", "reference-empty.png");
    assert.ok(inkFilled > inkEmpty, `premise: a capture of this page shows what the password field holds (${inkFilled} vs ${inkEmpty} dark pixels)`);
  } finally {
    await browser.close();
  }

  // The artefact the job actually left behind, twice, with two different
  // generated passwords: as many dark pixels as a form with nothing in those
  // fields, and fewer than one with a password in them. The password was not
  // photographed.
  const first = await shot();
  const second = await shot();
  assert.equal(first, inkEmpty, `the hand-back capture drew more than an empty form (${first} vs ${inkEmpty} dark pixels): the password was in the picture`);
  assert.equal(second, inkEmpty, "the second hand-back capture carried its password");
  assert.ok(first < inkFilled, "the hand-back capture drew as much as a filled form");
});

// ---------------------------------------------------------------------------
// Refusals that need no browser
// ---------------------------------------------------------------------------

test("a secret write names its project and refuses a slot or project it cannot vouch for, and puts neither the value nor a flag on the command line", () => {
  const calls = [];
  const spawn = (bin, args, options) => {
    calls.push({ bin, args, options });
    return { status: 0, stderr: "" };
  };
  // Assembled rather than written out: a quoted run of this length after
  // `secret =` is what the repository's secret scan is looking for, and a
  // fixture that trips the scan costs every later reader the time to work out
  // that it is nothing. Same remedy as `key_shaped()` in
  // qip-data-finder's registration.rs (e71c397).
  const secret = ["not", "a", "real", "value", "just", "a", "test", "string"].join("-");
  const written = writeSecret("/bin/gcloud", PROJECT, "mock-venue-password", secret, spawn);
  assert.equal(written.ok, true, JSON.stringify(written));
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].args, ["secrets", "versions", "add", "mock-venue-password", "--data-file=-", `--project=${PROJECT}`]);
  assert.equal(calls[0].options.input, secret, "the value goes on stdin");
  assert.ok(!calls[0].args.some((a) => a.includes(secret)), "the value reached the command line, where ps shows it to every process on the host");

  // A name the approval let through would still be an argument, so the shape
  // is enforced here, at the one place the argument list is built. Refused,
  // never trimmed into something acceptable.
  for (const slot of ["../../etc/passwd", "--data-file=/etc/passwd", "slot name", "-leading-dash", ""]) {
    const bad = writeSecret("/bin/gcloud", PROJECT, slot, secret, spawn);
    assert.equal(bad.ok, false, `slot ${JSON.stringify(slot)} was accepted`);
    assert.match(bad.reason, /is not a Secret Manager secret name/);
  }
  for (const project of ["", "Mock-Venue", "shrt", "trailing-", "--project=other", undefined]) {
    const bad = writeSecret("/bin/gcloud", project, "mock-venue-password", secret, spawn);
    assert.equal(bad.ok, false, `project ${JSON.stringify(project)} was accepted`);
    assert.match(bad.reason, /is not a Google Cloud project id/);
  }
  assert.equal(calls.length, 1, "a refused name still ran gcloud");

  // And the approval is where the project is named at all: without one, the
  // write would land in whatever the operator's ambient gcloud config points
  // at, which is not a decision anyone reviewed.
  const r = recipe("clean");
  const { project, ...noProject } = approval();
  assert.equal(typeof project, "string", "premise: the fixture approval names a project");
  assert.ok(approvalProblems(noProject, r).some((p) => p.includes("'project' is blank")), JSON.stringify(approvalProblems(noProject, r)));
  assert.ok(approvalProblems(approval({ project: "Not A Project" }), r).some((p) => p.includes("not a Google Cloud project id")));
  assert.deepEqual(approvalProblems(approval(), r), [], "premise: the fixture approval is otherwise accepted");
});

test("the browser is given the variables it needs and not the environment it was launched from", () => {
  // Everything in the launching process's environment is readable by anything
  // that gets code execution in the browser rendering a venue's page, and by
  // every process of the same user through /proc/<pid>/environ. A signup form
  // has no business near a cloud credential path or another tool's token.
  const child = childEnvironment(
    {
      PATH: "/usr/bin",
      HTTPS_PROXY: "http://proxy.test:3128",
      NO_PROXY: "localhost",
      LANG: "en_GB.UTF-8",
      HOME: "/home/operator",
      GOOGLE_APPLICATION_CREDENTIALS: "/home/operator/.config/gcloud/adc.json",
      COMPANY_IDENTITY_FILE: "/run/company/identity.json",
      AWS_SECRET_ACCESS_KEY: "not-a-real-value",
      NODE_TLS_REJECT_UNAUTHORIZED: "1",
    },
    "/tmp/profile-x",
  );
  assert.deepEqual(Object.keys(child).sort(), ["HOME", "HTTPS_PROXY", "LANG", "NO_PROXY", "PATH"].sort(), JSON.stringify(child));
  assert.equal(child.HOME, "/tmp/profile-x", "the profile is the browser's home, so what it writes is what is deleted");
  assert.equal(child.HTTPS_PROXY, "http://proxy.test:3128", "the egress proxy is still honoured");
  assert.equal(child.NODE_TLS_REJECT_UNAUTHORIZED, undefined);
  for (const [key, value] of Object.entries({ GOOGLE_APPLICATION_CREDENTIALS: "adc.json", COMPANY_IDENTITY_FILE: "identity.json", AWS_SECRET_ACCESS_KEY: "not-a-real-value" })) {
    assert.equal(child[key], undefined, `${key} reached the browser`);
    assert.ok(!JSON.stringify(child).includes(value), `${key}'s value reached the browser`);
  }
  // Empty is not a value: an unset proxy stays unset rather than becoming "".
  assert.equal(childEnvironment({ PATH: "/usr/bin", HTTPS_PROXY: "" }, "/p").HTTPS_PROXY, undefined);
});

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
