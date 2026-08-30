# Threat model

What an attacker would go after in this platform, what actually stops them
today, and — for each threat — what does not.

The second half of every entry is the part worth reading. A threat model that
lists only mitigations is marketing: it tells a reviewer what the authors were
proud of and nothing about where the platform is thin. Where a control is
absent, partial, or rests on an assumption this build cannot check, it is said
plainly and in the same breath as the control it sits next to.

Credentials — what exists, what is still required, and how to authenticate
without creating a service-account key — are in [credentials.md](credentials.md)
and are not repeated here. This document assumes them.

**Executable assertions live in
[`backend/crates/tests/qip-acceptance/tests/security.rs`](../../backend/crates/tests/qip-acceptance/tests/security.rs).**
Threats with a test there are marked *asserted*; threats without one are
documentation only, and that is itself part of the honest position.

---

## 1. What is being defended

| Asset | Why an attacker wants it |
|---|---|
| The venue order-entry credential | It places real orders |
| The capital-envelope key | It mints authority to spend |
| The five API bearer tokens | Operator holds the kill switch and the autonomy level |
| The book and the position record | A wrong position makes every number downstream fiction |
| The research corpus and fitted models | A poisoned input is a durable, quiet loss |
| The event log | It is the only account of what happened |

Two trust boundaries carry most of the weight, and they are the ones ADR 0008
draws — see [edge cells decide alone](../adr/0008-edge-cells-decide-alone.md):

* **Cell / centre.** A cell trades without asking. Its authority is a signed,
  bounded, expiring `CapitalEnvelope` rather than a policy, so a cell that has
  been taken over or cut off is bounded in both size and time.
* **Decision core / everything else.** The crates that decide, price, size,
  risk or place a trade keep `serde` and `serde_json` and nothing else; the I/O
  edge may take an allowlist. See
  [ADR 0009](../adr/0009-tiered-dependency-policy.md). A dependency that cannot
  reach the core cannot change what the core computes.

---

## 2. Two properties stated precisely

These are the two things the platform genuinely has that most systems in this
shape do not. Both are easy to overclaim, so both are stated at exactly their
real strength.

### External text cannot command a trade

Not "we prompt carefully". Two independent mechanisms, neither of which relies
on anyone remembering a rule.

**The hot path is structurally unable to reach a language model.** No crate
under `backend/crates/edge`, and none of the five decision engines
(`qip-execution-engine`, `qip-risk-engine`, `qip-portfolio-engine`,
`qip-optimization-engine`, `qip-simulation-engine`), declares a dependency edge
that reaches `qip-ai` — transitively, so an intermediate crate cannot launder
one. This is asserted over the parsed manifests by
`no_safety_critical_engine_can_reach_a_language_model` and
`no_edge_cell_can_reach_a_language_model` in
[`architecture.rs`](../../backend/crates/tests/qip-acceptance/tests/architecture.rs).
A second, finer check —
`nothing_that_decides_or_executes_names_the_language_model_interface` — greps
the same crates' source for `qip_ai::language` and `LanguageModel`, because
`qip-ai` also holds the deterministic retrieval machinery and a manifest edge
to it is not proof of a model call. ADR 0008's third consequence is where the
rule comes from: a compiled strategy IR has no node that calls out, and a model
contributes by being distilled into fixed coefficients ahead of time.

**A model has nowhere to put a number.**
`qip_agents::finding::NumericProvenance` has exactly two variants, `Observed`
(source, as-of, record id) and `Computed` (the function, and the named inputs).
There is no third. Every number an agent reports is a `NumericFact` carrying
one of the two, `AgentFinding::validate` refuses a `Computed` fact that names
no inputs, and `qip_ai::language::NumericGuard::enforce` rejects any structured
completion containing a numeric leaf at all — applied inside
`LanguageModel::complete_structured`, before the completion reaches the agent.
`AgentContext::complete` is the only route to a model and it goes through the
`CallLanguageModel` capability gate first.

*Asserted:* `text_from_a_hostile_page_cannot_become_a_number_a_calculation_depends_on`,
`an_agent_that_has_read_a_hostile_page_still_cannot_reach_a_model_or_the_market`.

**What this does not claim.** Narrative is still attacker-influenced. An
injected page can change what an agent *says* — its claim, its falsifiers, its
caveats — and that text reaches a human reviewer and the qualitative side of
the reasoning engine. Confidence is arithmetic, but the evidence the arithmetic
runs over is chosen from what was read. There is no provenance on prose the way
there is on numbers, and nothing downstream labels a sentence as having come
from attacker-controlled content.

