/**
 * Development-agent boundaries, enforced in CI (R12 GOV-9067, SEC-9068,
 * CICD-9076; policy in `scripts/agent-policy.json`).
 *
 *   node scripts/agent-guard.mjs change --policy scripts/agent-policy.json \
 *     --head-ref <branch> --head-sha <sha> \
 *     --files files.json --commits commits.json --reviews reviews.json
 *   node scripts/agent-guard.mjs dispatch --policy scripts/agent-policy.json \
 *     --actor <login> --workflow deploy.yml
 *
 * The agents in `.claude/agents/` each carry a "Paths you may change" line
 * and a "Never" list, and until this script nothing outside the agent's own
 * session read either: a coder agent that edited a workflow, or a planner
 * that edited code, produced a pull request indistinguishable from a
 * person's, and the only thing between it and `main` was whoever happened
 * to review it. This turns three of those sentences into refusals:
 *
 * 1. **Role boundaries.** A commit carrying an `Agent-Role: <role>` trailer
 *    may change only the paths that role's `may_edit` globs admit. A
 *    planner or reviewer role, whose list is empty or docs-only, cannot
 *    land code at all. An unknown role is refused, not ignored.
 * 2. **Protected paths need a person.** An agent-authored change touching
 *    a protected path — the workflows, the agents' own instructions, the
 *    gates and their policies, the paper boundary's Terraform layer — is
 *    refused until a reviewer who is not an agent has approved the head
 *    commit. An approval of an earlier commit does not carry forward.
 * 3. **No agent merges or deploys.** `dispatch` refuses an agent actor for
 *    every workflow that deploys, applies or reaches a cloud credential.
 *
 * What it cannot see is stated rather than hidden: an agent that commits
 * under its user's name, with no trailer, on an ordinary branch, is a
 * person as far as this script knows. The markers are in the policy file.
 *
 * Dev tooling. Node built-ins only.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** A path glob as a whole-string regular expression: `**` crosses `/`, `*` does not. */
export function globToRegExp(glob) {
  let pattern = "";
  for (let i = 0; i < glob.length; i += 1) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      if (glob[i + 2] === "/") {
        pattern += "(?:.*/)?";
        i += 2;
      } else {
        pattern += ".*";
        i += 1;
      }
    } else if (c === "*") {
      pattern += "[^/]*";
    } else if (c === "?") {
      pattern += "[^/]";
    } else {
      pattern += c.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    }
  }
  return new RegExp(`^${pattern}$`);
}

export const matchesAny = (path, globs) => globs.some((glob) => globToRegExp(glob).test(path));

const lower = (value) => String(value ?? "").trim().toLowerCase();

/** The `Key: value` trailers of a commit message, keys lower-cased. */
export function trailers(message) {
  const found = [];
  for (const line of String(message ?? "").split("\n")) {
    const match = /^([A-Za-z][A-Za-z-]*):\s*(.+)$/.exec(line.trim());
    if (match) found.push({ key: match[1].toLowerCase(), value: match[2].trim() });
  }
  return found;
}

/** Whether one commit, in GitHub's API shape or flattened, carries an agent marker. */
export function isAgentCommit(commit, markers) {
  const inner = commit?.commit ?? commit;
  const name = lower(inner?.author?.name ?? commit?.author_name);
  const email = lower(inner?.author?.email ?? commit?.author_email);
  if ((markers.author_names ?? []).some((n) => lower(n) === name)) return true;
  if ((markers.author_emails ?? []).some((e) => lower(e) === email)) return true;
  const coAuthors = trailers(inner?.message).filter((t) => t.key === "co-authored-by");
  return coAuthors.some((t) => (markers.co_author_emails ?? []).some((e) => lower(t.value).includes(`<${lower(e)}>`)));
}

export function isAgentActor(actor, markers) {
  const login = lower(actor);
  if (!login) return false;
  return (markers.actor_patterns ?? []).some((p) => globToRegExp(lower(p)).test(login));
}

