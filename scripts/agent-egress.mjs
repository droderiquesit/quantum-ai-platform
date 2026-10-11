/**
 * The agent sandbox's one way out (R12 SEC-9077, SEC-9078; policy in
 * `scripts/agent-policy.json` under `sandbox`; decision in ADR 0117).
 *
 *   node scripts/agent-egress.mjs serve   --policy <p> --port <n> --log <jsonl> [--ready <file>]
 *   node scripts/agent-egress.mjs verdict --policy <p> --log <jsonl>
 *
 * Until this file the sandbox held no secret and could write nothing back,
 * but a lane's acceptance command could still open a connection to any host
 * on the internet — post the checkout somewhere, call a model provider
 * directly with a key it brought in its own diff, or pull a tool nobody
 * reviewed. "Holds nothing worth stealing" is not "can send nothing out".
 *
 * `agent-sandbox.yml` runs the lane's commands under a group the runner's
 * firewall lets reach loopback and nothing else (DNS included), so this
 * proxy, listening on loopback and running outside that group, is the only
 * route off the machine. It admits:
 *
 * - an HTTPS `CONNECT` to a host on `sandbox.egress_allow`, on port 443,
 *   and nothing else — no plain HTTP, no other port, no IP literal, no
 *   model provider (none is on the list, and `the_allow_list_names_no_model_provider`
 *   holds it so). A host is resolved only after it is admitted, so a refused
 *   name is never looked up and DNS cannot carry it out;
 * - at most `egress_max_requests` tunnels and `egress_max_bytes` in both
 *   directions over the whole run. Past either, new tunnels are refused and
 *   open ones are cut.
 *
 * Every decision is one JSONL line. `verdict` reads the log afterwards and
 * fails the run on any refusal: a lane that tried to leave and was stopped
 * is a lane whose acceptance cannot be believed, because the command that
 * was refused may be the one that would have told us so.
 *
 * What it cannot see: the bytes inside an admitted tunnel (TLS is end to
 * end), so an allow-listed host that accepts uploads — github.com does — is
 * a channel. The list is kept to package registries and the source host for
 * that reason, and ADR 0117 names it.
 *
 * Dev tooling. Node built-ins only.
 */
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, connect } from "node:net";
import { isIP } from "node:net";
import { fileURLToPath } from "node:url";

/** A hostname as the proxy will judge it: lower-cased, no trailing dot. */
const normalise = (host) => String(host ?? "").trim().toLowerCase().replace(/\.$/, "");

const HOSTNAME = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$/;

/**
 * Whether `host:port` may be reached. `allow` holds exact hostnames and
 * `*.suffix` patterns; a pattern admits subdomains, never the bare suffix,
 * so `*.githubusercontent.com` does not quietly admit a host somebody
 * registers as `evilgithubusercontent.com`. Returns a refusal or undefined.
 */
export function judgeTarget(host, port, { allow, ports = [443] }) {
  const name = normalise(host);
  if (!name) return "names no host";
  if (isIP(name.replace(/^\[|\]$/g, "")) !== 0) return `${name} is an address, not a name; an address cannot be judged against a list of names`;
  if (!HOSTNAME.test(name)) return `${JSON.stringify(host)} is not a hostname`;
  const p = Number(port);
  if (!Number.isInteger(p) || !ports.includes(p)) return `port ${port} is not one the sandbox may use (${ports.join(", ")})`;
  const admitted = allow.some((entry) => {
    const rule = normalise(entry);
    if (rule.startsWith("*.")) return name.endsWith(rule.slice(1)) && name.length > rule.length - 1;
    return name === rule;
  });
  return admitted ? undefined : `${name} is not on the sandbox's egress allow-list`;
}

/** The `host:port` of a CONNECT request target, or undefined. */
export function parseConnectTarget(target) {
  const match = /^(\[[0-9a-fA-F:.]+\]|[^:\s/]+):(\d{1,5})$/.exec(String(target ?? ""));
  return match ? { host: match[1], port: Number(match[2]) } : undefined;
}

