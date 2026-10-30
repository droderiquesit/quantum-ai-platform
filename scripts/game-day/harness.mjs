/**
 * The game-day harness (R12 RES-10186 and the G3 §25 drills; W5.5).
 *
 *   node scripts/game-day/harness.mjs run --scenario scripts/game-day/scenarios/broker-loss.json \
 *     --bin backend/target/debug --out docs/ops/game-days/results
 *   node scripts/game-day/harness.mjs validate docs/ops/game-days/results/*.json
 *
 * A game day is a fault injected on purpose against the platform's real
 * binaries, with the recovery timed and the data loss counted, so that an
 * RTO or RPO in a runbook is a number somebody measured rather than one
 * somebody hoped. Until this harness there was no drill of any kind: the
 * degradation envelopes were unit-tested inside one process each, and
 * nothing had ever killed a broker under a running cell and watched what
 * the cell did.
 *
 * ## What a scenario is
 *
 * A JSON file under `scenarios/`: the processes to start (a binary from
 * `--bin`, its environment, the probe that says it is ready), the steady
 * state that must hold before anything is broken, one fault (`kill` —
 * SIGKILL, a crash; `pause` — SIGSTOP, a partition that hangs connections
 * rather than refusing them; `write_file` — a file the process reads, such
 * as a halt flag or a stale policy), what must stay true while the fault
 * lasts, the recovery (`restart` with the same data, `resume`, or
 * `remove_file`), and the objectives the measurement is judged against.
 *
 * Optionally (R12-RES-10153, the frozen-sink drill): `load`, one-shot
 * processes such as `qip event-fabric produce` started once every process
 * is ready and before the steady state is judged, each waited for after the
 * recovery and failed unless it exits 0, its `RESULT {json}` line kept in
 * the record beside what the broker counted; `final`, checks read once the
 * load has stopped, for counts that must hold over the whole run; and a
 * metric check's `absent_as`, the value a series nothing has minted yet
 * reads as — a refusal counter is created by its first refusal, so "never
 * refused" is an absent series. An `after_recovery` check with `increases`
 * is judged against the value read at the fault.
 *
 * ## What it measures, exactly
 *
 * - `detected_ms`: fault to the first probe that sees it (the target's
 *   readiness failing, or a metric moving). Null if nothing saw it.
 * - `rto_ms`: recovery action to every process ready and every
 *   `after_recovery` check holding. This is the number a runbook's RTO is
 *   compared with.
 * - `outage_ms`: fault to that same moment, the user-visible gap.
 * - `rpo_records`: for a scenario that names a durable position (a fabric
 *   high watermark, a consumer lag), how many records the recovered system
 *   holds fewer of than it had acknowledged before the fault. Never
 *   negative; null when the scenario names no position.
 *
 * ## What it refuses to claim
 *
 * A scenario whose steady state never establishes, or whose expectation
 * the local binaries cannot exercise, is `inconclusive`, never `met`. A
 * timeout is `missed` with the measurement it reached. A result is written
 * for every run, including the ones that fail, because a drill that only
 * records its passes is a drill that has stopped being one.
 *
 * Paper only, local only: every process here runs on loopback with the
 * simulated venue feed, and no scenario may set a venue adapter, an
 * autonomy level or a cloud endpoint — `loadScenario` refuses one that does.
 *
 * Dev tooling. Node built-ins only.
 */
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const RESULT_SCHEMA = "qip.game-day.result/v1";
const VERDICTS = ["met", "missed", "inconclusive", "aborted"];
const FAULTS = ["kill", "pause", "write_file", "remove_path"];
const RECOVERIES = ["restart", "resume", "remove_file", "make_dir", "none"];

/**
 * Environment names a scenario may never set: each would move a process off
 * the paper, local, simulated configuration this harness exists to drill.
 */
const REFUSED_ENVIRONMENT = [
  /^QIP_VENUE_ADAPTER$/,
  /^QIP_AUTONOMY/,
  /^QIP_GCP_/,
  /^GOOGLE_/,
  /^QIP_VENUE_ORDER_ENTRY_ACKNOWLEDGED$/,
];

