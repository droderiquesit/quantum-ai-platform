/**
 * Agent lanes: decomposition, acceptance, budgets and ownership (R12
 * CICD-9082, CICD-9083; policy in `scripts/agent-policy.json`).
 *
 *   node scripts/agent-lanes.mjs validate   --policy <p> --manifest <m>
 *   node scripts/agent-lanes.mjs budget     --policy <p> --manifest <m> --lane <id>
 *   node scripts/agent-lanes.mjs check-diff --policy <p> --manifest <m> --lane <id> --files <list>
 *   node scripts/agent-lanes.mjs accept     --policy <p> --manifest <m> --lane <id> --cwd <checkout>
 *
 * A lane manifest is the orchestrator's plan, committed before any lane
 * starts: one entry per lane, naming the role it runs as, the paths it
 * owns, the lanes it waits for, the commands that decide whether it is
 * done, and what it may spend.
 *
 *   { "lanes": [ { "id": "r2-infra", "role": "cloud-platform-engineer",
 *       "owns": ["infrastructure/terraform/modules/build-pool/**"],
 *       "depends_on": [], "acceptance": ["terraform test"],
 *       "budget": { "minutes": 120, "usd": 10 } } ] }
 *
 * What it refuses, and the failure each prevents:
 *
 * - **Two lanes owning overlapping paths with no order between them.** On
 *   2026-09-14 six parallel lanes each read the ADR register, each wrote the
 *   same next number, and 74 files had to be renumbered. Overlap is allowed
 *   only when one lane depends, directly or through others, on the other —
 *   which serialises them; `validate` prints the waves that order implies.
 * - **A dependency cycle**, which is a plan no wave can start.
 * - **A lane with no acceptance command**, or with a budget that is zero,
 *   missing or above the policy's ceiling. "Done" with no criterion is a
 *   lane's own opinion, and an unbounded lane is a bill nobody approved.
 * - **A lane owning a path its role may not edit** — the role table is the
 *   one `agent-guard.mjs` enforces on the pull request, so the plan cannot
 *   promise a lane what the merge will refuse it.
 * - **A lane's diff leaving what it owns** (`check-diff`), which is the
 *   overlap the plan ruled out arriving anyway.
 *
 * Dev tooling. Node built-ins only.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { matchesAny } from "./agent-guard.mjs";

const LANE_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;

/** The literal directory prefix of a glob: everything before its first wildcard. */
export function literalPrefix(glob) {
  const at = glob.search(/[*?]/);
  return at < 0 ? glob : glob.slice(0, at);
}

/** Whether two ownership globs can name a common path, judged by their literal prefixes. */
export function globsOverlap(a, b) {
  const pa = literalPrefix(a);
  const pb = literalPrefix(b);
  if (!/[*?]/.test(a) && !/[*?]/.test(b)) return a === b;
  if (!/[*?]/.test(a)) return matchesAny(a, [b]);
  if (!/[*?]/.test(b)) return matchesAny(b, [a]);
  return pa.startsWith(pb) || pb.startsWith(pa);
}

/** A representative path under a glob, for testing it against a role's grants. */
const sample = (glob) => glob.replace(/\*\*/g, "x").replace(/\*/g, "x").replace(/\?/g, "x");

/**
 * Validate a manifest. Returns `{ problems, waves }`; `waves` is the lane ids
 * in dependency order, each wave runnable in parallel, empty when there are
 * problems.
 */
