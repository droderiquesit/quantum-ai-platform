/**
 * The Algorik worker model gateway.
 *
 * Routes bounded, low-risk worker tasks to a configured external model so the
 * orchestrator's own budget is spent on the work only it can do. Zero
 * dependencies: Node's built-in `fetch`, because a gateway that needed an SDK
 * would put a supply chain in front of the thing that reads this repository's
 * source.
 *
 * It is **dev tooling**. Nothing in `backend/crates/` or `frontend/packages/` may import it,
 * and the platform does not depend on it existing.
 *
 * ## What it refuses, and why
 *
 * The account holder has authorized sharing this repository's source with an
 * external provider. That decision is theirs and this gateway honours it. It
 * does not extend to credentials: a key that happens to sit in a file is not
 * source anybody meant to share, and once posted to a third party it is
 * burned. So every payload is screened for credential-shaped content and the
 * call is refused — not scrubbed — if any is found. Scrubbing invites the
 * habit of sending files that need scrubbing.
 *
 * It also fails closed on configuration. No key, no base URL, no model, no
 * budget: no call. A gateway that silently fell back to "send it anyway" or
 * "pretend it worked" would produce exactly the unverifiable output the
 * orchestration policy exists to prevent.
 *
 * ## Usage
 *
 *   ALGORIK_WORKER_BASE_URL=https://api.deepseek.com \
 *   ALGORIK_WORKER_MODEL=deepseek-chat \
 *   ALGORIK_WORKER_API_KEY_FILE=/run/secrets/worker-key \
 *   node scripts/model-gateway.mjs --task task.json
 *
 *   ALGORIK_WORKER_PROVIDER=huggingface \
 *   ALGORIK_WORKER_MODEL=Qwen/Qwen2.5-Coder-32B-Instruct \
 *   HF_TOKEN_FILE=/run/secrets/hf-token \
 *   node scripts/model-gateway.mjs --task task.json
 *
 *   node scripts/model-gateway.mjs --check     # configuration and reachability
 *   node scripts/model-gateway.mjs --probe     # reachability only, no key, nothing sent
 *
 * The key is read from a *file* by default, never from an argument and never
 * from the environment where a crash dump would hold it. `_FILE` indirection
 * matches how the platform reads every other credential.
 *
 * ## Providers
 *
 * A provider preset fixes the base URL so a worker cannot be pointed at a
 * host nobody authorised by editing one variable. `huggingface` is the
 * Hugging Face Inference Providers router, which speaks the OpenAI chat
 * shape at `/v1/chat/completions` and lists models keylessly at
 * `/v1/models`; the account holder authorised it on 2026-09-04
 * (`docs/plan/algorik-orchestration-policy.md` §4). The router forwards to
 * a third-party inference provider chosen per model, so the privacy
 * position is that provider's, which is why `--probe` prints the providers
 * a model resolves to before any key is spent on it.
 */
