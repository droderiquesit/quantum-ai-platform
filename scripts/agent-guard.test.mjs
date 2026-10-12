/**
 * The agent boundary's tests: `node --test scripts/agent-guard.test.mjs`.
 *
 * Each names the mutation of `agent-guard.mjs` or `agent-policy.json` that
 * should make it fail. Commits and reviews are in the shape GitHub's REST
 * API returns them, because that is what the workflow hands the script.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gatherRange, globToRegExp, isAgentCommit, judgeChange, judgeDispatch, judgeRange, judgeSandboxWorkflow, loadLanes, main, matchesAny } from "./agent-guard.mjs";

const POLICY = JSON.parse(readFileSync(new URL("./agent-policy.json", import.meta.url), "utf8"));
const HEAD = "a".repeat(40);

const agentCommit = (role) => ({
  commit: {
    author: { name: "Claude", email: "noreply@anthropic.com" },
    message: `Do a thing\n\nCo-Authored-By: Claude <noreply@anthropic.com>${role ? `\nAgent-Role: ${role}` : ""}\n`,
  },
});
const personCommit = { commit: { author: { name: "Dana", email: "dana@example.invalid" }, message: "Do a thing by hand\n" } };
// What the workflow reads from GitHub for each reviewer.
const PERMS = { dana: "write", erin: "admin", rita: "read" };
const approval = (login, commit_id = HEAD, type = "User") => ({ user: { login, type }, state: "APPROVED", commit_id });

test("globs cross directories only where they say so", () => {
  assert.ok(globToRegExp(".github/**").test(".github/workflows/deploy.yml"));
  assert.ok(globToRegExp("**/CLAUDE.md").test("CLAUDE.md"));
  assert.ok(globToRegExp("**/CLAUDE.md").test("infrastructure/CLAUDE.md"));
  assert.ok(globToRegExp("scripts/check-*.sh").test("scripts/check-secrets.sh"));
  assert.ok(!globToRegExp("scripts/check-*.sh").test("scripts/fleet/check-x.sh"));
  assert.ok(!globToRegExp("frontend/**").test("frontendx/a.ts"));
});

// Mutation: drop the co-author marker; a commit whose author is the user
// but whose trailer names the agent must still be recognised.
test("an agent commit is recognised by its co-author trailer alone", () => {
  const borrowed = {
    commit: { author: { name: "Dana", email: "dana@example.invalid" }, message: "x\n\nCo-Authored-By: Claude Opus <noreply@anthropic.com>\n" },
  };
  assert.ok(isAgentCommit(borrowed, POLICY.agent_markers));
  assert.ok(!isAgentCommit(personCommit, POLICY.agent_markers));
});

// GOV-9067. Mutation: give chief-orchestrator a may_edit list, or skip the
// role check.
test("a planner role cannot land code", () => {
  const { agent, refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/plan", headSha: HEAD,
    files: ["backend/crates/runtime/qip-kernel/src/cycle.rs"],
    commits: [agentCommit("chief-orchestrator")], reviews: [approval("dana")],
  });
  assert.ok(agent);
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /chief-orchestrator role \(planner\) may not change/);
});

// The admitting half of the role check: a coder inside its own paths, with
// nothing protected, needs nobody. Mutation: refuse every agent change.
test("a coder inside its own paths is admitted without a review", () => {
  const { agent, refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/x", headSha: HEAD,
    files: ["frontend/portal/src/app/page.tsx"], commits: [agentCommit("frontend-engineer")], reviews: [],
  });
  assert.ok(agent);
  assert.deepEqual(refusals, []);
});

test("a coder outside its own paths is refused", () => {
  const { refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/x", headSha: HEAD,
    files: ["frontend/portal/src/app/page.tsx", "infrastructure/terraform/main.tf"],
    commits: [agentCommit("frontend-engineer")], reviews: [],
  });
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /infrastructure\/terraform\/main\.tf/);
});

test("an undeclared role is refused rather than ignored", () => {
  const { refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/x", headSha: HEAD,
    files: ["docs/x.md"], commits: [agentCommit("root")], reviews: [],
  });
  assert.match(refusals[0], /does not define/);
});