/** Validate a parsed scenario. Returns a list of problems. */
export function checkScenario(scenario) {
  const problems = [];
  if (typeof scenario?.name !== "string" || !/^[a-z0-9-]+$/.test(scenario.name)) problems.push("name must be lowercase letters, digits and -");
  if (typeof scenario?.drill !== "string" || scenario.drill.trim() === "") problems.push("drill must say, in a sentence, what is being proved");
  const processes = scenario?.processes;
  if (!Array.isArray(processes) || processes.length === 0) problems.push("processes must list at least one process");
  const names = new Set();
  for (const p of processes ?? []) {
    if (typeof p?.name !== "string" || !/^[a-z0-9-]+$/.test(p.name)) problems.push(`a process name is not lowercase letters, digits and -: ${JSON.stringify(p?.name)}`);
    if (names.has(p?.name)) problems.push(`two processes are named ${p.name}`);
    names.add(p?.name);
    if (typeof p?.binary !== "string" || !/^[a-z0-9-]+$/.test(p.binary)) problems.push(`${p?.name}: binary must be a bare binary name, found under --bin`);
    if (!p?.ready?.url) problems.push(`${p?.name}: ready.url is required; a process with no readiness probe cannot be timed`);
    for (const key of Object.keys(p?.env ?? {})) {
      if (REFUSED_ENVIRONMENT.some((pattern) => pattern.test(key))) problems.push(`${p.name}: ${key} may not be set by a game day; drills run paper, local and simulated`);
    }
  }
  const fault = scenario?.fault;
  if (!FAULTS.includes(fault?.kind)) problems.push(`fault.kind must be one of ${FAULTS.join(", ")}`);
  if ((fault?.kind === "kill" || fault?.kind === "pause") && !names.has(fault?.process)) problems.push("fault.process must name a declared process");
  if ((fault?.kind === "write_file" || fault?.kind === "remove_path") && typeof fault?.path !== "string") problems.push(`fault.path is required for ${fault.kind}`);
  if ((scenario?.recovery?.kind === "remove_file" || scenario?.recovery?.kind === "make_dir") && typeof scenario?.recovery?.path !== "string") problems.push(`recovery.path is required for ${scenario.recovery.kind}`);
  const recovery = scenario?.recovery;
  if (!RECOVERIES.includes(recovery?.kind)) problems.push(`recovery.kind must be one of ${RECOVERIES.join(", ")}`);
  if ((recovery?.kind === "restart" || recovery?.kind === "resume") && !names.has(recovery?.process)) problems.push("recovery.process must name a declared process");
  const objectives = scenario?.objectives;
  if (!Number.isFinite(objectives?.rto_ms) || objectives.rto_ms <= 0) problems.push("objectives.rto_ms must be a positive number");
  if (scenario?.rpo && (!Number.isFinite(objectives?.rpo_records) || objectives.rpo_records < 0)) problems.push("a scenario that measures RPO must state objectives.rpo_records");
  for (const field of ["hold_ms", "timeout_ms"]) {
    if (!Number.isFinite(scenario?.[field]) || scenario[field] <= 0) problems.push(`${field} must be a positive number`);
  }
  for (const [name, ceiling] of Object.entries(scenario?.memory_ceiling_kib ?? {})) {
    if (!names.has(name)) problems.push(`memory_ceiling_kib names ${name}, which is not a declared process`);
    if (!Number.isFinite(ceiling) || ceiling <= 0) problems.push(`memory_ceiling_kib.${name} must be a positive number of KiB`);
  }
  if (scenario?.load !== undefined && !Array.isArray(scenario.load)) problems.push("load must be a list of one-shot load processes");
  for (const l of scenario?.load ?? []) {
    if (typeof l?.name !== "string" || !/^[a-z0-9-]+$/.test(l.name)) problems.push(`a load name is not lowercase letters, digits and -: ${JSON.stringify(l?.name)}`);
    if (names.has(l?.name)) problems.push(`two processes or loads are named ${l?.name}`);
    names.add(l?.name);
    if (typeof l?.binary !== "string" || !/^[a-z0-9-]+$/.test(l.binary)) problems.push(`${l?.name}: binary must be a bare binary name, found under --bin`);
    if (!Array.isArray(l?.args) || l.args.some((a) => typeof a !== "string")) problems.push(`${l?.name}: args must be a list of strings`);
    for (const key of Object.keys(l?.env ?? {})) {
      if (REFUSED_ENVIRONMENT.some((pattern) => pattern.test(key))) problems.push(`${l.name}: ${key} may not be set by a game day; drills run paper, local and simulated`);
    }
  }
  if (scenario?.final !== undefined && !Array.isArray(scenario.final)) problems.push("final must be a list of checks");
  return problems;
}

