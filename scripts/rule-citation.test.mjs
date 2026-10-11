/**
 * The rule-citation gate's tests: `node --test scripts/rule-citation.test.mjs`.
 *
 * Each test names the mutation of `rule-citation.mjs` that should make it
 * fail.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { citation, isRule, judge, main } from "./rule-citation.mjs";

const BODY = (rule) => `## Rule Exceptions\n\n**Rule changed:** ${rule}\n\n## Summary\n`;

// Mutation: make `isRule` return false for `.claude/rules/` paths.
test("a change to a rule file without naming it is refused", () => {
  const verdict = judge(BODY("none"), [
    "backend/crates/libs/qip-core/src/lib.rs",
    ".claude/rules/01-security-and-safety.md",
  ]);
  assert.equal(verdict.ok, false);
  assert.match(verdict.problem, /\.claude\/rules\/01-security-and-safety\.md/);
});

// Mutation: drop the `uncited` filter so any citation passes.
test("naming one changed rule does not cite another", () => {
  const verdict = judge(BODY("scripts/dependency-allowlist.txt"), [
    "scripts/dependency-allowlist.txt",
    ".claude/hooks/guard-dangerous-command.py",
  ]);
  assert.equal(verdict.ok, false);
  assert.match(verdict.problem, /guard-dangerous-command\.py/);
  assert.doesNotMatch(verdict.problem, /dependency-allowlist/);
});

// The admitting half: without it, a gate that refused everything would pass
// every test above.
test("a change that names every rule it changes is admitted", () => {
  const verdict = judge(
    BODY("`.claude/rules/01-security-and-safety.md`, `docs/adr/0109-dependencies.md`"),
    [".claude/rules/01-security-and-safety.md", "docs/adr/0109-dependencies.md", "Makefile"],
  );
  assert.equal(verdict.ok, true);
  assert.deepEqual(verdict.rules, [
    ".claude/rules/01-security-and-safety.md",
    "docs/adr/0109-dependencies.md",
  ]);
});

test("a change touching no rule is admitted with 'none'", () => {
  const verdict = judge(BODY("none"), ["backend/Cargo.toml", "frontend/portal/app/page.tsx"]);
  assert.equal(verdict.ok, true);
  assert.deepEqual(verdict.rules, []);
});

// Mutation: return `{ ok: true }` when `citation` is null.
test("a body with the line removed is refused even when no rule changed", () => {
  const verdict = judge("## Summary\nfix a typo\n", ["README.md"]);
  assert.equal(verdict.ok, false);
  assert.match(verdict.problem, /no '\*\*Rule changed:\*\*' line/);
});

// Mutation: match `CLAUDE.md` only at the root.
test("a nested CLAUDE.md is a rule file and an ordinary markdown file is not", () => {
  assert.equal(isRule("CLAUDE.md"), true);
  assert.equal(isRule("infrastructure/CLAUDE.md"), true);
  assert.equal(isRule("docs/adr/0006-classical-baseline.md"), true);
  assert.equal(isRule("docs/ops/threat-model.md"), false);
  assert.equal(isRule("scripts/dependency-allowlist.txt.bak"), false);
});

test("the citation is the text after the label, trimmed", () => {
  assert.equal(citation("x\n  **Rule changed:**  `CLAUDE.md`  \ny"), "`CLAUDE.md`");
  assert.equal(citation(null), null);
});

// Mutation: treat a non-pull-request event as nothing to judge (exit 0).
test("the command refuses an event that is not a pull request, and exits 1 on an uncited rule", () => {
  const dir = mkdtempSync(join(tmpdir(), "rule-citation-"));
  try {
    const changed = join(dir, "changed.txt");
    writeFileSync(changed, ".claude/settings.json\n");
    const push = join(dir, "push.json");
    writeFileSync(push, JSON.stringify({ ref: "refs/heads/main" }));
    assert.equal(main(["--event", push, "--changed", changed]), 2);

    const uncited = join(dir, "pr.json");
    writeFileSync(uncited, JSON.stringify({ pull_request: { body: BODY("none") } }));
    assert.equal(main(["--event", uncited, "--changed", changed]), 1);

    const cited = join(dir, "cited.json");
    writeFileSync(
      cited,
      JSON.stringify({ pull_request: { body: BODY(".claude/settings.json") } }),
    );
    assert.equal(main(["--event", cited, "--changed", changed]), 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