### Unknown is not permission

`qip_data_finder::legal::Legality` is three-valued — `Permitted { basis }`,
`Forbidden { rule }`, `Unknown { question }` — and `is_permitted()` answers
`false` for `Unknown`. The only combinator is `Legality::and`, which keeps the
*least* permissive of two verdicts; there is deliberately no `or`, because a
source forbidden by robots and licensed by contract must not come out
permitted. `require_permitted` refuses an `Unknown` with the sentence "absence
of a prohibition is not a permission".

The verdict then reaches routing as an argument, not as a score:
`Routing::decide(&legality, &scores)` takes legality first, `Routing`'s fields
are private, and this is its only constructor — so a source scoring 1.0 on
every axis and carrying an undetermined licence is `RoutingClass::Rejected`,
and the composite is recorded in the basis so the record shows what was given
up. `LicensingPosture` also distinguishes *undetermined* (no terms found — the
remedy is a fetch) from *ambiguous* (terms found, not mappable — the remedy is
a lawyer), and a source licensed for research and asked about trading comes
back `Forbidden`, not `Unknown`, so a research feed cannot be promoted by
anyone who only checks for the absence of a prohibition.

*Asserted:* `a_source_whose_licensing_is_undetermined_cannot_become_tradeable_input`.

**What this does not claim.** It governs whether the platform may *collect* and
what it may *use a dataset for*. It says nothing about whether the data is
true, and `NetworkProbe` reports itself unavailable in this build, so no part
of this has run against a real licence page.

---

## 3. The threats

### 3.1 Credential theft

**The threat.** An attacker obtains a bearer token, the capital-envelope key,
the central plane's signing secret, or the deploy identity.

**What they would target.** `QIP_TOKEN_OPERATOR` (the kill switch and the
autonomy level), `qip-capital-envelope-key` (authority to spend), the venue
order-entry credential, and the GCP identity that applies Terraform.

