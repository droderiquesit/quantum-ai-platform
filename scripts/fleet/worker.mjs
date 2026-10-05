/**
 * The fleet worker (ADR 0102): one Cloud Run Job task, one packet, one call.
 *
 * It reads `packets/$FLEET_RUN/$CLOUD_RUN_TASK_INDEX.json` from the fleet
 * bucket, makes at most one chat call through the gateway's `vertex` preset,
 * and leaves `output/$FLEET_RUN/$INDEX.json` and one ledger object behind.
 * It holds nothing between packets and has nothing to execute with: no shell,
 * no checkout, no tool. **The model's answer is data.** It is hashed, written
 * to the bucket as a string and never parsed, evaluated or branched on beyond
 * "was it empty", because a worker that acts on what a model wrote has the
 * whole prompt as its attack surface.
 *
 * ## What it refuses, in the order it refuses
 *
 * 1. `gs://$FLEET_BUCKET/HALT` exists. Checked first, and again immediately
 *    before the call. Not being able to tell is also a refusal: a kill switch
 *    read as "absent" whenever the read fails is not a switch.
 * 2. The packet is missing, is not JSON, or fails `validatePacket`. Nothing is
 *    repaired, defaulted or clamped: a packet silently corrected is a
 *    dispatcher bug that survives.
 * 3. The packet's model has no row in `prices.json`. No price, no worst case;
 *    no worst case, no run. An estimate is not a price.
 * 4. The worst case (both token bounds at the row's prices) exceeds the slot
 *    cap.
 * 5. The day's ledger total plus that worst case would pass 80% of the daily
 *    ceiling.
 * 6. The packet's ledger object already exists. Its creation is the packet's
 *    lock, so a re-execution of the same run cannot bill the same packet
 *    twice whatever the Job's retry setting says.
 *
 * ## The ledger is written twice and exists once
 *
 * The object is created *before* the call, as a reservation billed at the
 * worst case, and overwritten after it with what the API reported. A task
 * Cloud Run stops at its timeout therefore leaves a record that overstates
 * its spend, where a ledger written only on the way out would leave none, and
 * spend with no ledger object is the one thing ADR 0102 says halts the fleet.
 * `cost_basis` says which kind of number each object holds: `reported` is a
 * measurement, `worst_case` is a bound, `worst_case_no_response` is a bound
 * because a transport failure does not say whether the model ran, `no_call`
 * is zero because nothing was sent, and `refused_unbilled` is zero because
 * the provider answered 429 or 503 and so processed nothing.
 *
 * ## Retry (ADR 0102, measured 2026-10-05)
 *
 * Vertex answered 429 to 17 of 40 calls on one model and 4 of 17 on another
 * at Job parallelism 8. The one chat call is retried on 429 and 503 only, at
 * most four attempts in all, and billed for what ran: a refusal that carries
 * no usage costs nothing, where billing it at the worst case booked 22,440 of
 * a run's 27,252 micro-USD as spend that never happened.
 *
 * ## Money
 *
 * Integer micro-USD throughout, from decimal text. No amount is ever a binary
 * float: `8.2 * 1e6` is `8199999.999999999` in this language, and a ledger
 * summed from numbers like that disagrees with the invoice by an amount
 * nobody can explain. A cost is rounded **up** to the micro-dollar, once per
 * packet, so the ledger can overstate by under a millionth of a dollar a
 * packet and can never understate.
 *
 * Dev tooling, like the gateway it calls: nothing in `backend/crates/` may
 * import it. Node built-ins only.
 */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  WORKER_SYSTEM_PROMPT,
  chatCompletion,
  completionText,
  configure,
  metadataToken,
  screenPayload,
} from "../model-gateway.mjs";

/** The committed price table. Every row is dated and sourced, or it is not a row. */
export const PRICES = JSON.parse(readFileSync(new URL("./prices.json", import.meta.url), "utf8"));

/**
 * ADR 0102 decision 1's roster. There is no architect, integrator, security,
 * risk, execution or merge role at any size: those stay with the lead, and
 * their absence from this list is how a packet for one is refused.
 */
export const ROLES = ["scout", "fixture", "drafter", "refactor", "tester", "reviewer", "ops-watch", "catalogue"];

/**
 * Orchestration policy §4 gate 6: sources that do not leave this machine at
 * any price, whatever the provider's terms say.
 */
