/**
 * A pull request that changes a rule names the rule it changes (R12 GOV-9080).
 *
 *   node scripts/rule-citation.mjs --event "$GITHUB_EVENT_PATH" --changed <file>
 *
 * The working agreement says an agent proposing an exception must cite the
 * rule it is changing. Without this, the citation was a request in prose: a
 * pull request could relax `.claude/rules/01-security-and-safety.md`, widen
 * `scripts/dependency-allowlist.txt` or edit the guard hook with a summary
 * that never mentioned it, and the reviewer's only signal was the diff's
 * file list. This reads the pull request's body from the event payload and
 * the list of changed paths, and refuses when a rule file changed and the
 * body's `**Rule changed:**` line does not name it by path.
 *
 * ## What counts as a rule
 *
 * `RULE_PATHS` below: the instruction files every agent loads, the ADR
 * register, the committed policies scripts enforce, the guard hooks and the
 * code-owner map. Each of them is a file whose edit changes what some other
 * change is allowed to do.
 *
 * ## It fails closed
 *
 * - No `**Rule changed:**` line in the body: refused, whatever changed. The
 *   template carries the line; a body without it was written around it.
 * - A rule file changed and the line does not name it: refused, naming
 *   every uncited path.
 * - The event file missing, unparseable, or not a pull request: refused.
 *
 * Read from the event file rather than from an expression pasted into the
 * script, because a pull request's body is text a stranger wrote.
 *
 * Dev tooling. Node built-ins only.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** Paths whose change changes a rule. A prefix ending in `/` is a directory. */
export const RULE_PATHS = [
  "CLAUDE.md",
  ".claude/rules/",
  ".claude/settings.json",
  ".claude/hooks/",
  "docs/adr/",
  "scripts/dependency-allowlist.txt",
  "scripts/agent-policy.json",
  "scripts/sast-policy.json",
  ".github/CODEOWNERS",
];

/** Whether `path` is a rule file. `CLAUDE.md` counts at any depth. */
export function isRule(path) {
  if (path === "CLAUDE.md" || path.endsWith("/CLAUDE.md")) return true;
  return RULE_PATHS.some((rule) =>
    rule.endsWith("/") ? path.startsWith(rule) : path === rule,
  );
}

/** The text after `**Rule changed:**`, or null when the line is absent. */
export function citation(body) {
  for (const line of String(body ?? "").split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith("**Rule changed:**")) {
      return trimmed.slice("**Rule changed:**".length).trim();
    }
  }
  return null;
}

/**
 * The verdict for one pull request: `{ ok: true }` or `{ ok: false, problem }`.
 */
export function judge(body, changed) {
  const cited = citation(body);
  if (cited === null) {
    return {
      ok: false,
      problem:
        "the pull request body has no '**Rule changed:**' line. Restore it from " +
        ".github/pull_request_template.md and write 'none' or the rule file's path.",
    };
  }
  const rules = changed.filter(isRule);
  const uncited = rules.filter((path) => !cited.includes(path));
  if (uncited.length > 0) {
    return {
      ok: false,
      problem:
        `this pull request changes ${uncited.length} rule file(s) its '**Rule changed:**' ` +
        `line does not name: ${uncited.join(", ")}. Name each one by path, and say in ` +
        "the summary what it relaxes or tightens.",
    };
  }
  return { ok: true, rules };
}

export function main(argv) {
  const at = (flag) => {
    const index = argv.indexOf(flag);
    return index >= 0 ? argv[index + 1] : undefined;
  };
  const eventPath = at("--event");
  const changedPath = at("--changed");
  if (!eventPath || !changedPath) {
    console.error("usage: rule-citation.mjs --event <event.json> --changed <paths.txt>");
    return 2;
  }
  let event;
  let changed;
  try {
    event = JSON.parse(readFileSync(eventPath, "utf8"));
    changed = readFileSync(changedPath, "utf8")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
  } catch (error) {
    console.error(`rule-citation: cannot read the inputs: ${error.message}`);
    return 2;
  }
  if (!event || typeof event.pull_request !== "object" || event.pull_request === null) {
    console.error("rule-citation: the event is not a pull request; nothing to judge is a refusal.");
    return 2;
  }
  const verdict = judge(event.pull_request.body, changed);
  if (!verdict.ok) {
    console.error(`rule-citation: ${verdict.problem}`);
    return 1;
  }
  console.log(
    verdict.rules.length === 0
      ? "rule-citation: no rule file changed"
      : `rule-citation: every changed rule is cited: ${verdict.rules.join(", ")}`,
  );
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
