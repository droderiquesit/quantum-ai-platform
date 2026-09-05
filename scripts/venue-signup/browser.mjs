/**
 * The smallest browser driver the signup job can be honest with.
 *
 * Playwright is not resolvable from this repository — `frontend/portal` does
 * not depend on it and nothing else does — so rather than add a package for
 * one dev-tooling job, this drives the Chromium that Playwright's browser
 * bundle provides (`/opt/pw-browsers/chromium`) directly over the DevTools
 * protocol, on the `--remote-debugging-pipe` file descriptors. No port is
 * opened, so no other process on the machine can attach to the session that
 * is typing the company's identity into a form.
 *
 * It does five things: launch, navigate, evaluate, screenshot, close. It has
 * no notion of a "solve", a retry, or a wait-for-human; a page it cannot
 * proceed on is a page it hands back.
 *
 * ## What it refuses
 *
 * - Running with TLS verification off. `NODE_TLS_REJECT_UNAUTHORIZED=0` in the
 *   environment stops the launch, and the driver never passes
 *   `--ignore-certificate-errors`. If the egress proxy's certificate is not in
 *   the system store the job fails on TLS, and that is the correct outcome:
 *   the fix is installing the CA, not trusting whatever answers.
 * - A proxy setting it cannot honour. `HTTPS_PROXY` becomes Chromium's
 *   `--proxy-server` and `NO_PROXY` its bypass list, matching how every other
 *   tool in this repository leaves the machine.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const DEFAULT_CHROMIUM = "/opt/pw-browsers/chromium";

/**
 * Why a launch is refused before a process is spawned, or `null`.
 *
 * Takes the environment map rather than reading `process.env` so the rule
 * is testable without mutating the process, the same reason the gateway's
 * `configure` does.
 */
export function launchRefusal(env = process.env) {
  if (env.NODE_TLS_REJECT_UNAUTHORIZED !== undefined && env.NODE_TLS_REJECT_UNAUTHORIZED.trim() === "0") {
    return (
      "NODE_TLS_REJECT_UNAUTHORIZED=0 is set, which disables TLS verification; the job does " +
      "not run without it. If the egress proxy's certificate is not trusted, install its CA " +
      "in the system store rather than trusting whatever answers."
    );
  }
  return null;
}

/** The Chromium arguments the job runs with. Exported so a test can assert what is and is not passed. */
export function chromiumArguments({ env = process.env, profileDir, uid = process.getuid?.() }) {
  const args = [
    "--headless=new",
    "--remote-debugging-pipe",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-gpu",
    "--disable-extensions",
    "--disable-background-networking",
    "--disable-sync",
    "--password-store=basic",
    `--user-data-dir=${profileDir}`,
  ];
  const proxy = (env.HTTPS_PROXY ?? env.https_proxy ?? "").trim();
  if (proxy) args.push(`--proxy-server=${proxy}`);
  const noProxy = (env.NO_PROXY ?? env.no_proxy ?? "").trim();
  if (noProxy) args.push(`--proxy-bypass-list=${noProxy}`);
  // Chromium refuses to start its own sandbox as root. The flag drops
  // Chromium's renderer sandbox and nothing else; the operator's machine
  // should not be running this as root in the first place, and a container
  // that does is told so on stderr at launch.
  if (uid === 0) args.push("--no-sandbox");
  args.push("about:blank");
  return args;
}

/**
 * Launch Chromium and attach to its first page.
 *
 * Every protocol call carries a deadline; a browser that stops answering is
 * killed rather than waited on, because a job that hangs on a venue's page
 * is a job the operator cannot tell from one that is working.
 */