export const RESERVED_SOURCE = [
  "backend/crates/services/qip-risk-engine",
  "backend/crates/services/qip-execution-engine",
  "backend/crates/services/qip-capital",
  "backend/crates/libs/qip-compliance",
  "backend/crates/edge",
];

/** Also what makes a run id safe as one `--update-env-vars` value: no comma, no `=`. */
export const RUN_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;
export const BUCKET_NAME = /^[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]$/;
const PACKET_ID = /^[a-z0-9][a-z0-9._-]{0,62}$/;
const TEXT_FIELDS = ["packet_id", "role", "model", "why_this_tier", "task", "context", "acceptance", "escalate_if"];
const TOKEN_FIELDS = ["max_input_tokens", "max_output_tokens"];
const PATH_FIELDS = ["paths", "source_paths"];
const FIELDS = [...TEXT_FIELDS, ...PATH_FIELDS, ...TOKEN_FIELDS];

/** ADR 0102 retry policy: 429 and 503 only, four attempts in all. */
export const RETRY_STATUSES = [429, 503];
export const MAX_ATTEMPTS = 4;
const BACKOFF_BASE_MS = 2000;
const MAX_WAIT_MS = 60_000;

/** Below the Job's default 600 s task timeout, so the worker settles its own ledger. */
const CHAT_TIMEOUT_MS = 300_000;
const STORAGE = "https://storage.googleapis.com";
const METADATA_PROJECT_URL = "http://metadata.google.internal/computeMetadata/v1/project/project-id";

/**
 * Decimal text to integer micro-USD, exactly; `null` for anything else.
 *
 * Text only, and only plain decimals: `1e3`, `-1`, `.5` and a seventh decimal
 * place are refused rather than interpreted, and a number is refused because
 * by the time a value is a number the float has already happened.
 */
export function usdToMicro(text) {
  if (typeof text !== "string") return null;
  const match = /^(\d{1,9})(?:\.(\d{1,6}))?$/.exec(text);
  if (!match) return null;
  return Number(match[1]) * 1_000_000 + Number((match[2] ?? "").padEnd(6, "0"));
}

/** Micro-USD as a decimal string, by integer arithmetic only. */
export function usd(micro) {
  return `${Math.floor(micro / 1_000_000)}.${String(micro % 1_000_000).padStart(6, "0")}`;
}

/** The worker's environment, each value validated; nothing is defaulted. */
export function readEnv(env) {
  const problems = [];
  const take = (name, pattern, what) => {
    const value = env[name];
    if (typeof value === "string" && pattern.test(value)) return value;
    problems.push(`${name} is ${value === undefined ? "not set" : `'${value}'`}; it must be ${what}`);
    return undefined;
  };
  const money = (name) => {
    const micro = usdToMicro(env[name]);
    if (micro) return micro;
    problems.push(
      `${name} is ${env[name] === undefined ? "not set" : `'${env[name]}'`}; it must be a positive USD amount ` +
        "as plain decimal text, such as 25 or 0.5 (infrastructure/fleet sets it)",
    );
    return undefined;
  };
  const bucket = take("FLEET_BUCKET", BUCKET_NAME, "the fleet bucket's name (infrastructure/fleet sets it)");
  const run = take("FLEET_RUN", RUN_ID, "a run id of lower-case letters, digits and hyphens (execute with --update-env-vars FLEET_RUN=<id>)");
  const index = take("CLOUD_RUN_TASK_INDEX", /^(0|[1-9]\d{0,3})$/, "the task index Cloud Run sets");
  const ceilingMicro = money("FLEET_DAILY_USD_CEILING");
  const slotCapMicro = money("FLEET_SLOT_USD_CAP");
  return { problems, bucket, run, index: index === undefined ? undefined : Number(index), ceilingMicro, slotCapMicro };
}

/**
 * Why a path may not be a fleet packet's, or `null` when it may.
 *
 * Compared segment by segment: `qip-capital-extra` is a different crate from
 * `qip-capital` and a substring test calls them the same. `./` and doubled
 * slashes are normalised for the comparison; `..` is refused outright,
 * because the path somebody meant is the one it resolves to and they can
 * write that. A directory that *holds* a reserved one is refused with it:
 * "the context is `backend/crates`" names all five.
 */
