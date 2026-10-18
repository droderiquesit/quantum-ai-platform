# ADR 0138: The hosted language-model providers leave the reasoning engine for a slow-lane crate only the deep brain links

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It moves one module into a new
  in-tree crate; it admits no third-party crate and changes no provider,
  credential or route.
- **Date**: 2026-10-18
- **Extends**: ADR 0037 (Hugging Face is the first hosted provider), ADR 0116
  (lanes, and the node's closure checked by acceptance tests),
  `.claude/rules/architecture/00-boundaries.md` (an adapter that opens a
  socket belongs in a service).
- **Keeps**: ADR 0117 (the platform's one model route, pinned by
  `model_gateway_boundary.rs`), `qip_ai::language::LanguageModel` as the port
  and `DeterministicModel` as the stand-in, `Determinism::Required`.

## Context

R12-MODEL-4002 asks that a language model is "never consulted on the order
path — structurally". Today it holds by construction at the call site and
not by the dependency graph:

- The port is `qip_ai::language::LanguageModel`; `qip-kernel` takes an
  `Arc<dyn LanguageModel>` and imports `DeterministicModel`
  (`grep -n 'LanguageModel' backend/crates/runtime/qip-kernel/src/platform.rs`).
- The hosted adapter lives in `qip-reasoning-engine/src/providers/`
  (`huggingface.rs`), "in this crate rather than in `qip-ai` because a
  library performs no I/O". Only `qip-deepbrain` uses it
  (`qip-deepbrain/src/language.rs`, `config.rs`), and `egress.rs` in the
  acceptance suite names it.
- `qip-kernel` depends on `qip-reasoning-engine` for its reasoners (engine,
  evidence, hypothesis, rules, temporal and others), and `qip-fastbrain`
  depends on `qip-kernel`. So the fast brain's closure contains the hosted
  provider's code even though nothing in it constructs one.

A closure that contains the adapter means "the fast brain cannot consult a
hosted model" is a fact about today's `main.rs`, not about what the binary
can link.

Locate the module with `ls backend/crates/services/qip-reasoning-engine/src/providers`
and its users with
`grep -rln 'qip_reasoning_engine::providers' backend/crates --include=*.rs`.

## Decision

1. **The providers module becomes `services/qip-language-providers`.** It
   holds the hosted adapters and their credential types
   (`HF_TOKEN_VARIABLE`, `HuggingFaceToken`) and depends on `qip-ai` for the
   port and on `qip-transport` for the client. `qip-reasoning-engine` keeps
   every reasoner and loses the module; its own closure no longer contains a
   socket-owning adapter.
2. **Only `qip-deepbrain` depends on it.** The deep brain constructs the
   provider and hands the kernel an `Arc<dyn LanguageModel>` as it does
   today. `qip-kernel`, `qip-api` and `qip-fastbrain` do not depend on it,
   and the kernel keeps taking the trait object, so nothing in the cycle
   changes.
3. **`lane_placement.rs` pins the property by closure.** A test asserts that
   `qip-language-providers` is absent from the closures of `qip-fastbrain`,
   `qip-api`, `qip-kernel` and `qip-edge-node`, asserts its premise (the
   closure of each was read and contains `qip-contracts`), and is
   mutation-verified by adding the dependency to `qip-fastbrain`.
4. **The model gateway boundary does not move.** `model_gateway_boundary.rs`
   and `egress.rs` follow the module to its new path in the same commit;
   neither is weakened.

## Consequences

- "Never consulted on the order path" becomes a dependency-graph fact for
  every binary that is not the deep brain.
- The workspace gains one crate.
- Code slices left: the move, the dependency edges, the `lane_placement.rs`
  test with its mutation, and the two acceptance suites' paths (MODEL-4002).

## What it costs

- One more crate to version and build, and an import path change in the deep
  brain and two acceptance suites.
- A second hosted provider must land in the new crate, not beside the
  reasoners; a contributor who reaches for the old location finds nothing.

## What would make this wrong

- If the fast brain ever had a legitimate need for a hosted model (it does
  not today, and the cost router's `Determinism::Required` arm refuses one
  for pre-trade checks), decision 2 would have to be reopened by a record
  that says why the fast lane may wait on a network call.
- If the reasoners themselves came to call a provider directly rather than
  through the port, the split would push that call into the kernel's closure
  again; the reasoners must keep taking `LanguageModel` as a parameter.

## Rows

R12-MODEL-4002. It stays PARTIAL until the move lands with its mutated test.
