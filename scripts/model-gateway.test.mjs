/**
 * The gateway's own tests: `node --test scripts/model-gateway.test.mjs`.
 *
 * Each test names the failure it prevents. The gateway is the one program
 * here that sends repository source to a third party, so what it refuses
 * matters more than what it does.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  METADATA_TOKEN_URL,
  PROVIDERS,
  chatCompletion,
  completionText,
  configure,
  inapplicable,
  metadataToken,
  probe,
  run,
  screenPayload,
  spent,
} from "./model-gateway.mjs";

test("an empty completion is a refusal naming the finish reason, not an empty success", () => {
  // Premise: a non-empty completion is accepted as-is.
  assert.deepEqual(completionText({ choices: [{ message: { content: "x" } }] }), { ok: true, text: "x" });
  const out = completionText({
    choices: [{ finish_reason: "length", message: { content: "", reasoning: "…" } }],
    usage: { completion_tokens: 6000 },
  });
  assert.equal(out.ok, false);
  assert.ok(out.reason.includes("length") && out.reason.includes("6000"), out.reason);
});

test("an extra body is merged but may not override the model, messages or budget", () => {
  const good = configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "m", HF_TOKEN: "hf_x", ...budget, ALGORIK_WORKER_EXTRA_BODY: '{"chat_template_kwargs":{"enable_thinking":false}}' },
    () => "",
  );
  assert.deepEqual(good.extraBody, { chat_template_kwargs: { enable_thinking: false } });
  assert.deepEqual(good.problems, []);
  const bad = configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "m", HF_TOKEN: "hf_x", ...budget, ALGORIK_WORKER_EXTRA_BODY: '{"model":"other"}' },
    () => "",
  );
  assert.ok(bad.problems.some((p) => p.includes("may not set model")), JSON.stringify(bad.problems));
});

const budget = { ALGORIK_WORKER_MAX_CALLS: "5" };

test("a hugging face token in a payload is refused by name, never sent", () => {
  // Premise: the shape is plausible for a real token and clean text passes.
  const token = `hf_${"A".repeat(34)}`;
  assert.deepEqual(screenPayload("fn main() {}"), []);
  const found = screenPayload(`configured with HF_TOKEN=${token}`);
  assert.ok(found.includes("Hugging Face token"), `found ${JSON.stringify(found)}`);
});

test("the huggingface preset fixes the base url and reads the vendor's own token variable from a file", () => {
  const config = configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", HF_TOKEN_FILE: "/run/secrets/hf", ...budget },
    (path) => {
      assert.equal(path, "/run/secrets/hf");
      return "hf_secret\n";
    },
  );
  assert.deepEqual(config.problems, []);
  assert.equal(config.baseUrl, PROVIDERS.huggingface.baseUrl);
  assert.equal(config.provider, "huggingface");
  assert.equal(config.apiKey, "hf_secret");
});

test("a preset and a disagreeing base url is refused rather than one silently winning", () => {
  const config = configure(
    {
      ALGORIK_WORKER_PROVIDER: "huggingface",
      ALGORIK_WORKER_BASE_URL: "https://example.invalid",
      ALGORIK_WORKER_MODEL: "org/model",
      HF_TOKEN: "hf_x",
      ...budget,
    },
    () => "",
  );
  assert.ok(config.problems.some((p) => p.includes("disagrees")), JSON.stringify(config.problems));
});

test("without any credential the gateway is not configured, for the preset as for a custom provider", () => {
  for (const env of [
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", ...budget },
    { ALGORIK_WORKER_BASE_URL: "https://api.example", ALGORIK_WORKER_MODEL: "m", ...budget },
  ]) {
    const config = configure(env, () => "");
    assert.equal(config.apiKey, null);
    assert.ok(config.problems.some((p) => p.startsWith("no credential")), JSON.stringify(config.problems));
  }
});

test("a token file and a token variable both set is an ambiguity the gateway refuses", () => {
  const config = configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", HF_TOKEN_FILE: "/f", HF_TOKEN: "hf_x", ...budget },
    () => "hf_from_file",
  );
  assert.equal(config.apiKey, null);
  assert.ok(config.problems.some((p) => p.includes("both set")), JSON.stringify(config.problems));
});

test("an unknown provider name is refused rather than treated as custom", () => {
  const config = configure({ ALGORIK_WORKER_PROVIDER: "someone-else", ALGORIK_WORKER_MODEL: "m", ...budget }, () => "");
  assert.ok(config.problems.some((p) => p.includes("not a known provider")), JSON.stringify(config.problems));
});

test("the probe sends no authorization header and names the providers a model resolves to", async () => {
  const requests = [];
  const fetchImpl = async (url, init) => {
    requests.push({ url, init });
    return {
      ok: true,
      status: 200,
      json: async () => ({
        data: [{ id: "org/model", providers: [{ provider: "acme", status: "live", is_free: false }] }],
      }),
    };
  };
  const lines = [];
  const original = console.log;
  console.log = (line) => lines.push(line);
  let code;
  try {
    code = await probe({ provider: "huggingface", baseUrl: "https://router.example", model: "org/model" }, fetchImpl);
  } finally {
    console.log = original;
  }
  assert.equal(code, 0);
  assert.equal(requests.length, 1, "exactly one request");
  assert.equal(requests[0].url, "https://router.example/v1/models");
  assert.equal(requests[0].init.headers, undefined, "no headers at all, so no credential can be sent");
  assert.ok(lines.some((l) => l.includes("resolves to: acme")), JSON.stringify(lines));
});

test("the probe reports a model absent from the catalogue as a failure", async () => {
  const fetchImpl = async () => ({ ok: true, status: 200, json: async () => ({ data: [{ id: "other" }] }) });
  const original = console.log;
  console.log = () => {};
  let code;
  try {
    code = await probe({ provider: "huggingface", baseUrl: "https://router.example", model: "org/model" }, fetchImpl);
  } finally {
    console.log = original;
  }
  assert.equal(code, 1);
});

// --- the vertex preset (ADR 0102, slice step 3) ----------------------------

const VERTEX_BASE =
  "https://aiplatform.googleapis.com/v1/projects/algorik-platform-dev/locations/global/endpoints/openapi";
const vertex = {
  ALGORIK_WORKER_PROVIDER: "vertex",
  ALGORIK_WORKER_VERTEX_PROJECT: "algorik-platform-dev",
  ALGORIK_WORKER_MODEL: "google/gemini-2.5-flash-lite",
  ...budget,
};

/** A fetch that answers from a list and remembers what it was asked. */
function scripted(...answers) {
  const requests = [];
  const fetchImpl = async (url, init) => {
    requests.push({ url, init });
    return answers[requests.length - 1];
  };
  return { requests, fetchImpl };
}

