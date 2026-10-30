/**
 * The gateway's circuit breaker and the timeout it is built on (R12-GCP-8119:
 * warm-service clients enforce timeouts and breakers).
 *
 * The end-to-end test drives the real route handler over a real socket with
 * the `request` fixture. It cannot use `page.route`: Playwright would fulfil
 * the request before the gateway ran, and a breaker test that never reaches
 * the breaker passes whatever the breaker does. The upstream is a server this
 * file opens and closes on the port `playwright.config.ts` points the breaker
 * instance at, so the test controls exactly when the platform is "down" — and,
 * crucially, can bring it back *while the circuit is open* and prove the
 * gateway still did not ask it.
 *
 * The unit tests below it exercise `upstream()`'s refusal of a bad timeout and
 * `CircuitBreakers` against an injected clock. They run in the test process,
 * not against a server, because what they prove is a function's contract.
 */
import { createServer, type Server } from "node:http";
import { expect, test } from "@playwright/test";
import { CircuitBreakers, MAX_TRACKED_UPSTREAMS } from "../src/lib/server/breaker";
import { upstream, UpstreamNotConfigured } from "../src/lib/server/upstream";

const UPSTREAM_PORT = Number(process.env.PLAYWRIGHT_BREAKER_UPSTREAM_PORT ?? 3320);
/** Must match the breaker instance's `QIP_API_BREAKER_*` in the config. */
const THRESHOLD = 3;
const COOLDOWN_MS = 4_000;

interface Platform {
  readonly server: Server;
  hits: number;
  /** Delay before answering, so a trial can be held in flight. */
  delayMs: number;
}

async function openPlatform(): Promise<Platform> {
  const platform: Platform = { server: createServer(), hits: 0, delayMs: 0 };
  platform.server.on("request", (_request, response) => {
    platform.hits += 1;
    setTimeout(() => {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ status: "ok" }));
    }, platform.delayMs);
  });
  await new Promise<void>((resolve, reject) => {
    platform.server.once("error", reject);
    platform.server.listen(UPSTREAM_PORT, "127.0.0.1", () => resolve());
  });
  return platform;
}

function closePlatform(platform: Platform): Promise<void> {
  platform.server.closeAllConnections();
  return new Promise((resolve) => platform.server.close(() => resolve()));
}

test("the console gateway opens its breaker after consecutive upstream failures and refuses fast until the cool-down", async ({
  request,
}) => {
  test.setTimeout(30_000);

  // Premise: nothing listens upstream, so each request is a real transport
  // failure the gateway reports as `unreachable` — not `open`. If the first
  // were already `open` the circuit was tripped by something else and every
  // assertion below would be about that.
  for (let attempt = 1; attempt <= THRESHOLD; attempt += 1) {
    const failed = await request.get("/api/gateway/health");
    expect(failed.status(), `failure ${attempt}`).toBe(502);
    expect(failed.headers()["x-qip-gateway"], `failure ${attempt}`).toBe("unreachable");
  }
  const openedAt = Date.now();

  // The platform comes back while the circuit is open. The gateway must not
  // find out: refusing fast means not asking, and a hit count above zero
  // would mean it asked and merely reported the answer as a refusal.
  const platform = await openPlatform();
  try {
    const started = Date.now();
    const refused = await request.get("/api/gateway/health");
    const took = Date.now() - started;
    expect(refused.status()).toBe(503);
    expect(refused.headers()["x-qip-gateway"]).toBe("open");
    // A refusal is never cached: `next.config.ts` once overwrote every
    // response's Cache-Control with a year-long `immutable`, this one included.
    expect(refused.headers()["cache-control"]).toBe("no-store");
    expect(Number(refused.headers()["retry-after"])).toBeGreaterThanOrEqual(1);
    const body = (await refused.json()) as { error: string; gateway: string; upstream: string };
    expect(body.gateway).toBe("open");
    expect(body.upstream).toBe("/api/v1/health");
    expect(body.error).toContain(`open after ${THRESHOLD} consecutive failures`);
    // The upstream address is configuration the public may not see; the
    // refusal names the path, as the other gateway errors do.
    expect(body.error).not.toContain(String(UPSTREAM_PORT));
    expect(platform.hits, "an open circuit must not touch the network").toBe(0);
    // Fast: well inside the one-second upstream timeout it is spared.
    expect(took).toBeLessThan(750);

    // Wait out the cool-down, measured from the third failure.
    const remaining = COOLDOWN_MS - (Date.now() - openedAt) + 250;
    if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining));

    // Half-open: exactly one trial. Hold it in flight and send a second
    // request beside it; the second is refused rather than joining a stampede.
    platform.delayMs = 600;
    const trial = request.get("/api/gateway/health");
    await expect.poll(() => platform.hits, { timeout: 2_000 }).toBe(1);
    const beside = await request.get("/api/gateway/health");
    expect(beside.status()).toBe(503);
    expect(beside.headers()["x-qip-gateway"]).toBe("open");
    expect(((await beside.json()) as { error: string }).error).toContain("half-open");
    const answered = await trial;
    expect(answered.status()).toBe(200);
    expect(answered.headers()["x-qip-gateway"]).toBe("upstream");
    expect(platform.hits).toBe(1);

    // The trial succeeded, so the circuit is closed and requests flow again.
    platform.delayMs = 0;
    const flowing = await request.get("/api/gateway/health");
    expect(flowing.status()).toBe(200);
    expect(flowing.headers()["x-qip-gateway"]).toBe("upstream");
    expect(platform.hits).toBe(2);
  } finally {
    await closePlatform(platform);
  }
});

