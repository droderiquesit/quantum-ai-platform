/**
 * The game-day harness's tests: `node --test scripts/game-day/harness.test.mjs`.
 *
 * The end-to-end tests drive real child processes — `fixtures/fake-service.mjs`
 * copied into a scratch `--bin` — through a kill and a restart, so what is
 * proven is the timing and the counting, not a mock of them. Each test
 * names the mutation of `harness.mjs` that should make it fail.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkScenario, defaultLaunch, evaluate, get, isLoopbackUrl, judgeMemory, loadResult, parsePrometheus, readPeakRssKib, runScenario, sumSeries, validateResult } from "./harness.mjs";
import { attempts, main as escalationMain, runDrill, validateRecord as validateEscalation } from "./agent-escalation.mjs";
import { readFileSync as readFileForDrill, readdirSync as readdirForDrill } from "node:fs";
import { join as joinForDrill } from "node:path";
import { fileURLToPath as urlToPathForDrill } from "node:url";

async function withScratch(fn) {
  const root = mkdtempSync(join(tmpdir(), "game-day-test-"));
  const bin = join(root, "bin");
  const work = join(root, "work");
  mkdirSync(bin);
  mkdirSync(work);
  copyFileSync(new URL("./fixtures/fake-service.mjs", import.meta.url), join(bin, "fake-service"));
  chmodSync(join(bin, "fake-service"), 0o755);
  try {
    return await fn({ bin, work });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

/** A broker-loss shaped scenario over the fake: kill the broker, the cell keeps answering. */
const scenario = (overrides = {}) => ({
  name: "fake-broker-loss",
  drill: "the cell keeps answering while the broker is down, and the broker comes back with what it acknowledged",
  hold_ms: 400,
  timeout_ms: 8000,
  processes: [
    { name: "broker", binary: "fake-service", env: { FAKE_PORT: "${PORT:broker}", FAKE_STATE: "${WORK}/broker.state", FAKE_PERSIST_EVERY: "1" }, ready: { url: "http://127.0.0.1:${PORT:broker}/health" } },
    { name: "cell", binary: "fake-service", env: { FAKE_PORT: "${PORT:cell}" }, ready: { url: "http://127.0.0.1:${PORT:cell}/health" } },
  ],
  steady: [{ name: "the cell is counting", url: "http://127.0.0.1:${PORT:cell}/metrics", metric: { name: "fake_records_total", increases: true } }],
  fault: { kind: "kill", process: "broker" },
  detect: { name: "the broker's readiness fails", url: "http://127.0.0.1:${PORT:broker}/health", status: 0 },
  during: [{ name: "the cell keeps counting", url: "http://127.0.0.1:${PORT:cell}/metrics", metric: { name: "fake_records_total", increases: true } }],
  recovery: { kind: "restart", process: "broker" },
  rpo: { url: "http://127.0.0.1:${PORT:broker}/metrics", metric: { name: "fake_records_total" } },
  objectives: { rto_ms: 5000, rpo_records: 0 },
  ...overrides,
});

test("a scenario that would leave paper, local and simulated is refused", () => {
  const bad = scenario();
  bad.processes[1].env.QIP_VENUE_ADAPTER = "live";
  assert.ok(checkScenario(bad).some((p) => /QIP_VENUE_ADAPTER may not be set/.test(p)));
  assert.deepEqual(checkScenario(scenario()), []);
  const noReady = scenario();
  delete noReady.processes[0].ready;
  assert.ok(checkScenario(noReady).length > 0);
  assert.ok(checkScenario(scenario({ fault: { kind: "unplug", process: "broker" } })).length > 0);
  assert.ok(checkScenario(scenario({ objectives: { rto_ms: 1000 } })).length > 0, "an RPO scenario with no RPO objective");
});

test("Prometheus exposition is summed over the labels asked for", () => {
  const samples = parsePrometheus('# HELP x\nqip_a{stream="s",partition="0"} 5\nqip_a{stream="s",partition="1"} 7\nqip_a{stream="t",partition="0"} 100\nqip_b 1\n');
  assert.equal(sumSeries(samples, "qip_a", { stream: "s" }), 12);
  assert.equal(sumSeries(samples, "qip_a"), 112);
  assert.equal(sumSeries(samples, "qip_c"), null);
});

