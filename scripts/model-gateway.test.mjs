/**
 * The gateway's own tests: `node --test scripts/model-gateway.test.mjs`.
 *
 * Each test names the failure it prevents. The gateway is the one program
 * here that sends repository source to a third party, so what it refuses
 * matters more than what it does.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  METADATA_TOKEN_URL,
  PROVIDERS,
  chatCompletion,
  completionText,
  configure,
  inapplicable,
  metadataToken,
  probe,
  screenPayload,
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