test.describe("upstream() refuses a timeout it cannot honour rather than clamping it", () => {
  const saved = {
    base: process.env.QIP_API_BASE_URL,
    timeout: process.env.QIP_API_TIMEOUT_MS,
    threshold: process.env.QIP_API_BREAKER_THRESHOLD,
    cooldown: process.env.QIP_API_BREAKER_COOLDOWN_MS,
  };

  function restore(name: string, value: string | undefined) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }

  test.beforeEach(() => {
    process.env.QIP_API_BASE_URL = "http://127.0.0.1:9";
    delete process.env.QIP_API_TIMEOUT_MS;
    delete process.env.QIP_API_BREAKER_THRESHOLD;
    delete process.env.QIP_API_BREAKER_COOLDOWN_MS;
  });

  test.afterEach(() => {
    restore("QIP_API_BASE_URL", saved.base);
    restore("QIP_API_TIMEOUT_MS", saved.timeout);
    restore("QIP_API_BREAKER_THRESHOLD", saved.threshold);
    restore("QIP_API_BREAKER_COOLDOWN_MS", saved.cooldown);
  });

  test("an unset timeout is the documented default and a positive one is honoured exactly", () => {
    expect(upstream().timeoutMs).toBe(10_000);
    process.env.QIP_API_TIMEOUT_MS = "1500";
    expect(upstream().timeoutMs).toBe(1500);
    expect(upstream().breaker).toEqual({ threshold: 5, cooldownMs: 30_000 });
  });

  // Each of these was silently turned into 10 000ms before. `-5` is first on
  // purpose: a negative timeout is the case the clamp most plausibly hid.
  for (const bad of ["-5", "0", "abc", "", "1.5", "1e3", "Infinity", "2147483648"]) {
    test(`a QIP_API_TIMEOUT_MS of ${JSON.stringify(bad)} stops the gateway as misconfigured`, () => {
      process.env.QIP_API_TIMEOUT_MS = bad;
      expect(() => upstream()).toThrow(UpstreamNotConfigured);
      expect(() => upstream()).toThrow(/QIP_API_TIMEOUT_MS must be a whole number/);
    });
  }

  for (const [name, bad] of [
    ["QIP_API_BREAKER_THRESHOLD", "0"],
    ["QIP_API_BREAKER_COOLDOWN_MS", "-1"],
  ] as const) {
    test(`a ${name} of ${JSON.stringify(bad)} is refused too`, () => {
      process.env[name] = bad;
      expect(() => upstream()).toThrow(new RegExp(`${name} must be a whole number`));
    });
  }
});

test.describe("CircuitBreakers against a clock the test controls", () => {
  const settings = { threshold: 2, cooldownMs: 1_000 };

  test("an answer between failures resets the count, so only consecutive failures open it", () => {
    const breakers = new CircuitBreakers(() => 0);
    breakers.failed("a", settings);
    breakers.succeeded("a");
    breakers.failed("a", settings);
    expect(breakers.stateOf("a")).toBe("closed");
    breakers.failed("a", settings);
    expect(breakers.stateOf("a")).toBe("open");
  });

  test("a failed trial re-opens the circuit with a fresh cool-down", () => {
    let now = 0;
    const breakers = new CircuitBreakers(() => now);
    breakers.failed("a", settings);
    breakers.failed("a", settings);
    now = 999;
    expect(breakers.admit("a", settings).admitted).toBe(false);
    now = 1_000;
    expect(breakers.admit("a", settings)).toEqual({ admitted: true, trial: true });
    expect(breakers.admit("a", settings).admitted).toBe(false);
    breakers.failed("a", settings);
    expect(breakers.stateOf("a")).toBe("open");
    now = 1_999;
    expect(breakers.admit("a", settings).admitted).toBe(false);
    now = 2_000;
    expect(breakers.admit("a", settings).admitted).toBe(true);
  });

  test("one upstream's open circuit does not refuse another's, and the map stays bounded", () => {
    const breakers = new CircuitBreakers(() => 0);
    breakers.failed("down", settings);
    breakers.failed("down", settings);
    expect(breakers.admit("down", settings).admitted).toBe(false);
    expect(breakers.admit("up", settings).admitted).toBe(true);
    for (let index = 0; index < MAX_TRACKED_UPSTREAMS * 3; index += 1) {
      breakers.failed(`upstream-${index}`, settings);
    }
    expect(breakers.size).toBe(MAX_TRACKED_UPSTREAMS);
  });
});
