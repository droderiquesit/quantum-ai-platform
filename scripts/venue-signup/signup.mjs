/**
 * The venue signup job.
 *
 * Performs a venue's signup form under the company's own identity, after a
 * named operator has approved that venue's terms, and stops — by design, not
 * by failure — at every step that must be a person's. It is **dev tooling**
 * for the operator's machine: nothing in `backend/` or `frontend/` imports it
 * and the platform does not depend on it existing.
 *
 * `docs/operations/registering-a-venue.md` says signup may not be automated
 * because the account is a person's agreement with the venue. This job does
 * not change that. The agreement is the operator's approval record — a named
 * person, the terms they read, and when — and the job is the typing. What
 * the runbook lists as a person's act stays a person's act: the job refuses
 * to solve a captcha, to enter an identity document, tax id or date of
 * birth, to read a verification code, to tick a terms box the approval does
 * not name, or to fill a field the reviewed recipe does not list. Each of
 * those is a hand-back with a reason and a screenshot, never a retry.
 *
 * ## Usage
 *
 *   COMPANY_IDENTITY_FILE=/run/company/identity.json \
 *   node scripts/venue-signup/signup.mjs --venue alpaca --approval approval.json
 *
 * The identity file lives outside the repository and holds exactly the five
 * fields a signup form is allowed to receive. The approval record is the
 * platform's `RegistrationRecord` exported as JSON plus the Secret Manager
 * slot names the credentials go to. The password is generated in memory and
 * reaches disk nowhere: it goes to `gcloud secrets versions add` on stdin, as
 * does any API key the venue shows, and neither is ever printed.
 *
 * ## Exit codes
 *
 * Named because the operator's next action depends on which one it was.
 */