/**
 * Judge each process's peak resident size against its ceiling. One check
 * per ceiling; a peak that could not be read fails, because a ceiling that
 * was never compared is not a ceiling that held.
 */
export function judgeMemory(peaks, ceilings) {
  return Object.entries(ceilings ?? {}).map(([name, ceiling]) => {
    const peak = peaks?.[name];
    return {
      name: `${name} stayed under ${ceiling} KiB resident`,
      ...(peak === null || peak === undefined || !Number.isFinite(peak)
        ? { passed: false, detail: "its peak resident size could not be read, so the ceiling is unproven" }
        : { passed: peak <= ceiling, detail: `peak ${peak} KiB` }),
    };
  });
}

/**
 * The last `RESULT {json}` line a load process printed, parsed, or null.
 * `qip event-fabric produce` prints one so a drill keeps what the producer
 * saw beside what the broker counted; the two are independent claims about
 * the same load, and a reader should be able to see both.
 */
export function loadResult(text) {
  const lines = String(text).split("\n").filter((l) => l.startsWith("RESULT "));
  if (lines.length === 0) return null;
  try {
    return JSON.parse(lines.at(-1).slice("RESULT ".length));
  } catch {
    return null;
  }
}

/**
 * The peak resident set of a process, in KiB, from the kernel's own high
 * water mark (`VmHWM` in `/proc/<pid>/status`). The kernel keeps the peak, so
 * one read at the end of a drill sees a spike that a poll between two samples
 * would have missed. Null when the process is gone or the platform has no
 * such file: an unreadable peak is reported as unreadable, never as zero,
 * because a zero would read as a broker that used no memory.
 */
export function readPeakRssKib(pid, procRoot = "/proc") {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  let status;
  try {
    status = readFileSync(join(procRoot, String(pid), "status"), "utf8");
  } catch {
    return null;
  }
  const match = /^VmHWM:\s+(\d+)\s+kB$/m.exec(status);
  return match ? Number(match[1]) : null;
}

/** Read and validate a scenario file. Throws with every problem named. */
export function loadScenario(path) {
  const scenario = JSON.parse(readFileSync(path, "utf8"));
  const problems = checkScenario(scenario);
  if (problems.length > 0) throw new Error(`${path} is not a usable scenario:\n  ${problems.join("\n  ")}`);
  return scenario;
}

/**
 * Replace `${WORK}`, `${REPO}`, `${PORT:<name>}` and `${SHA256:<text>}` in a
 * string. The last is how a scenario writes an identities file for a token
 * it also writes, without committing a digest that a secret scanner would
 * rightly stop on.
 */
export function expand(text, context) {
  return String(text).replace(/\$\{(WORK|REPO|PORT:([a-z0-9-]+)|SHA256:([A-Za-z0-9._-]+))\}/g, (whole, key, port, plain) => {
    if (key === "WORK") return context.work;
    if (key === "REPO") return context.repo;
    if (plain !== undefined) return createHash("sha256").update(plain).digest("hex");
    if (!(port in context.ports)) throw new Error(`no port named ${port} was allocated`);
    return String(context.ports[port]);
  });
}

/** Every `${PORT:<name>}` a scenario mentions. */
export function portsIn(scenario) {
  return [...new Set([...JSON.stringify(scenario).matchAll(/\$\{PORT:([a-z0-9-]+)\}/g)].map((m) => m[1]))].sort();
}

/** Parse Prometheus text exposition into `[{ name, labels, value }]`. */
export function parsePrometheus(text) {
  const samples = [];
  for (const raw of String(text).split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const match = /^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{([^}]*)\})?\s+(\S+)/.exec(line);
    if (!match) continue;
    const labels = {};
    for (const pair of (match[3] ?? "").matchAll(/([a-zA-Z_][a-zA-Z0-9_]*)="((?:[^"\\]|\\.)*)"/g)) labels[pair[1]] = pair[2];
    const value = Number(match[4]);
    if (Number.isFinite(value)) samples.push({ name: match[1], labels, value });
  }
  return samples;
}

/** The sum of every sample of `name` whose labels include `labels`, or null if none. */
export function sumSeries(samples, name, labels = {}) {
  const matching = samples.filter((s) => s.name === name && Object.entries(labels).every(([k, v]) => s.labels[k] === v));
  return matching.length === 0 ? null : matching.reduce((total, s) => total + s.value, 0);
}

/**
 * Whether `url` is a plain-HTTP address on this machine's loopback. A game
 * day drives binaries the harness started itself; a scenario file naming any
 * other host would turn a drill into a request to somewhere nobody chose.
 */