export function reservedPath(path) {
  if (typeof path !== "string" || path === "" || path.startsWith("/") || path.includes("\\")) {
    return "is not a repo-relative path, so it cannot be classified";
  }
  const segments = path.toLowerCase().split("/").filter((part) => part !== "" && part !== ".");
  if (segments.includes("..")) return "contains '..', so it cannot be classified as written; give the path it resolves to";
  if (segments.length === 0) return "names the whole repository, which holds every reserved source";
  for (const reserved of RESERVED_SOURCE) {
    const fence = reserved.split("/");
    const shared = Math.min(fence.length, segments.length);
    if (fence.slice(0, shared).every((part, at) => part === segments[at])) {
      return `is ${segments.length < fence.length ? "a directory holding" : "under"} ${reserved}, which does not leave this machine at any price`;
    }
  }
  return null;
}

/** The user message, exactly as it is sent and hashed. */
export function payloadOf(packet) {
  return [
    packet.task,
    "",
    "Context:",
    packet.context,
    "",
    `Acceptance: ${packet.acceptance}`,
    `Files you may change: ${packet.paths.length > 0 ? packet.paths.join(", ") : "none; return findings only"}`,
    `Stop and say so, rather than widening the task, if: ${packet.escalate_if}`,
  ].join("\n");
}

/**
 * Every reason a packet may not run, each naming what to do. Empty means valid.
 *
 * The dispatcher and the worker call this one function, so a packet the
 * dispatcher admits is not refused in the cloud for a rule only the worker
 * knew, and the reverse.
 */
export function validatePacket(packet) {
  if (packet === null || typeof packet !== "object" || Array.isArray(packet)) {
    return ["the packet is not a JSON object"];
  }
  const problems = [];
  for (const key of Object.keys(packet)) {
    // An unknown field is somebody's `max_tokens` being quietly ignored.
    if (!FIELDS.includes(key)) problems.push(`'${key}' is not a packet field; remove it (fields: ${FIELDS.join(", ")})`);
  }
  for (const field of TEXT_FIELDS) {
    if (typeof packet[field] !== "string" || packet[field].trim() === "") {
      problems.push(`'${field}' must be a non-empty string`);
    }
  }
  if (typeof packet.packet_id === "string" && packet.packet_id.trim() !== "" && !PACKET_ID.test(packet.packet_id)) {
    problems.push(`packet_id '${packet.packet_id}' must be lower-case letters, digits, '.', '_' or '-', at most 63`);
  }
  if (typeof packet.role === "string" && packet.role.trim() !== "" && !ROLES.includes(packet.role)) {
    problems.push(
      `role '${packet.role}' has no row in ADR 0102's roster (${ROLES.join(", ")}); ` +
        "architecture, integration, security, risk, execution and merge stay with the lead",
    );
  }
  for (const field of TOKEN_FIELDS) {
    if (!Number.isSafeInteger(packet[field]) || packet[field] <= 0) {
      problems.push(`'${field}' must be a positive whole number of tokens: it is a bound the worst case is priced on`);
    }
  }
  for (const field of PATH_FIELDS) {
    if (!Array.isArray(packet[field])) {
      problems.push(
        `'${field}' must be an array of repo-relative paths, empty if there are none` +
          (field === "source_paths"
            ? "; policy §4 gate 6 cannot classify a packet that does not say where its context came from"
            : ""),
      );
      continue;
    }
    packet[field].forEach((path, at) => {
      const why = reservedPath(path);
      if (why) {
        problems.push(
          `${field}[${at}] ${JSON.stringify(path)} ${why} (policy §4 gate 6). ` +
            "Take it out of the packet; work on reserved source stays with the lead",
        );
      }
    });
  }
  // What follows reads the fields above as well-formed.
  if (problems.length > 0) return problems;

  const payload = payloadOf(packet);
  const found = screenPayload(payload);
  if (found.length > 0) {
    problems.push(
      `the payload carries ${found.join(", ")}. Sharing source is authorised and sharing a credential is not: ` +
        "remove it from the context (policy §4 gate 6)",
    );
  }
  // ponytail: bytes stand in for tokens, because a tokenizer in-tree is a
  // dependency. A tokenizer emits no more tokens than the text has bytes, so
  // this is conservative by roughly four on prose; the provider's chat
  // template adds a handful of its own that this cannot see, and `over_bound`
  // after the call is what catches those. Count tokens properly if the factor
  // of four ever prices a real packet out of its slot.
  const bytes = Buffer.byteLength(WORKER_SYSTEM_PROMPT + payload, "utf8");
  if (bytes > packet.max_input_tokens) {
    problems.push(
      `the prompt is ${bytes} bytes and max_input_tokens is ${packet.max_input_tokens}, so the input bound the ` +
        `worst case is priced on could be exceeded. Raise max_input_tokens to at least ${bytes} or shrink the context`,
    );
  }
  return problems;
}

