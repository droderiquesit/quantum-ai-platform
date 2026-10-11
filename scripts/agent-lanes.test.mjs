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
import { globsOverlap, main, outsideOwnership, runAcceptance, validate } from "./agent-lanes.mjs";

const POLICY = JSON.parse(readFileSync(new URL("./agent-policy.json", import.meta.url), "utf8"));

const lane = (id, owns, extra = {}) => ({
  id,
  role: "cloud-platform-engineer",
  owns,
  depends_on: [],
  acceptance: ["terraform test"],
  budget: { minutes: 60, usd: 5 },
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
    lane("x", ["scripts/x/**"], { budget: { usd: 5 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: 0, usd: 5 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: 60, usd: POLICY.lane_limits.max_usd + 1 } }),
    lane("x", ["scripts/x/**"], { budget: { minutes: POLICY.lane_limits.max_minutes + 1, usd: 1 } }),
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
    writeFileSync(manifest, JSON.stringify({ lanes: [lane("x", ["scripts/x/**"], { budget: { minutes: 45, usd: 2 } })] }));
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
