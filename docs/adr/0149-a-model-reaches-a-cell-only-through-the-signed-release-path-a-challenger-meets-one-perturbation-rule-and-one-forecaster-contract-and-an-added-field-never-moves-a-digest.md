# ADR 0149: A model reaches a cell only through the signed release path, a challenger meets one perturbation rule and one forecaster contract, and an added field never moves a digest

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises the decisions below inside the tree;
  it does not authorise a billed resource, a risk-limit value, an edit to a
  protected path, or any live path, and the "Not decided here" section lists
  each of those it leaves open.
- **Date**: 2026-10-19
- **Extends**: ADR 0110 (signed model packs, two-person publish, Amendment 1's
  per-cycle payload carrying only what a pack covers), ADR 0131 (promotion
  drafts the signable pack; reflex packages sign under `model-pack`), ADR 0133
  (the node issues its own reflex forecast; one source per fact), ADR 0140
  (a material forecast is a `Forecast`), ADR 0142 (shadow precedes paper).
- **Keeps**: ADR 0003, ADR 0006 (a classical baseline always), ADR 0088 (a
  slot never gains, loses or retypes a field in place), ADR 0125 (an advisory
  stage narrows and never enlarges), ADR 0109 (no new crate).

## Context

Nineteen rows ask the same family of question about models and strategies:
what may name one to a cell, what a promotion has to show, what may compete in
the tournament, how long the ledgers that hold them live, and what an added
field does to a digest somebody pinned. Reading the code against the rows
changes the answer to several.

**The release path exists; the gap is its first step.** The register says the
contract `qip_contracts::policy::ModelPack` "has no production constructor" and
that the kernel "cannot supply" its signature. The consuming half is built and
fail-closed: `Platform::admit_model_packs` (`qip-kernel/src/model_trust.rs`)
verifies every pack against the trust bundle's `model-pack` key and refuses the
whole set on the first failure; `qip-api`'s `hold_models_to_release` calls it
from `QIP_MODEL_PACKS_PATH` and, outside `local`, holds the slot to an empty
release when the variable is missing. `Platform::model_manifest_for` then names
a distillate only when an admitted pack covers its artifact *and* its inline
model. The signing half is a workflow, `model-pack-publish.yml`, whose header
says it refuses until `qip-cli` grows a `model-pack` subcommand. So the missing
piece is neither a contract split nor a kernel that signs: it is the unsigned
draft the workflow signs, and the CLI step that signs it. ADR 0131 decision 2
already decided the draft; nothing built it.

**Two types share the name `ModelPack`**, and only one is a pack. The kernel's
(`qip-kernel/src/model_serving.rs`) is, in its own words, "the pack's *version
and lineage* and nothing more": an id digested over a predecessor and a
membership, not journalled, rebuilt from promotion records by
`pack_membership`. **Two types share `BeliefState`** the same way: the
reasoning engine's (`qip-reasoning-engine/src/belief.rs`) is a freshness clock
with a timestamp and a count; the contract's (`qip-contracts/src/policy.rs`) is
the six-field belief. ADR 0088 already decided what crosses to cells for the
second (a `Beliefs` slot beside `BeliefPriors`, never an in-place change), so
the register's "wire change" is a decision already taken elsewhere, and only
the name is open.

**Several rows presuppose a thing that is not in the tree.** `qip_training::distil`
takes a `TrainedTeacher`, a fitted model with its dataset; the NOW stage's
estimator is a local-level filter and is not one. `WorldModelSpawner`
(`qip-world-model/src/spawning.rs`) proposes a federation branch and has no
caller outside its own crate. The tournament's detector entrant is stamped
`cost: 0.0` and a zero `latency` in `model_tournament.rs`, which the influence
arithmetic reads as free and instant, not as unmeasured. A `Signal`
(`qip-contracts/src/signal.rs`) carries a kind (`Enter`, `Exit`, `Hedge`,
`Stand`), a `Conviction` and a quantity, but no above-or-below direction, so
"the strategy's forecast of the next close" is not something a strategy states.
`DistilledModel` is cited at "over a hundred" construction sites; its fields are
private and its digest is a plain join of named parts, which matters because a
field can be appended to a join without touching the old string.