export function validate(manifest, policy) {
  const problems = [];
  const lanes = manifest?.lanes;
  if (!Array.isArray(lanes) || lanes.length === 0) return { problems: ["the manifest declares no lanes"], waves: [] };
  const limits = policy.lane_limits;
  const byId = new Map();
  for (const lane of lanes) {
    const id = lane?.id;
    if (typeof id !== "string" || !LANE_ID.test(id)) {
      problems.push(`a lane id is not lowercase letters, digits and -: ${JSON.stringify(id)}`);
      continue;
    }
    if (byId.has(id)) problems.push(`two lanes are named ${id}`);
    byId.set(id, lane);
    const role = policy.roles?.[lane.role];
    if (!role) problems.push(`${id} runs as ${JSON.stringify(lane.role)}, which is not a role in agent-policy.json`);
    if (!Array.isArray(lane.owns) || lane.owns.some((g) => typeof g !== "string" || g === "")) {
      problems.push(`${id} does not list the paths it owns`);
    } else if (role) {
      for (const glob of lane.owns) {
        if (!matchesAny(sample(glob), role.may_edit)) problems.push(`${id} owns ${glob}, which the ${lane.role} role may not edit`);
      }
    }
    if (!Array.isArray(lane.acceptance) || lane.acceptance.length === 0 || lane.acceptance.some((c) => typeof c !== "string" || c.trim() === "")) {
      problems.push(`${id} has no acceptance command; "done" would be the lane's own opinion`);
    }
    const minutes = lane.budget?.minutes;
    const usd = lane.budget?.usd;
    if (!Number.isFinite(minutes) || minutes <= 0 || minutes > limits.max_minutes) {
      problems.push(`${id}'s budget.minutes must be in (0, ${limits.max_minutes}]`);
    }
    if (!Number.isFinite(usd) || usd < 0 || usd > limits.max_usd) {
      problems.push(`${id}'s budget.usd must be in [0, ${limits.max_usd}]`);
    }
    if (lane.depends_on !== undefined && !Array.isArray(lane.depends_on)) problems.push(`${id}'s depends_on is not a list`);
  }
  for (const [id, lane] of byId) {
    for (const dep of lane.depends_on ?? []) {
      if (!byId.has(dep)) problems.push(`${id} depends on ${dep}, which is not a lane`);
    }
  }
  if (problems.length > 0) return { problems, waves: [] };

  // Waves by Kahn's algorithm; anything left over is in a cycle.
  const remaining = new Map([...byId].map(([id, lane]) => [id, new Set(lane.depends_on ?? [])]));
  const waves = [];
  while (remaining.size > 0) {
    const ready = [...remaining].filter(([, deps]) => deps.size === 0).map(([id]) => id).sort();
    if (ready.length === 0) {
      problems.push(`a dependency cycle among: ${[...remaining.keys()].sort().join(", ")}`);
      return { problems, waves: [] };
    }
    waves.push(ready);
    for (const id of ready) remaining.delete(id);
    for (const deps of remaining.values()) for (const id of ready) deps.delete(id);
  }

  // Ownership overlap must be ordered.
  const ancestors = new Map();
  const ancestorsOf = (id) => {
    if (ancestors.has(id)) return ancestors.get(id);
    const found = new Set();
    for (const dep of byId.get(id).depends_on ?? []) {
      found.add(dep);
      for (const a of ancestorsOf(dep)) found.add(a);
    }
    ancestors.set(id, found);
    return found;
  };
  const ids = [...byId.keys()].sort();
  for (let i = 0; i < ids.length; i += 1) {
    for (let j = i + 1; j < ids.length; j += 1) {
      const a = byId.get(ids[i]);
      const b = byId.get(ids[j]);
      const shared = a.owns.flatMap((ga) => b.owns.filter((gb) => globsOverlap(ga, gb)).map((gb) => `${ga} ~ ${gb}`));
      if (shared.length === 0) continue;
      const ordered = ancestorsOf(a.id).has(b.id) || ancestorsOf(b.id).has(a.id);
      if (!ordered) {
        problems.push(`${a.id} and ${b.id} own overlapping paths (${shared.join("; ")}) and neither waits for the other; make one depend on the other so they are serialised`);
      }
    }
  }
  return problems.length > 0 ? { problems, waves: [] } : { problems, waves };
}

