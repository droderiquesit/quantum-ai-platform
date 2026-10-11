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
 * 4. **No marker is not proof of a person, on a protected path** (GOV-9067).
 *    A change with no agent marker that touches a protected path needs
 *    every commit signed by a key a person registered (not GitHub's
 *    web-flow key), or a person's approval of the head commit.
 *
 * What it cannot see is stated rather than hidden: an agent that commits
 * under its user's name, with no trailer, on an ordinary branch, is a
 * person as far as this script knows *outside* the protected paths; and on
 * them, an agent running on its user's machine with its user's signing key
 * signs as that person. The markers are in the policy file; ADR 0117 holds
 * the rest.
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

/**
 * Whether one commit, in GitHub's API shape or flattened, carries an agent
 * marker. The committer is read as well as the author: a session that
 * rewrites the author to its user but commits under its own identity is
 * still marked.
 */
export function isAgentCommit(commit, markers) {
  const inner = commit?.commit ?? commit;
  const names = [inner?.author?.name ?? commit?.author_name, inner?.committer?.name ?? commit?.committer_name].map(lower);
  const emails = [inner?.author?.email ?? commit?.author_email, inner?.committer?.email ?? commit?.committer_email].map(lower);
  if ((markers.author_names ?? []).some((n) => names.includes(lower(n)))) return true;
  if ((markers.author_emails ?? []).some((e) => emails.includes(lower(e)))) return true;
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

/**
 * Whether a commit carries a signature GitHub verified against a key a
 * person registered — and not GitHub's own web-flow key, which signs every
 * commit made through the API or the web editor, an agent's included. A
 * session working through its user's token can produce a verified commit;
 * it cannot produce one signed by a key that lives on its user's machine
 * unless it runs on that machine with that key, which ADR 0117 names.
 */
export function isPersonSigned(commit, unmarked) {
  const inner = commit?.commit ?? commit;
  if (inner?.verification?.verified !== true) return false;
  const committer = lower(inner?.committer?.email ?? commit?.committer_email);
  return !(unmarked.web_flow_committer_emails ?? []).some((e) => lower(e) === committer);
}

/**
 * Whether a person who is not an agent has an approval standing on the head
 * commit. A reviewer's standing verdict is their latest approve,
 * request-changes or dismissal; a later plain comment does not withdraw an
 * approval, which is how GitHub itself reads them.
 */
export function approvedByAPerson(reviews, headSha, markers) {
  const latest = new Map();
  for (const review of reviews) {
    const login = review?.user?.login;
    if (!login || !["APPROVED", "CHANGES_REQUESTED", "DISMISSED"].includes(review.state)) continue;
    latest.set(login, review);
  }
  return [...latest.values()].some(
    (review) =>
      review.state === "APPROVED" &&
      review.commit_id === headSha &&
      review.user?.type !== "Bot" &&
      !isAgentActor(review.user?.login, markers),
  );
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
  const protectedTouched = files.filter((f) => matchesAny(f, policy.protected_paths.globs));
  if (!agent) {
    // GOV-9067. With no marker the change is a person's only by default, and
    // an agent pushing as its user with no trailer is exactly that default.
    // On a protected path the default is not enough: every commit must carry
    // a person's own signature, or a person must approve the head commit.
    // Either is evidence an agent session working through its user's token
    // cannot manufacture; the absence of a marker is evidence of nothing.
    const unmarked = policy.unmarked_protected_change;
    if (!unmarked) {
      refusals.push("scripts/agent-policy.json has no unmarked_protected_change block; a protected change with no marker cannot be judged");
    } else if (protectedTouched.length > 0) {
      // A merge commit is exempt: GitHub's merge button writes one, signs it
      // with the web-flow key, and a person's signed pull request would
      // otherwise be refused for the button that merged it. What that admits
      // — a merge commit carrying its own content — is named in ADR 0117.
      const isMerge = (c) => (c?.parents?.length ?? 0) >= 2;
      const unsigned = commits.filter((c) => !isMerge(c) && !isPersonSigned(c, unmarked));
      if (unsigned.length > 0 && !approvedByAPerson(reviews, headSha, markers)) {
        refusals.push(
          `a change with no agent marker touches protected paths, ${unsigned.length} of its ${commits.length} commit(s) carry no ` +
            `person's verified signature, and no person has approved commit ${headSha}: ${protectedTouched.join(", ")}. ` +
            "Sign the commits with a key registered on GitHub, or have a person approve the head commit",
        );
      }
    }
    return { agent, refusals };
  }

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

  // 2. Protected paths need a person's approval of the head commit. A
  //    signature does not stand in for it here: an agent's commit signed by
  //    its user's key is still an agent's commit.
  if (protectedTouched.length > 0) {
    if (!approvedByAPerson(reviews, headSha, markers)) {
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
 * credential, no environment, nothing written back, a time limit on every
 * job (SEC-9068, SEC-9084, CICD-9076), and the lane's network confined to
 * the egress proxy with its verdict taken (SEC-9077, ADR 0117). Reads the file's non-comment
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
  // SEC-9077: the lane's network is the egress proxy and nothing else. Each
  // piece is required by name because each alone is the hole: the firewall
  // without no_new_privs is undone by the runner's own `sudo`, the proxy
  // without the firewall is a suggestion a lane can ignore, and either without
  // the verdict refuses an attempt silently and passes the run.
  const confinement = [
    [/--gid-owner\s+lane-egress\b[^\n]*-j\s+REJECT/, "does not firewall the lane's group off everything but loopback"],
    [/--no-new-privs\b/, "runs the lane without no_new_privs, so the runner's sudo can lift it out of its firewall"],
    [/--regid="?\$lane_gid"?/, "does not run the lane under the firewalled group"],
    [/agent-egress\.mjs serve\b/, "starts no egress proxy, so the allow-list and quota are enforced by nothing"],
    [/agent-egress\.mjs verdict\b/, "takes no egress verdict, so a refused attempt does not fail the run"],
  ];
  for (const [pattern, problem] of confinement) if (!pattern.test(body)) problems.push(problem);
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
      else out(`${path} holds no secret, no credential and no write, every job is time-limited, and the lane's network is the egress proxy`);
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
  for (const refusal of refusals) err(refusal);
  if (!agent) {
    if (refusals.length > 0) return 1;
    out(`no agent marker on ${commits.length} commit(s), and nothing protected is touched without a person's signature or approval`);
    return 0;
  }
  if (refusals.length > 0) return 1;
  out(`agent-authored change of ${files.length} file(s) is inside its role boundaries and needs no further approval, or has it`);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