export function isLoopbackUrl(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  return parsed.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(parsed.hostname);
}

/** One HTTP GET with a hard timeout, to loopback only. Resolves `{ status, body }`, or `{ status: 0, error }`. */
export async function get(url, { fetchImpl = fetch, timeoutMs = 2000 } = {}) {
  if (!isLoopbackUrl(url)) {
    return { status: 0, error: `refused: ${url} is not a loopback http address` };
  }
  try {
    const response = await fetchImpl(url, { signal: AbortSignal.timeout(timeoutMs) });
    return { status: response.status, body: await response.text() };
  } catch (cause) {
    return { status: 0, error: String(cause?.message ?? cause) };
  }
}

/**
 * Evaluate one check: `{ url, status?, metric?: { name, labels?, min?, max?, increases? } }`.
 * `increases` compares against `previous`, the value the same check read last.
 * Returns `{ passed, value, detail }`.
 */
export async function evaluate(check, context, { fetchImpl, previous } = {}) {
  const url = expand(check.url, context);
  const response = await get(url, { fetchImpl });
  if (check.status !== undefined && response.status !== check.status) {
    return { passed: false, value: null, detail: `${url} answered ${response.status || response.error}, expected ${check.status}` };
  }
  // `status: 0` is how a scenario says "nothing answers", which is what a
  // killed or paused process looks like from outside.
  if (check.status !== undefined && !check.metric) {
    return { passed: true, value: response.status, detail: `${url} answered ${response.status || "nothing"}, as expected` };
  }
  if (!check.metric) return { passed: response.status !== 0, value: response.status, detail: `${url} answered ${response.status || response.error}` };
  if (response.status !== 200) return { passed: false, value: null, detail: `${url} answered ${response.status || response.error}` };
  // `absent_as` is how a scenario says "a counter nothing has incremented
  // yet reads as this": a refusal counter is minted on its first refusal, so
  // "P0 was never refused" is an absent series, and without this the check
  // could only fail. Opt-in per check, so a series a scenario expects to
  // exist still fails when it is missing.
  const summed = sumSeries(parsePrometheus(response.body), check.metric.name, check.metric.labels);
  const value = summed === null && Number.isFinite(check.metric.absent_as) ? check.metric.absent_as : summed;
  if (value === null) return { passed: false, value, detail: `${check.metric.name} is not exposed at ${url}` };
  const { min, max, increases } = check.metric;
  if (min !== undefined && value < min) return { passed: false, value, detail: `${check.metric.name} = ${value} < ${min}` };
  if (max !== undefined && value > max) return { passed: false, value, detail: `${check.metric.name} = ${value} > ${max}` };
  if (increases && previous !== undefined && previous !== null && !(value > previous)) {
    return { passed: false, value, detail: `${check.metric.name} did not increase (${previous} -> ${value})` };
  }
  return { passed: true, value, detail: `${check.metric.name} = ${value}${summed === null ? " (absent, read as stated)" : ""}` };
}

/** A free loopback port. */
export function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePort(port));
    });
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Poll `fn` until it resolves truthy or `timeoutMs` passes. Returns elapsed ms or null. */
export async function until(fn, { timeoutMs, intervalMs = 100, now = Date.now } = {}) {
  const started = now();
  while (now() - started <= timeoutMs) {
    if (await fn()) return now() - started;
    await sleep(intervalMs);
  }
  return null;
}

function sha256File(path) {
  try {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
  } catch {
    return null;
  }
}

/**
 * Run one scenario end to end and return its result. `launch` is injectable
 * so the harness's own tests drive it with stand-in processes.
 */