import { accessSync, constants, mkdirSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { randomInt } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { screenPayload } from "../model-gateway.mjs";
import { DEFAULT_CHROMIUM, launch, launchRefusal } from "./browser.mjs";

export const EXIT = {
  ok: 0,
  usage: 64,
  recipe_refused: 65,
  identity_refused: 66,
  approval_refused: 67,
  prerequisite_missing: 69,
  hard_stop: 70,
  venue_failed: 71,
  secret_write_failed: 72,
};

const RECIPES_DIR = join(dirname(fileURLToPath(import.meta.url)), "recipes");

/** The five things a signup form may receive. Nothing else is in the identity file. */
export const IDENTITY_FIELDS = ["legal_name", "contact_email", "phone", "address", "country"];

/** What a recipe step may ask the job to fill. */
export const RECIPE_FIELDS = [...IDENTITY_FIELDS, "password", "password_confirm", "accept_terms"];

const RECIPE_KEYS = [
  "venue",
  "source_id",
  "signup_url",
  "identity_verification_required",
  "steps",
  "submit",
  "after_submit",
  "success",
  "notes",
];

const APPROVAL_MAX_AGE_MS = 24 * 60 * 60 * 1000;
const APPROVAL_FUTURE_SKEW_MS = 5 * 60 * 1000;
const SLOT_SHAPE = /^[A-Za-z0-9][A-Za-z0-9_-]{0,254}$/;

/**
 * Value shapes that mean the identity file carries something a signup form
 * must never be handed: a tax id, or a credential. Refused, never stripped,
 * and never echoed — the refusal names the key, not the value.
 */
const TAX_ID_SHAPES = [
  { name: "US SSN", re: /\b\d{3}-\d{2}-\d{4}\b/ },
  { name: "US EIN", re: /\b\d{2}-\d{7}\b/ },
  { name: "nine-digit identifier", re: /\b\d{9}\b/ },
  { name: "UK NINO", re: /\b[A-CEGHJ-PR-TW-Z]{2}\d{6}[A-D]\b/ },
];
const LONG_TOKEN = { name: "key-shaped run", re: /[A-Za-z0-9+/=_-]{32,}/ };

export function screenValue(text) {
  const found = screenPayload(text);
  for (const { name, re } of TAX_ID_SHAPES) if (re.test(text)) found.push(name);
  if (LONG_TOKEN.re.test(text)) found.push(LONG_TOKEN.name);
  return found;
}

// ---------------------------------------------------------------------------
// Recipes
// ---------------------------------------------------------------------------

/** Every reason a recipe is not one this job will run. Empty means it is. */
export function recipeProblems(recipe, venueId) {
  const problems = [];
  if (recipe === null || typeof recipe !== "object" || Array.isArray(recipe)) {
    return ["the recipe is not a JSON object"];
  }
  for (const key of Object.keys(recipe)) {
    if (!RECIPE_KEYS.includes(key)) problems.push(`recipe key '${key}' is not one a recipe may carry; a recipe is reviewed data and a key nobody reviewed is refused`);
  }
  if (recipe.venue !== venueId) problems.push(`the recipe's 'venue' is '${recipe.venue}', not '${venueId}'`);
  if (typeof recipe.source_id !== "string" || recipe.source_id.trim() === "") problems.push("the recipe must name the platform 'source_id' it registers");
  if (typeof recipe.identity_verification_required !== "boolean") {
    problems.push("the recipe must state 'identity_verification_required' as true or false; an unstated requirement is not 'no'");
  }
  let url = null;
  try {
    url = new URL(recipe.signup_url);
  } catch {
    problems.push("the recipe's 'signup_url' is not a URL");
  }
  if (url) {
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) {
      problems.push(`the recipe's 'signup_url' is ${url.protocol.replace(":", "")}, not https; plaintext is permitted only to loopback, which is the test's mock`);
    }
  }
  // A venue that needs a person's documents needs nothing else from a recipe.
  if (recipe.identity_verification_required === true) return problems;

  if (!Array.isArray(recipe.steps) || recipe.steps.length === 0) {
    problems.push("the recipe must list at least one step");
  } else {
    const seen = new Set();
    recipe.steps.forEach((step, index) => {
      if (step === null || typeof step !== "object") return problems.push(`step ${index} is not an object`);
      const keys = Object.keys(step).filter((k) => !["selector", "field", "terms"].includes(k));
      if (keys.length) problems.push(`step ${index} carries keys a step may not: ${keys.join(", ")}`);
      if (typeof step.selector !== "string" || step.selector.trim() === "") problems.push(`step ${index} has no selector`);
      else if (seen.has(step.selector)) problems.push(`step ${index} repeats selector '${step.selector}'`);
      else seen.add(step.selector);
      if (!RECIPE_FIELDS.includes(step.field)) problems.push(`step ${index} fills '${step.field}', which is not one of ${RECIPE_FIELDS.join(", ")}`);
      if (step.field === "accept_terms" && (typeof step.terms !== "string" || step.terms.trim() === "")) {
        problems.push(`step ${index} ticks a terms box but does not cite which terms; the approval record must name the same reference`);
      }
      if (step.field !== "accept_terms" && step.terms !== undefined) problems.push(`step ${index} cites terms but is not an accept_terms step`);
    });
  }
  if (typeof recipe.submit !== "string" || recipe.submit.trim() === "") problems.push("the recipe must name the submit control's selector");
  if (!["success", "email_verification"].includes(recipe.after_submit)) {
    problems.push("the recipe must say what follows submit: 'success' or 'email_verification' (a hard stop the job hands back at)");
  }
  if (recipe.after_submit === "success") {
    if (recipe.success === null || typeof recipe.success !== "object" || typeof recipe.success.selector !== "string") {
      problems.push("a recipe whose submit leads to success must give 'success.selector'");
    } else if (recipe.success.api_key_selector !== undefined && typeof recipe.success.api_key_selector !== "string") {
      problems.push("'success.api_key_selector' must be a selector string when present");
    }
  } else if (recipe.success !== undefined && recipe.success !== null) {
    problems.push("a recipe that stops at email verification cannot also describe a success page it never reaches");
  }
  return problems;
}

export function loadRecipe(venueId, readFile = (p) => readFileSync(p, "utf8"), recipesDir = RECIPES_DIR) {
  if (typeof venueId !== "string" || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(venueId)) {
    return { ok: false, code: EXIT.usage, reason: "--venue must be a lowercase recipe id such as 'alpaca'" };
  }
  const path = join(recipesDir, `${venueId}.json`);
  let text;
  try {
    text = readFile(path);
  } catch {
    return { ok: false, code: EXIT.recipe_refused, reason: `no recipe for '${venueId}' at ${path}; recipes are committed, reviewed data and are not generated` };
  }
  let recipe;
  try {
    recipe = JSON.parse(text);
  } catch {
    return { ok: false, code: EXIT.recipe_refused, reason: `${path} is not valid JSON` };
  }
  const problems = recipeProblems(recipe, venueId);
  if (problems.length) return { ok: false, code: EXIT.recipe_refused, reason: `recipe ${path} refused:\n  - ${problems.join("\n  - ")}` };
  return { ok: true, recipe, path };
}

// ---------------------------------------------------------------------------
// The company identity
// ---------------------------------------------------------------------------