// Protected paths. Mutation: drop `.github/**` from protected_paths, or the
// approval check.
test("an agent change to a workflow needs a person's approval of the head commit", () => {
  const base = {
    policy: POLICY, permissions: PERMS, headRef: "claude/ci", headSha: HEAD,
    files: [".github/workflows/deploy.yml"], commits: [agentCommit("cloud-platform-engineer")],
  };
  // Premise: the role admits the path, so only the protection can refuse.
  assert.ok(matchesAny(".github/workflows/deploy.yml", POLICY.roles["cloud-platform-engineer"].may_edit));

  assert.equal(judgeChange({ ...base, reviews: [] }).refusals.length, 1, "no review");
  assert.equal(judgeChange({ ...base, reviews: [approval("dana", "b".repeat(40))] }).refusals.length, 1, "approval of an older commit");
  assert.equal(judgeChange({ ...base, reviews: [approval("claude-reviewer")] }).refusals.length, 1, "approval by an agent");
  assert.equal(judgeChange({ ...base, reviews: [approval("helper[bot]", HEAD, "Bot")] }).refusals.length, 1, "approval by a bot");
  assert.deepEqual(judgeChange({ ...base, reviews: [approval("dana")] }).refusals, [], "a person approved the head");
  const withdrawn = [approval("dana"), { user: { login: "dana", type: "User" }, state: "CHANGES_REQUESTED", commit_id: HEAD }];
  assert.equal(judgeChange({ ...base, reviews: withdrawn }).refusals.length, 1, "approval later withdrawn");
  const commented = [approval("dana"), { user: { login: "dana", type: "User" }, state: "COMMENTED", commit_id: HEAD }];
  assert.deepEqual(judgeChange({ ...base, reviews: commented }).refusals, [], "a later comment does not withdraw");
});

// The paper boundary's Terraform layer is protected even inside the cloud
// engineer's own territory. Mutation: drop it from protected_paths.
test("the autonomy ceiling's validation file is protected", () => {
  const { refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/x", headSha: HEAD,
    files: ["infrastructure/terraform/variables.tf"], commits: [agentCommit("cloud-platform-engineer")], reviews: [],
  });
  assert.equal(refusals.length, 1);
});

// Outside the protected paths a change with no marker is a person's, and
// needs nobody. Mutation: refuse every unmarked change.
test("an unmarked change outside the protected paths is outside the agent boundary", () => {
  const { agent, refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "fix/x", headSha: HEAD, files: ["backend/crates/runtime/qip-kernel/src/cycle.rs"], commits: [personCommit], reviews: [],
  });
  assert.equal(agent, false);
  assert.deepEqual(refusals, []);
});

const signed = (email) => ({
  commit: { ...personCommit.commit, committer: { name: "Dana", email }, verification: { verified: true, reason: "valid" } },
});

// GOV-9067. This test replaced "a person's own change is outside the agent
// boundary", which asserted that an unmarked, unsigned, unapproved change to
// deploy.yml was admitted — the exact shape an agent pushing as its user
// with no trailer takes. Mutation: return early for every unmarked change,
// as the guard did before.
test("an unmarked change to a protected path needs a person's signature or a person's approval", () => {
  const judge = (commits, reviews = []) =>
    judgeChange({ policy: POLICY, permissions: PERMS, headRef: "fix/x", headSha: HEAD, files: [".github/workflows/deploy.yml"], commits, reviews });

  const bare = judge([personCommit]);
  assert.equal(bare.agent, false);
  assert.equal(bare.refusals.length, 1);
  assert.match(bare.refusals[0], /no agent marker touches protected paths, 1 of its 1 commit\(s\) carry no person's verified signature/);

  // The admitting halves, each on its own.
  assert.deepEqual(judge([signed("dana@example.invalid")]).refusals, [], "signed by a person's key");
  assert.deepEqual(judge([personCommit], [approval("erin")]).refusals, [], "approved by a person");
});

// GitHub signs every API and web-editor commit itself, an agent's included.
// Mutation: empty web_flow_committer_emails, or read `verified` alone.
test("a commit signed by GitHub's web-flow key is not a person's signature", () => {
  const { refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "fix/x", headSha: HEAD, files: [".claude/settings.json"],
    commits: [signed("dana@example.invalid"), signed("noreply@github.com")], reviews: [],
  });
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /1 of its 2 commit\(s\) carry no person's verified signature/);
  // The merge button's own commit, web-flow signed, is not held against a
  // signed pull request. Mutation: drop the merge exemption.
  const merged = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "fix/x", headSha: HEAD, files: [".claude/settings.json"],
    commits: [signed("dana@example.invalid"), { ...signed("noreply@github.com"), parents: [{ sha: "c" }, { sha: "d" }] }], reviews: [],
  });
  assert.deepEqual(merged.refusals, []);
  // Nor is an approval of an earlier commit, the agent rule's own reading.
  const stale = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "fix/x", headSha: HEAD, files: [".claude/settings.json"],
    commits: [personCommit], reviews: [approval("erin", "b".repeat(40))],
  });
  assert.equal(stale.refusals.length, 1);
});