import { readFileSync, existsSync, appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const LEDGER = process.env.ALGORIK_WORKER_LEDGER ?? ".worker-spend.jsonl";

/**
 * Providers whose base URL is fixed here rather than taken from the
 * environment. The credential variables are the vendor's own names, so a
 * token issued for one vendor is never read as another's.
 */
export const PROVIDERS = {
  huggingface: {
    baseUrl: "https://router.huggingface.co",
    keyFileVariable: "HF_TOKEN_FILE",
    keyVariable: "HF_TOKEN",
  },
};

/** Patterns that mean "this payload carries a credential". Refuse, never strip. */
const CREDENTIAL_SHAPES = [
  { name: "AWS access key id", re: /\bAKIA[0-9A-Z]{16}\b/ },
  { name: "Google API key", re: /\bAIza[0-9A-Za-z_-]{35}\b/ },
  { name: "GitHub token", re: /\bgh[pousr]_[0-9A-Za-z]{36,}\b/ },
  { name: "Slack token", re: /\bxox[abprs]-[0-9A-Za-z-]{10,}\b/ },
  { name: "Stripe secret key", re: /\bsk_(live|test)_[0-9A-Za-z]{16,}\b/ },
  { name: "OpenAI-style key", re: /\bsk-[A-Za-z0-9]{32,}\b/ },
  // A Hugging Face user or fine-grained token. Screened because the gateway
  // can now be configured with one, and the payload most likely to carry it
  // is a worker's own context describing how it was configured.
  { name: "Hugging Face token", re: /\bhf_[A-Za-z0-9]{30,}\b/ },
  { name: "private key block", re: /-----BEGIN [A-Z ]*PRIVATE KEY-----/ },
  { name: "JSON service-account key", re: /"type"\s*:\s*"service_account"/ },
  { name: "bearer token literal", re: /\b[Aa]uthorization\s*:\s*Bearer\s+[A-Za-z0-9._-]{20,}/ },
  { name: "assigned secret literal", re: /\b(password|passwd|secret|api[_-]?key|access[_-]?token)\s*[=:]\s*["'][^"'\s]{12,}["']/i },
];

/** Returns the names of every credential shape found. Empty means clean. */
export function screenPayload(text) {
  return CREDENTIAL_SHAPES.filter(({ re }) => re.test(text)).map(({ name }) => name);
}

/**
 * Resolve the configuration from an environment map.
 *
 * Takes the map rather than reading `process.env` so the rule is testable
 * without mutating the process environment, the same reason
 * `qip_core::secret::resolve` takes its two sources as arguments.
 *
 * `readFile` is injectable for the same reason; the default reads the disk.
 */
export function configure(env = process.env, readFile = (path) => readFileSync(path, "utf8")) {
  const problems = [];
  const providerName = env.ALGORIK_WORKER_PROVIDER?.trim();
  const preset = providerName ? PROVIDERS[providerName] : undefined;
  if (providerName && !preset) {
    problems.push(
      `ALGORIK_WORKER_PROVIDER is '${providerName}', which is not a known provider ` +
        `(known: ${Object.keys(PROVIDERS).join(", ")})`,
    );
  }

  const explicitBaseUrl = env.ALGORIK_WORKER_BASE_URL?.trim();
  if (preset && explicitBaseUrl && explicitBaseUrl !== preset.baseUrl) {
    // A preset and a different URL is two claims about where the source
    // goes. Refuse rather than pick, because whichever one loses is the one
    // somebody meant.
    problems.push(
      `ALGORIK_WORKER_PROVIDER=${providerName} fixes the base URL to ${preset.baseUrl}; ` +
        `ALGORIK_WORKER_BASE_URL=${explicitBaseUrl} disagrees. Unset one.`,
    );
  }
  const baseUrl = preset?.baseUrl ?? explicitBaseUrl;
  const model = env.ALGORIK_WORKER_MODEL?.trim();
  const keyFileVariable = preset?.keyFileVariable ?? "ALGORIK_WORKER_API_KEY_FILE";
  const keyVariable = preset?.keyVariable ?? "ALGORIK_WORKER_API_KEY";
  const keyFile = env[keyFileVariable]?.trim();
  const inlineKey = env[keyVariable]?.trim();

  if (!baseUrl) problems.push("ALGORIK_WORKER_BASE_URL is not set (or set ALGORIK_WORKER_PROVIDER)");
  if (!model) problems.push("ALGORIK_WORKER_MODEL is not set");

  let apiKey = null;
  if (keyFile && inlineKey) {
    // The platform's `_FILE` rule: both set is an ambiguity, not a choice.
    problems.push(`${keyFileVariable} and ${keyVariable} are both set; set exactly one`);
  } else if (keyFile) {
    try {
      apiKey = readFile(keyFile).trim();
    } catch {
      problems.push(`${keyFileVariable} points at a file that cannot be read: ${keyFile}`);
    }
    if (apiKey === "") problems.push(`${keyFileVariable} points at an empty file: ${keyFile}`);
  } else if (inlineKey) {
    // Permitted, but say why it is second best exactly once, here.
    console.error(
      `note: reading the key from ${keyVariable} in the environment. A file is safer — an ` +
        "environment variable is visible in /proc/<pid>/environ, in every " +
        "child process, and in every crash dump.",
    );
    apiKey = inlineKey;
  } else {
    problems.push(`no credential: set ${keyFileVariable} (preferred) or ${keyVariable}`);
  }

  const maxCalls = Number(env.ALGORIK_WORKER_MAX_CALLS ?? 0);
  if (!Number.isFinite(maxCalls) || maxCalls <= 0) {
    problems.push("ALGORIK_WORKER_MAX_CALLS must be a positive number — an unbounded budget is not a budget");
  }
  const maxTokens = Number(env.ALGORIK_WORKER_MAX_TOKENS ?? 4000);

  // Provider-specific request fields, merged into the body as given. The
  // case that needed it: a reasoning model spends its whole output budget on
  // a hidden `reasoning` field and returns an empty `content`, and the switch
  // that stops it (`chat_template_kwargs.enable_thinking=false`) is not in
  // the OpenAI shape. Only object-valued JSON is accepted; `model`,
  // `messages` and `max_tokens` cannot be overridden, so the ledger's record
  // of what ran stays true.
  let extraBody = {};
  const extraRaw = env.ALGORIK_WORKER_EXTRA_BODY?.trim();
  if (extraRaw) {
    try {
      const parsed = JSON.parse(extraRaw);
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        problems.push("ALGORIK_WORKER_EXTRA_BODY must be a JSON object");
      } else if (["model", "messages", "max_tokens"].some((key) => key in parsed)) {
        problems.push("ALGORIK_WORKER_EXTRA_BODY may not set model, messages or max_tokens");
      } else {
        extraBody = parsed;
      }
    } catch {
      problems.push("ALGORIK_WORKER_EXTRA_BODY is not valid JSON");
    }
  }

  return { provider: providerName ?? "custom", baseUrl, model, apiKey, maxCalls, maxTokens, extraBody, problems };
}