/**
 * Run a lane's acceptance commands, in order, inside `cwd`, within its
 * budget's minutes: every command is given only what the budget has left,
 * and one that runs past it is killed and counts as a failure. Stops at the
 * first failure. `spawn` is injectable for the tests. Returns
 * `{ passed, results: [{ command, status, ms, timedOut }] }`.
 */
export function runAcceptance(lane, { cwd, spawn = spawnSync, now = Date.now, budgetMs } = {}) {
  const deadline = now() + (budgetMs ?? lane.budget.minutes * 60_000);
  const results = [];
  for (const command of lane.acceptance) {
    const remaining = deadline - now();
    if (remaining <= 0) {
      results.push({ command, status: null, ms: 0, timedOut: true });
      return { passed: false, results };
    }
    const started = now();
    const run = spawn("bash", ["-c", command], { cwd, stdio: "inherit", timeout: remaining, killSignal: "SIGKILL" });
    const timedOut = run.error?.code === "ETIMEDOUT" || (run.signal === "SIGKILL" && now() >= deadline);
    results.push({ command, status: run.status, ms: now() - started, timedOut });
    if (run.status !== 0 || timedOut) return { passed: false, results };
  }
  return { passed: true, results };
}

/** The files a lane's diff touches that it does not own. */
export function outsideOwnership(lane, files) {
  return files.filter((f) => !matchesAny(f, lane.owns));
}

function option(argv, name) {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 ? argv[at + 1] : undefined;
}

/** The command line, end to end. Returns the exit code. */
export function main(argv, { out = console.log, err = console.error } = {}) {
  const [mode] = argv;
  const policyPath = option(argv, "policy");
  const manifestPath = option(argv, "manifest");
  if (!["validate", "budget", "check-diff", "accept"].includes(mode) || !policyPath || !manifestPath) {
    err("usage: agent-lanes.mjs validate|budget|check-diff|accept --policy <p> --manifest <m> [--lane <id>] [--files <list>] [--cwd <dir>]");
    return 64;
  }
  let policy;
  let manifest;
  try {
    policy = JSON.parse(readFileSync(policyPath, "utf8"));
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (cause) {
    err(`cannot read the policy or the manifest: ${cause.message}`);
    return 1;
  }
  const { problems, waves } = validate(manifest, policy);
  if (problems.length > 0) {
    for (const problem of problems) err(problem);
    return 1;
  }
  if (mode === "validate") {
    waves.forEach((wave, i) => out(`wave ${i + 1}: ${wave.join(", ")}`));
    return 0;
  }
  const lane = manifest.lanes.find((l) => l.id === option(argv, "lane"));
  if (!lane) {
    err(`no lane ${JSON.stringify(option(argv, "lane"))} in the manifest`);
    return 1;
  }
  if (mode === "budget") {
    out(String(lane.budget.minutes));
    return 0;
  }
  if (mode === "accept") {
    const { passed, results } = runAcceptance(lane, { cwd: option(argv, "cwd") ?? process.cwd() });
    for (const r of results) {
      out(`${r.timedOut ? "OUT OF BUDGET" : r.status === 0 ? "pass" : `fail (${r.status})`}  ${r.ms} ms  ${r.command}`);
    }
    if (!passed) {
      err(`${lane.id} did not meet its acceptance criteria within ${lane.budget.minutes} minute(s)`);
      return 1;
    }
    out(`${lane.id} met all ${results.length} acceptance command(s)`);
    return 0;
  }
  const filesPath = option(argv, "files");
  if (!filesPath) {
    err("check-diff needs --files, one path per line");
    return 64;
  }
  const files = readFileSync(filesPath, "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
  const outside = outsideOwnership(lane, files);
  if (outside.length > 0) {
    err(`${lane.id} changed paths it does not own: ${outside.join(", ")}`);
    return 1;
  }
  out(`${lane.id}'s ${files.length} changed file(s) are all inside what it owns`);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
