# 0037 — Hugging Face Inference Providers is the first hosted language-model provider, behind the numeric guard, reachable only from the deep brain

**Status:** accepted for the development lane and *proposed* for the platform
lane, by owner instruction of 2026-09-04 ("use huggingface for development"),
repeated after the cost of a credential in chat was stated. Nothing in the
platform lane is applied: no environment sets the variables below, and no
deployed process has an outbound path to the router.
**Relates to:** ADR 0002 and 0009 (no new crate — the adapter is `serde_json`
over `qip_transport`), ADR 0005 (confidence is arithmetic; a model never
supplies a number), ADR 0008 and 0032 (nothing on the fast path consults a
model, and the fast brain has no egress proxy by design), ADR 0024 (secrets as
files, the egress proxy as the only outbound path), ADR 0034 (the bar a new
upstream clears before it is written into a security control).
**Does not touch:** the paper-trading boundary's three layers; the REASON
stage's review bar, attenuation and concentration penalty; `NumericGuard`.

## Context

`qip_ai::language::LanguageModel` is the port through which the platform
turns evidence into readable reasoning — a causal chain, a falsification
statement, a summary — and `NumericGuard` refuses any number a model emits, so
expected returns, confidences and sizes come from deterministic code only
(ADR 0005). Two implementations exist: `DeterministicModel`, a template
stand-in every test and every deployment runs on, and `RemoteModel`, which
reports itself unavailable by construction because this build had no
credential path, no TLS client and no egress rule for any provider. Every
deployed reasoning run has therefore narrated through templates. That is
honest, and it is also the reason the operator-facing narrative reads the
same on every hypothesis.

Two lanes were asked for at once and are decided separately here, because
they cross different boundaries:

- **Development lane.** `scripts/model-gateway.mjs` dispatches bounded,
  machine-verifiable worker tasks to an external model under the
  orchestration policy (`docs/plan/algorik-orchestration-policy.md`). It
  sends repository source, which the owner has authorised, and it must never
  send a credential.
- **Platform lane.** A deployed binary calls a hosted model for narrative.
  It sends the REASON stage's evidence blocks — features, detector findings,
  analyst stances — for the instruments under review. It must never send a
  credential, never reach a venue, and never run on the fast path.

## Decision

1. **The provider is Hugging Face Inference Providers**, through its router
   at `router.huggingface.co`, which speaks the OpenAI chat shape at
   `/v1/chat/completions` and lists its catalogue anonymously at `/v1/models`.
   Chosen because the owner holds the account, the router fronts several
   inference vendors under one credential and one host (one allowlist entry,
   one Envoy cluster, one secret), and the catalogue publishes per-provider
   price and terms so gates 4 and 5 of the orchestration policy can be
   answered per model before a key is spent on it.

2. **Development lane, applied.** The gateway gains a `huggingface` preset
   that fixes the base URL, reads the token from `HF_TOKEN_FILE` (preferred)
   or `HF_TOKEN`, refuses both set at once, screens a Hugging Face token shape
   out of every payload, and offers `--probe`, which sends nothing and names
   the providers a model resolves to. `node --test
   scripts/model-gateway.test.mjs` holds each of those, mutation-verified.

3. **Platform lane, proposed and built dark.** A `HuggingFaceModel`
   implementing `LanguageModel` lives in the reasoning service, not in the
   `qip-ai` library — a library performs no I/O (`architecture/00-boundaries`)
   — over `qip_transport::HttpClient` with an explicit deadline on every call.
   It is pointed at a loopback Envoy listener, never at the vendor, exactly as
   the Frankfurter connector is; the listener's route admits `POST
   /v1/chat/completions` and nothing else, so the rest of the router's
   surface 404s at the boundary. The credential is read through
   `qip_core::secret` as `QIP_HF_TOKEN` with the `_FILE` indirection, never
   logged, held in a type that redacts in `Debug` and implements neither
   `Serialize` nor `Deserialize`. Every completion passes through
   `complete_structured`, so `NumericGuard` and the request's schema stand
   between the model and the record. `router.huggingface.co` is added to
   `egress_allowed_upstreams` and to the bootstrap in the same commit, and
   `egress.rs` holds the three names to one value.

4. **Only the deep brain may construct it.** `qip-deepbrain` reads
   `QIP_LANGUAGE_MODEL_PROVIDER=huggingface`, `QIP_LANGUAGE_MODEL`, and
   `QIP_LANGUAGE_MODEL_BASE_URL` (the loopback listener) and installs the
   adapter first in a `FallbackChain` ahead of the deterministic model, so a
   provider outage degrades to templates rather than stopping reasoning.
   `qip-fastbrain` reads none of them: ADR 0008 keeps every model off the fast
   path and ADR 0032 gives the fast brain no proxy, and `manifest_wiring.rs`
   refuses the variables on that binary. `qip-api` reads none of them either;
   it serves what the brains recorded.

5. **What leaves.** A `ModelRequest`'s system text, prompt and named context
   blocks, for instruments under review. Nothing from risk, execution,
   capital, compliance or the edge is ever placed in a context block, and the
   request builder in the reasoning service takes its inputs from the world
   model and the panel only. The response's text and structured fields are
   recorded on the hypothesis as narrative; no field of it is read as a
   number.

## Cost

- A third-party model now sees the evidence the platform reasons over, per
  instrument, per cycle, at the model's price. The router's `/v1/models`
  publishes the price; `ComputeLedger` bills what ran (principle 6), and the
  cost router's tier for the call is recorded with its rationale.
- The narrative is no longer reproducible from the event log alone: the same
  request to the same model can answer differently. The request, the model
  id, the provider the router chose and the response are all recorded, so the
  *decision* stays reproducible — the numbers never came from the model — and
  what is lost is only the ability to regenerate prose byte for byte. That is
  stated here because "every decision reproducible from the log alone" is a
  standing product decision and this is the first place it is qualified.
- A new host in the egress allowlist, and the first that is a model vendor
  rather than a cloud the platform runs on or a data vendor. The bar of ADR
  0034 applies: a shipped configuration names it, its terms are read before
  any environment sets the variables, and the acceptance suite refuses the
  bootstrap and the allowlist disagreeing.

## What would make this wrong

- Any number from a completion reaching a size, a confidence or a limit.
  `NumericGuard` exists so that this is a test failure rather than a
  discovery; if it is ever bypassed, this record is void and the adapter comes
  out.
- The fast brain acquiring a route to the listener. The port is not declared
  on its Cloud Run service and the acceptance suite refuses the variables on
  that binary; if either guard is weakened, ADR 0008 is what has been
  reopened, not this record.
- A provider on the router's list whose terms permit training on inputs. The
  owner's authorisation covers source and evidence; it does not choose a
  provider whose terms were not read. `--probe` names them so that reading is
  possible before configuring.

## Nothing is applied by this record

No `terraform.tfvars` sets a language-model variable, no RunService manifest
carries `QIP_HF_TOKEN_FILE`, and no Secret Manager secret exists for it. The
sequence to apply, each a person's act: read the terms of the providers the
chosen model resolves to; create the secret through Secret Manager and mount
it as a file on the deep brain (ADR 0024); set the three variables on the deep
brain's manifest from root variables; apply the egress change through
`infra.yml` with the plan read. Until then `HuggingFaceModel::is_available`
answers `false` and every reasoning run narrates through templates, which is
the state today.
