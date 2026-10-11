/**
 * The CodeQL blocking gate (R12 CICD-9105).
 *
 *   node scripts/sast-gate.mjs --policy scripts/sast-policy.json <sarif-dir>
 *
 * `github/codeql-action/analyze` uploads its findings and succeeds. Until
 * this script, that success was the whole of the `sast` job, so "static
 * analysis passed" in `deploy.yml`'s gate meant "the analysis ran" and never
 * "it found nothing" — a commit with a critical CodeQL alert was built,
 * signed and attested exactly like one without. This reads the SARIF the
 * analysis wrote and exits non-zero when any result's rule carries a
 * `security-severity` at or above the committed threshold in
 * `scripts/sast-policy.json`. The job then fails, ci fails, and `deploy.yml`
 * — which already refuses a ci run whose `sast / …` jobs did not all
 * succeed — refuses the commit.
 *
 * ## It fails closed
 *
 * - No SARIF file in the directory: refused. An analysis that wrote nothing
 *   is not an analysis that found nothing.
 * - A SARIF file that does not parse, or holds no run: refused.
 * - A policy that is missing, unparseable, or whose threshold is not a
 *   number in [0, 10]: refused. There is no default threshold; a gate whose
 *   bar came from nowhere is a bar nobody set.
 * - A rule's `security-severity` present but not a number: counted as
 *   blocking, because an unreadable severity on a security rule is not
 *   evidence that it is low.
 *
 * Results with no `security-severity` at all are the code-quality half of
 * the `security-and-quality` suite. They are printed and do not block; the
 * policy file says so in its own words.
 *
 * Dev tooling. Node built-ins only.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** The policy, validated. Returns `{ policy }` or `{ problem }`. */
export function readPolicy(text) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    return { problem: `the SAST policy is not JSON: ${cause.message}` };
  }
  const threshold = parsed?.min_security_severity;
  if (typeof threshold !== "number" || !Number.isFinite(threshold) || threshold < 0 || threshold > 10) {
    return { problem: "the SAST policy's min_security_severity is not a number between 0 and 10" };
  }
  const baseline = parsed?.baseline;
  if (baseline !== undefined && (typeof baseline !== "string" || baseline.length === 0)) {
    return { problem: "the SAST policy's baseline, when present, must be a file path" };
  }
  return { policy: { minSecuritySeverity: threshold, band: String(parsed.band ?? ""), baseline } };
}

/** Every rule a run declares, by id, from the driver and any extensions. */
function rulesOf(run) {
  const rules = new Map();
  const components = [run?.tool?.driver, ...(run?.tool?.extensions ?? [])];
  for (const component of components) {
    for (const rule of component?.rules ?? []) {
      if (rule && typeof rule.id === "string") rules.set(rule.id, rule);
    }
  }
  return rules;
}

/** The rule a result names: by index into the driver when given, else by id. */
function ruleFor(run, result, rules) {
  const index = result?.rule?.index ?? result?.ruleIndex;
  const toolComponent = result?.rule?.toolComponent?.index;
  if (Number.isInteger(index)) {
    const component = toolComponent === undefined ? run?.tool?.driver : run?.tool?.extensions?.[toolComponent];
    const rule = component?.rules?.[index];
    if (rule) return rule;
  }
  const id = result?.ruleId ?? result?.rule?.id;
  return typeof id === "string" ? rules.get(id) : undefined;
}

/**
 * Judge parsed SARIF documents against the policy.
 * Returns `{ blocking, informational, problems }`; blocking is a list of
 * `{ ruleId, severity, location }`.
 */
export function judge(documents, policy) {
  const blocking = [];
  const informational = [];
  const problems = [];
  if (documents.length === 0) {
    problems.push("no SARIF was produced; an analysis that wrote nothing is not an analysis that found nothing");
  }
  for (const { name, sarif } of documents) {
    const runs = sarif?.runs;
    if (!Array.isArray(runs) || runs.length === 0) {
      problems.push(`${name} holds no analysis run`);
      continue;
    }
    for (const run of runs) {
      const rules = rulesOf(run);
      for (const result of run?.results ?? []) {
        const rule = ruleFor(run, result, rules);
        const ruleId = result?.ruleId ?? rule?.id ?? "<unnamed rule>";
        const physical = result?.locations?.[0]?.physicalLocation;
        const location = physical
          ? `${physical.artifactLocation?.uri ?? "?"}:${physical.region?.startLine ?? "?"}`
          : "<no location>";
        const raw = rule?.properties?.["security-severity"] ?? result?.properties?.["security-severity"];
        if (raw === undefined || raw === null) {
          informational.push({ ruleId, location });
          continue;
        }
        const severity = Number(raw);
        if (!Number.isFinite(severity)) {
          blocking.push({ ruleId, severity: String(raw), location });
        } else if (severity >= policy.minSecuritySeverity) {
          blocking.push({ ruleId, severity, location });
        } else {
          informational.push({ ruleId, location, severity });
        }
      }
    }
  }
  return { blocking, informational, problems };
}