/**
 * The run's quotas, shared by every tunnel. `admit()` reserves one request
 * or refuses; `spend(n)` books bytes and says whether the run is still
 * inside its byte quota.
 */
export function quota({ maxRequests, maxBytes }) {
  let requests = 0;
  let bytes = 0;
  return {
    admit() {
      if (requests >= maxRequests) return `the run has used all ${maxRequests} egress requests its quota allows`;
      if (bytes > maxBytes) return `the run has passed its egress quota of ${maxBytes} bytes`;
      requests += 1;
      return undefined;
    },
    spend(n) {
      bytes += n;
      return bytes <= maxBytes;
    },
    get used() {
      return { requests, bytes };
    },
  };
}

/** The sandbox block of the policy, validated. Throws naming what is wrong. */
export function sandboxPolicy(policy) {
  const s = policy?.sandbox;
  const problems = [];
  if (!s) throw new Error("agent-policy.json has no sandbox block; with no allow-list the proxy would have to guess");
  if (!Array.isArray(s.egress_allow) || s.egress_allow.length === 0 || s.egress_allow.some((h) => typeof h !== "string" || !h.trim())) {
    problems.push("sandbox.egress_allow must be a non-empty list of hostnames");
  }
  for (const key of ["egress_max_requests", "egress_max_bytes"]) {
    if (!Number.isSafeInteger(s[key]) || s[key] <= 0) problems.push(`sandbox.${key} must be a positive integer`);
  }
  if (problems.length > 0) throw new Error(problems.join("; "));
  return { allow: s.egress_allow, maxRequests: s.egress_max_requests, maxBytes: s.egress_max_bytes };
}

/**
 * Start the proxy. Resolves to the listening server. `log(entry)` receives
 * every decision; `connectImpl` and `ports` are injectable so the tests can
 * reach a loopback upstream on an ephemeral port.
 */
export function serve({ allow, maxRequests, maxBytes, port = 0, host = "127.0.0.1", log, ports = [443], connectImpl = connect }) {
  const budget = quota({ maxRequests, maxBytes });
  const tunnels = new Set();
  // Logged once: a cut tunnel can still deliver chunks already in flight,
  // and one line per chunk would bury the refusal that matters.
  let exhausted = false;
  const record = (entry) => log({ at: new Date().toISOString(), ...entry, used: budget.used });
  const cutAll = () => {
    for (const socket of tunnels) socket.destroy();
  };
  const server = createServer((client) => {
    client.on("error", () => {});
    let head = Buffer.alloc(0);
    const onData = (chunk) => {
      head = Buffer.concat([head, chunk]);
      const end = head.indexOf("\r\n\r\n");
      if (end < 0) {
        if (head.length > 16_384) {
          record({ decision: "refused", reason: "a request head larger than 16 KiB" });
          client.end("HTTP/1.1 431 Request Header Fields Too Large\r\nConnection: close\r\n\r\n");
        }
        return;
      }
      client.off("data", onData);
      const [requestLine] = head.subarray(0, end).toString("latin1").split("\r\n");
      const [method, target] = requestLine.split(" ");
      const refuse = (status, reason, detail = {}) => {
        record({ decision: "refused", method, target, reason, ...detail });
        client.end(`HTTP/1.1 ${status}\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n${reason}\n`);
      };
      if (method !== "CONNECT") return refuse("405 Method Not Allowed", `${method} is refused; the sandbox reaches the internet only over HTTPS CONNECT`);
      const parsed = parseConnectTarget(target);
      if (!parsed) return refuse("400 Bad Request", "the CONNECT target is not host:port");
      const refusal = judgeTarget(parsed.host, parsed.port, { allow, ports });
      if (refusal) return refuse("403 Forbidden", refusal);
      const over = budget.admit();
      if (over) return refuse("429 Too Many Requests", over);
      const upstream = connectImpl({ host: normalise(parsed.host), port: parsed.port });
      tunnels.add(client);
      tunnels.add(upstream);
      const forget = () => {
        tunnels.delete(client);
        tunnels.delete(upstream);
      };
      upstream.on("error", (cause) => {
        record({ decision: "failed", target, reason: cause.code ?? cause.message });
        client.destroy();
        forget();
      });
      upstream.on("close", forget);
      upstream.once("connect", () => {
        record({ decision: "admitted", target });
        client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
        const rest = head.subarray(end + 4);
        if (rest.length > 0) upstream.write(rest);
        const meter = (from, to) =>
          from.on("data", (chunk) => {
            if (!budget.spend(chunk.length)) {
              if (!exhausted) record({ decision: "refused", target, reason: `the run passed its egress quota of ${maxBytes} bytes; every tunnel was cut` });
              exhausted = true;
              cutAll();
              return;
            }
            to.write(chunk);
          });
        meter(client, upstream);
        meter(upstream, client);
        client.on("close", () => upstream.destroy());
        upstream.on("close", () => client.destroy());
      });
    };
    client.on("data", onData);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, host, () => resolve(server));
  });
}