/**
 * The price row for a model, or `null` if the table has none.
 *
 * `Object.hasOwn`, not a property read: `constructor` and `toString` are
 * properties of every object and neither is a model anybody priced.
 */
export function priceRow(prices, model) {
  if (!Object.hasOwn(prices.models, model)) return null;
  const row = prices.models[model];
  const input = usdToMicro(row.input_usd_per_mtok);
  const output = usdToMicro(row.output_usd_per_mtok);
  // A zero price is refused with the malformed ones: it would make every
  // worst case zero, and a cap compared against zero cannot fire.
  if (!input || !output || !/^\d{4}-\d{2}-\d{2}$/.test(row.observed ?? "") || !String(row.source ?? "").startsWith("https://")) {
    throw new Error(
      `the price row for ${model} is malformed: it needs positive input_usd_per_mtok and output_usd_per_mtok ` +
        "as decimal text, the date it was observed, and an https source",
    );
  }
  return { model, ...row, input_micro_usd_per_mtok: input, output_micro_usd_per_mtok: output };
}

/** Tokens at a row's prices, in micro-USD, rounded up and never down. */
export function costMicroUsd(row, tokensIn, tokensOut) {
  for (const count of [tokensIn, tokensOut]) {
    if (!Number.isSafeInteger(count) || count < 0) {
      throw new Error(`a token count must be a non-negative whole number, not ${count}`);
    }
  }
  const numerator =
    BigInt(tokensIn) * BigInt(row.input_micro_usd_per_mtok) + BigInt(tokensOut) * BigInt(row.output_micro_usd_per_mtok);
  const micro = (numerator + 999_999n) / 1_000_000n;
  if (micro > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error("the cost is too large to be a real one; check the token counts");
  return Number(micro);
}

/**
 * One packet against the validator, the price table and the slot cap.
 *
 * `row` and `worstMicro` are present only when the packet is valid and priced.
 */
export function assess(packet, { prices = PRICES, slotCapMicro }) {
  const problems = validatePacket(packet);
  if (problems.length > 0) return { problems };
  const row = priceRow(prices, packet.model);
  if (!row) {
    return {
      problems: [
        `model '${packet.model}' has no row in the price table (scripts/fleet/prices.json), so the packet has no ` +
          `worst case and is not run on an estimate. Use a priced model (${Object.keys(prices.models).join(", ")}), ` +
          "or observe the price and commit a dated row",
      ],
    };
  }
  const worstMicro = costMicroUsd(row, packet.max_input_tokens, packet.max_output_tokens);
  if (worstMicro > slotCapMicro) {
    return {
      problems: [
        `the worst case is ${usd(worstMicro)} USD (${packet.max_input_tokens} tokens in and ` +
          `${packet.max_output_tokens} out at ${packet.model}'s prices), above the slot cap of ${usd(slotCapMicro)} USD. ` +
          "Lower max_input_tokens or max_output_tokens, or split the packet",
      ],
    };
  }
  return { problems: [], row, worstMicro };
}

/**
 * The ceiling rule: the day's ledger total plus a worst case may reach 80% of
 * the daily ceiling and may not pass it. `null` when it holds.
 *
 * Cross-multiplied, so "80%" is never a fraction somebody had to round.
 */
export function ceilingRefusal({ dayTotalMicro, worstMicro, ceilingMicro }) {
  if ((BigInt(dayTotalMicro) + BigInt(worstMicro)) * 100n <= BigInt(ceilingMicro) * 80n) return null;
  return (
    `the day's ledger total of ${usd(dayTotalMicro)} USD plus a worst case of ${usd(worstMicro)} USD would pass 80% ` +
    `of the ${usd(ceilingMicro)} USD daily ceiling. Nothing more runs today (UTC). Raising the ceiling is an ` +
    "amendment to ADR 0102, not an edit"
  );
}

/** A ledger object's cost in micro-USD, or `null` if it does not state one. */
export function ledgerCost(text) {
  try {
    const cost = JSON.parse(text).cost_micro_usd;
    return Number.isSafeInteger(cost) && cost >= 0 ? cost : null;
  } catch {
    return null;
  }
}

/**
 * How long to wait after failed attempt number `attempt` (1-based), in ms.
 *
 * A valid `Retry-After` (whole seconds) is honoured; anything else, an
 * HTTP-date included, falls back to full jitter over base * 2^(attempt-1),
 * `random` being injected so a test is deterministic. Never more than 60 s,
 * and never more than the task's `remainingMs`.
 * ponytail: HTTP-date Retry-After is not parsed; Vertex was not observed to send one.
 */
export function retryDelayMs({ attempt, retryAfter, random, remainingMs }) {
  const asked = typeof retryAfter === "string" && /^\d{1,6}$/.test(retryAfter.trim()) ? Number(retryAfter.trim()) * 1000 : null;
  const wait = asked ?? Math.floor(random() * BACKOFF_BASE_MS * 2 ** (attempt - 1));
  return Math.max(0, Math.min(wait, MAX_WAIT_MS, remainingMs));
}

/**
 * The one chat call, retried on 429 and 503. `send(timeoutMs)` returns a
 * response; `halted()` is looked up before every attempt after the first.
 *
 * Returns `{ outcome, attempts, statuses, waitedMs }`, or `{ halted: true, ... }`
 * when HALT appeared between attempts. A thrown error is never retried: bytes
 * may have been sent and the provider may have run the model.
 */
export async function callWithRetry({ send, halted, random, sleep, now, deadlineMs }) {
  const began = now();
  const statuses = [];
  let waitedMs = 0;
  for (let attempt = 1; ; attempt += 1) {
    const remainingMs = deadlineMs - (now() - began);
    const result = (extra) => ({ attempts: statuses.length, statuses, waitedMs, ...extra });
    if (attempt > 1 && (await halted())) return result({ halted: true });
    let response;
    try {
      response = await send(Math.max(1, Math.min(CHAT_TIMEOUT_MS, remainingMs)));
      if (response.ok) {
        const body = await response.json();
        statuses.push(response.status);
        return result({ outcome: { body } });
      }
    } catch (cause) {
      statuses.push(null);
      return result({ outcome: { error: String(cause?.message ?? cause) } });
    }
    statuses.push(response.status);
    const left = deadlineMs - (now() - began);
    if (!RETRY_STATUSES.includes(response.status) || attempt >= MAX_ATTEMPTS || left <= 0) {
      return result({ outcome: { httpStatus: response.status } });
    }
    const wait = retryDelayMs({ attempt, retryAfter: response.headers?.get?.("retry-after"), random, remainingMs: left });
    await sleep(wait);
    waitedMs += wait;
  }
}

/** What the ledger says before the call: the worst case, held. */
export const reservation = (worstMicro) => ({ status: "reserved", cost_micro_usd: worstMicro, cost_basis: "worst_case" });

const NO_ATTEMPTS = { attempts: 0, statuses: [], waitedMs: 0 };

/** What the ledger says when HALT stopped the call after the reservation. */
export const NO_CALL = { status: "halted", cost_micro_usd: 0, cost_basis: "no_call" };

const attemptsOf = (tries) => ({ attempts: tries.attempts, attempt_http_statuses: tries.statuses, waited_ms: tries.waitedMs });

/**
 * What one call cost, from what the API said about it, or from its silence.
 *
 * `outcome` is `{ body }` for a 2xx answer, `{ httpStatus }` for any other,
 * and `{ error }` when no answer arrived. **Anything the API did not put a
 * usage figure on is billed at the worst case and labelled as a bound**: a
 * timeout does not say whether the model ran, and a ledger that records zero
 * for "unknown" is the understatement that lets a day run past its ceiling.
 * The exception is a 429 or 503, where the provider said it processed nothing:
 * that is billed zero as `refused_unbilled`. `tries` is `callWithRetry`'s
 * attempt record and is carried into the ledger as it is.
 */
export function settle({ packet, row, worstMicro, outcome, tries = NO_ATTEMPTS }) {
  const body = outcome.body ?? {};
  const completion = completionText(body);
  const said = {
    model_reported: typeof body.model === "string" ? body.model : null,
    finish_reason: body.choices?.[0]?.finish_reason ?? null,
    text: completion.ok ? completion.text : "",
  };
  const bound = (status, http_status = null) => ({
    status,
    http_status,
    cost_micro_usd: worstMicro,
    cost_basis: "worst_case",
    ...attemptsOf(tries),
    prompt_tokens: null,
    completion_tokens: null,
    total_tokens: null,
    billed_output_tokens: null,
    ...said,
  });
  if (outcome.error) return { ...bound("no_response"), cost_basis: "worst_case_no_response" };
  if (outcome.httpStatus) {
    const refused = bound("provider_refused", outcome.httpStatus);
    return RETRY_STATUSES.includes(outcome.httpStatus) ? { ...refused, cost_micro_usd: 0, cost_basis: "refused_unbilled" } : refused;
  }

  const { prompt_tokens: prompt, completion_tokens: completed, total_tokens: total } = body.usage ?? {};
  if (![prompt, completed].every((count) => Number.isSafeInteger(count) && count >= 0)) return bound("usage_missing");

  // UNPROVEN, and resolved toward the larger bill: whether this endpoint
  // counts a reasoning model's hidden tokens in `completion_tokens` or only
  // in `total_tokens` has not been observed. Whatever the total holds beyond
  // the prompt was generated and is priced as output.
  const billedOutput = Number.isSafeInteger(total) && total - prompt > completed ? total - prompt : completed;

  let status = "ok";
  if (!completion.ok) status = "empty";
  if (said.model_reported !== packet.model) status = "model_mismatch";
  if (prompt > packet.max_input_tokens || billedOutput > packet.max_output_tokens) status = "over_bound";
  return {
    status,
    http_status: null,
    cost_micro_usd: costMicroUsd(row, prompt, billedOutput),
    cost_basis: "reported",
    ...attemptsOf(tries),
    prompt_tokens: prompt,
    completion_tokens: completed,
    total_tokens: Number.isSafeInteger(total) ? total : null,
    billed_output_tokens: billedOutput,
    ...said,
  };
}

const sha256 = (text) => createHash("sha256").update(text, "utf8").digest("hex");

/** The one ledger object a packet leaves. `settled` is a reservation, `NO_CALL` or `settle`'s result. */
export function ledgerEntry({ packet, run, index, day, row, worstMicro, inputSha256, at, ms, settled }) {
  return {
    at,
    day,
    run,
    index,
    packet_id: packet.packet_id,
    role: packet.role,
    why_this_tier: packet.why_this_tier,
    model: packet.model,
    model_reported: settled.model_reported ?? null,
    price: {
      input_usd_per_mtok: row.input_usd_per_mtok,
      output_usd_per_mtok: row.output_usd_per_mtok,
      observed: row.observed,
      source: row.source,
    },
    max_input_tokens: packet.max_input_tokens,
    max_output_tokens: packet.max_output_tokens,
    worst_case_micro_usd: worstMicro,
    prompt_tokens: settled.prompt_tokens ?? null,
    completion_tokens: settled.completion_tokens ?? null,
    total_tokens: settled.total_tokens ?? null,
    billed_output_tokens: settled.billed_output_tokens ?? null,
    cost_micro_usd: settled.cost_micro_usd,
    cost_basis: settled.cost_basis,
    status: settled.status,
    http_status: settled.http_status ?? null,
    attempts: settled.attempts ?? 0,
    attempt_http_statuses: settled.attempt_http_statuses ?? [],
    waited_ms: settled.waited_ms ?? 0,
    input_sha256: inputSha256,
    output_sha256: settled.text ? sha256(settled.text) : null,
    ms,
  };
}

// --- I/O -------------------------------------------------------------------

/**
 * The fleet bucket through the Cloud Storage JSON API.
 *
 * `token` is asked for on every request rather than once: the metadata
 * server caches, and a token taken before a five-minute model call may not
 * outlive it. Every answer that is neither success nor a plain 404 throws,
 * so "could not tell" never reads as "absent".
 */
export function bucketStore({ bucket, token, fetchImpl = fetch }) {
  const call = async (url, init = {}) =>
    fetchImpl(url, {
      ...init,
      headers: { ...init.headers, authorization: `Bearer ${await token()}` },
      signal: AbortSignal.timeout(30_000),
    });
  const object = (name) => `${STORAGE}/storage/v1/b/${bucket}/o/${encodeURIComponent(name)}`;
  const failed = (verb, name, response) =>
    new Error(`could not ${verb} gs://${bucket}/${name}: Cloud Storage answered ${response.status}`);
  return {
    async exists(name) {
      const response = await call(object(name));
      if (response.status === 404) return false;
      if (!response.ok) throw failed("look for", name, response);
      return true;
    },
    async read(name) {
      const response = await call(`${object(name)}?alt=media`);
      if (response.status === 404) return null;
      if (!response.ok) throw failed("read", name, response);
      return response.text();
    },
    async list(prefix) {
      const names = [];
      let pageToken;
      do {
        const query = new URLSearchParams({ prefix, fields: "items(name),nextPageToken" });
        if (pageToken) query.set("pageToken", pageToken);
        const response = await call(`${STORAGE}/storage/v1/b/${bucket}/o?${query}`);
        if (!response.ok) throw failed("list", prefix, response);
        const page = await response.json();
        for (const item of page.items ?? []) names.push(item.name);
        pageToken = page.nextPageToken;
      } while (pageToken);
      return names;
    },
    /** `false` only when `onlyIfAbsent` was asked for and the object exists. */
    async write(name, text, { onlyIfAbsent = false } = {}) {
      const query = new URLSearchParams({ uploadType: "media", name });
      if (onlyIfAbsent) query.set("ifGenerationMatch", "0");
      const response = await call(`${STORAGE}/upload/storage/v1/b/${bucket}/o?${query}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: text,
      });
      if (onlyIfAbsent && response.status === 412) return false;
      if (!response.ok) throw failed("write", name, response);
      return true;
    },
  };
}

/**
 * The sum of every ledger object for one UTC day, in micro-USD.
 *
 * An object that states no cost makes the total unknown, and an unknown
 * total is a refusal, not a zero.
 *
 * ponytail: one GET per ledger object, in sequence. Fine for the first rungs
 * (tens of packets a day); move the cost into the listing (object metadata)
 * or a daily roll-up when a day holds more than a few hundred.
 */
export async function dayTotalMicroUsd(store, day) {
  let total = 0;
  for (const name of await store.list(`ledger/${day}/`)) {
    const cost = ledgerCost((await store.read(name)) ?? "");
    if (cost === null) {
      throw new Error(
        `ledger object ${name} states no cost_micro_usd, so the day's total is unknown and nothing is run on a guess. ` +
          "Read the object, repair or remove it by hand, and dispatch again",
      );
    }
    total += cost;
  }
  return total;
}

/** The project this task runs in, from the same server that issues its token. */
async function metadataProject(fetchImpl) {
  const response = await fetchImpl(METADATA_PROJECT_URL, {
    method: "GET",
    headers: { "Metadata-Flavor": "Google" },
    signal: AbortSignal.timeout(5000),
  });
  if (!response.ok) throw new Error(`the metadata server refused the project id (${response.status})`);
  return (await response.text()).trim();
}

/** One task: one packet, at most one call. Returns the process exit code. */
export async function runTask({
  env = process.env,
  fetchImpl = fetch,
  now = () => new Date(),
  prices = PRICES,
  log = console.error,
  random = Math.random,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
} = {}) {
  const refuse = (code, message) => {
    log(`refused: ${message}`);
    return code;
  };
  const settings = readEnv(env);
  if (settings.problems.length > 0) return refuse(78, settings.problems.join("; "));
  const { bucket, run, index, ceilingMicro, slotCapMicro } = settings;
  const store = bucketStore({ bucket, token: () => metadataToken(fetchImpl), fetchImpl });
  const halted =
    `gs://${bucket}/HALT exists, so the fleet is halted and no call was made. ` +
    "Find out who halted it and why; deleting the object resumes dispatch";

  if (await store.exists("HALT")) return refuse(3, halted);

  const packetName = `packets/${run}/${index}.json`;
  const packetText = await store.read(packetName);
  if (packetText === null) {
    return refuse(2, `there is no packet at gs://${bucket}/${packetName}. Upload it, or execute with --tasks equal to the number of packets uploaded`);
  }
  let packet;
  try {
    packet = JSON.parse(packetText);
  } catch {
    return refuse(2, `gs://${bucket}/${packetName} is not JSON. Fix the packet and dispatch it under a new run`);
  }
  const { problems, row, worstMicro } = assess(packet, { prices, slotCapMicro });
  if (problems.length > 0) return refuse(2, `gs://${bucket}/${packetName}: ${problems.join("; ")}`);

  const started = now();
  const day = started.toISOString().slice(0, 10);
  const over = ceilingRefusal({ dayTotalMicro: await dayTotalMicroUsd(store, day), worstMicro, ceilingMicro });
  if (over) return refuse(7, over);

  // The task's own environment is passed through, so a key somebody set on
  // the Job is refused by the preset here rather than sitting unnoticed.
  const config = configure({
    ...env,
    ALGORIK_WORKER_PROVIDER: "vertex",
    ALGORIK_WORKER_VERTEX_PROJECT: await metadataProject(fetchImpl),
    ALGORIK_WORKER_MODEL: packet.model,
    ALGORIK_WORKER_MAX_CALLS: "1",
    ALGORIK_WORKER_MAX_TOKENS: String(packet.max_output_tokens),
  });
  if (config.problems.length > 0) return refuse(78, config.problems.join("; "));

  const payload = payloadOf(packet);
  const inputSha256 = sha256(payload);
  const ledgerName = `ledger/${day}/${run}-${index}.json`;
  const record = (settled) =>
    JSON.stringify(
      ledgerEntry({ packet, run, index, day, row, worstMicro, inputSha256, at: now().toISOString(), ms: now() - started, settled }),
      null,
      2,
    );

  if (!(await store.write(ledgerName, record(reservation(worstMicro)), { onlyIfAbsent: true }))) {
    return refuse(
      8,
      `gs://${bucket}/${ledgerName} already exists: this packet has run or is running, and a second call would bill ` +
        "it twice. If it must run again, dispatch it under a new FLEET_RUN",
    );
  }
  if (await store.exists("HALT")) {
    await store.write(ledgerName, record(NO_CALL));
    return refuse(3, halted);
  }

  const messages = [
    { role: "system", content: WORKER_SYSTEM_PROMPT },
    { role: "user", content: payload },
  ];
  const result = await callWithRetry({
    send: (timeoutMs) => chatCompletion(config, messages, { fetchImpl, timeoutMs }),
    halted: () => store.exists("HALT"),
    random,
    sleep,
    now: () => now().getTime(),
    deadlineMs: CHAT_TIMEOUT_MS,
  });
  if (result.halted) {
    // Every earlier attempt was a 429 or 503, which the provider did not process.
    await store.write(
      ledgerName,
      record({
        status: "halted",
        http_status: result.statuses.at(-1),
        cost_micro_usd: 0,
        cost_basis: "refused_unbilled",
        ...attemptsOf(result),
      }),
    );
    return refuse(3, halted);
  }
  const settled = settle({ packet, row, worstMicro, outcome: result.outcome, tries: result });

  // The output first: if the task dies between the two writes the ledger
  // still holds the reservation, which overstates and never understates.
  await store.write(
    `output/${run}/${index}.json`,
    JSON.stringify(
      {
        packet_id: packet.packet_id,
        run,
        index,
        role: packet.role,
        model: packet.model,
        status: settled.status,
        finish_reason: settled.finish_reason,
        output_sha256: settled.text ? sha256(settled.text) : null,
        text: settled.text,
      },
      null,
      2,
    ),
  );
  await store.write(ledgerName, record(settled));

  const summary = `${packet.packet_id} ended '${settled.status}', ${usd(settled.cost_micro_usd)} USD (${settled.cost_basis}), gs://${bucket}/${ledgerName}`;
  if (settled.status === "ok") {
    log(`done: ${summary}`);
    return 0;
  }
  return refuse(5, `${summary}. The output object says what came back; a retry is the dispatcher's decision and a new run`);
}

// Only the entry point runs a task; a test imports the functions above.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(
    await runTask().catch((cause) => {
      console.error(`failed: ${cause?.message ?? cause}`);
      return 1;
    }),
  );
}