/**
 * The completion text, or the reason there is none.
 *
 * An empty `content` with `finish_reason: "length"` is a worker that spent
 * its budget and produced nothing — the first batch on a reasoning model did
 * exactly that, and the gateway reported exit 0 with an empty file, which a
 * caller read as a finished task. So an empty completion is a refusal here,
 * naming the finish reason and the tokens spent.
 */
export function completionText(body) {
  const choice = body.choices?.[0];
  const text = choice?.message?.content ?? "";
  if (text.trim() === "") {
    const reason = choice?.finish_reason ?? "unknown";
    const spentTokens = body.usage?.completion_tokens ?? "?";
    return {
      ok: false,
      reason: `the provider returned no content (finish_reason ${reason}, ${spentTokens} completion tokens spent, ${
        choice?.message?.reasoning ? "a reasoning field was present" : "no reasoning field"
      })`,
    };
  }
  return { ok: true, text };
}

/**
 * Reachability without a credential, and without sending anything.
 *
 * The Hugging Face router lists its catalogue anonymously, so this answers
 * the orchestration policy's gate 1 (availability) and, for the configured
 * model, names the providers it resolves to — gate 4 (privacy) is decided
 * per provider, and a model that routes to a provider nobody has read the
 * terms of is not yet a model this gateway should be handed a key for.
 */
export async function probe(config, fetchImpl = fetch) {
  if (!config.baseUrl) {
    console.error("nothing to probe: set ALGORIK_WORKER_PROVIDER or ALGORIK_WORKER_BASE_URL");
    return 1;
  }
  const response = await fetchImpl(`${config.baseUrl}/v1/models`, { method: "GET" }).catch((cause) => ({
    ok: false,
    status: 0,
    statusText: String(cause),
  }));
  console.log(`provider  ${config.provider} (${config.baseUrl})`);
  console.log(`reachable ${response.ok ? "yes" : `no (${response.status} ${response.statusText ?? ""})`}`);
  if (!response.ok) return 1;
  const body = await response.json().catch(() => ({}));
  const models = Array.isArray(body.data) ? body.data : [];
  console.log(`catalogue ${models.length} model(s) listed anonymously`);
  if (config.model) {
    const entry = models.find((m) => m.id === config.model);
    if (!entry) {
      console.log(`model     ${config.model} is NOT in the catalogue`);
      return 1;
    }
    const providers = (entry.providers ?? []).map(
      (p) => `${p.provider}${p.status && p.status !== "live" ? ` (${p.status})` : ""}${p.is_free ? " free" : ""}`,
    );
    console.log(`model     ${config.model} resolves to: ${providers.join(", ") || "(no provider listed)"}`);
  }
  return 0;
}

/** Calls spent so far, counted from the ledger rather than from memory. */
function spent() {
  if (!existsSync(LEDGER)) return 0;
  return readFileSync(LEDGER, "utf8").split("\n").filter((line) => line.trim()).length;
}