const tokenAnswer = { ok: true, status: 200, json: async () => ({ access_token: "tok", token_type: "Bearer" }) };

test("the vertex preset fixes the base url to the project's global openapi endpoint and holds no key", () => {
  const config = configure(vertex, () => "");
  assert.deepEqual(config.problems, []);
  // The whole URL, not a fragment of it: `locations/global` is the only
  // place the model was observed answering, and a host or a location that
  // drifted would send source somewhere nobody read the terms of.
  assert.equal(config.baseUrl, VERTEX_BASE);
  assert.equal(config.chatPath, "/chat/completions");
  assert.equal(config.credential, "metadata");
  assert.equal(config.apiKey, null);
});

test("the vertex preset refuses a missing or malformed project id rather than building a url from it", () => {
  // Premise: the same environment with a good project is accepted.
  assert.deepEqual(configure(vertex, () => "").problems, []);
  for (const project of [undefined, "", "Algorik-Platform", "short", "ends-with-hyphen-", "a/locations/us", "algorik-platform-dev/../x"]) {
    const config = configure({ ...vertex, ALGORIK_WORKER_VERTEX_PROJECT: project }, () => "");
    assert.ok(
      config.problems.some((p) => p.startsWith("ALGORIK_WORKER_VERTEX_PROJECT is")),
      `project ${JSON.stringify(project)} was accepted: ${JSON.stringify(config.problems)}`,
    );
    assert.equal(config.baseUrl, undefined, `a url was built from ${JSON.stringify(project)}`);
  }
});