// A session that rewrites the author but commits as itself is still marked.
// Mutation: read the author fields only.
test("an agent committer is recognised even under a person's author name", () => {
  const disguised = {
    commit: { author: { name: "Dana", email: "dana@example.invalid" }, committer: { name: "Claude", email: "noreply@anthropic.com" }, message: "x\n" },
  };
  assert.ok(isAgentCommit(disguised, POLICY.agent_markers));
});

// A signature does not excuse an agent's protected change from approval.
// Mutation: let isPersonSigned stand in for approval on the agent branch.
test("an agent's protected change still needs approval even when signed", () => {
  const agentSigned = { commit: { ...agentCommit("technical-writer").commit, verification: { verified: true }, committer: { email: "dana@example.invalid" } } };
  const { agent, refusals } = judgeChange({
    policy: POLICY, permissions: PERMS, headRef: "claude/x", headSha: HEAD, files: [".claude/agents/technical-writer.md"], commits: [agentSigned], reviews: [],
  });
  assert.ok(agent);
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /no person has approved/);
});

// GOV-9067: no agent deploys. Mutation: empty the refused list.
test("an agent identity may not start a deploying workflow and a person may", () => {
  for (const workflow of ["deploy.yml", "infra.yml", "fleet.yml"]) {
    assert.ok(judgeDispatch({ policy: POLICY, actor: "claude-agent", workflow }), workflow);
    assert.ok(judgeDispatch({ policy: POLICY, actor: "release[bot]", workflow }), workflow);
    assert.equal(judgeDispatch({ policy: POLICY, actor: "dana", workflow }), undefined, workflow);
  }
  assert.equal(judgeDispatch({ policy: POLICY, actor: "claude-agent", workflow: "ci.yml" }), undefined);
});

// The policy's roles are the agents' own "Paths you may change", so neither
// can widen alone. Mutation: add a path to one side.
test("the policy's role table matches every agent definition", () => {
  const dir = new URL("../.claude/agents/", import.meta.url);
  const files = readdirSync(dir).filter((f) => f.endsWith(".md"));
  assert.ok(files.length >= 10, `found only ${files.length} agent definitions`);
  for (const file of files) {
    const role = file.replace(/\.md$/, "");
    const text = readFileSync(new URL(file, dir), "utf8");
    // The first non-empty line under the heading is the list; prose below
    // it names other files in backticks and is not part of the grant.
    const section = (text.split("## Paths you may change")[1] ?? "").split("\n").find((line) => line.trim() !== "") ?? "";
    const paths = [...section.matchAll(/`([^`]+)`/g)].map((m) => m[1]).filter((p) => !p.startsWith("#[") && (p.includes("/") || p.includes(".")));
    assert.ok(POLICY.roles[role], `${role} has no entry in agent-policy.json`);
    assert.deepEqual([...POLICY.roles[role].may_edit].sort(), paths.sort(), `${role}'s paths differ`);
  }
  assert.equal(Object.keys(POLICY.roles).length, files.length, "agent-policy.json defines a role no agent file has");
});