/** Every reason the identity file may not be typed into a form. Values are never repeated. */
export function identityProblems(identity) {
  if (identity === null || typeof identity !== "object" || Array.isArray(identity)) return ["the identity file is not a JSON object"];
  const problems = [];
  for (const key of Object.keys(identity)) {
    if (!IDENTITY_FIELDS.includes(key)) {
      problems.push(`'${key}' is not one of the five fields a signup form may receive (${IDENTITY_FIELDS.join(", ")}); remove it rather than trusting the job to leave it out`);
    }
  }
  for (const key of IDENTITY_FIELDS) {
    const value = identity[key];
    if (typeof value !== "string" || value.trim() === "") {
      problems.push(`'${key}' is missing or blank`);
      continue;
    }
    const shapes = screenValue(value);
    if (shapes.length) problems.push(`'${key}' looks like ${shapes.join(", ")}; the identity file carries no secret and no tax id`);
  }
  if (typeof identity.contact_email === "string" && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(identity.contact_email.trim())) {
    problems.push("'contact_email' is not an e-mail address");
  }
  if (typeof identity.country === "string" && !/^[A-Z]{2}$/.test(identity.country.trim())) {
    problems.push("'country' must be an ISO 3166-1 alpha-2 code such as US, so a form's country control is matched and not guessed");
  }
  return problems;
}

export function loadIdentity(env, readFile = (p) => readFileSync(p, "utf8")) {
  const path = env.COMPANY_IDENTITY_FILE?.trim();
  if (!path) return { ok: false, code: EXIT.identity_refused, reason: "COMPANY_IDENTITY_FILE is not set; it names a JSON file outside the repository with legal_name, contact_email, phone, address, country" };
  let text;
  try {
    text = readFile(path);
  } catch {
    return { ok: false, code: EXIT.identity_refused, reason: `COMPANY_IDENTITY_FILE points at a file that cannot be read: ${path}` };
  }
  let identity;
  try {
    identity = JSON.parse(text);
  } catch {
    return { ok: false, code: EXIT.identity_refused, reason: `${path} is not valid JSON` };
  }
  const problems = identityProblems(identity);
  if (problems.length) return { ok: false, code: EXIT.identity_refused, reason: `identity file refused:\n  - ${problems.join("\n  - ")}` };
  const clean = {};
  for (const key of IDENTITY_FIELDS) clean[key] = identity[key].trim();
  return { ok: true, identity: clean };
}

// ---------------------------------------------------------------------------
// The approval record
// ---------------------------------------------------------------------------

/**
 * Every reason the approval does not authorise this signup. Empty means it does.
 *
 * `now` is a parameter so the 24-hour window is a fact the test can move.
 */
export function approvalProblems(approval, recipe, now = Date.now()) {
  if (approval === null || typeof approval !== "object" || Array.isArray(approval)) return ["the approval record is not a JSON object"];
  const problems = [];
  const text = (key) => (typeof approval[key] === "string" ? approval[key].trim() : "");
  if (text("source_id") === "") problems.push("'source_id' is blank; the approval must name the source it registers");
  else if (approval.source_id !== recipe.source_id) problems.push(`'source_id' is '${approval.source_id}' but the ${recipe.venue} recipe registers '${recipe.source_id}'`);
  if (text("operator") === "") problems.push("'operator' is blank; a signup nobody approved is the anonymous one the platform refuses");
  if (text("terms") === "") problems.push("'terms' is blank; the approval must cite the terms the operator read");
  const readAt = Date.parse(text("terms_read_at"));
  if (text("terms_read_at") === "" || Number.isNaN(readAt)) {
    problems.push("'terms_read_at' is missing or not an RFC 3339 instant");
  } else if (now - readAt > APPROVAL_MAX_AGE_MS) {
    problems.push(`'terms_read_at' is ${Math.floor((now - readAt) / 3_600_000)} hours old; an approval is good for 24 hours, so the operator must re-read the terms and approve again`);
  } else if (readAt - now > APPROVAL_FUTURE_SKEW_MS) {
    problems.push("'terms_read_at' is in the future");
  }
  for (const key of ["operator", "terms", "source_id"]) {
    const shapes = typeof approval[key] === "string" ? screenValue(approval[key]) : [];
    if (shapes.length) problems.push(`'${key}' looks like ${shapes.join(", ")}; an approval record names things and holds no value`);
  }
  const slots = approval.secret_slots;
  if (slots === null || typeof slots !== "object" || Array.isArray(slots)) {
    problems.push("'secret_slots' must be an object naming the Secret Manager secret for 'password' (and 'api_key' when the recipe reads one)");
  } else {
    for (const key of Object.keys(slots)) {
      if (!["password", "api_key"].includes(key)) problems.push(`'secret_slots.${key}' is not a slot this job writes`);
    }
    if (typeof slots.password !== "string" || !SLOT_SHAPE.test(slots.password)) {
      problems.push("'secret_slots.password' must be a Secret Manager secret name (letters, digits, '-' and '_')");
    }
    const readsKey = recipe.success?.api_key_selector !== undefined;
    if (readsKey && (typeof slots.api_key !== "string" || !SLOT_SHAPE.test(slots.api_key))) {
      problems.push("the recipe reads an API key on success, so 'secret_slots.api_key' must name where it goes");
    }
    if (!readsKey && slots.api_key !== undefined) problems.push("'secret_slots.api_key' is named but the recipe shows no API key; remove one or the other");
  }
  if (Array.isArray(recipe.steps)) {
    for (const step of recipe.steps) {
      if (step.field === "accept_terms" && step.terms !== approval.terms) {
        problems.push(`the recipe ticks a box accepting '${step.terms}' and the approval names '${approval.terms ?? ""}'; the job ticks no box the operator did not accept`);
      }
    }
  }
  return problems;
}