async function check(config) {
  if (config.problems.length > 0) {
    console.error("gateway not configured:");
    for (const problem of config.problems) console.error(`  - ${problem}`);
    return 1;
  }
  const response = await fetch(`${config.baseUrl}/v1/models`, {
    method: "GET",
    headers: { authorization: `Bearer ${config.apiKey}` },
  }).catch((cause) => ({ ok: false, status: 0, statusText: String(cause) }));
  console.log(`provider  ${config.provider} (${config.baseUrl})`);
  console.log(`model     ${config.model}`);
  console.log(`budget    ${spent()} of ${config.maxCalls} calls used`);
  console.log(`reachable ${response.ok ? "yes" : `no (${response.status} ${response.statusText ?? ""})`}`);
  return response.ok ? 0 : 1;
}

/**
 * One task, one call.
 *
 * The task file carries the whole worker contract — the same five fields the
 * orchestration policy requires of any worker, because a task without
 * acceptance criteria produces output nobody can judge.
 */
async function run(config, taskPath) {
  const task = JSON.parse(readFileSync(taskPath, "utf8"));
  for (const field of ["task", "context", "acceptance", "paths"]) {
    if (!task[field]) {
      console.error(`task file is missing '${field}'; the worker contract requires it`);
      return 2;
    }
  }

  const used = spent();
  if (used >= config.maxCalls) {
    console.error(`budget exhausted: ${used} of ${config.maxCalls} calls already spent`);
    return 3;
  }

  const payload = [
    task.task,
    "",
    "Context:",
    task.context,
    "",
    `Acceptance: ${task.acceptance}`,
    `Files you may change: ${task.paths.join(", ")}`,
  ].join("\n");

  const found = screenPayload(payload);
  if (found.length > 0) {
    console.error(`refused: the payload carries ${found.join(", ")}.`);
    console.error("Sharing this repository's source is authorized; sharing a credential is not.");
    console.error("Remove the credential from the context and try again.");
    return 4;
  }

  const started = Date.now();
  const response = await fetch(`${config.baseUrl}/v1/chat/completions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${config.apiKey}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      ...config.extraBody,
      model: config.model,
      max_tokens: config.maxTokens,
      messages: [
        {
          role: "system",
          content:
            "You are a bounded worker. Do exactly the task. Return only the " +
            "requested output. Do not invent files, do not widen scope, and " +
            "state plainly if the task cannot be completed as specified.",
        },
        { role: "user", content: payload },
      ],
    }),
  });

  if (!response.ok) {
    console.error(`provider refused: ${response.status} ${response.statusText}`);
    return 5;
  }
  const body = await response.json();
  const usage = body.usage ?? {};
  const completion = completionText(body);
  if (!completion.ok) {
    // Still billed: the tokens were spent whether or not anything came back.
    appendFileSync(
      LEDGER,
      `${JSON.stringify({
        at: new Date().toISOString(),
        task: task.task.slice(0, 120),
        model: config.model,
        prompt_tokens: usage.prompt_tokens ?? null,
        completion_tokens: usage.completion_tokens ?? null,
        ms: Date.now() - started,
        empty: true,
      })}\n`,
    );
    console.error(`refused: ${completion.reason}`);
    return 6;
  }
  const text = completion.text;

  // The ledger is the budget's source of truth: counting in memory loses the
  // count on every crash, and a budget that resets on failure is not a budget.
  appendFileSync(
    LEDGER,
    `${JSON.stringify({
      at: new Date().toISOString(),
      task: task.task.slice(0, 120),
      model: config.model,
      prompt_tokens: usage.prompt_tokens ?? null,
      completion_tokens: usage.completion_tokens ?? null,
      ms: Date.now() - started,
    })}\n`,
  );

  process.stdout.write(text);
  return 0;
}

// Only the entry point runs the command line; a test imports the functions
// above without a task being dispatched.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const config = configure();
  const args = process.argv.slice(2);
  if (args.includes("--probe")) {
    process.exit(await probe(config));
  }
  if (args.includes("--check")) {
    process.exit(await check(config));
  }
  const taskIndex = args.indexOf("--task");
  if (taskIndex === -1 || !args[taskIndex + 1]) {
    console.error("usage: node scripts/model-gateway.mjs --task <file.json> | --check | --probe");
    process.exit(64);
  }
  if (config.problems.length > 0) {
    console.error("gateway not configured:");
    for (const problem of config.problems) console.error(`  - ${problem}`);
    process.exit(78);
  }
  process.exit(await run(config, args[taskIndex + 1]));
}