// SEC-9068, SEC-9084, CICD-9076: the workflow agent work runs in holds no
// secret, no cloud credential and no write. Mutation: let `secrets.` or
// `id-token: write` through.
test("a sandbox workflow with a secret, a credential, a write or no time limit is refused", () => {
  const clean = [
    "name: agent sandbox",
    "on:",
    "  workflow_dispatch:",
    "permissions:",
    "  contents: read",
    "jobs:",
    "  run:",
    "    runs-on: ubuntu-latest",
    "    timeout-minutes: 30",
    "    steps:",
    "      # secrets.NOTHING is mentioned only in this comment",
    "      - run: echo hi",
    "      - run: |",
    "          sudo iptables -A OUTPUT -m owner --gid-owner lane-egress -j REJECT",
    "          node agent-egress.mjs serve --port 3128",
    '          sudo setpriv --regid="$lane_gid" --no-new-privs -- true',
    "          node agent-egress.mjs verdict --log egress.jsonl",
    "",
  ].join("\n");
  assert.deepEqual(judgeSandboxWorkflow(clean), []);
  // Each variant must be refused for its own reason, not because it also
  // trips a neighbouring rule: `id-token: write` is also a write, and a
  // test that only counted problems passed with the OIDC rule deleted.
  const variants = [
    ["secret", clean.replace("echo hi", "echo ${{ secrets.TOKEN }}"), /secret/],
    ["oidc", clean.replace("    timeout-minutes: 30", "    timeout-minutes: 30\n    permissions:\n      id-token: write"), /OIDC/],
    ["environment", clean.replace("    timeout-minutes: 30", "    timeout-minutes: 30\n    environment: prod"), /environment/],
    ["write", clean.replace("  contents: read", "  contents: write"), /write permission/],
    ["untimed", clean.replace("    timeout-minutes: 30\n", ""), /no timeout-minutes/],
    ["target", clean.replace("  workflow_dispatch:", "  pull_request_target:"), /pull_request_target/],
  ];
  for (const [name, text, reason] of variants) {
    const problems = judgeSandboxWorkflow(text);
    assert.ok(problems.some((p) => reason.test(p)), `${name}: ${JSON.stringify(problems)}`);
  }
});

// SEC-9077: the lane's network is confined, and each piece of the
// confinement is required on its own, because each alone is the hole.
// Mutation: drop any one entry from `confinement` in judgeSandboxWorkflow.
test("a sandbox workflow that leaves the lane's network open in any one way is refused, for that reason", () => {
  const clean = [
    "permissions:",
    "  contents: read",
    "jobs:",
    "  run:",
    "    timeout-minutes: 30",
    "    steps:",
    "      - run: |",
    "          sudo iptables -A OUTPUT -m owner --gid-owner lane-egress -j REJECT",
    "          node agent-egress.mjs serve --port 3128",
    '          sudo setpriv --regid="$lane_gid" --no-new-privs -- true',
    "          node agent-egress.mjs verdict --log egress.jsonl",
    "",
  ].join("\n");
  assert.deepEqual(judgeSandboxWorkflow(clean), [], "premise: the confined fixture is admitted");
  const variants = [
    ["firewall", clean.replace(" -j REJECT", " -j ACCEPT"), /firewall/],
    ["privs", clean.replace(" --no-new-privs", ""), /no_new_privs/],
    ["group", clean.replace('--regid="$lane_gid"', "--regid=0"), /firewalled group/],
    ["proxy", clean.replace("agent-egress.mjs serve", "true"), /no egress proxy/],
    ["verdict", clean.replace("agent-egress.mjs verdict", "true"), /no egress verdict/],
    // A comment naming the pieces is not the pieces.
    ["comment", clean.replace("          node agent-egress.mjs verdict --log egress.jsonl", "          # agent-egress.mjs verdict"), /no egress verdict/],
  ];
  for (const [name, text, reason] of variants) {
    const problems = judgeSandboxWorkflow(text);
    assert.ok(problems.some((p) => reason.test(p)), `${name}: ${JSON.stringify(problems)}`);
  }
});

// The wiring half of GOV-9067: every workflow the policy refuses to agents
// actually calls the judge, naming itself, before its first credential.
// This one reads text — the behaviour is the `judgeDispatch` test above —
// and it exists so a workflow added to the list without the step, or a
// step naming the wrong file, fails here. Mutation: delete the guard from
// infra.yml, or name fleet.yml in image.yml's guard.
test("every workflow refused to agents runs the dispatch judge on itself before authenticating", () => {
  for (const workflow of POLICY.dispatch_refused_for_agents.workflows) {
    const text = readFileSync(new URL(`../.github/workflows/${workflow}`, import.meta.url), "utf8");
    const guard = text.indexOf(`--actor "$who" --workflow ${workflow}\n`);
    assert.ok(guard > 0, `${workflow} does not run agent-guard.mjs dispatch on itself`);
    const auth = text.indexOf("google-github-actions/auth");
    assert.ok(auth < 0 || guard < auth, `${workflow} authenticates before it refuses an agent`);
  }
});