**Locate every symbol named here** with
`grep -n 'pub struct ModelPack' backend/crates/libs/qip-contracts/src/policy.rs backend/crates/runtime/qip-kernel/src/model_serving.rs`,
`grep -n 'pub fn admit_model_packs' backend/crates/runtime/qip-kernel/src/model_trust.rs`,
`grep -n 'pub fn hold_models_to_release' backend/crates/apps/qip-api/src/signing.rs`,
`grep -n 'pub type ControlPackAnnouncement' backend/crates/libs/qip-contracts/src/control_pack.rs`,
`grep -n 'pub fn distil\|pub struct TrainedTeacher' backend/crates/services/qip-training/src/distill.rs backend/crates/services/qip-training/src/local.rs`,
`grep -n 'pub fn promote_model_from' backend/crates/runtime/qip-kernel/src/model_serving.rs`,
`grep -n 'pub fn require_calibrated_baseline' backend/crates/runtime/qip-kernel/src/central/models.rs`,
`grep -n 'pub struct DistilledModel\|pub fn digest' backend/crates/edge/qip-strategy/src/model.rs`,
`grep -n 'pub struct LifecycleLedger' backend/crates/services/qip-lifecycle/src/ledger.rs` and
`grep -n 'pub struct WorldModelSpawner' backend/crates/services/qip-world-model/src/spawning.rs`.

## Decision

### A. The release path

1. **The contract `ModelPack` is the only record that names a model to a cell;
   it is not split into a body and an envelope** (R12-CONTRACT-7064). The
   signature is inside the type because `signing_payload` is over every other
   field, and a "body" type would be a second shape of the same facts whose
   digest could drift from the signed one. The kernel does not construct it
   signed and never will: the kernel's side of the seam is
   `admit_model_packs`, which already exists, and its constructor is the
   publish workflow. What is missing is the first step, so the work is a
   `qip-cli model-pack` subcommand with the three verbs the workflow header
   names (`signing-message`, `attach-signature`, `verify`), each a thin call
   into `ModelPack::signing_payload`, `ModelPack::signed_by`'s message and
   `ModelPack::verify`, so the bytes are produced by the Rust that defines
   them and not re-implemented in shell.

2. **The kernel's lineage record is renamed `ModelSetVersion`.** A type called
   `ModelPack` that is not the thing a cell verifies is the cause of the
   register's confusion, and it will cause the next reader's. The rename
   covers the struct, `pack_membership` (to `set_membership`),
   `model_pack_lineage` (to `model_set_lineage`) and the `pack` field of
   `ModelManifestIssue`. It moves no wire: the record is "not journalled as a
   record of its own", so no bytes carry the name. The contract keeps
   `ModelPack`.

3. **Promotion drafts the signable pack as a file the workflow signs**
   (completing ADR 0131 decision 2). When `promote_model` admits a member the
   deep brain writes an unsigned contract `ModelPack` beside the artifact:
   artifact digest, features, the calibration the card records, allowed
   universes and budget from the promotion record, `rollback_parent` as the
   previous *signed* pack's artifact digest, and the lineage id of the
   `ModelSetVersion` that promotion made. The signature and publication are
   empty, so the draft fails `ModelPack::verify` and `admit_packs` by
   construction; it cannot be admitted by being left in the wrong directory.
   Expiry, universes and budget are copied from what the promotion records and
   refused when absent; they are not defaulted, because a default expiry is a
   release nobody reviewed. The lineage id enters the signed bytes under
   decision 17's rule.

4. **A `ControlPackAnnouncement` is the release's rollout record, produced by
   the release and consumed read-only by the centre** (R12-CONTRACT-7091). The
   row asks who signs, targets, windows and acknowledges. The answers: the
   *signer* is the two persons already recorded in the pack's `Publication`,
   named in the announcement's `signer` field; the *attestation* is the SHA-256
   of the signed pack's canonical bytes plus signature; the *target regions and
   nodes*, the *activation window* and `required_acks` are fields of the
   reviewed release file in the same commit as the pack, so choosing them is
   one of the two people's reviewed decisions and no process invents them;
   the *acknowledgements* are not a new transport, they are the cells'
   `Decision::PolicyApplied` journal records (ADR 0133 decision 4) naming the
   pack's inline model, counted by the centre. The kernel never constructs an
   announcement. When one is supplied beside the packs, `model_manifest_for`
   withholds a distillate from a cell outside its target nodes or window, which
   can only narrow what a cell is told (ADR 0125); an expired or untargeted
   announcement leaves the cell with the manifest it would have had without the
   model, not with a wider one. The safety of a cell does not depend on this
   record, since the cell verifies the pack itself; the announcement answers
   "who is meant to run this and has it taken", which an operator needs and a
   cell does not.