test("the vertex preset refuses every variable a key could arrive in, because its only credential is the metadata token", () => {
  // One variable at a time: a loop that set all four would pass with three
  // of them forgotten. A key file is in the list on purpose; that is what a
  // downloaded service-account key looks like from here.
  for (const name of ["ALGORIK_WORKER_API_KEY", "ALGORIK_WORKER_API_KEY_FILE", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS"]) {
    const config = configure({ ...vertex, [name]: "x" }, () => "x");
    assert.ok(
      config.problems.some((p) => p.startsWith(`${name} is set`) && p.includes("takes no key")),
      `${name} was accepted: ${JSON.stringify(config.problems)}`,
    );
    assert.equal(config.apiKey, null);
  }
});

test("the vertex preset refuses a differing base url, and a refused project never lets that url through", () => {
  const disagreeing = configure({ ...vertex, ALGORIK_WORKER_BASE_URL: "https://example.invalid" }, () => "");
  assert.ok(disagreeing.problems.some((p) => p.includes("disagrees")), JSON.stringify(disagreeing.problems));
  assert.equal(disagreeing.baseUrl, VERTEX_BASE);
  // The hole a `preset ?? explicit` fallback opens: no project, so no preset
  // URL, so the environment's URL becomes the destination.
  const noProject = configure(
    { ...vertex, ALGORIK_WORKER_VERTEX_PROJECT: "", ALGORIK_WORKER_BASE_URL: "https://example.invalid" },
    () => "",
  );
  assert.equal(noProject.baseUrl, undefined);
  assert.ok(noProject.problems.some((p) => p.includes("disagrees")), JSON.stringify(noProject.problems));
});

test("the metadata token is asked for with the Metadata-Flavor header and a deadline", async () => {
  const { requests, fetchImpl } = scripted(tokenAnswer);
  assert.equal(await metadataToken(fetchImpl), "tok");
  assert.equal(requests.length, 1);
  assert.equal(
    requests[0].url,
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token",
  );
  assert.equal(requests[0].url, METADATA_TOKEN_URL);
  // Without the header the server answers 403; without the signal a machine
  // that is not Google's hangs here instead of refusing.
  assert.deepEqual(requests[0].init.headers, { "Metadata-Flavor": "Google" });
  assert.ok(requests[0].init.signal instanceof AbortSignal, "no deadline on the metadata request");
});

test("a metadata server that refuses or answers without a token is an error, never a blank bearer", async () => {
  // Premise: a good answer yields its token.
  assert.equal(await metadataToken(scripted(tokenAnswer).fetchImpl), "tok");
  await assert.rejects(metadataToken(scripted({ ok: false, status: 403 }).fetchImpl), /refused a token \(403\)/);
  for (const body of [{}, { access_token: "" }, { access_token: 7 }]) {
    await assert.rejects(
      metadataToken(scripted({ ok: true, status: 200, json: async () => body }).fetchImpl),
      /without an access_token/,
      `accepted ${JSON.stringify(body)}`,
    );
  }
});

test("a vertex chat call carries the metadata token to the preset's chat path with the configured model and max_tokens", async () => {
  const config = configure({ ...vertex, ALGORIK_WORKER_MAX_TOKENS: "321" }, () => "");
  assert.deepEqual(config.problems, []);
  const { requests, fetchImpl } = scripted(tokenAnswer, { ok: true, status: 200 });
  await chatCompletion(config, [{ role: "user", content: "hi" }], { fetchImpl, timeoutMs: 1000 });
  assert.deepEqual(
    requests.map((r) => r.url),
    [METADATA_TOKEN_URL, `${VERTEX_BASE}/chat/completions`],
  );
  assert.equal(requests[1].init.headers.authorization, "Bearer tok");
  assert.ok(requests[1].init.signal instanceof AbortSignal, "the caller's deadline did not reach the request");
  assert.deepEqual(JSON.parse(requests[1].init.body), {
    model: "google/gemini-2.5-flash-lite",
    max_tokens: 321,
    messages: [{ role: "user", content: "hi" }],
  });
});

test("the vertex preset refuses --probe and --check, which would ask a catalogue it lacks with a key it does not hold", () => {
  const keyless = configure(vertex, () => "");
  for (const flag of ["--probe", "--check"]) {
    assert.ok(inapplicable(keyless, [flag])?.startsWith("--probe and --check do not apply to vertex"), flag);
  }
  // Only those two, and only that preset: a task still runs, and the
  // keyless probe still works for the provider it was written for.
  assert.equal(inapplicable(keyless, ["--task", "task.json"]), null);
  const keyed = configure({ ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "m", HF_TOKEN_FILE: "/f", ...budget }, () => "t");
  assert.equal(inapplicable(keyed, ["--probe"]), null);
  assert.equal(inapplicable(keyed, ["--check"]), null);
});

test("a keyed provider's chat call is unchanged: its own key, the v1 path, and no metadata request", async () => {
  const config = configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", HF_TOKEN_FILE: "/f", ...budget },
    () => "from-file\n",
  );
  assert.deepEqual(config.problems, []);
  const { requests, fetchImpl } = scripted({ ok: true, status: 200 });
  await chatCompletion(config, [{ role: "user", content: "hi" }], { fetchImpl });
  assert.equal(requests.length, 1, "a keyed provider asked the metadata server for something");
  assert.equal(requests[0].url, "https://router.huggingface.co/v1/chat/completions");
  assert.equal(requests[0].init.headers.authorization, "Bearer from-file");
  // No deadline was given, so none is set: the command line never had one.
  assert.equal(requests[0].init.signal, undefined);
  assert.equal(JSON.parse(requests[0].init.body).max_tokens, 4000);
});

