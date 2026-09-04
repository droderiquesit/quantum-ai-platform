/**
 * The gateway's own tests: `node --test scripts/model-gateway.test.mjs`.
 *
 * Each test names the failure it prevents. The gateway is the one program
 * here that sends repository source to a third party, so what it refuses
 * matters more than what it does.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { PROVIDERS, configure, probe, screenPayload } from "./model-gateway.mjs";

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