// A `met` verdict is a claim the record must support. Mutation: drop the
// checks under `if (result?.verdict === "met")`.
test("a result claiming met with a failed check or an RTO over objective is refused", () => {
  const good = {
    schema: "qip.game-day.result/v1", scenario: "x", drill: "y", run_at: "2026-10-11T00:00:00.000Z", commit: "c", environment: "local",
    binaries: {}, objectives: { rto_ms: 1000, rpo_records: 0 },
    measurements: { detected_ms: 5, outage_ms: 900, rto_ms: 800, rpo_records: 0 },
    checks: [{ phase: "steady", name: "n", passed: true, detail: "d" }], verdict: "met", notes: [],
  };
  assert.deepEqual(validateResult(good), []);
  assert.ok(validateResult({ ...good, checks: [{ ...good.checks[0], passed: false }] }).length > 0);
  assert.ok(validateResult({ ...good, measurements: { ...good.measurements, rto_ms: 1001 } }).length > 0);
  assert.ok(validateResult({ ...good, measurements: { ...good.measurements, rpo_records: null } }).length > 0);
  assert.ok(validateResult({ ...good, verdict: "great" }).length > 0);
  assert.deepEqual(validateResult({ ...good, verdict: "missed", measurements: { ...good.measurements, rto_ms: null } }), []);
});

// End to end: a crash and a restart, timed, with nothing lost. Mutation:
// measure rto from the fault instead of the recovery, or skip the restart.
test("a broker killed and restarted is timed, and a broker that persisted everything loses nothing", async () => {
  await withScratch(async ({ bin, work }) => {
    const result = await runScenario(scenario(), { bin, repo: work, work });
    assert.deepEqual(validateResult(result), []);
    assert.equal(result.verdict, "met", JSON.stringify(result.checks, null, 1) + result.notes.join("\n"));
    assert.ok(result.measurements.rto_ms !== null && result.measurements.rto_ms < 5000);
    assert.ok(result.measurements.outage_ms >= result.measurements.rto_ms + 300, "the outage spans the hold and the recovery");
    assert.ok(result.measurements.detected_ms !== null);
    assert.equal(result.measurements.rpo_records, 0);
    assert.ok(result.checks.some((c) => c.phase === "during" && c.passed));
  });
});

// RPO is counted, not assumed. This fake never persists within the test,
// and the steady state waits until it has acknowledged at least 25 records,
// so the kill loses every one of them. Mutation: report rpo as zero, or
// read the position after recovery late enough to count fresh writes.
test("a broker that acknowledged before it persisted is caught losing records", async () => {
  await withScratch(async ({ bin, work }) => {
    const lossy = scenario({
      steady: [{ name: "the broker has acknowledged records", url: "http://127.0.0.1:${PORT:broker}/metrics", metric: { name: "fake_records_total", min: 25 } }],
    });
    lossy.processes[0].env.FAKE_PERSIST_EVERY = "100000";
    const result = await runScenario(lossy, { bin, repo: work, work });
    assert.ok(result.measurements.rpo_records > 0, `rpo_records = ${result.measurements.rpo_records}`);
    assert.equal(result.verdict, "missed");
    assert.deepEqual(validateResult(result), []);
  });
});

// What must stay true during the fault is checked through the hold.
// Mutation: skip the `during` loop. Here the expectation is false — the
// broker itself is expected to keep answering while it is dead.
test("an expectation broken during the fault makes the drill missed", async () => {
  await withScratch(async ({ bin, work }) => {
    const wrong = scenario({ during: [{ name: "the broker answers while dead", url: "http://127.0.0.1:${PORT:broker}/health", status: 200 }] });
    const result = await runScenario(wrong, { bin, repo: work, work });
    assert.equal(result.verdict, "missed");
    assert.ok(result.checks.some((c) => c.phase === "during" && !c.passed));
  });
});

// An RPO over a position that was zero before the fault is true and proves
// nothing. Mutation: record 0 and call the drill met.
test("a drill whose durable position was empty before the fault is inconclusive", async () => {
  await withScratch(async ({ bin, work }) => {
    const empty = scenario({ rpo: { url: "http://127.0.0.1:${PORT:broker}/metrics", metric: { name: "fake_empty_total" } } });
    const result = await runScenario(empty, { bin, repo: work, work });
    assert.equal(result.verdict, "inconclusive");
    assert.equal(result.measurements.rpo_records, null);
    assert.ok(result.measurements.rto_ms !== null, "the restart was still timed");
  });
});