export async function runScenario(scenario, { bin, repo, work, fetchImpl = fetch, launch = defaultLaunch, now = Date.now, commit = "unknown", log = () => {}, readPeak = readPeakRssKib } = {}) {
  const ports = {};
  for (const name of portsIn(scenario)) ports[name] = await freePort();
  const context = { work, repo, ports };
  const checks = [];
  const notes = [];
  const measurements = { detected_ms: null, outage_ms: null, rto_ms: null, rpo_records: null, peak_rss_kib: {} };
  const running = new Map();
  const loads = new Map();
  const loadResults = {};
  const binaries = {};
  const record = (phase, name, outcome) => {
    checks.push({ phase, name, passed: outcome.passed, detail: outcome.detail });
    log(`${phase} ${name}: ${outcome.passed ? "ok" : "FAILED"} — ${outcome.detail}`);
    return outcome.passed;
  };
  const launchOne = (p, into) => {
    const env = Object.fromEntries(Object.entries(p.env ?? {}).map(([k, v]) => [k, expand(v, context)]));
    for (const file of p.files ?? []) {
      const path = expand(file.path, context);
      mkdirSync(join(path, ".."), { recursive: true });
      writeFileSync(path, expand(file.content, context));
    }
    for (const dir of p.dirs ?? []) mkdirSync(expand(dir, context), { recursive: true });
    const binary = join(bin, p.binary);
    binaries[p.binary] ??= sha256File(binary);
    into.set(p.name, launch(binary, (p.args ?? []).map((a) => expand(a, context)), env, join(work, `${p.name}.log`)));
  };
  const start = (p) => launchOne(p, running);
  const allReady = async () => {
    for (const p of scenario.processes) {
      const r = await evaluate({ url: p.ready.url, status: p.ready.status ?? 200 }, context, { fetchImpl });
      if (!r.passed) return false;
    }
    return true;
  };
  // The peak of each process as it stands now. A process killed and restarted
  // reports the restarted instance's peak: the first one's died with it.
  const samplePeaks = () => {
    for (const [name, handle] of running) measurements.peak_rss_kib[name] = readPeak(handle.pid);
  };
  const verdict = (v) => (samplePeaks(), {
    schema: RESULT_SCHEMA,
    scenario: scenario.name,
    drill: scenario.drill,
    run_at: new Date(now()).toISOString(),
    commit,
    environment: "local",
    binaries,
    objectives: { rto_ms: scenario.objectives.rto_ms, rpo_records: scenario.objectives.rpo_records ?? null },
    measurements,
    checks,
    verdict: v,
    notes,
    ...(scenario.load ? { load: loadResults } : {}),
  });

  try {
    for (const p of scenario.processes) start(p);

    // Steady state: every process ready, then every steady check holding.
    const readyIn = await until(allReady, { timeoutMs: scenario.timeout_ms, now });
    if (readyIn === null) {
      record("steady", "every process became ready", { passed: false, detail: `not ready within ${scenario.timeout_ms} ms` });
      notes.push("The steady state never established, so nothing was broken and nothing was measured.");
      return verdict("inconclusive");
    }
    record("steady", "every process became ready", { passed: true, detail: `${readyIn} ms` });
    // Load starts once every process answers and before the steady state
    // is judged, so a steady check can require the load to be flowing — a
    // consumer that has committed, a class that is being admitted — before
    // anything is broken.
    for (const l of scenario.load ?? []) launchOne(l, loads);
    const previous = new Map();
    for (const [i, check] of (scenario.steady ?? []).entries()) {
      const ok = await until(async () => {
        const r = await evaluate(check, context, { fetchImpl, previous: previous.get(i) });
        previous.set(i, r.value);
        return r.passed;
      }, { timeoutMs: scenario.timeout_ms, now });
      if (ok === null) {
        const last = await evaluate(check, context, { fetchImpl });
        record("steady", check.name ?? check.url, last);
        notes.push("A steady-state check never held; the expectation is not one these binaries exercise locally, or the setup is wrong.");
        return verdict("inconclusive");
      }
      record("steady", check.name ?? check.url, { passed: true, detail: `held after ${ok} ms` });
    }

    // The durable position before the fault, if the scenario names one.
    let before = null;
    if (scenario.rpo) {
      before = (await evaluate(scenario.rpo, context, { fetchImpl })).value;
      notes.push(`durable position before the fault: ${before}`);
    }

    // The fault.
    const fault = scenario.fault;
    const faultAt = now();
    if (fault.kind === "kill") running.get(fault.process).signal("SIGKILL");
    else if (fault.kind === "pause") running.get(fault.process).signal("SIGSTOP");
    else if (fault.kind === "remove_path") rmSync(expand(fault.path, context), { recursive: true, force: true });
    else writeFileSync(expand(fault.path, context), expand(fault.content ?? "", context));
    log(`fault: ${fault.kind} ${fault.process ?? fault.path}`);

    // An `increases` check after recovery is judged against where the series
    // stood when the fault hit, read here. Without that baseline it would
    // compare with nothing and pass on any value — including the stale one a
    // frozen consumer left behind, which is the reading a frozen-sink drill
    // exists to tell apart from a consumer that resumed.
    const atFault = new Map();
    for (const [i, check] of (scenario.after_recovery ?? []).entries()) {
      if (check.metric?.increases) atFault.set(i, (await evaluate({ ...check, metric: { ...check.metric, increases: false } }, context, { fetchImpl })).value);
    }

    if (scenario.detect) {
      const seen = await until(async () => (await evaluate(scenario.detect, context, { fetchImpl })).passed, { timeoutMs: scenario.timeout_ms, now });
      measurements.detected_ms = seen === null ? null : now() - faultAt;
      record("fault", scenario.detect.name ?? "the fault was detected", { passed: seen !== null, detail: seen === null ? "never seen" : `${measurements.detected_ms} ms` });
    }

    // What must stay true while the fault lasts, checked through the hold.
    // An `increases` check is judged over a window, not between two
    // samples: a cell that runs one pass a second shows the same count to
    // two probes 250 ms apart, and that is a cell working, not a cell
    // stopped. It fails when the series has not risen for `within_ms`
    // (default 3000) — three missed passes.
    const holdUntil = faultAt + scenario.hold_ms;
    const lastValue = new Map();
    const lastRise = new Map();
    let duringFailed = false;
    while (now() < holdUntil) {
      for (const [i, check] of (scenario.during ?? []).entries()) {
        const increases = Boolean(check.metric?.increases);
        const plain = increases ? { ...check, metric: { ...check.metric, increases: false } } : check;
        let r = await evaluate(plain, context, { fetchImpl });
        if (r.passed && increases) {
          const seen = lastValue.get(i);
          if (seen === undefined || r.value > seen) {
            lastValue.set(i, r.value);
            lastRise.set(i, now());
          } else if (now() - lastRise.get(i) > (check.metric.within_ms ?? 3000)) {
            r = { passed: false, value: r.value, detail: `${check.metric.name} stayed at ${r.value} for more than ${check.metric.within_ms ?? 3000} ms` };
          }
        }
        if (!r.passed && !duringFailed) {
          duringFailed = true;
          record("during", check.name ?? check.url, r);
        }
      }
      await sleep(Math.min(250, Math.max(1, holdUntil - now())));
    }
    if (!duringFailed) for (const check of scenario.during ?? []) record("during", check.name ?? check.url, { passed: true, detail: `held for ${scenario.hold_ms} ms` });

    // Recovery.
    const recovery = scenario.recovery;
    const recoveryAt = now();
    if (recovery.kind === "restart") {
      running.get(recovery.process).signal("SIGKILL");
      await running.get(recovery.process).exited;
      start(scenario.processes.find((p) => p.name === recovery.process));
    } else if (recovery.kind === "resume") {
      running.get(recovery.process).signal("SIGCONT");
    } else if (recovery.kind === "make_dir") {
      mkdirSync(expand(recovery.path, context), { recursive: true });
    } else if (recovery.kind === "remove_file") {
      rmSync(expand(recovery.path, context), { force: true });
    }
    log(`recovery: ${recovery.kind}`);

    // The durable position is read at the first moment the recovered
    // process answers, before it has had time to write anything new: a
    // position read later would count fresh writes as recovered ones and
    // hide a loss.
    let after = null;
    const recovered = await until(async () => {
      if (scenario.rpo && after === null) after = (await evaluate(scenario.rpo, context, { fetchImpl })).value;
      if (!(await allReady())) return false;
      for (const [i, check] of (scenario.after_recovery ?? []).entries()) {
        if (!(await evaluate(check, context, { fetchImpl, previous: atFault.has(i) ? atFault.get(i) ?? undefined : undefined })).passed) return false;
      }
      return true;
    }, { timeoutMs: scenario.timeout_ms, now });
    if (recovered === null) {
      for (const [i, check] of (scenario.after_recovery ?? []).entries()) {
        record("recovery", check.name ?? check.url, await evaluate(check, context, { fetchImpl, previous: atFault.has(i) ? atFault.get(i) ?? undefined : undefined }));
      }
      record("recovery", "every process ready again", { passed: await allReady(), detail: `not recovered within ${scenario.timeout_ms} ms` });
      return verdict("missed");
    }
    measurements.rto_ms = now() - recoveryAt;
    measurements.outage_ms = now() - faultAt;
    record("recovery", "every process ready again", { passed: true, detail: `${measurements.rto_ms} ms after the recovery action` });
    for (const check of scenario.after_recovery ?? []) record("recovery", check.name ?? check.url, { passed: true, detail: "held" });

    if (scenario.rpo) {
      notes.push(`durable position when the recovered process first answered: ${after}`);
      if (before === null || after === null) {
        notes.push("The durable position could not be read on both sides of the fault; RPO is not measured.");
      } else if (before === 0) {
        // Nothing acknowledged is nothing that could be lost: an RPO of zero
        // here would be true and prove nothing, so it is not recorded.
        notes.push("Nothing had been acknowledged before the fault, so there was nothing to lose; RPO is vacuous and not recorded.");
      } else {
        measurements.rpo_records = Math.max(0, before - after);
      }
    }

    // Every load runs to its own end, bounded by the scenario's timeout; one
    // still running then is killed and failed, never waited on forever.
    for (const [name, handle] of loads) {
      let timer;
      const ended = await Promise.race([
        handle.exited.then((how) => how ?? {}),
        new Promise((r) => { timer = setTimeout(() => r(null), scenario.timeout_ms); }),
      ]);
      clearTimeout(timer);
      if (ended === null) {
        handle.signal("SIGKILL");
        await handle.exited;
        record("load", `${name} ran to its end`, { passed: false, detail: `still running ${scenario.timeout_ms} ms after the recovery; killed` });
        continue;
      }
      let text = "";
      try {
        text = readFileSync(join(work, `${name}.log`), "utf8");
      } catch {
        text = "";
      }
      loadResults[name] = loadResult(text);
      if (loadResults[name]) notes.push(`load ${name} reported: ${JSON.stringify(loadResults[name])}`);
      record("load", `${name} ran to its end`, ended.code === 0
        ? { passed: true, detail: "exit 0" }
        : { passed: false, detail: `exit ${ended.code ?? ended.signal ?? "unknown"}: ${text.trim().split("\n").at(-1) ?? ""}` });
    }

    // What must be true of the whole run, read once the load has stopped:
    // a count that must never have moved is judged over the drill entire,
    // not only over the hold.
    for (const check of scenario.final ?? []) record("final", check.name ?? check.url, await evaluate(check, context, { fetchImpl }));

    samplePeaks();
    for (const judged of judgeMemory(measurements.peak_rss_kib, scenario.memory_ceiling_kib)) record("memory", judged.name, judged);

    const objectivesMet =
      measurements.rto_ms <= scenario.objectives.rto_ms &&
      (!scenario.rpo || (measurements.rpo_records !== null && measurements.rpo_records <= scenario.objectives.rpo_records));
    const allPassed = checks.every((c) => c.passed);
    if (scenario.rpo && measurements.rpo_records === null) return verdict("inconclusive");
    return verdict(allPassed && objectivesMet ? "met" : "missed");
  } catch (cause) {
    notes.push(`aborted: ${cause.message}`);
    return verdict("aborted");
  } finally {
    for (const handle of [...loads.values(), ...running.values()]) {
      handle.signal("SIGCONT");
      handle.signal("SIGKILL");
    }
    await Promise.all([...loads.values(), ...running.values()].map((h) => h.exited));
  }
}