export function isAgentBranch(ref, markers) {
  const name = String(ref ?? "").replace(/^refs\/heads\//, "");
  return (markers.branch_prefixes ?? []).some((prefix) => name.startsWith(prefix));
}

/** The `Agent-Role` a commit declares, or undefined. */
export function roleOf(commit) {
  const inner = commit?.commit ?? commit;
  return trailers(inner?.message).find((t) => t.key === "agent-role")?.value;
}

/**
 * Judge a set of changes. `files` are repository paths; `commits` GitHub
 * commit objects; `reviews` GitHub review objects.
 * Returns `{ agent, refusals: string[] }`.
 */
export function judgeChange({ policy, headRef, headSha, files, commits, reviews }) {
  const markers = policy.agent_markers;
  const agentCommits = commits.filter((c) => isAgentCommit(c, markers));
  const agent = agentCommits.length > 0 || isAgentBranch(headRef, markers);
  const refusals = [];
  if (!agent) return { agent, refusals };

  // 1. Role boundaries, per commit that names a role. The commit's own file
  //    list is not in a pull request's API shape, so the whole change is held
  //    to every role the agent commits declare: a change two roles wrote
  //    must lie inside both.
  const roles = [...new Set(agentCommits.map(roleOf).filter(Boolean))];
  for (const role of roles) {
    const entry = policy.roles?.[role];
    if (!entry) {
      refusals.push(`a commit declares Agent-Role: ${role}, which scripts/agent-policy.json does not define`);
      continue;
    }
    const outside = files.filter((f) => !matchesAny(f, entry.may_edit));
    if (outside.length > 0) {
      refusals.push(`the ${role} role (${entry.kind}) may not change: ${outside.join(", ")}`);
    }
  }

  // 2. Protected paths need a person's approval of the head commit.
  const protectedTouched = files.filter((f) => matchesAny(f, policy.protected_paths.globs));
  if (protectedTouched.length > 0) {
    const latest = new Map();
    // A reviewer's standing verdict is their latest approve, request-changes
    // or dismissal; a later plain comment does not withdraw an approval,
    // which is how GitHub itself reads them.
    for (const review of reviews) {
      const login = review?.user?.login;
      if (!login || !["APPROVED", "CHANGES_REQUESTED", "DISMISSED"].includes(review.state)) continue;
      latest.set(login, review);
    }
    const approvedByAPerson = [...latest.values()].some(
      (review) =>
        review.state === "APPROVED" &&
        review.commit_id === headSha &&
        review.user?.type !== "Bot" &&
        !isAgentActor(review.user?.login, markers),
    );
    if (!approvedByAPerson) {
      refusals.push(
        `an agent-authored change touches protected paths and no person has approved commit ${headSha}: ${protectedTouched.join(", ")}`,
      );
    }
  }
  return { agent, refusals };
}

/** Whether `actor` may start `workflow`. Returns a refusal string or undefined. */
export function judgeDispatch({ policy, actor, workflow }) {
  const refused = policy.dispatch_refused_for_agents.workflows;
  if (refused.includes(workflow) && isAgentActor(actor, policy.agent_markers)) {
    return `${actor} is an agent identity; ${workflow} deploys, applies or reaches a cloud credential, and no agent starts it. A person dispatches it.`;
  }
  return undefined;
}

/**
 * Whether a workflow file is fit to run agent work: no secret, no cloud
 * credential, no environment, nothing written back, and a time limit on
 * every job (SEC-9068, SEC-9084, CICD-9076). Reads the file's non-comment
 * lines, so a sentence explaining why `secrets.` is absent cannot trip it.
 * Returns a list of problems.
 */
export function judgeSandboxWorkflow(text) {
  const lines = String(text)
    .split("\n")
    .map((line) => line.replace(/\s+#.*$/, ""))
    .filter((line) => !line.trim().startsWith("#"));
  const body = lines.join("\n");
  const problems = [];
  if (/\bsecrets\./.test(body) || /secrets:\s*inherit/.test(body)) {
    problems.push("references a secret; an agent sandbox holds none");
  }
  if (/id-token:\s*write/.test(body)) problems.push("can mint an OIDC token, which is a cloud credential");
  if (/^\s*environment:/m.test(body)) problems.push("names a GitHub environment, whose secrets and approvals belong to deployments");
  if (/:\s*write\b/.test(body)) problems.push("grants a write permission; an agent sandbox reads");
  if (/pull_request_target/.test(body)) problems.push("runs on pull_request_target, which hands a fork's code this repository's token");
  if (!/^permissions:\s*\n\s+contents:\s*read\s*$/m.test(body)) problems.push("does not declare top-level `permissions: contents: read`");
  const jobsAt = lines.findIndex((line) => /^jobs:\s*$/.test(line));
  const jobNames = [];
  const timed = new Set();
  let current;
  for (const line of jobsAt >= 0 ? lines.slice(jobsAt + 1) : []) {
    const job = /^ {2}([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (job) {
      current = job[1];
      jobNames.push(current);
    } else if (current && /^ {4}timeout-minutes:/.test(line)) {
      timed.add(current);
    }
  }
  if (jobNames.length === 0) problems.push("declares no job");
  for (const name of jobNames) if (!timed.has(name)) problems.push(`job ${name} has no timeout-minutes`);
  return problems;
}

function option(argv, name) {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 ? argv[at + 1] : undefined;
}

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));

/** The command line, end to end. Returns the exit code. */
export function main(argv, { out = console.log, err = console.error } = {}) {
  const [mode] = argv;
  if (mode === "sandbox") {
    const paths = argv.slice(1);
    if (paths.length === 0) {
      err("usage: agent-guard.mjs sandbox <workflow.yml>…");
      return 64;
    }
    let failed = false;
    for (const path of paths) {
      let problems;
      try {
        problems = judgeSandboxWorkflow(readFileSync(path, "utf8"));
      } catch (cause) {
        problems = [`cannot be read: ${cause.message}`];
      }
      for (const problem of problems) err(`${path} ${problem}`);
      if (problems.length > 0) failed = true;
      else out(`${path} holds no secret, no credential and no write, and every job is time-limited`);
    }
    return failed ? 1 : 0;
  }
  const policyPath = option(argv, "policy");
  if (!policyPath || !["change", "dispatch"].includes(mode)) {
    err("usage: agent-guard.mjs change|dispatch --policy <agent-policy.json> … | sandbox <workflow.yml>…");
    return 64;
  }
  let policy;
  try {
    policy = readJson(policyPath);
  } catch (cause) {
    err(`cannot read the agent policy ${policyPath}: ${cause.message}`);
    return 1;
  }
  if (mode === "dispatch") {
    const actor = option(argv, "actor");
    const workflow = option(argv, "workflow");
    if (!actor || !workflow) {
      err("dispatch needs --actor and --workflow");
      return 64;
    }
    const refusal = judgeDispatch({ policy, actor, workflow });
    if (refusal) {
      err(refusal);
      return 1;
    }
    out(`${actor} may start ${workflow}`);
    return 0;
  }
  const paths = ["files", "commits", "reviews"].map((name) => option(argv, name));
  const headSha = option(argv, "head-sha");
  if (paths.some((p) => !p) || !headSha) {
    err("change needs --head-sha, --files, --commits and --reviews");
    return 64;
  }
  let files;
  let commits;
  let reviews;
  try {
    files = readJson(paths[0]).map((f) => (typeof f === "string" ? f : f.filename));
    commits = readJson(paths[1]);
    reviews = readJson(paths[2]);
  } catch (cause) {
    err(`cannot read the change: ${cause.message}`);
    return 1;
  }
  if (files.some((f) => typeof f !== "string")) {
    err("the change's file list names something that is not a path");
    return 1;
  }
  const { agent, refusals } = judgeChange({ policy, headRef: option(argv, "head-ref"), headSha, files, commits, reviews });
  if (!agent) {
    out(`no agent marker on ${commits.length} commit(s); the agent boundary does not apply`);
    return 0;
  }
  for (const refusal of refusals) err(refusal);
  if (refusals.length > 0) return 1;
  out(`agent-authored change of ${files.length} file(s) is inside its role boundaries and needs no further approval, or has it`);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
