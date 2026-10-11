/**
 * The CodeQL blocking gate's tests: `node --test scripts/sast-gate.test.mjs`.
 *
 * Each test names the mutation of `sast-gate.mjs` that should make it fail.
 * The SARIF fixtures are the shape `github/codeql-action/analyze` writes:
 * rules under `tool.driver.rules` carrying `properties["security-severity"]`
 * as a string, results pointing at them by `rule.index`.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { judge, main, readPolicy } from "./sast-gate.mjs";

const POLICY = { minSecuritySeverity: 7.0, band: "high" };

const sarif = (results, rules) => ({
  version: "2.1.0",
  runs: [
    {
      tool: { driver: { name: "CodeQL", rules } },
      results,
    },
  ],
});

const RULES = [
  { id: "rust/sql-injection", properties: { "security-severity": "8.8" } },
  { id: "rust/weak-hash", properties: { "security-severity": "5.3" } },
  { id: "rust/unused-variable", properties: {} },
  { id: "js/critical", properties: { "security-severity": "9.8" } },
];

const result = (index, uri = "backend/src/lib.rs", line = 10) => ({
  ruleId: RULES[index].id,
  rule: { id: RULES[index].id, index },
  locations: [{ physicalLocation: { artifactLocation: { uri }, region: { startLine: line } } }],
});

// Mutation: compare with `>` instead of `>=`, or drop the comparison.
test("a finding at or above the committed severity blocks", () => {
  const documents = [{ name: "rust.sarif", sarif: sarif([result(0)], RULES) }];
  const { blocking, problems } = judge(documents, POLICY);
  assert.deepEqual(problems, []);
  assert.equal(blocking.length, 1);
  assert.equal(blocking[0].ruleId, "rust/sql-injection");
  assert.equal(blocking[0].severity, 8.8);

  const exactly = judge([{ name: "x.sarif", sarif: sarif([result(0)], RULES) }], { minSecuritySeverity: 8.8, band: "x" });
  assert.equal(exactly.blocking.length, 1, "a finding exactly at the threshold must block");
});

// The admitting half. Mutation: make every result with a severity block.
test("findings below the bar and quality findings do not block", () => {
  const documents = [{ name: "rust.sarif", sarif: sarif([result(1), result(2)], RULES) }];
  const { blocking, informational, problems } = judge(documents, POLICY);
  // Premise: the two results were read at all.
  assert.equal(informational.length, 2);
  assert.deepEqual(blocking, []);
  assert.deepEqual(problems, []);
});

// Mutation: let an empty directory through as "nothing found".
test("an analysis that wrote no SARIF is refused rather than passed", () => {
  const { blocking, problems } = judge([], POLICY);
  assert.deepEqual(blocking, []);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /no SARIF/);
});

test("a SARIF document with no run is refused", () => {
  const { problems } = judge([{ name: "empty.sarif", sarif: { version: "2.1.0", runs: [] } }], POLICY);
  assert.equal(problems.length, 1);
});

// Mutation: treat an unparseable severity as zero.
test("a security severity that is not a number blocks rather than being read as low", () => {
  const rules = [{ id: "rust/odd", properties: { "security-severity": "high" } }];
  const documents = [{ name: "odd.sarif", sarif: sarif([{ ruleId: "rust/odd", rule: { index: 0 } }], rules) }];
  const { blocking } = judge(documents, POLICY);
  assert.equal(blocking.length, 1);
});

// CodeQL sometimes names a rule by id only. Mutation: resolve by index only.
test("a result naming its rule only by id is still judged", () => {
  const documents = [{ name: "js.sarif", sarif: sarif([{ ruleId: "js/critical" }], RULES) }];
  const { blocking } = judge(documents, POLICY);
  assert.equal(blocking.length, 1);
  assert.equal(blocking[0].severity, 9.8);
});

// Mutation: default the threshold when the policy omits it.
test("a policy with no numeric threshold is refused", () => {
  assert.ok(readPolicy("{}").problem);
  assert.ok(readPolicy('{"min_security_severity": "7"}').problem);
  assert.ok(readPolicy('{"min_security_severity": 11}').problem);
  assert.ok(readPolicy("not json").problem);
  assert.equal(readPolicy('{"min_security_severity": 7.0}').policy.minSecuritySeverity, 7);
});

// The committed policy is the one the workflow reads; it must stay at the
// high band unless somebody edits it on purpose.
test("the committed policy blocks the high band", () => {
  const text = readFileSync(new URL("./sast-policy.json", import.meta.url), "utf8");
  const { policy, problem } = readPolicy(text);
  assert.equal(problem, undefined);
  assert.equal(policy.minSecuritySeverity, 7.0);
});

// End to end through the command line, on files: exit 1 with a finding,
// exit 0 without, exit 1 on an empty directory.
test("the command exits non-zero on a blocking finding and zero on a clean analysis", () => {
  const dir = mkdtempSync(join(tmpdir(), "sast-gate-"));
  try {
    const policy = join(dir, "policy.json");
    writeFileSync(policy, JSON.stringify({ min_security_severity: 7.0, band: "high" }));
    const quiet = { out: () => {}, err: () => {} };

    const dirty = join(dir, "dirty");
    const clean = join(dir, "clean");
    const empty = join(dir, "empty");
    for (const d of [dirty, clean, empty]) mkdirSync(d);
    writeFileSync(join(dirty, "rust.sarif"), JSON.stringify(sarif([result(0)], RULES)));
    writeFileSync(join(clean, "rust.sarif"), JSON.stringify(sarif([result(1), result(2)], RULES)));

    assert.equal(main(["--policy", policy, dirty], quiet), 1);
    assert.equal(main(["--policy", policy, clean], quiet), 0);
    assert.equal(main(["--policy", policy, empty], quiet), 1);
    assert.equal(main(["--policy", policy, join(dir, "missing")], quiet), 1);
    assert.equal(main([clean], quiet), 64, "no policy is a usage error, never a default threshold");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// The ratchet. The gate's first run over the whole repository found 491
// findings that predate it; without a baseline nothing could ever deploy,
// and with an unbounded one nothing new would ever block. These pin both
// halves: a file's allowance admits that many and no more, a file absent
// from the baseline admits none, and an allowance larger than what was found
// is reported so the fixing change can lower it.
test("the baseline admits a file's existing findings and blocks the next one", async () => {
  const { applyBaseline } = await import("./sast-gate.mjs");
  const finding = (ruleId, location) => ({ ruleId, location, severity: 7.5 });
  const blocking = [
    finding("rust/cleartext-logging", "backend/a.rs:10"),
    finding("rust/cleartext-logging", "backend/a.rs:20"),
    finding("rust/cleartext-logging", "backend/a.rs:30"),
    finding("rust/path-injection", "backend/b.rs:5"),
  ];
  const allowances = { "rust/cleartext-logging backend/a.rs": 2, "rust/path-injection backend/c.rs": 1 };
  const result = applyBaseline(blocking, allowances);
  assert.equal(result.baselined.length, 2, "premise: the allowance admits exactly two");
  assert.deepEqual(result.blocking.map((f) => f.location), ["backend/a.rs:30", "backend/b.rs:5"]);
  assert.deepEqual(result.shrinkable, ["rust/path-injection backend/c.rs: allowance 1, found 0"]);
  assert.equal(applyBaseline(blocking, {}).blocking.length, 4, "no baseline admits nothing");
});

test("a baseline that does not parse, or carries a bad allowance, is refused", async () => {
  const { readBaseline } = await import("./sast-gate.mjs");
  assert.ok(readBaseline("{").problem);
  assert.ok(readBaseline("{}").problem);
  assert.ok(readBaseline('{"allowances":{"r f":-1}}').problem);
  assert.ok(readBaseline('{"allowances":{"r f":1.5}}').problem);
  assert.deepEqual(readBaseline('{"allowances":{"r f":2}}').allowances, { "r f": 2 });
});