5. **A distillate has a teacher that was fitted, or it is not made**
   (R12-MODEL-4011). `qip_training::distil` already refuses anything but a
   `TrainedTeacher`, which for the ambient model means the NOW estimator is not a
   teacher: and a student fitted to its
   outputs would be the filter relabelled with a pack's authority. An ambient
   distillate is therefore admissible only when a `TrainedTeacher` was fitted
   by the learning desk on ambient-derived features against a named outcome
   (for example a surprise at time *t* against the next bar's move), registered
   and promoted through the ordinary path, and then distilled and released
   under decisions 1 to 3. Until such a teacher exists the register states the
   requirement and builds nothing.

6. **The lane of a promoted result is derived from its compiled form; no
   latency-budget field is added** (R12-EXPAND-6052). A budget in
   microseconds is wall time, which two replays do not share. The unit a cell
   already charges is worst-case evaluation steps, and the artefacts already
   state it: `FidelityPolicy::maximum_student_cost` bounds a student, a
   `ReflexPackage` is bounded by `MAX_RULES`, `MAX_CONDITIONS` and
   `MAX_ADJUSTMENT_BPS`, and a plan's strategies are refused by the compiler
   when their cost exceeds the budget. So an artefact is *reflex-eligible* if
   and only if it compiles inside those bounds, and otherwise it stays
   research; no constant decides, and no campaign carries a number that a
   default could fill. `WarmService` is not built: no artefact form in the tree
   would occupy it, and a lane with no occupant is a name. Eligible is not
   published: publication still needs the signer (ADR 0131 decision 1), and
   `held_unsigned` is the honest outcome until one is installed.

7. **A detector is a named entry rule of a compiled strategy, not a new IR
   concept** (R12-REFLEX-5041). `qip-strategy`'s IR already has the thing a
   detector is: a `Rule` with `SignalKind::Enter` over a typed `Expr`, compiled
   to a shared node. `OpportunityManifest`'s "definition digest" is the digest
   of that rule's compiled predicate node. Nothing produces the manifest and no
   code reads it, and this record does not change that: reading it with no
   producer would gate nothing, and a producer with no reader would be a
   number nobody checks. When both are built (producer: the centre, from the
   plan it already ships; reader: `StrategyInstaller`), the manifest is a
   *narrowing filter*: a strategy the produced manifest does not name is not
   activated, and an unproduced slot changes nothing, which is today's
   behaviour. It may never activate what the signed plan does not contain.

8. **A symbolic rule compiles to a strategy candidate through a declared
   binding table, and only to a candidate** (R12-MODEL-4012). The rule pack's
   facts are free-text names over `Int`, `Bool` or `Text` values
   (`qip-reasoning-engine/src/rules.rs`); the IR's inputs are `FeatureKey`s
   with a declared `Type`. The mapping is a committed table binding each fact
   name to one catalogue feature and its type, and the translator refuses
   rather than guesses: a fact with no binding; any `Text` value; a `Present`
   test (the IR has no notion of defined-ness, only that an undefined input
   emits nothing); an `Int` bound to `Exact` without a stated minor-unit
   exponent in the binding; a negative `Int` bound to `Count`. `And`, `Or` and
   `Not` map to the IR's logical operators. A rule that reads an absent fact
   does not fire and an undefined IR input emits nothing, so the two semantics
   agree and the translation preserves the "never fires on nothing" property.
   The output is a `StrategySpec` handed to the challenger path
   (`qip-evolution`), which shadows it (ADR 0142) and walks it up the ladder
   under `AuthorisedPromotion`; it is never installed in a plan directly. The
   bound is the compiler's own: a candidate over the cost budget is refused by
   `StrategyCompiler`, and the translator adds no second budget. It lives in
   `qip-kernel`, which already links the rule engine and `qip-strategy`; a
   service may not depend on the edge crate.

### B. What a promotion has to show

9. **A promoted model's score is attacked by a perturbation suite whose
   measure is the share of held-out rows whose decision flips** (R12-MODEL-4021).
   A boosted-stump step function can jump the whole output range under an
   arbitrarily small move, so no Lipschitz-style tolerance exists to compare
   against and "output movement within a fraction of range" would refuse the
   registered challenger on its first threshold. What a step function's jump
   costs downstream is a changed *decision*, since a promoted model is an
   advisory input to a deterministic filter at a stated threshold (the same
   `decision_threshold` `distil` takes). So the suite, in
   `Platform::promote_model_from` beside the verifier society, scores each
   held-out row with `ServedModel::score`, moves one feature at a time by a
   fixed fraction of that feature's `training_bounds` width in each direction
   (staying inside the bounds, which the card already records and the cell
   already enforces as an extrapolation refusal), and counts rows whose score
   crosses the threshold. The same suite runs on the classical baseline over
   the same rows (ADR 0006). The candidate is refused when its flip share
   exceeds a ceiling the suite's policy states. The initial constants are
   stated here as model-acceptance parameters: a perturbation of one hundredth
   of each feature's bounds width, and a flip-share ceiling of one tenth of
   the held-out rows. **They are not asserted to be right.** The suite first
   ships measuring only: the share for each registered model is journaled on
   the `ModelVerified` record. Refusal is switched on in the same slice only
   if the registered stump challenger's measured share is under the ceiling; if
   it is not, the ceiling is raised by an amendment to this record that names
   the measurement, and the test is not weakened by exempting a model class.
   A refused promotion is authority not added, as with every critic.

10. **"Stressed regime" is defined from the labels the platform already
    classifies, and a stress claim needs a stress validation, not a stress
    calibration on every baseline** (R12-MODEL-6074). The register says the
    training data carries no regime label. The desk's scoreboard does: every
    round is keyed by `Platform::regime_context`, a `market/volatility` pair
    from `MarketRegime` and `VolatilityRegime`, and every registered card
    already records the regime it was judged in (`validated_regimes`), which
    `ModelRegistry::ineligible_in_regime` reads. A regime is *stressed* when
    its market component is `crisis` or `illiquid`, or its volatility
    component is `high` or `extreme`. The rule is then narrow: a candidate may
    not be promoted *on a comparative claim about a stressed regime* (that it
    beats the baseline there) unless the baseline's card lists that regime in
    `validated_regimes`; and a baseline whose card does not list a stressed
    regime is, in that regime, already ineligible by the existing check, which
    this record leaves exactly as it is. No baseline is refused registration
    for lacking a stress calibration, because that would refuse every baseline
    registered today and add nothing the per-regime eligibility does not
    already say.

11. **Regime drift gets no second column; the regime's change is already an
    event** (R12-TICK-2071). `StreamingDrift` runs over the five model
    features, of which `volatility_10` and `momentum_5` are the
    regime-sensitive ones; the platform's regime is classified from price
    history, and a drift test on a second series derived from the same prices
    would report the same fact twice under two names. The regime *change* is
    already consumed: `latest_regime_crossing` gates promotion and the
    regime-boundary suite pins it. Venue change is routed
    (`TickCampaignDetector::observe_venue`, which opens a `NewVenue`
    campaign). No execution-model residual drift is built because no execution
    model exists to drift; the twin's error against fills is recorded per
    venue as `qip_venue_fill_error_bps` and is read by nothing that decides.
    The register states that.

### C. The tournament forecaster contract

12. **A tournament entrant is a function that exists and answers the question,
    or it is not an entrant** (R12-MODEL-3089, R12-MODEL-3094,
    R12-MODEL-7198). A branch proposal is a hypothesis record and submitting
    forecasts for it would submit forecasts no model made. A temporary
    specialist is admitted only as the triple (an in-tree lattice
    `ModelFamily` member, the entity's subject, an expiry): the member is the
    forecasting function (`Martingale`, `Drift` and the named specialists
    already turn the NOW state into a distribution), the subject is the
    branch's, the expiry is its own, and the id is `branch:<branch id>/<family>`
    so a record names which branch it came from. It enters as
    `ForecasterKind::Temporary` through the same `admit` and `submit` every
    entrant uses, so the cap `MAX_FORECASTERS` and the eviction of expired
    temporaries are the only bound and no second number is invented. The
    residual that spawns it must be a measured one: the production caller of
    `WorldModelSpawner` is not built until a producer of the residual exists
    in the kernel (the spawner's input is an unexplained residual of P&L, and
    none is computed). A population of thousands is not a goal; it is whatever
    the measured residuals and real forecasting functions produce, and the
    register states the live entrants as the detector class and the panel
    analysts until then.

13. **An unmetered entrant is recorded as unmetered, not as free**
    (R12-MODEL-3090). A detector's hypothesis is formed at the pass instant, so
    a measured latency is zero by construction, and a wall-clock reading would
    not replay. The analyst's run duration is a recorded fact (start and finish
    are in its run record), so it replays as data; the detector has no such
    fact. The meter is therefore: `TournamentForecast::cost` and `latency`
    become optional, the detector's are `None`, and the influence arithmetic
    computes the cost and latency factors only among entrants that have a
    value, treating an absent one as the neutral factor and not as the best.
    When a counted-work meter is wanted, it is a deterministic step count the
    detector function increments (evidence items examined), recorded on the
    hypothesis, used as an ordinal rank within the lane and never converted to
    money or microseconds. Whether the aggregate may feed a decision: not
    now. The tournament is advisory by its own documentation, and under ADR
    0125 any future use narrows and never enlarges. The condition for a
    narrowing use is a measurement: the influence-weighted aggregate must beat
    the unweighted one on resolved questions, scored out of sample.

14. **The join between a strategy and a tournament forecaster is the id
    `strategy:<StrategyId>`, reserved now and not yet admitted; promotion
    does not require a tournament win** (R12-MODEL-9030). A strategy's
    `Signal` does not state above or below, and the tournament question is
    directional, so mapping `Enter` to "up" would invent a side. A strategy
    becomes an entrant only when its shadow output states a direction and a
    probability as a `Forecast` (ADR 0140). Promotion stays on its one path:
    the deflated-Sharpe challenger bar and the gate outcomes, then
    `AuthorisedPromotion`. A tournament win as a second gate would add a second
    authority over capital that the one-path property is built to avoid, and
    the tournament's own scores are not yet shown to predict live outcomes.
    Until then a strategy's tournament record, once it has one, is attached to
    the promotion rationale as read-only context.

### D. Lifecycle bound and versions

15. **The lifecycle ledger and the factory's candidates are bounded by
    compacting the retired and refusing the new, never by evicting the live**
    (R12-MODEL-1159). `LifecycleLedger` keeps every move of every strategy that
    was ever moved in a `BTreeMap`, and `StrategyFactory` keeps every candidate
    ever registered. A retired strategy is terminal by standing test, so its
    detailed history is a record the event log holds and the ledger need not:
    on retirement the ledger replaces the strategy's entries with a tombstone
    (the id, the path of stages with instants, the reason, a digest of the full
    history). `stage_of`, `path` and the refusal to re-propose an id keep
    working from the tombstone, which is the only part of a retired strategy
    that has a live consumer. Live (non-retired) strategies are bounded by
    `MAX_LIVE_STRATEGIES`, set equal to the tournament's `MAX_FORECASTERS` so
    the two registries are sized to one order; `register` refuses a new
    candidate at the bound with a message naming retirement as the way to make
    room. Nothing live is evicted, because a ledger that dropped a
    capital-holding strategy's record would hide it. The cap bounds records, not
    exposure; capital caps are the corridors' and the envelopes' and are not
    touched. The 1,000-strategy promote/demote test the row asks for is then a
    measurement of a bounded structure.

16. **A new version of a strategy is a new candidate with a recorded
    ancestor; there is no rollback, and no resurrection**
    (R12-MODEL-6009). Both halves are already the code's: `register` refuses a
    registered id ("a revised strategy is a new candidate under a new identity,
    so its evidence is re-earned rather than inherited") and `record_promotion`
    refuses a retired one. This record confirms them as standing decisions and
    adds one field: a candidate may carry `succeeds: Option<StrategyId>`, set by
    the generator or by an operator, journaled at registration, read by
    reviewers and by nothing that decides. It inherits no evidence and shortens
    no rung. A restore to any rung that holds capital is a walk up the ladder
    under `AuthorisedPromotion`, including the dual approval every
    capital-holding rung requires; `rollback_strategy` is not built, because
    its only distinct behaviour would be to skip that walk.

### E. A field is added without moving a digest

17. **An added field enters a digest or a signing payload only when present,
    under a version tag, and an old digest reads as "field absent"**
    (R12-REFLEX-5040, with decision 3). This is the rule `ModelPack`'s
    `inline_model` and ADR 0088's slots already follow, written once. For
    `DistilledModel`: add `calibration: Option<Calibration>` (the affine
    scale and offset `qip_training::local::Calibration` fits), `serde(default,
    skip_serializing_if)`, and append `|calibration/v1|<scale bits>|<offset
    bits>` to `digest()` only when it is `Some`. Every model digest, every
    manifest and every pack that names one is byte-identical after the change,
    because the constructors `linear` and `tree` produce `None`, and the "over
    a hundred sites" cost is not paid: the fields are private and a new
    constructor argument is not needed. A distilled model gains a calibration
    through a new `with_calibration` that returns a differently-digested model,
    so the cell's manifest check, which compares digests, names the calibrated
    and uncalibrated model as different things. `distil` sets it from the
    teacher's calibration; the cell applies it after the raw score and before
    any threshold. The kernel manifest producer reaching nodes is the signed
    release of decisions 1 to 4 and nothing separate.

18. **A synthetic episode records the input digest always and a seed only when
    something random was drawn** (R12-EXPAND-6033). The conditional scenario
    in `research.rs` is a deterministic function of an ambient signal and the
    last two prices of each affected subject: no stream is drawn, so a seed
    field would be a number nobody used. The record is `SyntheticEpisode {
    scenario name, generator (conditional, library, monte_carlo), input_digest,
    seed: Option<u64> }` in `qip-contracts`; `input_digest` is the SHA-256 over
    the canonical inputs (signal id, subject and the two prices as their
    bits); `seed` is `Some` exactly for a generator that takes an `Xoshiro256`
    (the Monte Carlo path), and a record naming a stochastic generator without
    a seed, or a deterministic one with a seed, is refused at construction. It
    is written where the scenario is generated or run on the default path
    (`research.rs` for the conditional scenario), and never by the
    evolution backtester, which replays recorded bars and would label a
    historical replay as synthetic.

19. **The reasoning engine's `BeliefState` is renamed `BeliefClock`**
    (R12-CONTRACT-7063). The type is a last-updated instant and a count; it has
    no probability, TTL or uncertainty, and a conversion to the contract's
    six-field `BeliefState` would invent four of them. The rename covers the
    struct, its re-export in `qip-reasoning-engine/src/lib.rs`, and the
    `ReasoningEngine::beliefs()` accessor (to `belief_clock()`). It moves no
    wire; the contract keeps the name. What crosses to cells is ADR 0088's
    `Beliefs` slot beside `BeliefPriors`, produced from the contract
    `BeliefState` built in `central/belief.rs`; this record adds nothing to
    that and does not change the receiver.

## Consequences

- A model reaches a cell by one path with one signed type, and the three
  records that were named alike (kernel pack, contract pack, two belief states)
  stop being named alike.
- The register can say, for 7064, 7091, 4011, 6052, 5041, 3089, 3094, 7198,
  3090 and 9030, what is decided and what stays unbuilt, without any of them
  reading as a defect a kernel patch would close.
- Added fields stop being negotiated row by row: decision 17 is the rule.
- The first run of the perturbation suite measures instead of refusing, so a
  wrong constant costs a recorded number and not a blocked release.

## What it costs

- A `qip-cli` subcommand, a draft writer and a rename that touches the kernel's
  tests (`serving.rs` uses `pack_membership`); the rename is mechanical but
  wide.
- Decision 13 changes the type of two fields on `TournamentForecast`, so every
  producer and every test that builds one is touched.
- The perturbation constants are guesses until measured, and a measuring-only
  first run delays the refusal it exists for.
- Tombstones keep one small record per retired strategy forever; the id
  reuse refusal depends on it. The cost is linear in retirements, not in moves.
- Decision 14 leaves strategies out of the tournament for now, so the
  comparison the row hoped for is not available until strategies state a
  direction.

## What would make this wrong

- A model named to a cell with no admitted pack covering its artifact and
  inline model (the failure decision 1 exists to keep out), or an unsigned
  draft that `admit_packs` accepts.
- A promoted model whose measured flip share under decision 9 exceeds the
  ceiling in production while the suite is measuring only: that would mean
  the constants were chosen and nobody read the journal.
- A stress claim promoted against a baseline whose card did not list the
  stressed regime.
- A temporary entrant admitted whose subject, family or expiry cannot be
  read from its id and its record, or a `WorldModelSpawner` caller with no
  measured residual.
- A tombstone that fails to refuse the re-proposal of its id, or a live
  strategy's record dropped to make room.
- A digest or signature of an existing artefact changing after decision 17's
  change lands: any manifest, pack or plan pinned before it must verify
  after.

## Dissent

**The perturbation suite should not exist, or should be a Lipschitz bound.**
The strongest form: a step function's behaviour under perturbation is a
property of where its thresholds happen to sit relative to the sample, so a
flip share is a measurement of the held-out sample as much as of the model,
and a ceiling picked without data will either pass everything or refuse the
challenger, which is the failure the row itself warns about. A Lipschitz
constant, by contrast, is a model-independent guarantee. It lost because a
step function has no finite Lipschitz constant, so the bound would be a
property of the model *class* and would refuse the stump on arrival; the
decision-flip share measures the thing a downstream filter pays for. The
objection about the constants is right and is why the suite ships measuring
first. It would be revived by a measured flip share that does not separate
models known to be fragile from models known to be robust, in which case the
suite is noise and decision 9 should be withdrawn, not tuned.

**Every baseline should carry a stress calibration, and a promotion in a
stressed regime should be refused without one.** The strongest form: stress
is where models fail, a baseline never scored there is a comparison point
with no experience of the day that matters (the `MarketRegime::Quiet`
documentation says this in as many words), and refusing every baseline is a
feature that forces the desk to earn stress data. It lost because the
per-regime eligibility check already makes an unscored baseline ineligible in
that regime, so the extra refusal would stop registration of baselines that are
fit for the regimes they were scored in, for no gain in the stressed regime
that the existing check does not already give. It would be revived by finding
a stressed-regime decision taken on a baseline the eligibility check passed.

**A strategy should have to win the tournament to be promoted.** The
strongest form: the tournament scores calibration and information against
resolved outcomes in regimes, which is exactly the evidence a promotion should
rest on, and keeping it advisory wastes it. It lost because a strategy is not
yet an entrant (decision 14), because a second gate over capital weakens the
one-path property, and because the tournament's weights are not yet shown to
predict anything live. It would be revived by a measured predictive record for
tournament standing against realised outcomes, and an amendment that adds the
gate as an additional *refusal*, never a substitute for the existing ones.

## Code it constrains

- `qip-kernel/src/model_serving.rs`: `ModelPack` (renamed `ModelSetVersion`),
  `pack_membership`, `Platform::model_pack_lineage`, `promote_model_from`,
  `model_manifest_for`. Locate with
  `grep -n 'pub struct ModelPack\|pub fn pack_membership\|pub fn promote_model_from\|pub fn model_manifest_for' backend/crates/runtime/qip-kernel/src/model_serving.rs`.
- `qip-kernel/src/model_trust.rs`: `admit_packs`, `Platform::admit_model_packs`.
- `qip-contracts/src/policy.rs`: `ModelPack::signing_payload`, `InlineModel`;
  `qip-contracts/src/control_pack.rs`: `ControlPackAnnouncementFields`;
  `qip-contracts/src/trust.rs`: `Publication`, `ArtefactClass`.
- `qip-cli` (`backend/crates/apps/qip-cli`): the missing `model-pack`
  subcommand; `.github/workflows/model-pack-publish.yml` is its caller and is
  not edited here.
- `qip-edge-node/src/strategies.rs`: `StrategyInstaller`;
  `qip-strategy/src/model.rs`: `DistilledModel::digest`;
  `qip-training/src/distill.rs`: `distil`, `FidelityPolicy`.
- `qip-kernel/src/central/models.rs`: `require_calibrated_baseline`;
  `qip-ai/src/registry.rs`: `validated_regimes`, `ineligible_in_regime`,
  `training_bounds`; `qip-deepbrain/src/learning.rs`: the scoreboard keyed by
  `regime_context`.
- `qip-kernel/src/model_tournament.rs`: the detector entrant's `cost` and
  `latency`; `qip-learning-engine/src/tournament.rs`: `TournamentForecast`,
  `ForecasterKind`, `MAX_FORECASTERS`; `qip-world-model/src/spawning.rs`:
  `WorldModelSpawner`.
- `qip-lifecycle/src/ledger.rs`: `LifecycleLedger`, `record_promotion`,
  `retire`; `qip-kernel/src/central/factory.rs`: `StrategyFactory::register`.
- `qip-reasoning-engine/src/belief.rs`: `BeliefState`;
  `qip-deepbrain/src/research.rs`: `ConditionalScenario`.

## Rows unblocked

- **R12-CONTRACT-7064**: no split; the release path already feeds the kernel
  (`admit_model_packs`); rename the kernel type; build the `qip-cli` signing
  verbs and the unsigned draft.
- **R12-CONTRACT-7091**: the announcement is the release's rollout record from
  the reviewed release file; the kernel consumes it read-only and only to
  narrow; acknowledgements are `PolicyApplied` records.
- **R12-MODEL-4011**: no teacher exists for the NOW filter; restate; an ambient
  distillate waits for a fitted `TrainedTeacher`.
- **R12-MODEL-4021**: perturbation suite on decision-flip share, measuring first,
  baseline run beside it.
- **R12-MODEL-6074**: stressed regime defined from `regime_context`'s enums; a
  stressed comparative claim needs the baseline's `validated_regimes` to list it;
  no blanket refusal.
- **R12-REFLEX-5040**: calibration on `DistilledModel` as an optional, tagged,
  append-only digest part; no existing digest moves.
- **R12-EXPAND-6052**: lane derived from compiled cost; no budget field;
  `WarmService` not built; publication still waits on the signer.
- **R12-REFLEX-5041**: a detector is a compiled `Enter` rule; the manifest is a
  narrowing filter built when both ends are; restate until then.
- **R12-MODEL-4012**: rule-to-candidate translator with a committed binding table
  and enumerated refusals, output a shadowed candidate.
- **R12-TICK-2071**: no second regime column; restate.
- **R12-MODEL-3089**: a temporary entrant is an in-tree family member over a
  measured branch subject with an expiry; no spawner caller until a residual
  exists.
- **R12-MODEL-3090**: unmetered is `None`, not zero; counted-work meter defined as
  an ordinal step count; the aggregate stays advisory.
- **R12-MODEL-3094**: as 3089, and the `branch:` id form.
- **R12-MODEL-7198**: as 3089; population size is not a goal.
- **R12-MODEL-9030**: `strategy:<id>` reserved; not admitted until a strategy states
  a direction and probability; no tournament gate on promotion.
- **R12-MODEL-1159**: tombstone on retirement, refusal at `MAX_LIVE_STRATEGIES`,
  nothing live evicted.
- **R12-MODEL-6009**: new version is a new candidate with `succeeds`; retirement
  stays terminal; no `rollback_strategy`.
- **R12-EXPAND-6033**: `SyntheticEpisode` with an input digest always and a seed
  only for a stochastic generator.
- **R12-CONTRACT-7063**: rename the engine type `BeliefClock`; no wire change;
  ADR 0088 stays the receiver's decision.

## Not decided here

- **PROTECTED**: the publish workflow emitting the announcement and calling the
  CLI verbs is an edit to `.github/workflows/model-pack-publish.yml`; the need
  is described in decisions 1 and 4 and the edit is a person's. Setting the
  packs and announcement paths for a deployed service is an environment tfvars
  edit (`infrastructure/environments/*`). Neither is made here (R12-CONTRACT-7064,
  R12-CONTRACT-7091).
- **PROTECTED / BILLED**: installing a KMS-backed `PackageSigner` in the deep
  brain needs the signing identity and key grants ADR 0131 declares; the key
  and its monthly charge are the owner's plan, not this record's
  (R12-EXPAND-6052).
- **RISK (borderline, flagged)**: the perturbation fraction, the flip-share
  ceiling (decision 9) and `MAX_LIVE_STRATEGIES` (decision 15) are
  model-acceptance and working-set constants, not exposure caps, and are decided
  here on that reading. If the owner reads a ceiling that refuses a promotion as
  a conduct bound, the *values* are theirs and the structure stands
  (R12-MODEL-4021, R12-MODEL-1159).
- **PAPER-BOUNDARY**: nothing here touches an order path. Every artefact named
  is advisory or shadowed until promoted by the existing ladder; the three
  layers are untouched. Stated so the absence is a finding and not an omission.