export function loadApproval(path, recipe, readFile = (p) => readFileSync(p, "utf8"), now = Date.now()) {
  if (!path) return { ok: false, code: EXIT.usage, reason: "--approval <path> is required: the operator's registration approval exported as JSON" };
  let text;
  try {
    text = readFile(path);
  } catch {
    return { ok: false, code: EXIT.approval_refused, reason: `approval record cannot be read: ${path}` };
  }
  if (text.trim() === "") return { ok: false, code: EXIT.approval_refused, reason: `approval record is blank: ${path}` };
  let approval;
  try {
    approval = JSON.parse(text);
  } catch {
    return { ok: false, code: EXIT.approval_refused, reason: `${path} is not valid JSON` };
  }
  const problems = approvalProblems(approval, recipe, now);
  if (problems.length) return { ok: false, code: EXIT.approval_refused, reason: `approval record refused:\n  - ${problems.join("\n  - ")}` };
  return { ok: true, approval };
}

// ---------------------------------------------------------------------------
// Secrets: generated in memory, written to Secret Manager, never elsewhere
// ---------------------------------------------------------------------------

const PASSWORD_CLASSES = ["ABCDEFGHJKLMNPQRSTUVWXYZ", "abcdefghijkmnopqrstuvwxyz", "23456789", "!#$%&*+-=?@^_~"];

/** 32 characters, at least one from each class, from the CSPRNG. Held in memory only. */
export function generatePassword(length = 32) {
  const alphabet = PASSWORD_CLASSES.join("");
  for (;;) {
    let out = "";
    for (let i = 0; i < length; i += 1) out += alphabet[randomInt(alphabet.length)];
    if (PASSWORD_CLASSES.every((cls) => [...out].some((c) => cls.includes(c)))) return out;
  }
}

/** The `gcloud` on PATH, or null. Looked up by PATH only: there is no override, so a secret goes to gcloud or nowhere. */
export function findGcloud(pathVariable = process.env.PATH ?? "") {
  for (const dir of pathVariable.split(":").filter(Boolean)) {
    const candidate = join(dir, "gcloud");
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      // Not here; keep looking.
    }
  }
  return null;
}

/**
 * `gcloud secrets versions add <slot> --data-file=-`, value on stdin.
 *
 * stdout is discarded and stderr is kept only for the failure message;
 * neither is where the value could appear, but the argument list is the
 * place it must never be — an argument is in `ps` for every user.
 */
export function writeSecret(gcloud, slot, value, spawn = spawnSync) {
  if (!SLOT_SHAPE.test(slot)) return { ok: false, reason: `slot '${slot}' is not a Secret Manager secret name` };
  const result = spawn(gcloud, ["secrets", "versions", "add", slot, "--data-file=-"], {
    input: value,
    stdio: ["pipe", "ignore", "pipe"],
    timeout: 60_000,
  });
  if (result.error) return { ok: false, reason: `gcloud could not be run: ${result.error.message}` };
  if (result.status !== 0) {
    const detail = String(result.stderr ?? "").trim().split("\n").pop() ?? "";
    return { ok: false, reason: `gcloud secrets versions add ${slot} exited ${result.status}: ${detail}` };
  }
  return { ok: true };
}

// ---------------------------------------------------------------------------
// The page: what is on it, and whether any of it is a person's step
// ---------------------------------------------------------------------------

const CAPTCHA_SELECTORS = [
  ".g-recaptcha",
  ".h-captcha",
  ".cf-turnstile",
  "[data-sitekey]",
  "#cf-challenge-running",
  "#challenge-form",
  "iframe[src*='recaptcha']",
  "iframe[src*='hcaptcha']",
  "iframe[src*='turnstile']",
  "iframe[src*='captcha']",
  "iframe[src*='challenges.cloudflare.com']",
  "[class*='captcha']",
  "[id*='captcha']",
  "input[name*='captcha']",
];
const CAPTCHA_TEXT = /i'?m not a robot|verify (that )?you are (a )?human|are you a (human|robot)|complete the (captcha|challenge|security check)|checking your browser/i;

