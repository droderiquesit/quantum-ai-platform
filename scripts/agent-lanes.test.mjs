/**
 * Agent lanes' tests: `node --test scripts/agent-lanes.test.mjs`.
 *
 * Each names the mutation of `agent-lanes.mjs` that should make it fail.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { globsOverlap, judgeSpend, main, outsideOwnership, runAcceptance, validate } from "./agent-lanes.mjs";

const POLICY = JSON.parse(readFileSync(new URL("./agent-policy.json", import.meta.url), "utf8"));

const lane = (id, owns, extra = {}) => ({
  id,
  role: "cloud-platform-engineer",
  owns,
  depends_on: [],
  acceptance: ["terraform test"],
  budget: { minutes: 60, usd: 5, tokens: 100000 },
  ...extra,
});

test("ownership overlap is judged by what the globs can both name", () => {
  assert.ok(globsOverlap("infrastructure/**", "infrastructure/terraform/main.tf"));
  assert.ok(globsOverlap("infrastructure/terraform/**", "infrastructure/**"));
  assert.ok(!globsOverlap("infrastructure/terraform/**", "scripts/**"));
  assert.ok(!globsOverlap("scripts/a.mjs", "scripts/b.mjs"));
});

// The admitting half. Mutation: refuse every manifest, or drop the waves.
test("independent lanes run in one wave and a dependent lane after", () => {
  const { problems, waves } = validate(
    { lanes: [lane("a", ["infrastructure/terraform/**"]), lane("b", ["scripts/fleet/**"]), lane("c", ["infrastructure/terraform/modules/x/**"], { depends_on: ["a"] })] },
    POLICY,
  );
  assert.deepEqual(problems, []);
  assert.deepEqual(waves, [["a", "b"], ["c"]]);
});

// CICD-9083. The 2026-09-14 ADR collision in miniature. Mutation: skip the
// overlap check, or treat any shared prefix as ordered.
test("two lanes owning the same boundary with no order between them are refused", () => {
  const { problems, waves } = validate(
    { lanes: [lane("adr-a", ["infrastructure/terraform/**"]), lane("adr-b", ["infrastructure/terraform/main.tf"])] },
    POLICY,
  );
  assert.equal(problems.length, 1);
  assert.match(problems[0], /adr-a and adr-b own overlapping paths/);
  assert.deepEqual(waves, []);
});

test("the same overlap is admitted once one lane waits for the other, transitively", () => {
  const { problems, waves } = validate(
    {
      lanes: [
        lane("first", ["infrastructure/terraform/**"]),
        lane("middle", ["scripts/fleet/**"], { depends_on: ["first"] }),
        lane("last", ["infrastructure/terraform/main.tf"], { depends_on: ["middle"] }),
      ],
    },
    POLICY,
  );
  assert.deepEqual(problems, []);
  assert.deepEqual(waves, [["first"], ["middle"], ["last"]]);
});

// Mutation: let Kahn's loop stop early without reporting.
test("a dependency cycle is refused", () => {
  const { problems } = validate({ lanes: [lane("a", ["scripts/a/**"], { depends_on: ["b"] }), lane("b", ["scripts/b/**"], { depends_on: ["a"] })] }, POLICY);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /cycle/);
});

// CICD-9082. Mutation: drop the acceptance or budget checks.
test("a lane with no acceptance command or no bounded budget is refused", () => {
  for (const bad of [
    lane("x", ["scripts/x/**"], { acceptance: [] }),
    lane("x", ["scripts/x/**"], { acceptance: [" "] }),
    lane("x", ["scripts/x/**"], { budget: { usd: 5, tokens: 1000 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: 0, usd: 5, tokens: 1000 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: POLICY.lane_limits.max_usd + 1, tokens: 1000 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: POLICY.lane_limits.max_minutes + 1, usd: 1, tokens: 1000 } }),
  ]) {
    assert.ok(validate({ lanes: [bad] }, POLICY).problems.length > 0, JSON.stringify(bad));
  }
});

// The plan cannot promise a lane what the merge refuses it. Mutation: skip
// the role check.
test("a lane owning a path its role may not edit is refused", () => {
  const { problems } = validate({ lanes: [lane("x", ["backend/crates/runtime/**"])] }, POLICY);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /may not edit/);
  const planner = validate({ lanes: [lane("p", ["docs/x.md"], { role: "chief-orchestrator" })] }, POLICY);
  assert.equal(planner.problems.length, 1, "a planner lane owns nothing");
});

test("a lane's diff outside what it owns is found", () => {
  const owner = lane("x", ["infrastructure/terraform/modules/build-pool/**"]);
  assert.deepEqual(outsideOwnership(owner, ["infrastructure/terraform/modules/build-pool/main.tf"]), []);
  assert.deepEqual(outsideOwnership(owner, ["infrastructure/terraform/main.tf"]), ["infrastructure/terraform/main.tf"]);
});

// CICD-9082: acceptance is run, in order, and a failure stops the lane.
// Mutation: ignore the exit status.
test("acceptance passes only when every command exits zero", () => {
  const ok = runAcceptance(lane("x", ["scripts/x/**"], { acceptance: ["true", "test 1 -eq 1"] }), { cwd: tmpdir() });
  assert.equal(ok.passed, true);
  assert.equal(ok.results.length, 2);
  const bad = runAcceptance(lane("x", ["scripts/x/**"], { acceptance: ["true", "exit 3", "true"] }), { cwd: tmpdir() });
  assert.equal(bad.passed, false);
  assert.equal(bad.results.length, 2, "the third command must not run after the second failed");
  assert.equal(bad.results[1].status, 3);
});

// The budget is enforced, not advisory. Mutation: drop the `timeout` passed
// to spawn, so a command runs past the lane's minutes.
test("a command running past the lane's budget is killed and fails the lane", () => {
  const started = Date.now();
  const late = runAcceptance(lane("x", ["scripts/x/**"], { acceptance: ["sleep 5"] }), { cwd: tmpdir(), budgetMs: 300 });
  assert.equal(late.passed, false);
  assert.ok(Date.now() - started < 4000, "the command was not stopped at the budget");
});

test("the command validates, prints a budget and refuses a diff outside ownership", () => {
  const dir = mkdtempSync(join(tmpdir(), "agent-lanes-"));
  try {
    const policy = new URL("./agent-policy.json", import.meta.url).pathname;
    const manifest = join(dir, "lanes.json");
    writeFileSync(manifest, JSON.stringify({ lanes: [lane("x", ["scripts/x/**"], { budget: { minutes: 45, usd: 2, tokens: 1000 } })] }));
    const inside = join(dir, "inside.txt");
    const outside = join(dir, "outside.txt");
    writeFileSync(inside, "scripts/x/a.mjs\n");
    writeFileSync(outside, "scripts/x/a.mjs\n.github/workflows/ci.yml\n");
    const lines = [];
    const capture = { out: (l) => lines.push(l), err: () => {} };
    assert.equal(main(["validate", "--policy", policy, "--manifest", manifest], capture), 0);
    assert.equal(main(["budget", "--policy", policy, "--manifest", manifest, "--lane", "x"], capture), 0);
    assert.equal(lines.at(-1), "45");
    assert.equal(main(["check-diff", "--policy", policy, "--manifest", manifest, "--lane", "x", "--files", inside], capture), 0);
    assert.equal(main(["check-diff", "--policy", policy, "--manifest", manifest, "--lane", "x", "--files", outside], capture), 1);
    assert.equal(main(["budget", "--policy", policy, "--manifest", manifest, "--lane", "nope"], capture), 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// SEC-9077. A token budget is part of every lane, bounded by the policy.
// Mutation: drop the tokens check in `validate`.
test("a lane with no token budget, or one above the ceiling, is refused", () => {
  for (const tokens of [undefined, -1, 1.5, POLICY.lane_limits.max_tokens + 1]) {
    const { problems } = validate({ lanes: [lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: 5, tokens } })] }, POLICY);
    assert.equal(problems.length, 1, `tokens ${tokens}`);
    assert.match(problems[0], /budget\.tokens/);
  }
});

const MODEL = "google/gemini-2.5-flash-lite";
const entry = (fields) => JSON.stringify({ agent: "x", billed: true, model: MODEL, prompt_tokens: 1000, completion_tokens: 1000, ...fields });

// The admitting half first: a lane inside both budgets passes, and lines for
// other agents and unbilled refusals are not its spend. Mutation: count every
// line regardless of agent, which pushes this over the token budget.
test("a lane's model spend inside its budget is admitted and only its own billed lines count", () => {
  const owner = lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: 1, tokens: 5000 } });
  const ledger = [entry({}), entry({ agent: "someone-else", prompt_tokens: 900000 }), entry({ billed: false, prompt_tokens: 900000 }), ""].join("\n");
  const { tokens, micro, problems } = judgeSpend(owner, ledger);
  assert.deepEqual(problems, []);
  assert.equal(tokens, 2000);
  // 1000 in at 0.10/Mtok and 1000 out at 0.40/Mtok: 100 + 400 micro-USD.
  assert.equal(micro, 500);
});

// Mutation: compare with `>=`/drop the token comparison; or drop the USD one.
test("a lane that spent past its token or its USD budget is refused, each for its own reason", () => {
  const tight = lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: 1, tokens: 1999 } });
  const overTokens = judgeSpend(tight, entry({}));
  assert.equal(overTokens.problems.length, 1);
  assert.match(overTokens.problems[0], /spent 2000 model tokens; its budget is 1999/);

  const poor = lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: 0, tokens: 5000 } });
  const overUsd = judgeSpend(poor, entry({}));
  assert.equal(overUsd.problems.length, 1);
  assert.match(overUsd.problems[0], /USD on models; its budget is 0/);
});

// A gap is not zero. Mutation: `continue` silently past an unreadable line.
test("a billed line whose spend cannot be read refuses the lane rather than counting as nothing", () => {
  const owner = lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: 1, tokens: 5000 } });
  for (const [ledger, reason] of [
    [entry({ completion_tokens: null }), /no readable token counts/],
    [entry({ model: "nobody/priced-this" }), /has no price row/],
    ["{not json", /is not JSON/],
  ]) {
    const { problems } = judgeSpend(owner, ledger);
    assert.equal(problems.length, 1, ledger);
    assert.match(problems[0], reason);
  }
});

test("the spend command refuses an over-budget lane and a missing ledger", () => {
  const dir = mkdtempSync(join(tmpdir(), "agent-lanes-spend-"));
  try {
    const policy = new URL("./agent-policy.json", import.meta.url).pathname;
    const manifest = join(dir, "lanes.json");
    writeFileSync(manifest, JSON.stringify({ lanes: [lane("x", ["scripts/x/**"], { budget: { minutes: 45, usd: 1, tokens: 2500 } })] }));
    const within = join(dir, "within.jsonl");
    const beyond = join(dir, "beyond.jsonl");
    writeFileSync(within, `${entry({})}\n`);
    writeFileSync(beyond, `${entry({})}\n${entry({})}\n`);
    const quiet = { out: () => {}, err: () => {} };
    const args = (ledger) => ["spend", "--policy", policy, "--manifest", manifest, "--lane", "x", "--ledger", ledger];
    assert.equal(main(args(within), quiet), 0);
    assert.equal(main(args(beyond), quiet), 1);
    assert.equal(main(args(join(dir, "absent.jsonl")), quiet), 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