// The committed sandbox workflow is held to the same rule, by running the
// judge on it rather than reading its text here.
test("the committed agent sandbox workflow passes the sandbox rule", () => {
  const text = readFileSync(new URL("../.github/workflows/agent-sandbox.yml", import.meta.url), "utf8");
  assert.ok(text.includes("agent-lanes.mjs"), "premise: this is the sandbox that runs lanes");
  assert.deepEqual(judgeSandboxWorkflow(text), []);
});

test("the command refuses a protected agent change and admits it once approved", () => {
  const dir = mkdtempSync(join(tmpdir(), "agent-guard-"));
  try {
    const write = (name, value) => {
      writeFileSync(join(dir, name), JSON.stringify(value));
      return join(dir, name);
    };
    const policy = new URL("./agent-policy.json", import.meta.url).pathname;
    const files = write("files.json", [{ filename: ".claude/settings.json" }]);
    const commits = write("commits.json", [agentCommit("technical-writer")]);
    const none = write("none.json", []);
    const approved = write("approved.json", [approval("dana")]);
    const perms = write("perms.json", PERMS);
    const quiet = { out: () => {}, err: () => {} };
    const args = (reviews) => ["change", "--policy", policy, "--head-ref", "claude/x", "--head-sha", HEAD, "--files", files, "--commits", commits, "--reviews", reviews, "--permissions", perms];
    // technical-writer may edit .claude/**/*.md only; settings.json is outside.
    assert.equal(main(args(none), quiet), 1);
    assert.equal(main(args(approved), quiet), 1, "approval does not widen a role");
    const md = write("md.json", [{ filename: ".claude/agents/technical-writer.md" }]);
    const mdArgs = (reviews) => ["change", "--policy", policy, "--head-ref", "claude/x", "--head-sha", HEAD, "--files", md, "--commits", commits, "--reviews", reviews, "--permissions", perms];
    assert.equal(main(mdArgs(none), quiet), 1, "protected and unapproved");
    assert.equal(main(mdArgs(approved), quiet), 0, "protected, inside the role, approved");
    assert.equal(main(["dispatch", "--policy", policy, "--actor", "claude-bot", "--workflow", "deploy.yml"], quiet), 1);
    assert.equal(main(["dispatch", "--policy", policy, "--actor", "dana", "--workflow", "deploy.yml"], quiet), 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// GOV-9067: an approval counts only from a writer. Mutation: drop the
// permission clause in approvedByAPerson.
test("an approval from a reviewer without write access, or one nobody could vouch for, does not count", () => {
  const judge = (reviews, permissions) =>
    judgeChange({
      policy: POLICY, permissions, headRef: "claude/x", headSha: HEAD,
      files: [".claude/agents/technical-writer.md"], commits: [agentCommit("technical-writer")], reviews,
    }).refusals;
  assert.deepEqual(judge([approval("dana")], PERMS), [], "premise: a writer's approval admits it");
  assert.equal(judge([approval("rita")], PERMS).length, 1, "a read-only collaborator");
  assert.equal(judge([approval("mallory")], PERMS).length, 1, "a login the workflow could not read");
  assert.equal(judge([approval("dana")], undefined).length, 1, "no permission map at all");
});

const rangeEntry = (sha, commit, files, pr = null) => ({ sha, ...commit, files, pr });
const pr = (reviews) => ({ head_sha: HEAD, head_ref: "claude/x", reviews, permissions: PERMS });

// The deploy gate used to judge only the deployed commit's first-parent diff.
// Mutation: judge only the last entry of the range.
test("every commit in a deployed range is judged, not only the last", () => {
  const clean = rangeEntry("b".repeat(40), personCommit, ["backend/crates/runtime/qip-kernel/src/cycle.rs"]);
  const earlier = rangeEntry("c".repeat(40), agentCommit("cloud-platform-engineer"), ["infrastructure/terraform/variables.tf"], pr([]));
  assert.deepEqual(judgeRange({ policy: POLICY, entries: [clean] }).refusals, [], "premise: a clean range passes");
  const { agent, refusals } = judgeRange({ policy: POLICY, entries: [earlier, clean] });
  assert.ok(agent);
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /^cccccccccccc: an agent-authored change touches protected paths/);
  // The same commit, approved by a writer on its pull request's head, passes.
  const approved = rangeEntry("c".repeat(40), agentCommit("cloud-platform-engineer"), ["infrastructure/terraform/variables.tf"], pr([approval("erin")]));
  assert.deepEqual(judgeRange({ policy: POLICY, entries: [approved, clean] }).refusals, []);
});

// Mutation: let a commit with no PR borrow another commit's approval, or skip
// an entry whose file list is missing.
test("a commit pushed straight to the branch has no approval to show, and an entry without files is refused", () => {
  const pushed = rangeEntry("d".repeat(40), personCommit, [".github/workflows/deploy.yml"]);
  // Beside it, a pull request a writer approved on the same head sha: its
  // approval must not be lent to the pushed commit.
  const reviewed = rangeEntry("f".repeat(40), personCommit, [".github/workflows/ci.yml"], pr([approval("erin")]));
  const { refusals } = judgeRange({ policy: POLICY, entries: [reviewed, pushed] });
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /^dddddddddddd: /);
  const blind = { sha: "e".repeat(40), ...personCommit, pr: null };
  assert.match(judgeRange({ policy: POLICY, entries: [blind] }).refusals[0], /file list is missing/);
});

test("the range command refuses an empty range and a protected agent commit", () => {
  const dir = mkdtempSync(join(tmpdir(), "agent-guard-range-"));
  try {
    const policy = new URL("./agent-policy.json", import.meta.url).pathname;
    const write = (name, value) => {
      writeFileSync(join(dir, name), JSON.stringify(value));
      return join(dir, name);
    };
    const quiet = { out: () => {}, err: () => {} };
    const run = (path) => main(["range", "--policy", policy, "--range", path], quiet);
    assert.equal(run(write("clean.json", [rangeEntry("b".repeat(40), personCommit, ["README.md"])])), 0);
    assert.equal(run(write("empty.json", [])), 1);
    assert.equal(run(write("bad.json", [rangeEntry("c".repeat(40), agentCommit("technical-writer"), [".claude/settings.json"], pr([]))])), 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

/**
 * A fake GitHub API: two commits, the first merged through PR 7. The compare
 * answer is pages, as `gh api --paginate --slurp` returns it; `split` puts
 * each commit on its own page, the shape a range over 250 commits takes.
 */
function fakeApi({ total = 2, split = false, repeat = false } = {}) {
  const B = "b".repeat(40);
  const C = "c".repeat(40);
  const first = { sha: B, ...agentCommit("technical-writer") };
  const second = repeat ? first : { sha: C, ...personCommit };
  const compare = split
    ? [{ total_commits: total, commits: [first] }, { total_commits: total, commits: [second] }]
    : [{ total_commits: total, commits: [first, second] }];
  const routes = {
    [`repos/o/r/compare/${"0".repeat(40)}...${C}?per_page=100`]: compare,
    [`repos/o/r/commits/${B}`]: [{ files: [{ filename: ".claude/settings.json" }] }],
    [`repos/o/r/commits/${C}`]: [{ files: [{ filename: "README.md" }] }],
    [`repos/o/r/commits/${B}/pulls`]: [{ number: 7, merged_at: "2026-10-11T00:00:00Z" }],
    [`repos/o/r/commits/${C}/pulls`]: [],
    "repos/o/r/pulls/7": { head: { sha: HEAD, ref: "claude/x" } },
    "repos/o/r/pulls/7/reviews?per_page=100": [[approval("dana"), approval("rita")]],
    "repos/o/r/collaborators/dana/permission": { permission: "write" },
  };
  return (path) => {
    if (!(path in routes)) throw new Error(`no route ${path}`);
    return routes[path];
  };
}

// Mutation: drop the truncation check, or attach the PR to the wrong commit,
// or let an unreadable permission default to a writer's.
test("a range is gathered commit by commit with each one's own pull request, and a truncated range is refused", () => {
  const entries = gatherRange({ api: fakeApi(), repository: "o/r", base: "0".repeat(40), head: "c".repeat(40) });
  assert.equal(entries.length, 2);
  assert.deepEqual(entries[0].files, [".claude/settings.json"]);
  assert.equal(entries[0].pr.head_ref, "claude/x");
  assert.deepEqual(entries[0].pr.permissions, { dana: "write" }, "rita's permission could not be read and is absent");
  assert.equal(entries[1].pr, null);
  assert.throws(() => gatherRange({ api: fakeApi({ total: 300 }), repository: "o/r", base: "0".repeat(40), head: "c".repeat(40) }), /judge it in parts/);
  assert.throws(() => gatherRange({ api: fakeApi({ repeat: true }), repository: "o/r", base: "0".repeat(40), head: "c".repeat(40) }), /judge it in parts/, "a page repeated in place of another is not the whole range");
  // technical-writer may not touch settings.json even approved: the gathered
  // range is refused for the earlier commit, which a first-parent check skipped.
  const { refusals } = judgeRange({ policy: POLICY, entries });
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /^bbbbbbbbbbbb: the technical-writer role/);
});

// Mutation: read only the first page of the compare answer. deploy.yml run
// 262 refused a 1,456-commit range on the single-response limit; a range the
// API pages through must be judged whole, the later pages included.
test("a range the compare endpoint pages through is gathered whole, and its later pages are judged", () => {
  const entries = gatherRange({ api: fakeApi({ split: true }), repository: "o/r", base: "0".repeat(40), head: "c".repeat(40) });
  assert.equal(entries.length, 2, "the commit on the second page was gathered");
  assert.equal(entries[1].sha, "c".repeat(40));
  assert.equal(entries[1].pr, null);
  const { refusals } = judgeRange({ policy: POLICY, entries });
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /^bbbbbbbbbbbb: the technical-writer role/);
});

// CICD-9083: the committed round-3 manifest is the one the guard reads, and
// an agent commit naming its lane is held to that lane's territory.
const LANES = loadLanes(POLICY, new URL("./agent-policy.json", import.meta.url).pathname);
const laneCommit = (role, lane) => ({
  commit: {
    author: { name: "Claude", email: "noreply@anthropic.com" },
    message: `Do a thing\n\nAgent-Role: ${role}\nAgent-Lane: ${lane}\nCo-Authored-By: Claude <noreply@anthropic.com>\n`,
  },
});

// Mutation: drop the lane check (1b) in judgeChange.
test("an agent commit outside its declared lane's territory is refused, and inside it is admitted", () => {
  assert.ok(LANES.lanes?.has("r3-capital-code"), `premise: the committed manifest loads (${LANES.problem})`);
  const judge = (files) =>
    judgeChange({
      policy: POLICY, permissions: PERMS, lanes: LANES, headRef: "ccr-dc3c4d84-dayxv0-r3-capital", headSha: HEAD,
      files, commits: [laneCommit("backend-engineer", "r3-capital-code")], reviews: [],
    }).refusals;
  assert.deepEqual(judge(["backend/crates/services/qip-risk-engine/src/pretrade.rs"]), []);
  // Inside the backend-engineer role, outside the capital lane: the trust
  // lane's file. This is the overlap the plan ruled out, arriving anyway.
  const refusals = judge(["backend/crates/runtime/qip-kernel/src/model_serving.rs"]);
  assert.equal(refusals.length, 1);
  assert.match(refusals[0], /the r3-capital-code lane does not own: backend\/crates\/runtime\/qip-kernel\/src\/model_serving\.rs/);
});

// Mutation: skip an unknown lane, or treat an unreadable manifest as no lanes.
test("a commit naming a lane the manifest lacks, or with no readable manifest, is refused", () => {
  const judge = (lanes, id) =>
    judgeChange({
      policy: POLICY, permissions: PERMS, lanes, headRef: "x", headSha: HEAD,
      files: ["backend/crates/services/qip-risk-engine/src/pretrade.rs"], commits: [laneCommit("backend-engineer", id)], reviews: [],
    }).refusals;
  assert.match(judge(LANES, "r9-nobody")[0], /does not define/);
  const unreadable = loadLanes(POLICY, "/nowhere/scripts/agent-policy.json");
  assert.ok(unreadable.problem);
  assert.match(judge(unreadable, "r3-capital-code")[0], /cannot be read/);
  assert.ok(loadLanes({ lane_manifest: { path: "../etc/passwd" } }, "scripts/agent-policy.json").problem);
});