// A steady state that never establishes measures nothing. Mutation: carry
// on to the fault anyway.
test("a scenario whose steady state never holds is inconclusive, not met", async () => {
  await withScratch(async ({ bin, work }) => {
    const never = scenario({ timeout_ms: 600, steady: [{ name: "a series nothing exposes", url: "http://127.0.0.1:${PORT:cell}/metrics", metric: { name: "absent_total" } }] });
    const result = await runScenario(never, { bin, repo: work, work });
    assert.equal(result.verdict, "inconclusive");
    assert.equal(result.measurements.rto_ms, null);
  });
});

// The scenarios committed for the platform's binaries are well formed.
test("every committed scenario passes the scenario check", () => {
  const dir = new URL("./scenarios/", import.meta.url);
  const files = readdirSync(dir).filter((f) => f.endsWith(".json"));
  assert.ok(files.length >= 4, `found ${files.length} scenarios; broker, ledger, isolation and stale policy are four`);
  for (const file of files) {
    const s = JSON.parse(readFileSync(new URL(file, dir), "utf8"));
    assert.deepEqual(checkScenario(s), [], file);
    assert.equal(`${s.name}.json`, file, "a scenario's file is named after it");
  }
});

// Every result committed under docs/ops/game-days/results is in the format.
test("every committed game-day result is in the committed format", () => {
  const dir = new URL("../../docs/ops/game-days/results/", import.meta.url);
  let files = [];
  try {
    files = readdirSync(dir).filter((f) => f.endsWith(".json"));
  } catch {
    files = [];
  }
  for (const file of files) {
    assert.deepEqual(validateResult(JSON.parse(readFileSync(new URL(file, dir), "utf8"))), [], file);
  }
});

// CodeQL flagged a scenario file's URL reaching an outbound request. A game
// day only ever probes binaries the harness started on this machine, so any
// other host is refused before a request is made, and nothing is fetched.
test("the harness probes loopback only and refuses any other host before fetching", async () => {
  assert.equal(isLoopbackUrl("http://127.0.0.1:9100/metrics"), true, "premise: loopback is admitted");
  assert.equal(isLoopbackUrl("http://localhost:8080/healthz"), true);
  for (const url of ["http://example.com/metrics", "https://127.0.0.1:9100/metrics", "http://10.0.0.5/", "not a url", "http://127.0.0.1.evil.test/"]) {
    assert.equal(isLoopbackUrl(url), false, url);
    let called = false;
    const result = await get(url, { fetchImpl: async () => { called = true; return { status: 200, text: async () => "" }; } });
    assert.equal(called, false, `fetched ${url}`);
    assert.equal(result.status, 0);
    assert.match(result.error, /not a loopback http address/);
  }
});

// --- R12-SEC-10157: the agent-authority drill (scripts/game-day/agent-escalation.mjs) ---
const POLICY_PATH = new URL("../agent-policy.json", import.meta.url);
const policy = () => JSON.parse(readFileForDrill(POLICY_PATH, "utf8"));
const RESULTS = urlToPathForDrill(new URL("../../docs/ops/game-days/agent-escalation/", import.meta.url));

// Premise first: the control is admitted, so a clean sweep below is selective
// refusal and not closed judges. Mutation: make `judgeChange` or `validate`
// refuse everything — the control's `held` goes false.
test("the committed judges admit the control and refuse every escalation attempt", () => {
  const record = runDrill({ policy: policy(), policyPath: urlToPathForDrill(POLICY_PATH), commit: "test" });
  const control = record.attempts.filter((a) => a.expect === "admitted");
  const attacks = record.attempts.filter((a) => a.expect === "refused");
  assert.equal(control.length, 1);
  assert.ok(attacks.length >= 10, `${attacks.length} attacks`);
  assert.equal(control[0].outcome, "admitted", JSON.stringify(control[0]));
  for (const attack of attacks) assert.equal(attack.outcome, "refused", attack.attempt);
  assert.equal(record.verdict, "held");
  assert.deepEqual(validateEscalation(record), []);
});