// --- one request, one audit line (CICD-035), and the call ceiling (CICD-030) ---

/** A scratch directory: `task(body)` writes a task file, `lines()` reads the ledger back. */
function desk() {
  const dir = mkdtempSync(join(tmpdir(), "gateway-"));
  const ledger = join(dir, "ledger.jsonl");
  let n = 0;
  return {
    ledger,
    task(body) {
      const path = join(dir, `task-${n++}.json`);
      writeFileSync(path, JSON.stringify(body));
      return path;
    },
    lines: () =>
      readFileSync(ledger, "utf8")
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line)),
    remove: () => rmSync(dir, { recursive: true, force: true }),
  };
}

const keyed = (maxCalls) =>
  configure(
    { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", HF_TOKEN_FILE: "/f", ALGORIK_WORKER_MAX_CALLS: String(maxCalls) },
    () => "k",
  );
const goodTask = { task: "summarise the module", context: "fn main() {}", acceptance: "one paragraph", paths: ["notes.md"] };
const answer = (content) => ({
  ok: true,
  status: 200,
  json: async () => ({ choices: [{ message: { content } }], usage: { prompt_tokens: 3, completion_tokens: 2 } }),
});

test("every request leaves exactly one audit line naming the agent, the model and the decision, whether it was allowed or refused", async () => {
  // The failure: the ledger was written only when the provider answered, so
  // the gateway's refusals left no trace and a count of ledger lines was not
  // a count of requests.
  const d = desk();
  const token = `hf_${"A".repeat(34)}`;
  const answers = [answer("done"), { ok: false, status: 503, statusText: "busy" }, answer(""), "throw"];
  const requests = [];
  const fetchImpl = async (url, init) => {
    requests.push({ url, init });
    const next = answers[requests.length - 1];
    if (next === "throw") throw new Error("socket hang up");
    return next;
  };
  const as = (config, task, agent = "test-engineer") => run(config, d.task(task), { agent, fetchImpl, ledger: d.ledger });
  const config = keyed(3);
  // Premise: the configuration is one the gateway accepts, so every refusal
  // below is the one its request was built to draw.
  assert.deepEqual(config.problems, []);

  const codes = [
    await as(config, goodTask),
    await as(config, { ...goodTask, acceptance: "" }),
    await as(config, { ...goodTask, context: `configured with ${token}` }),
    await as(config, goodTask),
    await as(config, goodTask),
    await as(config, goodTask),
    await as(config, goodTask),
    await as(configure({ ALGORIK_WORKER_PROVIDER: "huggingface", HF_TOKEN_FILE: "/f", ...budget }, () => "k"), goodTask),
    // No agent named at all. `null`, because `undefined` would take the default.
    await as(config, goodTask, null),
  ];
  assert.deepEqual(codes, [0, 2, 4, 5, 6, 5, 3, 78, 78]);

  const lines = d.lines();
  assert.equal(lines.length, codes.length, "the number of audit lines is not the number of requests");
  assert.deepEqual(
    lines.map((line) => line.decision),
    [
      "allowed",
      "refused:task-contract",
      "refused:credential",
      "refused:provider",
      "refused:empty-completion",
      "failed:transport",
      "refused:budget",
      "refused:unconfigured",
      "refused:unconfigured",
    ],
  );
  assert.deepEqual(
    lines.map((line) => line.agent),
    [...Array(8).fill("test-engineer"), null],
    "a line does not name the agent that asked, or names one for the request that gave none",
  );
  assert.deepEqual(
    lines.map((line) => line.model),
    ["org/model", "org/model", "org/model", "org/model", "org/model", "org/model", "org/model", null, "org/model"],
  );
  // Only what reached the provider was sent, and the refused credential is
  // in neither the requests nor the ledger.
  assert.equal(requests.length, 4);
  assert.ok(!readFileSync(d.ledger, "utf8").includes(token), "the ledger holds the credential it refused to send");
  assert.deepEqual(lines[2].shapes, ["Hugging Face token"]);
  d.remove();
});

test("a spent call ceiling refuses the next task before anything is sent, and a refusal spends none of it", async () => {
  // The failure: a ceiling nothing ever exercised. Every other test hands the
  // gateway a budget as a fixture and none reaches it.
  const d = desk();
  const { requests, fetchImpl } = scripted(answer("one"), answer("two"));
  const as = (task) => run(keyed(1), d.task(task), { agent: "test-engineer", fetchImpl, ledger: d.ledger });

  // A refusal first: it is audited and it is not spend.
  assert.equal(await as({ ...goodTask, paths: "" }), 2);
  assert.equal(spent(d.ledger), 0, "a refused request was counted against the call ceiling");

  // Premise: with the ceiling unspent the call goes out.
  assert.equal(await as(goodTask), 0);
  assert.equal(requests.length, 1);
  assert.equal(spent(d.ledger), 1);

  assert.equal(await as(goodTask), 3, "a second call was made past a ceiling of one");
  assert.equal(requests.length, 1, "the refused call was sent anyway");
  assert.deepEqual(d.lines().at(-1).decision, "refused:budget");
  d.remove();
});

test("without a positive call ceiling the gateway is not configured, and a task run against it sends nothing", async () => {
  const base = { ALGORIK_WORKER_PROVIDER: "huggingface", ALGORIK_WORKER_MODEL: "org/model", HF_TOKEN_FILE: "/f" };
  const complaint = (config) => config.problems.filter((p) => p.startsWith("ALGORIK_WORKER_MAX_CALLS must be a positive number"));
  // Premise: a positive ceiling draws no complaint, so the complaint below is
  // about the ceiling and not about the rest of the fixture.
  assert.deepEqual(configure({ ...base, ALGORIK_WORKER_MAX_CALLS: "5" }, () => "k").problems, []);
  for (const value of [undefined, "0", "-1", "many"]) {
    const config = configure({ ...base, ...(value === undefined ? {} : { ALGORIK_WORKER_MAX_CALLS: value }) }, () => "k");
    assert.equal(complaint(config).length, 1, `a ceiling of ${value} was accepted`);

    const d = desk();
    const { requests, fetchImpl } = scripted(answer("sent"));
    assert.equal(await run(config, d.task(goodTask), { agent: "test-engineer", fetchImpl, ledger: d.ledger }), 78);
    assert.equal(requests.length, 0, `a task was sent under a ceiling of ${value}`);
    d.remove();
  }
});

test("a ledger line the budget cannot read, or one written before refusals were recorded, counts as spent", () => {
  const d = desk();
  writeFileSync(
    d.ledger,
    [
      JSON.stringify({ at: "2026-09-01T00:00:00Z", task: "old", model: "m", ms: 1 }),
      "not json at all",
      JSON.stringify({ decision: "refused:budget", billed: false }),
      JSON.stringify({ decision: "allowed", billed: true }),
      "",
    ].join("\n"),
  );
  // Three: the legacy line, the unreadable one and the allowed one. Not four,
  // and not one.
  assert.equal(spent(d.ledger), 3);
  d.remove();
});
