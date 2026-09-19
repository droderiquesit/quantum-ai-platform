# ADR 0083: A trained model is served in-process by an in-tree provider for the forms the platform trains, and no inference crate is taken

- **Status**: Accepted, under the authority the owner delegated to the
  policy lane on 2026-09-19 over legal, business and policy decisions. The
  interface below is a specification for an implementing lane; nothing is
  built by this record.
- **Date**: 2026-09-19
- **Supersedes**: nothing. Retires the "BLOCKED-by-policy" reading of §21.2
  in the register.
- **Related**: ADR 0002 and ADR 0009 (two dependencies; the in-tree
  numerics they authorise), ADR 0005 (a language model never supplies a
  number), ADR 0006 (a classical baseline every time — applied here to a
  learned model against its linear baseline), ADR 0008 and ADR 0037
  (nothing on the fast path consults a model; the hosted model is narrative
  only), ADR 0012 (where a library earns its place), ADR 0043 (asymmetric
  signing is a gap no in-tree code closes — the "signed artifact" step lands
  on it), ADR 0074 (an estimator written in-tree on ADR 0002's test)

## Context

Blueprint §21.2 names a five-stage pipeline — features, train, evaluate,
promote, deploy — with runtimes attached: `polars`/`arrow`, `burn` and
`linfa`, ONNX emitted as protobuf via `prost`, signed with `ring`, the node
swapping the model set by atomic pointer. §21.3 says "training and inference
share the `tract` crate and the same feature computation code", so that
training-serving skew "is removed structurally rather than by discipline".
§39.1's row for cost, dispersion and regime estimation says "ONNX models,
shipped, in-process — advisory input to deterministic filters". §41.5 slot 1
ships "ten ONNX artifacts, signed". Rule 22's neighbours in §56 do not name
a runtime.

Every one of those crates is forbidden by the two-dependency rule, and the
register scored the back half of §21.2 as "BLOCKED-by-policy rather than
merely undone". The brief asked whether that is the right reading. It is
not, and the reason is what the platform actually trains.

### What `qip-training` produces, read from the declarations

`grep -n 'pub enum ModelFamily\|pub enum TeacherForm\|pub enum StudentForm' backend/crates/services/qip-training/src/local.rs backend/crates/services/qip-training/src/distill.rs`:

- **Teachers**: `ModelFamily::{Linear { ridge }, BoostedStumps { rounds, learning_rate, min_samples_leaf, candidate_splits }}`, fitted by
  `LocalTrainer::fit` into `TeacherForm::{Linear { intercept, coefficients }, BoostedStumps { base, learning_rate, stumps }}`,
  with a `Calibration { scale, offset }` and `FitDiagnostics`. `TeacherForm::predict(&[f64]) -> f64` exists.
- **Students**: `StudentForm::{Linear { ridge }, Tree { max_depth, min_samples_leaf, candidate_splits }}`,
  distilled by `distil` into `qip_strategy::model::DistilledModel`, whose
  `ModelForm` is `Linear { intercept, coefficients }` or
  `Tree { arity, nodes: Vec<TreeNode> }`.

That is the whole population: two teacher forms, two student forms, all of
them kilobytes of `f64`. §21.3 says so itself — "every model is a small
tabular or short-sequence model measured in kilobytes to low megabytes. None
is a large transformer." Nothing in the workspace trains a neural network,
a sequence model or an embedding.

### What already serves in-process, and where

The hot path already has an in-process model server, and it was built to be
the only one. `qip-strategy/src/model.rs` opens: "A distilled model is a
small fixed-size function that carries its own coefficients. It loads
nothing, calls nothing, and its worst-case cost is a property of the value
itself … it is the only form in which a learned function reaches the
execution path at all. What is deliberately absent: any way to fetch weights,
consult a service, or evaluate a model whose size is not known at compile
time." `DistilledModel` has `evaluate`, `arity`, `cost` and `digest`
(`grep -n 'pub fn' backend/crates/edge/qip-strategy/src/model.rs`); the
strategy IR carries it inline as `Expr::Model`; the compiler lowers it to
`Op::Model` after checking arity and input types
(`grep -n 'Expr::Model' backend/crates/edge/qip-strategy/src/compile.rs`).
Training-serving skew is removed the way §21.3 wants — by one code path —
and more structurally than sharing `tract` would, because there is no second
representation of the function at all.

What does **not** exist is a caller for a trained model off the hot path.
`TeacherForm::predict` is called only inside `qip-training` itself — by its
own diagnostics in `local.rs` and by its own `tests/training.rs`
(`grep -rn '\.predict(' backend/crates --include=*.rs` lands in those two
files and in no other crate). The deep brain fits,
distils, registers a card and promotes it (`grep -n 'LocalTrainer::new().fit\|distil(\|registry_mut().promote' backend/crates/apps/qip-deepbrain/src/learning.rs`),
and then nothing scores anything with the result. §39.1's advisory row —
cost, dispersion, regime estimation feeding deterministic filters — has no
consumer, and the promote and deploy stages of §21.2 have nothing to carry.

The wire already anticipates the shape: `qip_contracts::policy::ModelManifest`
is "a model named by digest. Weights never travel this fabric — a payload is
policy, and ten ONNX artifacts are an artifact store's business", and
`Slot<ModelManifest>` is the policy's `trained_models` slot
(`grep -n 'ModelManifest' backend/crates/libs/qip-contracts/src/policy.rs`).
The ONNX mention there is the only one in the workspace outside tests, and
it is prose.

## Decision

Option (b) of the brief: **keep the rule; serve in-tree.** The directive's
own preferred shape — Application, Interface, Provider — is taken, and the
provider needs no dependency because the forms it must serve are four small
structs the platform already owns.

### 1. The hot-path form is unchanged and stays the only one

`qip_strategy::model::DistilledModel` remains the only learned function the
execution path evaluates. No trait object, no provider and no loader
appears in `qip-strategy`; the module's "deliberately absent" list is the
guarantee and the acceptance suite may assert that `qip-strategy` gains no
dependency on the interface crate below. A model reaches a cell inline in a
compiled plan, as `Expr::Model`, and in no other way.

### 2. The interface lives in `qip-ai`, the provider in `qip-training`

`qip-ai` is the library that already holds the platform's model ports —
`ModelCard`, `ModelRegistry`, `LanguageModel` — and it depends on no service.
The interface goes beside them so that an application can name a served
model without naming who serves it:

```rust
// backend/crates/libs/qip-ai/src/serving.rs

/// The forms a model artifact may take on this platform. Closed on purpose:
/// an artifact naming a format not listed here is refused at the wire by
/// serde, before any provider is asked, and adding a variant is an ADR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    TeacherLinear,
    TeacherBoostedStumps,
    DistilledLinear,
    DistilledTree,
}

/// A model as it is stored, published and shipped: the card it belongs to,
/// its format, its serialised form, and the digest of that form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelArtifact {
    /// `ModelCard::reference()` — name and version.
    pub reference: String,
    pub format: ModelFormat,
    /// The provider's own serialisation of the form (serde_json). The
    /// interface does not know the form's type; the provider does.
    pub payload: serde_json::Value,
    /// SHA-256 of the canonical payload, in-tree hashing (ADR 0002).
    pub digest: String,
}

/// A model a provider has loaded and will score. Off the hot path only.
pub trait ServedModel: std::fmt::Debug {
    fn reference(&self) -> &str;
    fn format(&self) -> ModelFormat;
    /// Inputs the model reads. `score` refuses any other count.
    fn arity(&self) -> usize;
    /// Worst-case evaluation steps, the unit `DistilledModel::cost` uses.
    fn cost(&self) -> usize;
    /// One score. Refuses — never clamps — an input count other than
    /// `arity()` and any non-finite input; a finite model over finite inputs
    /// returns a finite `f64`, and a provider that cannot promise that is
    /// not conforming. Statistics may be `f64` (core-rust rules); the
    /// crossing into `Decimal` happens in the caller, where money is, and is
    /// commented there.
    fn score(&self, inputs: &[f64]) -> Result<f64>;
}

/// Where a served model comes from.
pub trait ModelProvider: std::fmt::Debug {
    fn name(&self) -> &str;
    /// The formats this provider serves. A caller may check before asking.
    fn serves(&self) -> &[ModelFormat];
    /// Load an artifact, or refuse naming the format and what to do instead.
    /// Refuses: a format outside `serves()`; a digest that does not match
    /// the payload; a payload the form's own constructor refuses (non-finite
    /// weight, empty coefficients, a tree that descends backwards).
    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>>;
}
```

The provider goes in `qip-training`, which already depends on
`qip-strategy` for `DistilledModel` and owns `TeacherForm`:

```rust
// backend/crates/services/qip-training/src/serve.rs
#[derive(Debug, Default)]
pub struct InTreeProvider;
impl ModelProvider for InTreeProvider { /* all four formats */ }
// impl ServedModel for a private wrapper over (TeacherForm, Calibration)
// and for a private wrapper over DistilledModel, each delegating to the
// `predict`/`evaluate` the form already has — no second arithmetic.
```

`ModelArtifact` construction is the provider's too: `InTreeProvider::pack(&TrainedTeacher) -> ModelArtifact`
and `pack_distilled(&DistilledModel) -> ModelArtifact`, so that the digest is
computed once, by the code that serialised the payload, and `serve` checks it
against the same canonical form.

### 3. The refusal, verbatim in shape

A model the provider cannot serve refuses with `Error::invalid`, and the
message names the three things a reader needs:

> model artifact `{reference}` is `{format}`, which `{provider}` does not
> serve in-process; it serves `teacher_linear`, `teacher_boosted_stumps`,
> `distilled_linear` and `distilled_tree`. Distil the model with
> `qip_training::distill` into one of those, or mark its card research-only
> — a model this platform cannot evaluate is not a model it may act on.

A format outside `ModelFormat` never reaches that message: serde refuses the
unknown variant on deserialisation, and the loader wraps that refusal with
the same text so an operator reading a log sees one sentence rather than
two. A digest mismatch refuses naming both digests. An arity mismatch at
`score` refuses naming the model's arity and the count given, exactly as the
strategy compiler already does for `Expr::Model`.

### 4. The application seam

The kernel composes. `Platform` holds a `Box<dyn ModelProvider>` handed in
by the composition root (`qip-deepbrain` and `qip-api` pass
`InTreeProvider`; a test passes a provider that serves nothing), and the
only production consumers this record authorises are §39.1's advisory row:
a served model's `score` may be an **input** to a deterministic filter —
a cost estimate, a dispersion estimate, a regime estimate — and may never be
a threshold, a size, a limit or a permission. That is ADR 0005's rule for
language models applied to numeric ones: the number is computed by a
function the platform owns and reviewed like a constant, and what it feeds
is code that could refuse it. `ModelRegistry::require_for_decision` remains
the gate on whether a card may inform a decision at all; the provider does
not consult it and the caller must.

### 5. Promote and deploy, within the rule

- **Promote** is `ModelRegistry::promote` on the card plus the artifact
  written where the composition root says — a file, today, because no
  Artifact Registry client exists and ADR 0009 forbids writing one. The
  artifact is "signed" in the sense this platform can sign: its digest is
  in-tree SHA-256 and the manifest that names it travels under the mesh
  envelope's HMAC. Asymmetric signing is ADR 0043's gap and is **not closed
  here**; a reader must not take a digest for a signature.
- **Deploy** is the policy fabric's job, not a new channel. `ModelManifest`
  names each model by digest in `Slot<ModelManifest>`; the hot-path model
  travels inline in the compiled plan as `Expr::Model`; and the cell installs
  a plan **only if every inline model's `DistilledModel::digest()` is named in
  the manifest it holds**, refusing otherwise. `Cell::apply_policy` applying
  one verified policy is the "atomic pointer swap" — the plan and the
  manifest arrive together or not at all. Whether that digest check exists
  today is answered by `grep -n 'digest' backend/crates/apps/qip-edge-node/src/strategies.rs backend/crates/edge/qip-edge/src/cell.rs`;
  if it does not, it is part of the implementing lane.
- **ONNX, protobuf, `prost`, `ring`, `burn`, `linfa`, `tract`, `polars` and
  `arrow` are retired from §21.2 and §39.1 as names.** The outcomes — a
  restartable trainer, evaluation against an incumbent on held-out folds,
  a digest-named artifact, a node that swaps atomically — are what the
  register scores.

## Consequences

- §21.2's "BLOCKED-by-policy" clause is withdrawn in the register: the
  back half of the pipeline is **undone**, not blocked, and this record says
  what building it looks like. The row stays `PARTIAL`.
- §39.1's "ONNX models, shipped, in-process" row is scored as: in-process,
  yes, by decision; ONNX, retired; advisory consumer, absent until the lane
  lands. The row stays `PARTIAL`.
- `qip-ai` gains one module and no dependency. `qip-training` gains one
  module and no dependency. `qip-strategy` gains nothing and must be shown
  to gain nothing.
- A test in the implementing lane must prove both halves: an artifact of
  each of the four formats round-trips through `pack` and `serve` and scores
  identically to the form's own `predict`/`evaluate` on the same inputs; and
  an artifact carrying an unknown format, a wrong digest and a wrong arity
  each refuse with the sentence above. Mutation-verified, per the testing
  rules.

## What it costs

**The platform serves four model forms and will serve four until an ADR
says five.** A sequence model, an embedding for §39.1's episodic retrieval,
or anything with a non-linearity a stump cannot express, is refused at the
wire. That is the price of a closed enum, paid on purpose: an open format
list is a dependency request waiting for a lane to make it.

**Training capacity stays what `LocalTrainer` is.** Ridge regression and
boosted stumps on tabular features, in-process, on a Cloud Run CPU. §21.2's
GPU row, spot instances and `burn`'s autodiff are not replaced by anything;
they are retired with the names. ADR 0069 already refuses the spot GPU pool
for want of a consumer, and this record does not create one.

**Two serialisations of a model exist and must agree.** The artifact's
payload is the provider's serde form of `TeacherForm`/`DistilledModel`, and
the compiled plan carries `DistilledModel` inline in the IR. They are the
same type serialised twice, which is why the digest is computed on the
canonical payload by one function — but a reader auditing a deployment has
two places to look, and this record says so rather than pretending the
manifest carries weights.

**A `Box<dyn ServedModel>` is a virtual call and an allocation.** Acceptable
off the hot path, which is the only place it is permitted, and the reason
decision 1 keeps it out of `qip-strategy` by structure rather than by
comment.

**No signature.** A digest under an HMAC envelope proves integrity and the
sender's possession of the mesh key; it does not prove who trained the
model. ADR 0043 owns that gap and this record inherits it unchanged.

## What would make this wrong

- **A model class `qip-training` cannot express, with a measured benefit.**
  If a candidate model outperforms the linear and stump baselines on the
  held-out folds by a margin the deflated-Sharpe gate accepts — ADR 0006's
  discipline, applied to machine learning — and it needs a form outside the
  four, the argument for an inference crate is then made from that
  measurement under ADR 0012's three conditions, with the transitive tree in
  the diff. `tract` would be the candidate to argue, and its tree (a tensor
  library, protobuf parsing, a dozen numeric crates) is the cost to state.
- **The advisory consumer turns out to need the hot path.** If a dispersion
  or cost estimate is wanted inside `Cell::work` rather than in the centre's
  filters, the answer is distillation into `Expr::Model` — the route that
  exists — and not a provider on the cell. If distillation loses too much
  fidelity (`FidelityReport` says so in four numbers), that is a finding
  about the model, not a reason to put a loader on the execution path.
- **A provider that serves everything.** A second `ModelProvider` whose
  `serves()` returns all four formats and whose `serve` delegates to a
  crate is the shape this record refuses and the shape that would pass the
  interface. The acceptance suite should hold `ModelProvider` implementors
  to the workspace's dependency list, which it already does for every crate.
- **The digest check on plan installation being skipped for convenience.**
  A cell that installs a plan whose inline model is not in its manifest has a
  model nobody promoted, and the deploy stage is then a formality that reads
  as a control.

## Alternatives considered

**(a) Authorise a specific inference crate.** `tract` is the blueprint's
name, and `ort` or `burn` the neighbours. Rejected. ADR 0012's first
condition — failure is silent — is met: a wrong number is a wrong number.
Its second — the problem is specialist or adversarial — is not, for this
population: evaluating a linear model is a dot product and a stump ensemble
is a loop of comparisons, and the platform already evaluates both in code
its own people read. Its third — a mature audited implementation whose
absence would be indefensible — is not met either, because the supply-chain
surface added is a general tensor runtime, a protobuf parser and their
closure, audited by nobody on this platform, in exchange for arithmetic the
platform has. The blueprint's own reason for `tract` — one crate for
training and serving so skew is structural — is met more strongly by one
type for both.

**(c) Route through the hosted `LanguageModel` adapter.** Rejected on the
type: `NumericGuard::enforce` refuses any number a language model emits, by
ADR 0005's design, and a numeric model's whole output is a number. It is
also the wrong latency class and the wrong determinism class, and ADR 0008
keeps every hosted call off the fast path. The `LanguageModel` pattern —
port in `qip-ai`, provider behind it, `is_available` for a deployment that
cannot reach it — is borrowed here in shape and nothing else.

**Serve from the strategy IR everywhere, no interface.** Every off-hot-path
consumer would then build a one-rule strategy to score a model, which is a
compiler invocation to do a dot product. Rejected as a misuse of a type
whose whole point is its bounded cost on the execution path.

**Leave the register at BLOCKED-by-policy.** Rejected: it was false. The
policy blocks five named crates; it does not block the pipeline, and a row
that says it does tells the next lane not to try.

## Dependency-direction argument

New edges: none. `qip-ai` (lib) gains a module and keeps depending only on
libs. `qip-training` (service) already depends on `qip-ai`? — it does
**not** today (`grep -n qip-ai backend/crates/services/qip-training/Cargo.toml`);
the implementing lane adds `qip-ai` to `qip-training`'s manifest, which is a
service depending on a lib and is the permitted direction. `qip-strategy`
(edge) is untouched and gains no dependency on `qip-ai`. `qip-kernel`
(runtime) already depends on both services and libs and composes the
provider. No lib depends on a service, no service on the runtime, nothing on
an app. The one pre-existing oddity — `qip-training` (service) depending on
`qip-strategy` (edge) — is not introduced here and is not widened.
