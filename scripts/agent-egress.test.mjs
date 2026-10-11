/**
 * The sandbox egress proxy's tests: `node --test scripts/agent-egress.test.mjs`.
 *
 * Each names the mutation of `agent-egress.mjs` that should make it fail.
 * The proxy is exercised for real on loopback, against a loopback upstream
 * that echoes, with `connectImpl` mapping the allow-listed name to it — so a
 * test that passes has watched bytes cross an admitted tunnel and watched a
 * refused one carry none.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { connect, createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { judgeTarget, main, parseConnectTarget, sandboxPolicy, serve, verdict } from "./agent-egress.mjs";

const POLICY = JSON.parse(readFileSync(new URL("./agent-policy.json", import.meta.url), "utf8"));

// SEC-9078's development half: a lane in the sandbox cannot call a model,
// because no model provider is a host it can reach. Mutation: add any of
// these to `sandbox.egress_allow`.
test("the allow-list names no model provider", () => {
  const providers = [
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "aiplatform.googleapis.com",
    "us-central1-aiplatform.googleapis.com",
    "router.huggingface.co",
    "api-inference.huggingface.co",
    "api.deepseek.com",
    "api.mistral.ai",
    "api.cohere.com",
    "api.groq.com",
    "api.together.xyz",
    "openrouter.ai",
    "bedrock-runtime.us-east-1.amazonaws.com",
  ];
  const { allow } = sandboxPolicy(POLICY);
  assert.ok(allow.length > 0, "the premise: the policy lists some host");
  for (const host of providers) assert.ok(judgeTarget(host, 443, { allow }), `${host} is reachable from the sandbox`);
});

// The admitting half, so a judge that refused everything fails here.
test("an allow-listed host on 443 is admitted, and a wildcard admits subdomains only", () => {
  assert.equal(judgeTarget("index.crates.io", 443, { allow: ["index.crates.io"] }), undefined);
  assert.equal(judgeTarget("INDEX.crates.io.", 443, { allow: ["index.crates.io"] }), undefined);
  assert.equal(judgeTarget("a.example.org", 443, { allow: ["*.example.org"] }), undefined);
  // Mutation: match the wildcard with a bare endsWith of the suffix text.
  assert.match(judgeTarget("evilexample.org", 443, { allow: ["*.example.org"] }), /not on the sandbox's egress allow-list/);
  assert.match(judgeTarget("example.org", 443, { allow: ["*.example.org"] }), /not on the sandbox's egress allow-list/);
});

// Mutation: drop the port check, or the address check.
test("another port, an IP literal and a host off the list are each refused for their own reason", () => {
  const allow = ["index.crates.io"];
  assert.match(judgeTarget("index.crates.io", 80, { allow }), /port 80 is not one/);
  assert.match(judgeTarget("140.82.112.3", 443, { allow: ["140.82.112.3"] }), /is an address/);
  assert.match(judgeTarget("[::1]", 443, { allow }), /is an address/);
  assert.match(judgeTarget("index.crates.io.evil.example", 443, { allow }), /not on the sandbox's egress allow-list/);
  assert.match(judgeTarget("bad_host!", 443, { allow }), /not a hostname/);
  assert.deepEqual(parseConnectTarget("index.crates.io:443"), { host: "index.crates.io", port: 443 });
  assert.equal(parseConnectTarget("index.crates.io"), undefined);
});

test("a policy with no sandbox block or a non-positive quota is refused, not defaulted", () => {
  assert.throws(() => sandboxPolicy({}), /no sandbox block/);
  assert.throws(() => sandboxPolicy({ sandbox: { egress_allow: ["a.b"], egress_max_requests: 0, egress_max_bytes: 1 } }), /egress_max_requests/);
  assert.throws(() => sandboxPolicy({ sandbox: { egress_allow: [], egress_max_requests: 1, egress_max_bytes: 1 } }), /egress_allow/);
});

/** A loopback upstream that echoes, and a proxy whose allowed name points at it. */
async function harness(limits) {
  const upstream = createServer((socket) => socket.pipe(socket));
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const upstreamPort = upstream.address().port;
  const log = [];
  const proxy = await serve({
    allow: ["allowed.test"],
    ports: [upstreamPort],
    ...limits,
    log: (entry) => log.push(entry),
    connectImpl: ({ port }) => connect({ host: "127.0.0.1", port }),
  });
  const close = () => Promise.all([new Promise((r) => proxy.close(r)), new Promise((r) => upstream.close(r))]);
  return { upstreamPort, proxyPort: proxy.address().port, log, close };
}

