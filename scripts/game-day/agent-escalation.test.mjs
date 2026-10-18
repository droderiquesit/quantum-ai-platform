/**
 * The agent-authority drill's tests: `node --test scripts/game-day/agent-escalation.test.mjs`.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { attempts, main, runDrill, validateRecord } from "./agent-escalation.mjs";

const POLICY_PATH = new URL("../agent-policy.json", import.meta.url);
const policy = () => JSON.parse(readFileSync(POLICY_PATH, "utf8"));
const RESULTS = fileURLToPath(new URL("../../docs/ops/game-days/agent-escalation/", import.meta.url));

// Premise first: the control is admitted, so a clean sweep below is selective
// refusal and not closed judges. Mutation: make `judgeChange` or `validate`
// refuse everything — the control's `held` goes false.
test("the committed judges admit the control and refuse every escalation attempt", () => {
  const record = runDrill({ policy: policy(), policyPath: fileURLToPath(POLICY_PATH), commit: "test" });
  const control = record.attempts.filter((a) => a.expect === "admitted");
  const attacks = record.attempts.filter((a) => a.expect === "refused");
  assert.equal(control.length, 1);
  assert.ok(attacks.length >= 10, `${attacks.length} attacks`);
  assert.equal(control[0].outcome, "admitted", JSON.stringify(control[0]));
  for (const attack of attacks) assert.equal(attack.outcome, "refused", attack.attempt);
  assert.equal(record.verdict, "held");
  assert.deepEqual(validateRecord(record), []);
});

// The drill can fail. Mutation: record `held: true` for every attempt in
// `runDrill` — the relaxed policy below then reads as held and this fails.
test("a policy that stops protecting the workflows is reported as breached, naming what got through", () => {
  const relaxed = policy();
  relaxed.protected_paths.globs = [];
  const record = runDrill({ policy: relaxed, policyPath: fileURLToPath(POLICY_PATH), commit: "test" });
  assert.equal(record.verdict, "breached");
  const breached = record.attempts.filter((a) => !a.held).map((a) => a.attempt);
  assert.ok(breached.includes("edit a deploy workflow with no approval"), breached.join("; "));
  assert.ok(breached.includes("edit a deploy workflow with a bot's approval of the head"), breached.join("; "));
  assert.deepEqual(validateRecord(record), []);
});

test("a record claiming held over a breached attempt, or with no control, is refused", () => {
  const record = runDrill({ policy: policy(), policyPath: fileURLToPath(POLICY_PATH), commit: "test" });
  const lying = structuredClone(record);
  lying.attempts[1].outcome = "admitted";
  lying.attempts[1].held = false;
  assert.ok(validateRecord(lying).some((p) => /held with an attempt/.test(p)));
  const noControl = structuredClone(record);
  noControl.attempts = noControl.attempts.filter((a) => a.expect !== "admitted");
  assert.ok(validateRecord(noControl).some((p) => /no control attempt/.test(p)));
});

test("every attempt names its own input, so a renamed judge cannot leave one silently empty", () => {
  const names = attempts(policy(), { problem: "none" }).map((a) => a.name);
  assert.equal(new Set(names).size, names.length);
});

test("every committed drill record is in the committed format and held", () => {
  const files = readdirSync(RESULTS).filter((f) => f.endsWith(".json"));
  assert.ok(files.length >= 1, "no drill record is committed");
  for (const file of files) {
    const record = JSON.parse(readFileSync(join(RESULTS, file), "utf8"));
    assert.deepEqual(validateRecord(record), [], file);
    assert.equal(record.verdict, "held", file);
  }
});

test("the command runs, exits zero on a held drill, and refuses an unknown mode", () => {
  const lines = [];
  assert.equal(main(["run"], { out: (l) => lines.push(l), err: () => {} }), 0);
  assert.match(lines.at(-1), /verdict: held/);
  assert.equal(main(["nonsense"], { out: () => {}, err: () => {} }), 64);
});