/** Every `*.sarif` file under `dir`, recursively, parsed. */
export function readSarifDirectory(dir) {
  const documents = [];
  const problems = [];
  const walk = (path) => {
    // The directory entry's own type, not a second stat of its path: a stat
    // followed by a read is a check-then-use the path can change between.
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const full = join(path, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && entry.name.endsWith(".sarif")) {
        try {
          documents.push({ name: full, sarif: JSON.parse(readFileSync(full, "utf8")) });
        } catch (cause) {
          problems.push(`${full} is not readable SARIF: ${cause.message}`);
        }
      }
    }
  };
  try {
    walk(dir);
  } catch (cause) {
    problems.push(`cannot read ${dir}: ${cause.message}`);
  }
  return { documents, problems };
}

/** The command line, end to end. Returns the exit code; writes to `out`/`err`. */
/**
 * Apply the ratchet: a blocking finding inside its `<rule> <file>`
 * allowance is moved to `baselined`, and anything beyond the allowance
 * stays blocking. Returns `{ blocking, baselined, shrinkable }`, where
 * `shrinkable` names allowances larger than what the analysis found, so
 * the change that fixed them can lower the number.
 */
export function applyBaseline(blocking, allowances = {}) {
  const used = new Map();
  const kept = [];
  const baselined = [];
  for (const finding of blocking) {
    const file = String(finding.location).replace(/:[^:]*$/, "");
    const key = `${finding.ruleId} ${file}`;
    const allowed = Number.isInteger(allowances[key]) && allowances[key] > 0 ? allowances[key] : 0;
    const n = used.get(key) ?? 0;
    if (n < allowed) {
      used.set(key, n + 1);
      baselined.push(finding);
    } else {
      kept.push(finding);
    }
  }
  const shrinkable = Object.entries(allowances)
    .filter(([key, allowed]) => Number.isInteger(allowed) && (used.get(key) ?? 0) < allowed)
    .map(([key, allowed]) => `${key}: allowance ${allowed}, found ${used.get(key) ?? 0}`);
  return { blocking: kept, baselined, shrinkable };
}

/** Read the baseline file the policy names; refuse one that does not parse. */
export function readBaseline(text) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    return { problem: `the SAST baseline is not JSON: ${cause.message}` };
  }
  const allowances = parsed?.allowances;
  if (!allowances || typeof allowances !== "object" || Array.isArray(allowances)) {
    return { problem: "the SAST baseline has no `allowances` object" };
  }
  for (const [key, value] of Object.entries(allowances)) {
    if (!Number.isInteger(value) || value < 0) return { problem: `baseline allowance for ${key} is not a non-negative integer` };
  }
  return { allowances };
}

export function main(argv, { out = console.log, err = console.error } = {}) {
  const policyAt = argv.indexOf("--policy");
  const policyPath = policyAt >= 0 ? argv[policyAt + 1] : undefined;
  const dirs = argv.filter((arg, i) => arg !== "--policy" && i !== policyAt + 1);
  if (!policyPath || dirs.length !== 1) {
    err("usage: sast-gate.mjs --policy <policy.json> <sarif-dir>");
    return 64;
  }
  let policyText;
  try {
    policyText = readFileSync(policyPath, "utf8");
  } catch (cause) {
    err(`cannot read the SAST policy ${policyPath}: ${cause.message}`);
    return 1;
  }
  const { policy, problem } = readPolicy(policyText);
  if (problem) {
    err(problem);
    return 1;
  }
  const { documents, problems: readProblems } = readSarifDirectory(dirs[0]);
  const judged = judge(documents, policy);
  const { informational, problems } = judged;
  let allowances = {};
  if (policy.baseline) {
    let baselineText;
    try {
      baselineText = readFileSync(policy.baseline, "utf8");
    } catch (cause) {
      err(`cannot read the SAST baseline ${policy.baseline}: ${cause.message}`);
      return 1;
    }
    const read = readBaseline(baselineText);
    if (read.problem) {
      err(read.problem);
      return 1;
    }
    allowances = read.allowances;
  }
  const { blocking, baselined, shrinkable } = applyBaseline(judged.blocking, allowances);
  for (const finding of baselined) {
    out(`baselined: ${finding.ruleId} at ${finding.location} (security-severity ${finding.severity}), allowed by ${policy.baseline}`);
  }
  for (const line of shrinkable) out(`baseline can shrink: ${line}`);
  for (const finding of informational) {
    out(`not blocking: ${finding.ruleId} at ${finding.location}${finding.severity === undefined ? " (no security-severity)" : ` (security-severity ${finding.severity})`}`);
  }
  const allProblems = [...readProblems, ...problems];
  for (const message of allProblems) err(message);
  for (const finding of blocking) {
    err(`BLOCKING: ${finding.ruleId} at ${finding.location}, security-severity ${finding.severity} >= ${policy.minSecuritySeverity}`);
  }
  if (allProblems.length > 0 || blocking.length > 0) {
    err(`static analysis refused: ${blocking.length} finding(s) at or above ${policy.minSecuritySeverity} (${policy.band}), ${allProblems.length} problem(s). Nothing this commit builds is deployable until they are fixed.`);
    return 1;
  }
  out(`static analysis: no finding at or above security-severity ${policy.minSecuritySeverity} (${policy.band}) in ${documents.length} SARIF file(s); ${informational.length} below the bar.`);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