/** Start a process with exactly `env` (nothing inherited but PATH), logging to a file. */
export function defaultLaunch(binary, args, env, logPath) {
  if (!existsSync(binary)) throw new Error(`${binary} does not exist; build it first (cargo build -p ${basename(binary)})`);
  const child = spawn(binary, args, { env: { PATH: process.env.PATH ?? "/usr/bin:/bin", ...env }, stdio: ["ignore", "pipe", "pipe"] });
  const chunks = [];
  child.stdout.on("data", (d) => chunks.push(d));
  child.stderr.on("data", (d) => chunks.push(d));
  // Resolves with how the process ended, so a one-shot load's exit code is
  // a fact the drill records rather than one it assumes.
  const exited = new Promise((r) => child.on("exit", (code, signal) => {
    writeFileSync(logPath, Buffer.concat(chunks));
    r({ code, signal });
  }));
  return {
    pid: child.pid,
    signal: (name) => {
      try {
        child.kill(name);
      } catch {
        /* already gone */
      }
    },
    exited: child.exitCode !== null ? Promise.resolve() : exited,
  };
}

/** Validate a result object against the committed format. Returns problems. */
export function validateResult(result) {
  const problems = [];
  if (result?.schema !== RESULT_SCHEMA) problems.push(`schema is not ${RESULT_SCHEMA}`);
  for (const field of ["scenario", "drill", "run_at", "commit", "environment"]) {
    if (typeof result?.[field] !== "string" || result[field] === "") problems.push(`${field} must be a non-empty string`);
  }
  if (Number.isNaN(Date.parse(result?.run_at))) problems.push("run_at is not a timestamp");
  if (!VERDICTS.includes(result?.verdict)) problems.push(`verdict must be one of ${VERDICTS.join(", ")}`);
  const m = result?.measurements ?? {};
  for (const field of ["detected_ms", "outage_ms", "rto_ms", "rpo_records"]) {
    if (!(field in m) || (m[field] !== null && !(Number.isFinite(m[field]) && m[field] >= 0))) problems.push(`measurements.${field} must be null or a non-negative number`);
  }
  // Optional: results recorded before the sampler existed carry none.
  for (const [name, peak] of Object.entries(m.peak_rss_kib ?? {})) {
    if (peak !== null && !(Number.isFinite(peak) && peak >= 0)) problems.push(`measurements.peak_rss_kib.${name} must be null or a non-negative number`);
  }
  if (!Number.isFinite(result?.objectives?.rto_ms)) problems.push("objectives.rto_ms must be a number");
  if (!Array.isArray(result?.checks)) problems.push("checks must be a list");
  // A `met` verdict is a claim; it must be one the record itself supports.
  if (result?.verdict === "met") {
    if (m.rto_ms === null || m.rto_ms > result.objectives.rto_ms) problems.push("verdict met with no RTO, or an RTO over its objective");
    if (result.objectives.rpo_records !== null && (m.rpo_records === null || m.rpo_records > result.objectives.rpo_records)) problems.push("verdict met with no RPO, or an RPO over its objective");
    if ((result.checks ?? []).some((c) => !c.passed)) problems.push("verdict met with a failed check");
    if ((result.checks ?? []).length === 0) problems.push("verdict met with no check recorded");
  }
  return problems;
}