/** Send one request head through the proxy, then `payload`; resolve to everything read back. */
function through(proxyPort, head, payload = "") {
  return new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port: proxyPort });
    let got = "";
    let sent = false;
    socket.on("data", (chunk) => {
      got += chunk.toString("latin1");
      if (!sent && got.includes("\r\n\r\n") && got.startsWith("HTTP/1.1 200")) {
        sent = true;
        socket.write(payload);
        setTimeout(() => socket.end(), 100);
      }
    });
    socket.on("error", () => resolve(got));
    socket.on("close", () => resolve(got));
    socket.write(head);
  });
}

// The proxy end to end. Mutation: forward without judging the target, or
// answer 200 before the judge runs.
test("the proxy tunnels to an allowed host and refuses others before any byte reaches them", async () => {
  const h = await harness({ maxRequests: 10, maxBytes: 1_000_000 });
  try {
    const admitted = await through(h.proxyPort, `CONNECT allowed.test:${h.upstreamPort} HTTP/1.1\r\n\r\n`, "ping");
    assert.match(admitted, /^HTTP\/1\.1 200/);
    assert.match(admitted, /ping$/, "the admitted tunnel carried the payload both ways");
    const refused = await through(h.proxyPort, `CONNECT api.openai.com:${h.upstreamPort} HTTP/1.1\r\n\r\n`, "secret");
    assert.match(refused, /^HTTP\/1\.1 403/);
    assert.doesNotMatch(refused, /secret/);
    const plain = await through(h.proxyPort, "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\n\r\n");
    assert.match(plain, /^HTTP\/1\.1 405/);
    assert.deepEqual(h.log.map((e) => e.decision), ["admitted", "refused", "refused"]);
  } finally {
    await h.close();
  }
});

// The request quota. Mutation: never refuse in `admit`.
test("a run past its request quota is refused further tunnels", async () => {
  const h = await harness({ maxRequests: 1, maxBytes: 1_000_000 });
  try {
    const head = `CONNECT allowed.test:${h.upstreamPort} HTTP/1.1\r\n\r\n`;
    assert.match(await through(h.proxyPort, head, "a"), /^HTTP\/1\.1 200/);
    assert.match(await through(h.proxyPort, head, "b"), /^HTTP\/1\.1 429/);
    assert.match(h.log.at(-1).reason, /all 1 egress requests/);
  } finally {
    await h.close();
  }
});

// The byte quota. Mutation: ignore `spend`'s answer, so the tunnel carries on.
test("a run past its byte quota has its tunnel cut and the refusal logged", async () => {
  const h = await harness({ maxRequests: 10, maxBytes: 8 });
  try {
    const got = await through(h.proxyPort, `CONNECT allowed.test:${h.upstreamPort} HTTP/1.1\r\n\r\n`, "x".repeat(64));
    assert.match(got, /^HTTP\/1\.1 200/);
    assert.ok(!got.includes("x".repeat(64)), "the over-quota payload came back whole");
    const refusals = h.log.filter((e) => e.decision === "refused");
    assert.equal(refusals.length, 1);
    assert.match(refusals[0].reason, /egress quota of 8 bytes/);
  } finally {
    await h.close();
  }
});

// The run's verdict. Mutation: return [] from `verdict`, or let a missing
// log pass.
test("a run whose log holds a refusal, an unreadable line, or no log at all is refused", async () => {
  assert.deepEqual(verdict(`${JSON.stringify({ decision: "admitted", target: "a:443" })}\n`), []);
  assert.equal(verdict(`${JSON.stringify({ decision: "refused", target: "b:443", reason: "off the list" })}\n`).length, 1);
  assert.equal(verdict("{oops\n").length, 1);
  const dir = mkdtempSync(join(tmpdir(), "agent-egress-"));
  try {
    const policy = new URL("./agent-policy.json", import.meta.url).pathname;
    const clean = join(dir, "clean.jsonl");
    const dirty = join(dir, "dirty.jsonl");
    writeFileSync(clean, `${JSON.stringify({ decision: "admitted", target: "index.crates.io:443" })}\n`);
    writeFileSync(dirty, `${JSON.stringify({ decision: "refused", target: "api.openai.com:443", reason: "not on the list" })}\n`);
    const quiet = { out: () => {}, err: () => {} };
    assert.equal(await main(["verdict", "--policy", policy, "--log", clean], quiet), 0);
    assert.equal(await main(["verdict", "--policy", policy, "--log", dirty], quiet), 1);
    assert.equal(await main(["verdict", "--policy", policy, "--log", join(dir, "absent.jsonl")], quiet), 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