export async function launch({
  executablePath = DEFAULT_CHROMIUM,
  env = process.env,
  callTimeoutMs = 30_000,
  stderr = process.stderr,
} = {}) {
  const refusal = launchRefusal(env);
  if (refusal) throw new Error(refusal);

  const profileDir = mkdtempSync(join(tmpdir(), "venue-signup-profile-"));
  const uid = process.getuid?.();
  if (uid === 0) stderr.write("note: running as root, so Chromium's own sandbox is off (--no-sandbox)\n");
  const args = chromiumArguments({ env, profileDir, uid });

  let child;
  try {
    child = spawn(executablePath, args, {
      // fd 3 is the browser's protocol input, fd 4 its output. stdout and
      // stderr are read and discarded: Chromium's D-Bus complaints would
      // otherwise bury the one line the operator needs.
      stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"],
      env: { ...env, HOME: profileDir },
    });
  } catch (cause) {
    rmSync(profileDir, { recursive: true, force: true });
    throw new Error(`Chromium could not be started from ${executablePath}: ${cause.message}`);
  }

  const lastStderr = [];
  child.stderr.on("data", (chunk) => {
    lastStderr.push(String(chunk));
    if (lastStderr.length > 20) lastStderr.shift();
  });

  const pending = new Map();
  const listeners = new Map();
  let nextId = 0;
  let buffer = "";
  let exited = null;
  const reader = child.stdio[4];
  const writer = child.stdio[3];

  reader.on("data", (chunk) => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\0")) !== -1) {
      const raw = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      let message;
      try {
        message = JSON.parse(raw);
      } catch {
        continue;
      }
      if (message.id !== undefined && pending.has(message.id)) {
        const { resolve, reject, timer } = pending.get(message.id);
        clearTimeout(timer);
        pending.delete(message.id);
        if (message.error) reject(new Error(`${message.error.message ?? "protocol error"}`));
        else resolve(message.result ?? {});
      } else if (message.method) {
        for (const handler of listeners.get(message.method) ?? []) handler(message.params ?? {}, message.sessionId);
      }
    }
  });
  reader.on("error", () => {});
  writer.on("error", () => {});

  const spawnFailure = new Promise((_, reject) => {
    child.on("error", (cause) => reject(new Error(`Chromium could not be started from ${executablePath}: ${cause.message}`)));
    child.on("exit", (code, signal) => {
      exited = { code, signal };
      for (const { reject: rejectCall, timer } of pending.values()) {
        clearTimeout(timer);
        rejectCall(new Error(`Chromium exited (${signal ?? code}) before answering`));
      }
      pending.clear();
      reject(new Error(`Chromium exited (${signal ?? code}) during launch: ${lastStderr.join("").trim().split("\n").pop() ?? ""}`));
    });
  });
  // Nobody awaits spawnFailure after launch has finished; keep the process from
  // treating the eventual exit as an unhandled rejection.
  spawnFailure.catch(() => {});

  function send(method, params = {}, sessionId) {
    if (exited) return Promise.reject(new Error(`Chromium has exited (${exited.signal ?? exited.code})`));
    const id = ++nextId;
    const message = sessionId ? { id, method, params, sessionId } : { id, method, params };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} did not answer within ${callTimeoutMs} ms`));
      }, callTimeoutMs);
      pending.set(id, { resolve, reject, timer });
      writer.write(`${JSON.stringify(message)}\0`);
    });
  }

  function on(method, handler) {
    if (!listeners.has(method)) listeners.set(method, []);
    listeners.get(method).push(handler);
    return () => {
      const list = listeners.get(method);
      const at = list.indexOf(handler);
      if (at !== -1) list.splice(at, 1);
    };
  }

  const version = await Promise.race([send("Browser.getVersion"), spawnFailure]);
  const { targetInfos } = await send("Target.getTargets");
  const page = targetInfos.find((t) => t.type === "page");
  if (!page) throw new Error("Chromium started but opened no page");
  const { sessionId } = await send("Target.attachToTarget", { targetId: page.targetId, flatten: true });
  await send("Page.enable", {}, sessionId);
  await send("Runtime.enable", {}, sessionId);

  let closed = false;
  async function close() {
    if (closed) return;
    closed = true;
    try {
      await Promise.race([send("Browser.close"), new Promise((r) => setTimeout(r, 2_000))]);
    } catch {
      // Already gone; the kill below is the backstop either way.
    }
    if (!exited) child.kill("SIGKILL");
    rmSync(profileDir, { recursive: true, force: true });
  }

  /** Navigate and wait for the load event, or fail with the navigation error text. */
  async function navigate(url, timeoutMs = callTimeoutMs) {
    const loaded = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off();
        reject(new Error(`${url} did not finish loading within ${timeoutMs} ms`));
      }, timeoutMs);
      const off = on("Page.loadEventFired", (_params, forSession) => {
        if (forSession !== sessionId) return;
        clearTimeout(timer);
        off();
        resolve();
      });
    });
    const result = await send("Page.navigate", { url }, sessionId);
    if (result.errorText) {
      throw new Error(`navigation to ${url} failed: ${result.errorText}`);
    }
    await loaded;
  }

  /**
   * Evaluate an expression in the page and return its JSON value.
   *
   * A page in the middle of a navigation has no execution context; that is
   * reported as `{ ready: false }` rather than thrown, so a poll loop can
   * try again after the next load rather than mistake it for a hard stop.
   */
  async function evaluate(expression) {
    let result;
    try {
      result = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true }, sessionId);
    } catch (cause) {
      if (/execution context|Cannot find context|Inspected target navigated/i.test(cause.message)) {
        return { ready: false };
      }
      throw cause;
    }
    if (result.exceptionDetails) {
      const text = result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? "unknown";
      throw new Error(`the page script threw: ${text.split("\n")[0]}`);
    }
    return { ready: true, value: result.result?.value };
  }

  async function screenshot(path) {
    const { data } = await send("Page.captureScreenshot", { format: "png" }, sessionId);
    writeFileSync(path, Buffer.from(data, "base64"));
    return path;
  }

  async function currentUrl() {
    const { value } = await evaluate("location.href");
    return value ?? "";
  }

  return { product: version.product, navigate, evaluate, screenshot, currentUrl, close };
}