// The drill can fail. Mutation: record `held: true` for every attempt in
// `runDrill` — the relaxed policy below then reads as held and this fails.
test("a policy that stops protecting the workflows is reported as breached, naming what got through", () => {
  const relaxed = policy();
  relaxed.protected_paths.globs = [];
  const record = runDrill({ policy: relaxed, policyPath: urlToPathForDrill(POLICY_PATH), commit: "test" });
  assert.equal(record.verdict, "breached");
  const breached = record.attempts.filter((a) => !a.held).map((a) => a.attempt);
  assert.ok(breached.includes("edit a deploy workflow with no approval"), breached.join("; "));
  assert.ok(breached.includes("edit a deploy workflow with a bot's approval of the head"), breached.join("; "));
  assert.deepEqual(validateEscalation(record), []);
});

test("a record claiming held over a breached attempt, or with no control, is refused", () => {
  const record = runDrill({ policy: policy(), policyPath: urlToPathForDrill(POLICY_PATH), commit: "test" });
  const lying = structuredClone(record);
  lying.attempts[1].outcome = "admitted";
  lying.attempts[1].held = false;
  assert.ok(validateEscalation(lying).some((p) => /held with an attempt/.test(p)));
  const noControl = structuredClone(record);
  noControl.attempts = noControl.attempts.filter((a) => a.expect !== "admitted");
  assert.ok(validateEscalation(noControl).some((p) => /no control attempt/.test(p)));
});

test("every attempt names its own input, so a renamed judge cannot leave one silently empty", () => {
  const names = attempts(policy(), { problem: "none" }).map((a) => a.name);
  assert.equal(new Set(names).size, names.length);
});

test("every committed drill record is in the committed format and held", () => {
  const files = readdirForDrill(RESULTS).filter((f) => f.endsWith(".json"));
  assert.ok(files.length >= 1, "no drill record is committed");
  for (const file of files) {
    const record = JSON.parse(readFileForDrill(joinForDrill(RESULTS, file), "utf8"));
    assert.deepEqual(validateEscalation(record), [], file);
    assert.equal(record.verdict, "held", file);
  }
});

test("the command runs, exits zero on a held drill, and refuses an unknown mode", () => {
  const lines = [];
  assert.equal(escalationMain(["run"], { out: (l) => lines.push(l), err: () => {} }), 0);
  assert.match(lines.at(-1), /verdict: held/);
  assert.equal(escalationMain(["nonsense"], { out: () => {}, err: () => {} }), 64);
});

