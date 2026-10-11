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
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkScenario, get, isLoopbackUrl, parsePrometheus, runScenario, sumSeries, validateResult } from "./harness.mjs";

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