function option(argv, name) {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 ? argv[at + 1] : undefined;
}

/** The command line. Returns the exit code. */
export async function main(argv, { out = console.log, err = console.error } = {}) {
  const [mode, ...rest] = argv;
  if (mode === "validate") {
    if (rest.length === 0) {
      err("usage: harness.mjs validate <result.json>…");
      return 64;
    }
    let failed = false;
    for (const path of rest) {
      let problems;
      try {
        problems = validateResult(JSON.parse(readFileSync(path, "utf8")));
      } catch (cause) {
        problems = [`unreadable: ${cause.message}`];
      }
      for (const p of problems) err(`${path}: ${p}`);
      if (problems.length > 0) failed = true;
    }
    if (!failed) out(`${rest.length} game-day result(s) are in the committed format`);
    return failed ? 1 : 0;
  }
  if (mode !== "run") {
    err("usage: harness.mjs run --scenario <file> --bin <dir> [--out <dir>] | validate <file>…");
    return 64;
  }
  const scenarioPath = option(argv, "scenario");
  const bin = option(argv, "bin");
  if (!scenarioPath || !bin) {
    err("run needs --scenario and --bin");
    return 64;
  }
  const scenario = loadScenario(scenarioPath);
  const repo = resolve(fileURLToPath(new URL("../..", import.meta.url)));
  const work = mkdtempSync(join(tmpdir(), `game-day-${scenario.name}-`));
  // The commit the binaries were built from is the caller's to state when
  // it differs from the checkout's; by default, the checkout's HEAD.
  const head = spawnSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8", timeout: 5000 });
  const commit = head.status === 0 && /^[0-9a-f]{40}$/.test(head.stdout.trim()) ? head.stdout.trim() : "unknown";
  const result = await runScenario(scenario, { bin: resolve(bin), repo, work, commit: option(argv, "commit") ?? commit, log: out });
  const problems = validateResult(result);
  if (problems.length > 0) {
    err(`the harness produced a result outside its own format: ${problems.join("; ")}`);
    return 1;
  }
  const text = `${JSON.stringify(result, null, 2)}\n`;
  const outDir = option(argv, "out");
  if (outDir) {
    mkdirSync(outDir, { recursive: true });
    const file = join(outDir, `${result.run_at.slice(0, 10)}-${scenario.name}.json`);
    writeFileSync(file, text);
    out(`wrote ${file}`);
  } else {
    out(text);
  }
  out(`${scenario.name}: ${result.verdict} (rto_ms=${result.measurements.rto_ms}, rpo_records=${result.measurements.rpo_records}); process logs in ${work}`);
  return result.verdict === "met" ? 0 : 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(await main(process.argv.slice(2)));
}