// The harness measured no memory, so a drill could not say a broker
// stayed small while a sink was frozen. Mutation: make readPeakRssKib return
// null — the first test's peak assertion fails; compare `peak <= ceiling` as
// `peak >= ceiling` — the ceiling tests fail.
test("the kernel's resident-set high water mark is read from /proc and an unreadable one is null, not zero", () => {
  const root = mkdtempSync(join(tmpdir(), "game-day-proc-"));
  try {
    mkdirSync(join(root, "4242"));
    writeFileSync(join(root, "4242", "status"), "Name:\tqip-fabricd\nVmPeak:\t  900000 kB\nVmHWM:\t  123456 kB\nVmRSS:\t   100000 kB\n");
    mkdirSync(join(root, "4243"));
    writeFileSync(join(root, "4243", "status"), "Name:\tkernel-thread\n");
    assert.equal(readPeakRssKib(4242, root), 123456, "VmHWM, not VmRSS or VmPeak");
    assert.equal(readPeakRssKib(4243, root), null, "a status with no VmHWM line");
    assert.equal(readPeakRssKib(9999, root), null, "a process that is gone");
    assert.equal(readPeakRssKib(0, root), null);
    assert.equal(readPeakRssKib("4242", root), null, "a pid that is not an integer");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a drill records each process's peak resident size, and a ceiling it exceeds makes the drill missed", async () => {
  await withScratch(async ({ bin, work }) => {
    const generous = await runScenario(scenario({ memory_ceiling_kib: { broker: 4 * 1024 * 1024, cell: 4 * 1024 * 1024 } }), { bin, repo: work, work });
    assert.deepEqual(validateResult(generous), []);
    for (const name of ["broker", "cell"]) {
      const peak = generous.measurements.peak_rss_kib[name];
      assert.ok(Number.isFinite(peak) && peak > 0, `premise: ${name} peak was measured, got ${peak}`);
    }
    assert.equal(generous.verdict, "met", JSON.stringify(generous.checks, null, 1));
    assert.ok(generous.checks.some((c) => c.phase === "memory" && c.passed));

    const tight = await runScenario(scenario({ memory_ceiling_kib: { broker: 1 } }), { bin, repo: work, work });
    assert.equal(tight.verdict, "missed");
    assert.ok(tight.checks.some((c) => c.phase === "memory" && !c.passed && /broker stayed under 1 KiB/.test(c.name)));
    assert.deepEqual(validateResult(tight), []);
  });
});

test("a ceiling on a process that is not declared is refused, and a negative peak in a result is refused", () => {
  const problems = checkScenario({ ...scenario(), memory_ceiling_kib: { sink: 1000, broker: 0 } });
  assert.ok(problems.some((p) => /names sink, which is not a declared process/.test(p)), problems.join("\n"));
  assert.ok(problems.some((p) => /memory_ceiling_kib\.broker must be a positive number/.test(p)), problems.join("\n"));
  assert.deepEqual(checkScenario(scenario()), []);
  const result = { schema: "qip.game-day.result/v1", scenario: "x", drill: "x", run_at: "2026-10-24T00:00:00Z", commit: "x", environment: "local", objectives: { rto_ms: 1, rpo_records: null }, measurements: { detected_ms: null, outage_ms: null, rto_ms: null, rpo_records: null, peak_rss_kib: { broker: -5 } }, checks: [], verdict: "aborted" };
  assert.ok(validateResult(result).some((p) => /peak_rss_kib\.broker/.test(p)));
});

// A ceiling that cannot be checked is not a ceiling that held. Mutation: record
// an unreadable peak as passed — this test fails.
test("a ceiling on a process whose peak cannot be read fails closed", async () => {
  await withScratch(async ({ bin, work }) => {
    const blind = (binary, args, env, log) => ({ ...defaultLaunch(binary, args, env, log), pid: undefined });
    const result = await runScenario(scenario({ memory_ceiling_kib: { broker: 4 * 1024 * 1024 } }), { bin, repo: work, work, launch: blind });
    assert.equal(result.measurements.peak_rss_kib.broker, null, "premise: the peak was unreadable");
    assert.equal(result.verdict, "missed");
    assert.ok(result.checks.some((c) => c.phase === "memory" && !c.passed && /could not be read/.test(c.detail)));
    assert.deepEqual(validateResult(result), []);
  });
});

// --- R12-RES-10153: the frozen-sink backpressure drill ----------------------

const FROZEN_SINK = new URL("./scenarios/frozen-sink.json", import.meta.url);
const frozenSinkResults = () => {
  const dir = new URL("../../docs/ops/game-days/results/", import.meta.url);
  return readdirSync(dir).filter((f) => f.endsWith("-frozen-sink.json")).map((f) => JSON.parse(readFileSync(new URL(f, dir), "utf8")));
};

// The ceiling is judged at its edge: a peak equal to the ceiling holds, one
// KiB more does not. Mutation: compare `peak < ceiling` — the equal row
// fails; compare `peak >= ceiling` or drop the comparison — the over row
// passes and this fails.
test("a memory ceiling one KiB below the observed peak fails, and the peak itself holds", () => {
  const [at] = judgeMemory({ fabricd: 25088 }, { fabricd: 25088 });
  assert.equal(at.passed, true, at.detail);
  const [below] = judgeMemory({ fabricd: 25088 }, { fabricd: 25087 });
  assert.equal(below.passed, false, below.detail);
  assert.match(below.name, /fabricd stayed under 25087 KiB/);
  const [unread] = judgeMemory({ fabricd: null }, { fabricd: 1_000_000 });
  assert.equal(unread.passed, false, "an unread peak is not a peak under the ceiling");
  assert.deepEqual(judgeMemory({ fabricd: 1 }, {}), [], "no ceiling, nothing judged");
});

// The committed drill's own numbers: the recorded fabricd peak is under the
// scenario's ceiling (so `met` was earned), and the same record judged
// against a ceiling one KiB below that peak would have failed — the ceiling
// is a comparison that could have gone the other way on this run's data.
test("the committed frozen-sink result would have failed under a ceiling below its own measured peak", () => {
  const scenario = JSON.parse(readFileSync(FROZEN_SINK, "utf8"));
  const results = frozenSinkResults();
  assert.ok(results.length >= 1, "premise: a frozen-sink game day is recorded");
  for (const result of results) {
    const peak = result.measurements.peak_rss_kib.fabricd;
    assert.ok(Number.isFinite(peak) && peak > 0, `premise: the broker's peak was measured, got ${peak}`);
    const [held] = judgeMemory(result.measurements.peak_rss_kib, { fabricd: scenario.memory_ceiling_kib.fabricd });
    assert.equal(held.passed, true, held.detail);
    const [tight] = judgeMemory(result.measurements.peak_rss_kib, { fabricd: peak - 1 });
    assert.equal(tight.passed, false, tight.detail);
    assert.equal(result.verdict, "met", JSON.stringify(result.checks.filter((c) => !c.passed)));
    assert.ok(result.checks.some((c) => c.phase === "memory" && c.passed), "the record carries its memory check");
  }
});

// End to end over real child processes: the same observed peak, judged
// against a ceiling one KiB under it and against the peak itself. The peak
// is pinned through a /proc stand-in so both runs see the same number — a
// real process's high water mark moves between runs, and a test comparing
// two different peaks would prove nothing about the edge.
test("a drill whose ceiling is below the peak it observes is missed end to end", async () => {
  const root = mkdtempSync(join(tmpdir(), "game-day-proc-"));
  try {
    mkdirSync(join(root, "77"));
    writeFileSync(join(root, "77", "status"), "Name:\tfake\nVmHWM:\t   40960 kB\n");
    assert.equal(readPeakRssKib(77, root), 40960, "premise: the pinned peak reads");
    const readPeak = () => readPeakRssKib(77, root);
    await withScratch(async ({ bin, work }) => {
      const over = await runScenario(scenario({ memory_ceiling_kib: { broker: 40959 } }), { bin, repo: work, work, readPeak });
      assert.equal(over.measurements.peak_rss_kib.broker, 40960);
      assert.equal(over.verdict, "missed", JSON.stringify(over.checks, null, 1));
      assert.ok(over.checks.some((c) => c.phase === "memory" && !c.passed && /peak 40960 KiB/.test(c.detail)));
      assert.deepEqual(validateResult(over), []);
      const at = await runScenario(scenario({ memory_ceiling_kib: { broker: 40960 } }), { bin, repo: work, work, readPeak });
      assert.equal(at.verdict, "met", JSON.stringify(at.checks, null, 1));
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// A refusal counter is minted on its first refusal, so "never refused" is
// an absent series. Mutation: ignore `absent_as` — the first assertion
// fails; apply it to a present series — the present-and-nonzero row passes
// and the third assertion fails.
test("an absent counter reads as its stated value only when the check says so", async () => {
  const body = 'qip_event_fabric_refusals_total{class="p2_market_journal",reason="quota"} 3\n';
  const fetchImpl = async () => ({ status: 200, text: async () => body });
  const context = { work: "/w", repo: "/r", ports: { f: 1 } };
  const never = (labels, extra = {}) => ({ url: "http://127.0.0.1:${PORT:f}/metrics", metric: { name: "qip_event_fabric_refusals_total", labels, max: 0, ...extra } });
  assert.equal((await evaluate(never({ class: "p0_control" }, { absent_as: 0 }), context, { fetchImpl })).passed, true);
  assert.equal((await evaluate(never({ class: "p0_control" }), context, { fetchImpl })).passed, false, "absent without absent_as is not exposed");
  assert.equal((await evaluate(never({ class: "p2_market_journal" }, { absent_as: 0 }), context, { fetchImpl })).passed, false, "a present 3 is not read as 0");
});

const loadEntry = (overrides = {}) => ({ name: "load-a", binary: "fake-service", args: ["--peer", "127.0.0.1:${PORT:broker}"], env: { FAKE_EXIT_AFTER_MS: "300" }, ...overrides });

// A load is a one-shot process the drill waits for and whose exit it
// records. Mutation: skip awaiting loads, or record every load as passed —
// the failing load below then reads as met.
test("a load that ran to its end is recorded with what it reported, and one that failed makes the drill missed", async () => {
  await withScratch(async ({ bin, work }) => {
    const good = await runScenario(scenario({ load: [loadEntry()] }), { bin, repo: work, work });
    assert.equal(good.verdict, "met", JSON.stringify(good.checks, null, 1));
    assert.ok(good.checks.some((c) => c.phase === "load" && c.passed && /load-a ran to its end/.test(c.name)));
    assert.deepEqual(good.load["load-a"], { attempted: 3, admitted: 3, refused: {} });
    assert.deepEqual(validateResult(good), []);

    const bad = await runScenario(scenario({ load: [loadEntry({ env: { FAKE_EXIT_AFTER_MS: "100", FAKE_EXIT_CODE: "1" } })] }), { bin, repo: work, work });
    assert.equal(bad.verdict, "missed");
    assert.ok(bad.checks.some((c) => c.phase === "load" && !c.passed && /exit 1/.test(c.detail)), JSON.stringify(bad.checks, null, 1));
  });
});

// Checks over the whole run are read after the load stops. Mutation: skip
// the `final` loop — the impossible final check below then passes.
test("a final check that does not hold once the load has stopped makes the drill missed", async () => {
  await withScratch(async ({ bin, work }) => {
    const result = await runScenario(scenario({
      load: [loadEntry()],
      final: [{ name: "a count that must stay at zero", url: "http://127.0.0.1:${PORT:cell}/metrics", metric: { name: "fake_records_total", max: 0 } }],
    }), { bin, repo: work, work });
    assert.equal(result.verdict, "missed");
    assert.ok(result.checks.some((c) => c.phase === "final" && !c.passed));
  });
});

// After recovery, `increases` is judged against the value read at the
// fault. Mutation: drop the `atFault` baseline (pass no `previous`) — a
// series that never moves then reads as having increased and this drill,
// whose after-recovery expectation is false, reads as met.
test("an after-recovery increase is measured from the fault, so a series that never moved fails", async () => {
  await withScratch(async ({ bin, work }) => {
    const frozen = scenario({
      fault: { kind: "pause", process: "broker" },
      recovery: { kind: "resume", process: "broker" },
      rpo: undefined,
      objectives: { rto_ms: 5000 },
      // Longer than a probe of the paused broker (2 s), so the detection
      // holds and the only thing that can miss is the expectation under test.
      timeout_ms: 4000,
      after_recovery: [{ name: "a constant that cannot rise", url: "http://127.0.0.1:${PORT:cell}/metrics", metric: { name: "fake_empty_total", increases: true } }],
    });
    delete frozen.rpo;
    const result = await runScenario(frozen, { bin, repo: work, work });
    assert.equal(result.verdict, "missed", JSON.stringify(result.checks, null, 1));
    assert.ok(result.checks.some((c) => c.phase === "recovery" && !c.passed && /did not increase/.test(c.detail)));

    const moving = scenario({
      fault: { kind: "pause", process: "broker" },
      recovery: { kind: "resume", process: "broker" },
      objectives: { rto_ms: 5000 },
      after_recovery: [{ name: "the resumed broker counts again", url: "http://127.0.0.1:${PORT:broker}/metrics", metric: { name: "fake_records_total", increases: true } }],
    });
    delete moving.rpo;
    const resumed = await runScenario(moving, { bin, repo: work, work });
    assert.equal(resumed.verdict, "met", JSON.stringify(resumed.checks, null, 1));
  });
});

test("a load that would leave paper, local and simulated, or names no args, is refused", () => {
  assert.deepEqual(checkScenario(scenario({ load: [loadEntry()] })), []);
  const venue = checkScenario(scenario({ load: [loadEntry({ env: { QIP_VENUE_ADAPTER: "live" } })] }));
  assert.ok(venue.some((p) => /load-a: QIP_VENUE_ADAPTER may not be set/.test(p)), venue.join("\n"));
  assert.ok(checkScenario(scenario({ load: [loadEntry({ args: "event-fabric produce" })] })).some((p) => /args must be a list/.test(p)));
  assert.ok(checkScenario(scenario({ load: [loadEntry({ name: "broker" })] })).some((p) => /two processes or loads are named broker/.test(p)));
});

test("a load's RESULT line is read back, and a load that printed none reports null", () => {
  assert.deepEqual(loadResult("PRODUCE x\nRESULT {\"admitted\":2}\nPAPER TRADING\n"), { admitted: 2 });
  assert.equal(loadResult("PRODUCE x\n"), null);
  assert.equal(loadResult("RESULT {not json"), null);
});