const SENSITIVE_FIELD = /\b(ssn|social security|tax(payer)? ?(id|number|identification)?|tin|ein|itin|vat|passport|driver'?s? licen[cs]e|national id|id number|identity (document|card|number)|document (number|upload)|government id|selfie|date of birth|birth ?date|dob|bday|birthday)\b/i;
const VERIFICATION_FIELD = /\b(one[- ]time|otp|verification code|verify code|confirmation code|security code|2fa|two[- ]factor|authenticator|code we (sent|emailed)|the code)\b/i;
const VERIFICATION_TEXT = /enter the (\d[- ]digit )?code|verification code|check your (e-?mail|inbox)|we('ve| have)? sent (you )?(a|an|the) (code|e-?mail|link)|confirm your e-?mail|verify your e-?mail/i;

/**
 * The script that inventories a page. Runs inside the browser and returns
 * plain data; the judgement is made here, in Node, where it can be tested
 * without a browser.
 */
export function inventoryScript(knownSelectors) {
  return `(() => {
    const known = ${JSON.stringify(knownSelectors)};
    const captchaSelectors = ${JSON.stringify(CAPTCHA_SELECTORS)};
    const squash = (s) => (s || "").replace(/\\s+/g, " ").trim();
    const visible = (el) => {
      const style = getComputedStyle(el);
      if (style.display === "none" || style.visibility === "hidden") return false;
      const box = el.getBoundingClientRect();
      return box.width > 0 || box.height > 0;
    };
    const labelOf = (el) => {
      let text = "";
      if (el.id) { const l = document.querySelector('label[for="' + CSS.escape(el.id) + '"]'); if (l) text = l.innerText; }
      if (!text) { const p = el.closest("label"); if (p) text = p.innerText; }
      if (!text) text = el.getAttribute("aria-label") || "";
      return squash(text).slice(0, 120);
    };
    const fields = [];
    for (const el of document.querySelectorAll("input, select, textarea")) {
      const type = (el.getAttribute("type") || (el.tagName === "INPUT" ? "text" : el.tagName.toLowerCase())).toLowerCase();
      if (["hidden", "submit", "button", "reset", "image"].includes(type)) continue;
      if (!visible(el)) continue;
      fields.push({
        tag: el.tagName.toLowerCase(),
        type,
        name: el.getAttribute("name") || "",
        id: el.id || "",
        autocomplete: el.getAttribute("autocomplete") || "",
        inputmode: el.getAttribute("inputmode") || "",
        placeholder: el.getAttribute("placeholder") || "",
        label: labelOf(el),
        knownAs: known.filter((s) => { try { return el.matches(s); } catch { return false; } }),
      });
    }
    const captcha = [];
    for (const s of captchaSelectors) { if (document.querySelector(s)) captcha.push(s); }
    const text = squash(document.body ? document.body.innerText : "").slice(0, 20000);
    return { url: location.href, title: document.title, text, fields, captcha };
  })()`;
}

/**
 * The first reason the page is a person's to continue, or null.
 *
 * Order is by how certain the signal is: a captcha element is unambiguous; a
 * field name is the venue's own word for what it wants; a consent box not
 * in the recipe and a field not in the recipe are the residue. Every one is
 * a stop, none is solved, and the reason names what the operator will find.
 */
export function judge(inventory, recipe) {
  if (inventory.captcha.length || CAPTCHA_TEXT.test(inventory.text)) {
    const what = inventory.captcha[0] ?? "page text asking for a human check";
    return { kind: "captcha", reason: `the page presents a captcha or bot challenge (${what}); telling a venue a person is present is that person's act` };
  }
  const describe = (f) => `${f.tag}[type=${f.type}]${f.name ? ` name="${f.name}"` : ""}${f.id ? ` id="${f.id}"` : ""}${f.label ? ` labelled "${f.label}"` : ""}`;
  for (const f of inventory.fields) {
    const words = [f.name, f.id, f.autocomplete, f.placeholder, f.label].join(" ");
    if (f.type === "file" || /^bday/.test(f.autocomplete) || f.type === "date" || SENSITIVE_FIELD.test(words)) {
      return { kind: "identity_or_tax_field", reason: `the form asks for an identity document, tax id or date of birth (${describe(f)}); those are a person's documents and a person's to give` };
    }
  }
  for (const f of inventory.fields) {
    const words = [f.name, f.id, f.autocomplete, f.placeholder, f.label].join(" ");
    if (f.autocomplete === "one-time-code" || VERIFICATION_FIELD.test(words)) {
      return { kind: "verification_code", reason: `the page asks for a verification or second-factor code (${describe(f)}); the job reads no mail and polls nothing, so entering it is the operator's step` };
    }
  }
  if (inventory.fields.length === 0 && VERIFICATION_TEXT.test(inventory.text)) {
    return { kind: "verification_code", reason: "the page says to check e-mail or enter a code and offers no field the recipe knows; the job reads no mail, so that step is the operator's" };
  }
  const steps = Array.isArray(recipe.steps) ? recipe.steps : [];
  for (const f of inventory.fields) {
    if (f.type !== "checkbox") continue;
    const step = steps.find((s) => f.knownAs.includes(s.selector));
    if (!step || step.field !== "accept_terms") {
      return { kind: "unapproved_consent", reason: `the form has a consent box the approval does not cover (${describe(f)}); the job ticks nothing the operator has not read` };
    }
  }
  const unexpected = inventory.fields.filter((f) => f.knownAs.length === 0);
  if (unexpected.length) {
    return { kind: "unexpected_field", reason: `the form has ${unexpected.length} field(s) the recipe does not list: ${unexpected.map(describe).join("; ")}. A field nobody reviewed is not filled; extend the recipe if it should be` };
  }
  return null;
}

function fillScript(selector, value, kind) {
  return `((selector, value, kind) => {
    const el = document.querySelector(selector);
    if (!el) return "missing";
    if (kind === "checkbox") { if (!el.checked) el.click(); return el.checked ? "ok" : "not ticked"; }
    if (el.tagName === "SELECT") {
      const options = [...el.options];
      const hit = options.find((o) => o.value === value) || options.find((o) => o.text.trim().toUpperCase() === value.toUpperCase());
      if (!hit) return "no option matches";
      el.value = hit.value;
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return "ok";
    }
    const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    el.focus();
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return el.value === value ? "ok" : "value did not take";
  })(${JSON.stringify(selector)}, ${JSON.stringify(value)}, ${JSON.stringify(kind)})`;
}

function clickScript(selector) {
  return `(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return "missing"; el.click(); return "ok"; })()`;
}

function successScript(success) {
  return `(() => {
    const done = document.querySelector(${JSON.stringify(success.selector)});
    if (!done) return { reached: false };
    const keyEl = ${success.api_key_selector === undefined ? "null" : `document.querySelector(${JSON.stringify(success.api_key_selector)})`};
    return { reached: true, apiKey: keyEl ? (keyEl.value || keyEl.textContent || "").trim() : null };
  })()`;
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Run one signup. Returns `{ code, outcome, reason?, screenshot?, wrote, submitted }`
 * and never throws for a venue's behaviour — a page it cannot proceed on is
 * an outcome, not an exception.
 *
 * `launchImpl` and `gcloud` are injected so the test can point the first at
 * a mock page and the second at a fake on PATH; nothing else is.
 */
export async function perform({
  recipe,
  identity,
  approval,
  env = process.env,
  gcloud = findGcloud(env.PATH),
  launchImpl = launch,
  executablePath = env.VENUE_SIGNUP_CHROMIUM?.trim() || DEFAULT_CHROMIUM,
  scratchDir = env.VENUE_SIGNUP_SCRATCH_DIR?.trim() || join(tmpdir(), "venue-signup"),
  budgetMs = 90_000,
  log = (line) => process.stderr.write(`${line}\n`),
  now = () => Date.now(),
}) {
  const wrote = [];
  const started = now();
  const deadline = started + budgetMs;

  if (recipe.identity_verification_required) {
    return {
      code: EXIT.hard_stop,
      outcome: "identity_verification_required",
      submitted: false,
      wrote,
      reason:
        `${recipe.venue} requires the venue's identity verification to open an account, and the recipe says so. ` +
        "This account must be opened by a person — the operator, with their own documents — and nothing here " +
        "will do that step (docs/operations/registering-a-venue.md). No browser was opened.",
    };
  }
  const refusal = launchRefusal(env);
  if (refusal) return { code: EXIT.prerequisite_missing, outcome: "refused", submitted: false, wrote, reason: refusal };
  if (!gcloud) {
    return {
      code: EXIT.prerequisite_missing,
      outcome: "refused",
      submitted: false,
      wrote,
      reason: "gcloud is not on PATH, so a password the venue accepted would have nowhere to go; install the Google Cloud SDK and authenticate before the venue is touched",
    };
  }

  mkdirSync(scratchDir, { recursive: true });
  const stamp = new Date(started).toISOString().replace(/[:.]/g, "-");
  const screenshotPath = (kind) => join(scratchDir, `${recipe.venue}-${stamp}-${kind}.png`);

  const password = generatePassword();
  const values = {
    ...identity,
    password,
    password_confirm: password,
  };

  let browser;
  try {
    browser = await launchImpl({ executablePath, env, callTimeoutMs: Math.min(30_000, budgetMs) });
  } catch (cause) {
    return { code: EXIT.prerequisite_missing, outcome: "refused", submitted: false, wrote, reason: cause.message };
  }
  log(`browser ${browser.product}; opening ${recipe.signup_url}`);

  const knownSelectors = [...recipe.steps.map((s) => s.selector), recipe.submit];
  let submitted = false;

  async function handBack(kind, reason) {
    let screenshot = null;
    try {
      screenshot = await browser.screenshot(screenshotPath(kind));
    } catch (cause) {
      log(`screenshot failed: ${cause.message}`);
    }
    // A submitted form may have created the account. The password is then
    // the only way into it, so it goes to its slot before the hand-back —
    // losing it would leave an account nobody can enter, which is worse than
    // a secret version nobody needs.
    if (submitted && !wrote.includes(approval.secret_slots.password)) {
      const written = writeSecret(gcloud, approval.secret_slots.password, password);
      if (written.ok) wrote.push(approval.secret_slots.password);
      else reason += `. And the password could not be stored (${written.reason}); the account, if the venue created it, cannot be entered and must be recovered by the operator`;
    }
    await browser.close();
    return { code: kind === "venue_failed" ? EXIT.venue_failed : EXIT.hard_stop, outcome: kind, reason, screenshot, submitted, wrote };
  }

  try {
    try {
      await browser.navigate(recipe.signup_url, Math.min(30_000, deadline - now()));
    } catch (cause) {
      return await handBack("venue_failed", `the signup page could not be loaded: ${cause.message}`);
    }

    const inventory = await browser.evaluate(inventoryScript(knownSelectors));
    if (!inventory.ready) return await handBack("venue_failed", "the signup page had no document to inspect after loading");
    const stop = judge(inventory.value, recipe);
    if (stop) return await handBack(stop.kind, stop.reason);

    for (const step of recipe.steps) {
      const kind = step.field === "accept_terms" ? "checkbox" : "text";
      // The approval already matched this step's terms reference; this is
      // the belt to that brace, in the one place the box is actually ticked.
      if (kind === "checkbox" && step.terms !== approval.terms) {
        return await handBack("unapproved_consent", `the recipe ticks '${step.terms}', which the approval does not name`);
      }
      const value = kind === "checkbox" ? "" : values[step.field];
      const filled = await browser.evaluate(fillScript(step.selector, value, kind));
      if (!filled.ready || filled.value !== "ok") {
        return await handBack("venue_failed", `filling ${step.field} at '${step.selector}' failed: ${filled.value ?? "page navigated"}. The recipe does not match the page the venue served; review it before running again`);
      }
      log(`filled ${step.field}`);
    }

    const clicked = await browser.evaluate(clickScript(recipe.submit));
    if (!clicked.ready || clicked.value !== "ok") {
      return await handBack("venue_failed", `the submit control '${recipe.submit}' was not found`);
    }
    submitted = true;
    log("submitted; waiting for the venue");

    while (now() < deadline) {
      await sleep(250);
      const after = await browser.evaluate(inventoryScript(knownSelectors));
      if (!after.ready) continue;
      // The same form, still there with nothing else: the venue has not
      // answered yet, or answered with a message the inventory cannot see.
      const stillTheForm = after.value.url === inventory.value.url && after.value.fields.every((f) => f.knownAs.length > 0) && after.value.fields.length === inventory.value.fields.length;
      const stopAfter = judge(after.value, recipe);
      if (stopAfter) {
        const expected = recipe.after_submit === "email_verification" && stopAfter.kind === "verification_code";
        return await handBack(stopAfter.kind, expected ? `${stopAfter.reason}. The recipe declares this step; the account, if created, is now the operator's to verify` : stopAfter.reason);
      }
      if (recipe.success) {
        const done = await browser.evaluate(successScript(recipe.success));
        if (done.ready && done.value?.reached) {
          const slots = approval.secret_slots;
          const wrotePassword = writeSecret(gcloud, slots.password, password);
          if (!wrotePassword.ok) {
            await browser.close();
            return { code: EXIT.secret_write_failed, outcome: "secret_write_failed", submitted, wrote, reason: `the venue accepted the signup but the password could not be stored: ${wrotePassword.reason}` };
          }
          wrote.push(slots.password);
          if (recipe.success.api_key_selector !== undefined) {
            const apiKey = done.value.apiKey;
            if (!apiKey) {
              await browser.close();
              return { code: EXIT.venue_failed, outcome: "venue_failed", submitted, wrote, reason: `the success page showed no API key at '${recipe.success.api_key_selector}'; the password is stored and the key is the operator's to create in the dashboard` };
            }
            const wroteKey = writeSecret(gcloud, slots.api_key, apiKey);
            if (!wroteKey.ok) {
              await browser.close();
              return { code: EXIT.secret_write_failed, outcome: "secret_write_failed", submitted, wrote, reason: `the API key the venue showed could not be stored: ${wroteKey.reason}` };
            }
            wrote.push(slots.api_key);
          }
          await browser.close();
          return { code: EXIT.ok, outcome: "registered", submitted, wrote, reason: null, screenshot: null };
        }
      }
      if (!stillTheForm && after.value.fields.length === 0 && !recipe.success) {
        // Declared to stop at e-mail verification, and the page after submit
        // has no field and no prompt the judge recognised. Hand it back as
        // what it is: something a person must read.
        return await handBack("verification_code", "the page after submit has no field the recipe knows; the recipe declares e-mail verification follows, and that step is the operator's");
      }
    }
    return await handBack("venue_failed", `the venue did not reach a state the recipe recognises within ${Math.round(budgetMs / 1000)} s of submitting; whether the account was created is unknown`);
  } catch (cause) {
    return await handBack("venue_failed", `the browser session failed: ${cause.message}`);
  }
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

export function parseArguments(args) {
  const out = { venue: null, approval: null, budgetSeconds: 90, problems: [] };
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    const next = () => {
      const v = args[i + 1];
      if (v === undefined || v.startsWith("--")) {
        out.problems.push(`${arg} needs a value`);
        return null;
      }
      i += 1;
      return v;
    };
    if (arg === "--venue") out.venue = next();
    else if (arg === "--approval") out.approval = next();
    else if (arg === "--budget-seconds") {
      const v = Number(next());
      if (!Number.isFinite(v) || v <= 0 || v > 600) out.problems.push("--budget-seconds must be between 1 and 600");
      else out.budgetSeconds = v;
    } else out.problems.push(`unknown argument ${arg}`);
  }
  if (!out.venue) out.problems.push("--venue <id> is required");
  if (!out.approval) out.problems.push("--approval <path> is required");
  return out;
}

export async function main(argv, env = process.env, deps = {}) {
  const log = deps.log ?? ((line) => process.stderr.write(`${line}\n`));
  const out = deps.out ?? ((line) => process.stdout.write(`${line}\n`));
  const args = parseArguments(argv);
  if (args.problems.length) {
    log("usage: COMPANY_IDENTITY_FILE=<path> node scripts/venue-signup/signup.mjs --venue <id> --approval <record.json> [--budget-seconds <n>]");
    for (const p of args.problems) log(`  - ${p}`);
    return EXIT.usage;
  }
  const recipe = loadRecipe(args.venue);
  if (!recipe.ok) {
    log(recipe.reason);
    return recipe.code;
  }
  // A venue that needs a person's documents is refused before the identity
  // or the approval is even read: nothing about them changes the answer.
  if (recipe.recipe.identity_verification_required) {
    const result = await perform({ recipe: recipe.recipe, identity: null, approval: null, env, log, launchImpl: deps.launchImpl });
    log(result.reason);
    out(JSON.stringify({ venue: args.venue, outcome: result.outcome, submitted: false, secrets_written: [] }));
    return result.code;
  }
  const identity = loadIdentity(env);
  if (!identity.ok) {
    log(identity.reason);
    return identity.code;
  }
  const approval = loadApproval(args.approval, recipe.recipe);
  if (!approval.ok) {
    log(approval.reason);
    return approval.code;
  }
  log(`recipe ${recipe.path}; approved by ${approval.approval.operator} under ${approval.approval.terms} read at ${approval.approval.terms_read_at}`);
  const result = await perform({
    recipe: recipe.recipe,
    identity: identity.identity,
    approval: approval.approval,
    env,
    budgetMs: args.budgetSeconds * 1000,
    log,
    launchImpl: deps.launchImpl,
  });
  if (result.reason) log(result.reason);
  if (result.screenshot) log(`screenshot: ${result.screenshot}`);
  out(
    JSON.stringify({
      venue: args.venue,
      outcome: result.outcome,
      submitted: result.submitted,
      secrets_written: result.wrote,
      screenshot: result.screenshot ?? null,
    }),
  );
  return result.code;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(await main(process.argv.slice(2)));
}