**What stops it today.** `Credential::from_token` hashes the token immediately
and stores only the SHA-256, so a serialised credential or a memory dump
contains nothing usable — `qip-api`'s own test asserts the token does not
survive serialisation. `Authenticator::authenticate` compares with
`qip_core::hash::constant_time_eq` and deliberately does not return early on a
match, so response latency does not reveal how far down the credential list the
match sits. Every credential carries `expires_at` and expiry is refused.
`qip-api` refuses to start with no credential configured and refuses any token
under 32 characters. Nothing secret enters the repository: Terraform creates
secret *containers* and never a version — `no_secret_value_appears_in_the_terraform`
refuses both `google_secret_manager_secret_version` and `secret_data`, and
`no_credential_appears_in_a_kubernetes_manifest` covers the manifests;
[`scripts/check-secrets.sh`](../../scripts/check-secrets.sh) is the CI gate. The
venue credential's IAM binding does not exist where `autonomy_ceiling =
"paper_trading"`, so in every shipped environment the credential is unreadable
rather than merely unused. The deploy pipeline holds no key at all: GitHub mints
an OIDC token and an `attribute_condition` pins the repository.

*Asserted:* `no_secret_value_appears_in_any_committed_configuration` composes
the two existing infrastructure tests and extends the scan to the workflow,
tfvars and ops files they do not cover.

**What does not.** A stolen *live* token is indistinguishable from its owner
until it expires. Tokens are bearer credentials with no proof of possession, no
mTLS, no binding to a source address, and no revocation list — rotating means
restarting the process with new environment variables, and `main.rs` mints them
with a thirty-day life. Failure lockout is keyed by subject, and the subject is
only known after a hash match, so an attacker guessing random tokens never
trips it. The rate limiter also runs *after* authentication, so an
unauthenticated request flood is bounded only by `ServerLimits::max_concurrent`.
The in-tree HTTP server speaks no TLS; transport security is assumed to be
terminated by a proxy this repository does not configure.

### 3.2 Malicious feed injection

**The threat.** A party on the network path injects well-formed market data
messages into a cell's feed.

**What they would target.** The multicast or TCP feed a cell decodes, and
through it the order book that the strategy and the pre-trade risk check both
read.

**What stops it today.** `qip_protocols::bytes` bounds-checks every read and
returns an error rather than panicking, so a truncated or oversized packet is
an everyday event rather than a dead process. `qip_sequencing::tracker` holds
out-of-order messages until their predecessors arrive, drops duplicates, and —
the case it exists for — declares an unrecoverable gap and emits
`MessageBody::Reset` rather than letting a consumer trade off a book with a
silent hole, with a bounded reorder buffer so a permanent gap cannot become an
out-of-memory kill. `qip_sequencing::arbitration` publishes whichever redundant
copy arrives first under one canonical feed name and holds a bounded seen-set,
so a slow line cannot re-deliver a morning. At the ingestion boundary
`qip_normalization::contract::DataContract` asserts field presence, ranges and
staleness, and `ScaleGuard` flags a price that moves by more than a configured
ratio. Whatever survives all of that is still bounded by the cell's
`CapitalEnvelope`.

**What does not.** **The feed is not authenticated.** There is no session MAC,
no TLS, no venue certificate check. Sequencing catches duplication, reordering
and loss — none of which is forgery. A party who can put packets on the path
with a plausible sequence number will have them decoded and applied. There is
no feed transport in this build at all, so none of the above has been exercised
against a real venue.

### 3.3 Compromised source

**The threat.** A registered data source starts returning attacker-chosen
values — a vendor breach, a DNS hijack, a scraped page that changed hands.

**What they would target.** Any source the data finder registered and that
downstream features are computed from.

**What stops it today.** `HostRules::verdict` decides whether a host may be
contacted at all, evaluating the denylist first and unconditionally, and
matching subdomains, because a publisher who asked us to stop did not mean "on
this hostname only". A claim is not evidence: `SourceCandidate` and `Source`
are different types and the second cannot be constructed without
`probe::ProbeEvidence`. Schema drift and health monitoring are per-source and
carry a severity. Contracts and `ScaleGuard` bound what a changed source can
push through normalisation. Anything derived is content-addressed: the
`ArtifactStore` will not admit bytes whose hash does not match their
`Provenance`, and `ProvenanceChain` walks a derived artifact back to datasets
explicitly registered as raw.

**What does not.** No source is cryptographically authenticated — there is no
signature over a vendor payload to check. A compromised source that keeps
returning schema-valid, in-range values is caught only by drift statistics,
which is a detection lag and not a prevention. `NetworkProbe` reports
`Unavailable` in this build, so the discovery logic is complete and the
transport is not.

### 3.4 Data poisoning

**The threat.** An attacker shapes the training or research corpus so that a
fitted model or a backtest reaches the conclusion they want.

**What they would target.** The lakehouse behind the central plane, and the
join between a registered dataset and a feature.

**What stops it today.** Point-in-time truth removes the most common way a
backtest lies to itself: `PointInTime` is built by *discarding* facts whose
known-time is after the as-of, so there is no accessor that could return one,
and `restrict_to` only ever narrows. `LeakageDetector` covers inputs that
arrived from outside a reader and names the ones that would not have been
knowable. Lineage is checked rather than asserted:
`ProvenanceChain::require_complete` demands both no breaks *and* at least one
registered raw dataset, so a model that declares no inputs does not pass as
fully traced, and a break names the exact digest and the artifact that
referenced it. `LicensedData::derive` carries the originating dataset onto the
derived value, so a licence cannot be laundered by a `map`.

**What does not.** Nothing detects a statistically plausible poison. Provenance
proves *where* a value came from, not that it is true, and there is no
distributional or outlier gate between a registered raw dataset and a fitted
model beyond the contract's declared ranges. The bitemporal control has its own
recorded caveat: a value stripped of its `Stamped` wrapper before it reaches a
reader is outside the control entirely.

### 3.5 Model poisoning

**The threat.** Swapped weights, altered coefficients, or a bad model talked
through admission.

**What they would target.** The artifact store, and `ModelRiskRegister::admit`.

**What stops it today.** `AdmittedOutput` has no public constructor and
`ModelRiskRegister::admit` is its only source; admission requires the `qip-ai`
eligibility check, a current risk file, an operating point inside the declared
boundary, and an `Explanation` — which cannot be constructed unless its
contributions reconcile to the output exactly, in `Decimal` arithmetic, with a
zero residual. A decision written against `AdmittedOutput` therefore cannot be
handed a raw model output. `ArtifactStore::store` performs two checks that
catch different failures: the digest catches bytes changed after signing, the
signature catches bytes never signed at all. The hot path is insulated
architecturally — a strategy runs distilled coefficients, so a swapped model
cannot change a live decision without going back through the approval ladder.

*Asserted:* `a_tampered_artifact_fails_its_provenance_check`,
`an_artifact_signed_under_a_foreign_key_is_refused`,
`an_artifact_whose_inputs_lead_nowhere_is_not_fully_traced`.

**What does not.** The signature is HMAC over a shared secret, so anyone who
can verify can also sign — see §4.1. Reconciliation proves the attribution sums
to the output, not that the attribution method is the right one; that is a
recorded caveat on the model-risk control. Nothing checks the training process
itself: a poisoned model whose explanation reconciles and whose risk file is
current is admitted.

### 3.6 Prompt injection through web content

**The threat.** A filing, a news page or a scraped document contains
instructions aimed at the agent reading it.

**What they would target.** The eighteen research agents, and through them the
proposal that reaches a human.

**What stops it today.** Everything in §2 above — the hot path cannot reach a
model, and a model has nowhere to put a number. Beyond that:
`ModelRequest.context` is a separate named map from `prompt`, so retrieved
evidence is not concatenated into the instruction text. Capability gating is
the containment that does not depend on the agent behaving: an agent that has
been entirely talked into acting still cannot reach a model without
`CallLanguageModel`, cannot reach the market without `SubmitOrder`, and cannot
raise the autonomy level at all, because
`AgentManifest::validate_separation_of_duties` refuses `ChangeAutonomyLevel`
for every role. `Gated<T>`'s inner value is private and its only accessor takes
the context, so a refusal happens whether or not the agent checked, and the
attempt is recorded in `denied_accesses` before the error returns.
`architecture.rs` additionally asserts that no agent holding
`CallLanguageModel` holds any market-touching capability.

*Asserted:* both tests named in §2, plus
`a_research_agent_cannot_be_granted_a_market_touching_capability`.

**What does not.** See §2: narrative is attacker-influenced, and nothing marks
it as such downstream. Nor is there any sanitisation, classification or
provenance on the *text* an agent ingests — the platform's answer is
containment, not detection.

### 3.7 Fake market events

**The threat.** A fabricated print, halt, corporate action or headline,
manufactured to trigger a trade.

**What they would target.** The anomaly detector, and any feature computed from
a single source.

**What stops it today.** `ScaleGuard` on the price-ratio jump, data contracts
with ranges and staleness at the boundary, and — after all of that — a
pre-trade risk check that runs against the state the order *would* produce
rather than the state it is in. The `CapitalEnvelope`'s `order_limit` bounds
any single order and its `loss_limit` is what the cell stops itself on, on its
own authority. Every number an agent reports about the event must be `Observed`
with a source and a record id, so a headline is not a number. Tripping the kill
switch needs no authority at all — a false stop is cheap and a missed one is
not.

**What does not.** **There is no cross-source corroboration requirement.** One
registered, in-contract source is enough to move a computed feature. There is
no venue-official-notice channel, no halt feed and no corporate-action
authority to check against. A fabricated event sized to sit inside the
contract's ranges passes every gate above, and detection then rests on
statistics that will fire after the fact.

### 3.8 Broker API compromise

**The threat.** The venue gateway or the broker itself reports fills that did
not happen, drops orders silently, or alters acknowledgements.

**What they would target.** The cell's own record of what it traded — which
comes from the same code path that did the trading and therefore agrees with
itself by construction.

**What stops it today.** `qip_edge::dropcopy::DropCopyReconciler` compares the
cell's record against the venue's independent drop-copy channel, and a
`Discrepancy` is not a warning: positions the platform believes it holds having
diverged from the ones it actually holds means every number downstream is
computed against a fiction, so a break halts the cell and stays halted until a
person has looked. `Platform::ingest_cell_report` routes that break to the
platform's own kill switch rather than to a second switch nobody watches, and
scopes the halt to the reporting cell so one cell's bookkeeping failure is not
the platform's outage. `Broker::is_simulated` is copied onto every `Fill`, so
reconciliation can tell paper from real without consulting the environment —
which is exactly what gets confused between a test and a deployment. `LiveBroker`
reports itself unavailable in this build and says why, so a deployment
configured for live trading fails at start-up with a legible message rather
than at the first order.

**What does not.** A drop copy is independent only if the venue's two channels
are independent, which a compromised *broker* defeats by definition — it can
report consistently on both. There is no order-entry message signing, no FIX
session authentication and no TLS in this build, because there is no transport
in this build. Nothing here has run against a real gateway.

### 3.9 Replay attacks

**The threat.** A captured, genuine, correctly signed message is presented
again — to another cell, at another time, or against another subject.

**What they would target.** A `CapitalEnvelope`, an `Approval`, a fill, or an
API request.

**What stops it today.** `VerifiedEnvelope::verify` is the control, and
construction is not: `CapitalEnvelope::new` is public because the allocator has
to build one somehow, so holding a well-typed envelope proves nothing, and
`VerifiedEnvelope`'s inner value is private with verification as its only
constructor. It refuses a bad signature, an envelope whose `cell()` is not the
cell presenting it — a correctly signed grant for a *different* cell is exactly
the replay a signature alone does not stop — and one outside its validity
window, with `is_live` re-checked at every use rather than once on arrival,
because a backstop consulted only on arrival is not one.
`CapitalEnvelope::signing_payload` covers every bound, so an envelope re-issued
with a wider gross limit and the original signature does not verify.
`SigningKey::sign` mixes the key id into the HMAC message as well as carrying
it alongside, so a signature made under one key cannot be replayed as though
made under another after a rotation. `ApprovalChain` requires a fresh
credential per approver and refuses an approval replayed for a different
strategy. On the feed, `tracker` drops duplicate sequence numbers and
`arbitration` holds a bounded seen-set.

*Asserted:* `a_capital_envelope_granted_to_another_cell_is_refused_when_replayed`,
`an_expired_envelope_cannot_be_replayed_after_the_window_closes`.

**What does not.** The HTTP API has no nonce, no request signing and no
idempotency key. A captured `POST /api/v1/kill-switch` or `POST /api/v1/cycle`
can be replayed for the life of the bearer token. Replay of an envelope *within*
the same cell and the same window is by design indistinguishable from
legitimate use — that is what "granted in advance" means.

### 3.10 Adversarial orders

**The threat.** Another participant trades against us deliberately: quote
stuffing, momentum ignition, layering designed to make our book state wrong or
our sizing large.

**What they would target.** The strategies, and the sizing that runs off the
book they read.

**What stops it today.** Pre-trade risk runs last in `OrderManager::submit`,
after the kill switch and the autonomy check, and against the projected state,
so its expensive computation is not spent on an order that was never going to
be sent. The `CapitalEnvelope` bounds gross exposure, any single order, and the
loss at which the cell stops the strategy itself. `Utilisation` is what
admission is decided against, so repeated small orders consume one budget
rather than each getting a fresh one. Every envelope expires, so a cell being
worked over is bounded in time as well as in size. The kill switch trips
without authority.

**What does not.** There is no order-flow toxicity model, no adverse-selection
monitor, no per-counterparty analytics and no venue-level anti-gaming logic.
Latency is not defended: a faster participant wins the race and nothing here
notices. The shadow gate catches a strategy whose live decisions disagree with
its backtest, which is after the fact rather than during. The limits above bound
the *loss*; they do not detect the *attack*.

### 3.11 Insider access

**The threat.** Someone with legitimate credentials — an operator, an engineer,
an agent owner — acts outside their remit.

**What they would target.** The autonomy level, the kill switch, an agent
manifest, or the pipeline identity.

**What stops it today.** Separation of duties is stated as prohibitions on
combinations rather than as a whitelist of roles, so a new role cannot inherit
an unsafe pairing: `AgentManifest::validate_separation_of_duties` refuses
propose+approve, propose+veto, override+submit, `SubmitOrder` outside the
Execution role, `SubmitOrder`+`PublishHypothesis` together, `TriggerKillSwitch`
outside the Control role, any market-touching capability on a research-side
role, and `ChangeAutonomyLevel` for every role without exception. Raising the
autonomy level needs an authenticated operator, a second and different
approver, a stated reason, and a credential authenticated within the last
fifteen minutes; `qip-cli` deliberately has no command for it, because a
command line cannot establish two people. `ApprovalChain::grant` needs an
`Approval` naming a human who is not the requester, a fresh credential per
approver, and two different approvers above the threshold. Probing leaves
evidence: `AgentContext::authorise` records a refusal *before* returning the
error. The event log is hash-chained and `EventLog::verify_chain` reports the
first broken record. The pipeline identity Terraform creates can push an image
and apply a manifest and nothing else — see
[credentials.md](credentials.md#the-bootstrap-account-is-not-the-pipeline-account).
Runbook: [permission-violation.md](../operations/permission-violation.md).

*Asserted:* `an_unauthenticated_caller_cannot_reach_a_privileged_route`,
`a_caller_with_the_wrong_role_cannot_reach_a_privileged_route`,
`a_research_agent_cannot_be_granted_a_market_touching_capability`.

**What does not.** One operator token both halts the platform and clears the
halt; there is no second pair of eyes on a clear. The freshness requirement on
that clear is weaker at the HTTP boundary than it looks: `qip-api` constructs
`OperatorIdentity::verified(subject, "api-bearer-token", now)` with the request
time, so `check_credential`'s fifteen-minute window is satisfied by presenting
a valid bearer token rather than by re-authenticating. The event log is
hash-chained but not counter-signed or externally anchored, so anyone who can
rewrite the file can rewrite the chain with it. `claude-builder` holds
project-level administrative roles and is the largest single insider blast
radius in the deployment; that is documented in
[credentials.md](credentials.md) rather than mitigated.

### 3.12 Cross-region compromise

**The threat.** One edge cell is taken over and used to reach its siblings or
the central plane.

**What they would target.** Another cell's capital, or the centre's view of
aggregate exposure.

**What stops it today.** A cell's authority is a data structure, not a
policy — every bound in `CapitalEnvelope` is private with no setter, and
widening means asking the centre for a new grant. A compromised cell presenting
a sibling's envelope is refused by the cell check in `VerifiedEnvelope::verify`.
Halts are scoped, so one cell's reconciliation break does not stop the others.
The infrastructure half: a default-deny `NetworkPolicy` in the namespace,
private nodes with no public addresses and a private control plane, a
per-deployable service account rather than the default compute one, workload
identity so no key file lives on disk, and `GKE_METADATA` with the legacy
endpoints disabled so a compromised pod cannot read the node's credentials.
All of those are asserted structurally in
[`infrastructure.rs`](../../backend/crates/tests/qip-acceptance/tests/infrastructure.rs).

**What does not.** **Halt state is process-local** — a recorded caveat, and the
one with the most operational consequence here: a halt recorded at the centre
does not reach a partitioned cell, and the only thing that stops that cell is
its envelope's expiry. Aggregate exposure at the centre is stale by the round
trip from the cells, which ADR 0008 states as a cost rather than a bug: three
cells can each stay inside their own bounds and jointly hold a concentrated
position nobody authorised, for as long as it takes the centre to notice and
recall. And because envelope signing is symmetric, **a compromised cell holding
the shared envelope key can mint its own envelope** with any bounds it likes;
the cell check stops replay between cells, not forgery by a cell that has the
key. There is no per-cell key derivation in this build — the deployment models
one `qip-capital-envelope-key`.

---

## 4. Known-weak areas

Recorded here so nobody is surprised. Each of these is already carried as a
caveat by the compliance plane rather than invented for this document:
`CompliancePlane::report` builds one `ControlStatus` per control from
`Control::all`, each with its `caveats`, and
[`plane.rs`](../../backend/crates/libs/qip-compliance/src/plane.rs) is where the wording
lives. `ComplianceReport::caveats()` returns the honest list for a reviewer who
wants it rather than the headline. `compliance_proof.rs` asserts the caveats
survive, because a report whose honest gaps were tidied away is a regression
dressed as an improvement.

### 4.1 HMAC signing proves possession, not identity

Signing is HMAC-SHA256 over a shared secret. Bytes that change do not verify
and a signature cannot be produced without the secret, so it is a genuine
integrity control — but HMAC is symmetric, and whoever can verify can also
sign. A verified artifact is attributable to *someone holding the key*, never
to the person or service named in `Provenance::signer`. This is the caveat on
the signed-artifacts control, and `crate::signing` states what a real signing
path needs: asymmetric signing with the private key in a KMS, a certificate
chain binding key to signer, rotation and revocation, and independent
countersigned timestamping. None of those exist here, and there is no key
material in this environment at all. The key id travels with every signature so
that when asymmetric signing arrives, existing records say which key they were
made under.

The same limitation is what §3.12 turns into a concrete attack: it is why a
compromised cell can forge its own capital envelope.

### 4.2 The central plane's default signing secret is reproducible

`central_signing_secret(seed)` in `qip-kernel` derives the plane's secret from
`PlatformConfig::seed` by hashing a fixed label with it. That is exactly what a
test and a replay want and exactly what a deployment must not have: anyone who
knows the seed can mint an envelope. The plane knows this about itself —
`CentralPlane::compliance_report` attaches an extra caveat to the
signed-artifacts control whenever its key is reproducible, so the property
appears in the audit artifact and not only in a comment. A deployment builds
`CentralPlane::new` with a secret from its key store and swaps it in through
`Platform::set_central`.

### 4.3 Halt state is process-local

The kill switch and the incident log live in the process that recorded them.
The recorded caveat says what a distributed deployment needs: the same record
replicated, and an edge cell that cannot reach it failing closed on its own
capital envelope's expiry. Today the expiry is the whole of that guarantee.

### 4.4 Trait-surface absence is not compiler-enforced

The manifest-level rule — no decision crate has a dependency edge reaching
`qip-ai` — is enforced by Cargo and therefore by the compiler. The finer rule —
that no such crate so much as *names* `qip_ai::language` or `LanguageModel` —
is a string search over source files in `architecture.rs`. It exists because
`qip-ai` also holds deterministic retrieval machinery that the world model
legitimately uses, so a manifest edge is not proof of a model call. But a grep
is a grep: a re-export, a rename or a type alias defeats it, and the crate that
legitimately holds the trait (`qip-agents`) is exempt by construction. The
compiler-checked half is the manifest edge; the source-level half is a review
aid that fails loudly in the same commit as the offending line, which is what
it is for.

### 4.5 Other recorded caveats, in one place

* An unapproved `CapitalEnvelope` can be constructed anywhere, because
  `CapitalEnvelope::new` is public. It cannot become an `ApprovedCapital` or a
  `VerifiedEnvelope`, so components must take those types rather than the bare
  envelope — and every capital decision in a cell does.
* Licence expiry is checked against the timestamp the caller passes. The plane
  reads no ambient clock, which is what makes a replay reproduce, and it means
  a caller passing a stale `now` weakens the control.
* A value stripped of its `Stamped` wrapper before it reaches a point-in-time
  reader is outside the bitemporal control.
* Attribution reconciliation proves the contributions sum to the output, not
  that the attribution method is the right one.

---

## 5. What is not modelled

Stated so the boundary of this document is legible:

* **Transport security.** No TLS anywhere in-tree, for the feed, the gateway or
  the API. See [ADR 0009](../adr/0009-tiered-dependency-policy.md) for why an
  in-tree TLS stack is the worst of the available options and what the I/O edge
  tier now permits instead.
* **Anything that has run against a real venue.** There is no feed transport,
  no order-entry transport and no drop-copy transport in this build;
  [external-dependencies.md](../operations/external-dependencies.md) names each
  requirement.
* **Denial of service beyond the request limits.** `ServerLimits` bounds what
  one connection can make the server allocate. There is no upstream rate
  limiting, no connection admission control and no capacity model.
* **Physical and supply-chain security of the runner.** CI runs
  `cargo audit` and produces an SBOM; nothing verifies the toolchain itself.
* **The residual risk of the managed services**, which
  [ADR 0009](../adr/0009-tiered-dependency-policy.md) accepts explicitly: the
  edge tier's dependency tree will be too large to read line by line, and that
  is a real reduction in auditability rather than an accident.

---

## 6. Where the assertions are

| Suite | What it defends |
|---|---|
| [`security.rs`](../../backend/crates/tests/qip-acceptance/tests/security.rs) | This document, made executable |
| [`architecture.rs`](../../backend/crates/tests/qip-acceptance/tests/architecture.rs) | The absent dependency edges — §2, §3.6 |
| [`infrastructure.rs`](../../backend/crates/tests/qip-acceptance/tests/infrastructure.rs) | The deployment's structural properties — §3.1, §3.12 |
| [`compliance_proof.rs`](../../backend/crates/tests/qip-acceptance/tests/compliance_proof.rs) | That the six controls' mechanisms and caveats are real — §4 |
| [`scripts/check-secrets.sh`](../../scripts/check-secrets.sh) | Credentials in the history — §3.1 |
| [`scripts/check-dependencies.sh`](../../scripts/check-dependencies.sh) | The supply chain the core rests on — §1 |

Run them together with `make security`. How to run each kind of test is in the
[developer guide](../developer/README.md).