/**
 * Judge a run by its log. Returns the refusals, one string each; an empty
 * list means the run stayed inside its egress limits. A line that does not
 * parse is itself a refusal: a log that cannot be read cannot vouch for the
 * run.
 */
export function verdict(text) {
  const problems = [];
  for (const [i, line] of String(text).split("\n").entries()) {
    if (!line.trim()) continue;
    let entry;
    try {
      entry = JSON.parse(line);
    } catch {
      problems.push(`line ${i + 1} of the egress log is not JSON`);
      continue;
    }
    if (entry.decision === "refused") problems.push(`${entry.target ?? entry.method ?? "a request"}: ${entry.reason}`);
  }
  return problems;
}

function option(argv, name) {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 ? argv[at + 1] : undefined;
}

/** The command line, end to end. Resolves to an exit code, or to a server for `serve`. */
export async function main(argv, { out = console.log, err = console.error } = {}) {
  const [mode] = argv;
  const policyPath = option(argv, "policy");
  const logPath = option(argv, "log");
  if (!["serve", "verdict"].includes(mode) || !policyPath || !logPath) {
    err("usage: agent-egress.mjs serve --policy <p> --port <n> --log <jsonl> [--ready <file>] | verdict --policy <p> --log <jsonl>");
    return 64;
  }
  let limits;
  try {
    limits = sandboxPolicy(JSON.parse(readFileSync(policyPath, "utf8")));
  } catch (cause) {
    err(`cannot use the agent policy ${policyPath}: ${cause.message}`);
    return 1;
  }
  if (mode === "verdict") {
    let text;
    try {
      text = readFileSync(logPath, "utf8");
    } catch (cause) {
      // No log is not a clean run: it is a run whose proxy never started or
      // whose record is gone, and either way nothing vouches for its egress.
      err(`cannot read the egress log ${logPath}: ${cause.message}`);
      return 1;
    }
    const problems = verdict(text);
    for (const problem of problems) err(`egress refused — ${problem}`);
    if (problems.length > 0) {
      err(`the lane tried to leave the sandbox ${problems.length} time(s); its acceptance is not accepted`);
      return 1;
    }
    out(`the lane stayed inside the sandbox's egress allow-list and quota (${limits.maxRequests} requests, ${limits.maxBytes} bytes)`);
    return 0;
  }
  const port = Number(option(argv, "port"));
  if (!Number.isInteger(port) || port <= 0 || port > 65_535) {
    err("serve needs --port, a TCP port");
    return 64;
  }
  writeFileSync(logPath, "");
  const server = await serve({ ...limits, port, log: (entry) => appendFileSync(logPath, `${JSON.stringify(entry)}\n`) });
  const ready = option(argv, "ready");
  if (ready) writeFileSync(ready, `${port}\n`);
  out(`agent egress proxy on 127.0.0.1:${port}, ${limits.allow.length} host rule(s)`);
  return server;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const result = await main(process.argv.slice(2));
  if (typeof result === "number") process.exit(result);
}
