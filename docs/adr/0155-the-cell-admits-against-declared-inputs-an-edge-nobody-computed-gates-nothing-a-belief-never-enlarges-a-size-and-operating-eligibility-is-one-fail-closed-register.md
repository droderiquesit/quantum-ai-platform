# ADR 0155: The cell admits against declared inputs, an edge nobody computed gates nothing, a belief never enlarges a size, and operating eligibility is one fail-closed register

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises the structural choices below. It does
  not authorise any risk limit value, any billed resource, any edit to a
  protected path, or anything near the paper-trading boundary; those are listed
  under "Not decided here" and are untouched.
- **Date**: 2026-10-19
- **Extends**: ADR 0126 (the signed envelope is the one downward capital
  authority and gains fields additively), ADR 0125 (an advisory input only
  narrows), ADR 0088 (a belief's slot is versioned and no cell sizes on its
  mean), ADR 0130 (the simulated venue observes settlement).
- **Keeps**: ADR 0003, ADR 0005 (confidence is arithmetic, never assigned),
  ADR 0008 (cells decide alone), ADR 0021, ADR 0062 (a venue is withdrawn only
  on feasibility evidence), ADR 0108, ADR 0109 (no dependency is added).

## Context

Thirteen register rows are PARTIAL for one reason in thirteen costumes: a
control wants an input and nothing in the tree produces it. The repository has
two bad answers to that and has been bitten by both. Wire the control anyway
and the missing input reads as an empty value: either the gate refuses every
order (`admit_bounded` with no product class to pass) or it admits every order
(`MaxExpectedShortfall`, shipped for a long time with nothing to fire on).
Leave the control unwired and it is a line in a module comment. This record
gives each input a declared source and a stated reading of its absence, and
where no honest source exists it says so and builds nothing.

Reading the code against the rows changed four of them.

1. **A leverage measure exists, twice.** `CapitalEnvelope::check_exposure`
   (`qip-contracts/src/capital.rs`) measures gross after the order over
   `capital` less realised loss, and the cell enforces it through
   `exposure_verdict` in `qip-edge/src/cell.rs`. `AuthorityBounds::max_leverage`
   documents a different ratio, gross "over the envelope's gross limit", which
   the money limits already cap at one, so as written it could not fire. Row
   GOV-6120 asks for a measure to be defined; it was defined and then defined
   again differently.
2. **A regulated-function vocabulary exists.** `Cell::send` already receives
   the `ExecutionMode` an order performs and consults `ModeGate` with it, and
   the node enables two modes under the recorded jurisdiction `"paper"`
   (`qip-edge-node/src/lib.rs`, the EXEC-020 block). `ExecutionMode` has no
   `as_str`.
3. **`DexVenue` is more than the row says.** `DexVenue::feasibility_model` in
   `qip-brokers/src/dex.rs` already derives the venue feasibility the cell
   reads, and `VenueModel` in the cell already carries `VenueClass::
   DecentralisedExchange` with a mandatory gas floor. What is missing for
   COVERAGE-5103 is not a DEX on the order path.
4. **"The eligibility register" is three things.** The compliance
   `EligibilityRegister` (venue, product, entity, jurisdiction, strategy: may
   this operator trade here), `qip-capital`'s user `EligibilityRegistry` (may
   this user be funded) and `ProductCatalogue` (may a strategy family be
   offered to a user in a jurisdiction) answer different questions. Only the
   first is the one GOV-6118 wants on the promotion path.

Locate them with
`grep -n 'pub struct AuthorityBounds\|pub fn admit_bounded\|pub fn check_exposure' backend/crates/libs/qip-contracts/src/capital.rs`,
`grep -n 'fn exposure_verdict\|ModeGate\|mode_gate' backend/crates/edge/qip-edge/src/cell.rs`,
`grep -n 'pub fn feasibility_model' backend/crates/services/qip-brokers/src/dex.rs`,
`grep -rn 'EligibilityRegister' --include=*.rs backend/crates | grep -v /tests/` and
`grep -n 'fn product_refusal' backend/crates/runtime/qip-kernel/src/platform.rs`.

## Decision

### A. The rule every input below follows

1. **A control reads an input from a declared source, and its absence has one
   of two stated meanings.** Where the control is armed (a `CellConfig`
   switch, in the register of `with_exposure_bounds_required`) an absent input
   refuses, naming the input. Where the control is not armed it does not
   exist: no default makes it pass or fail. A gate whose input has no producer
   is not built, which is what `feasibility.rs`'s module doc already says of
   the fee floor on directional intents.

### B. GOV-6120: what `admit_bounded` is given

2. **Product class is `AssetClass::as_str()` of the instrument's class**
   (`qip-financial/src/asset_class.rs`), fifteen values, spelled exactly as
   `AuthorityBounds::product_classes` already stores them. The cell holds
   strings and gains no `qip-financial` edge: `CellConfig` gains a
   `product_classes` map from `ObjectId` to class, built by the composition
   root from the committed catalogue record's `instrument_type.asset_class()`,
   next to the per-instrument `Granularity` map `VenueModel` already holds. An
   instrument with no entry, under an armed cell, is refused under a new gate
   `envelope_product`, never read as "the first class on the list".
3. **The regulated function is the order's `ExecutionMode`**, passed as
   `Some(mode.as_str())` using a new `as_str` on `ExecutionMode`. It is the
   value `Cell::send` already derives; no second vocabulary is invented.
4. **There is one leverage measure**: gross committed after the order over
   `capital` less realised loss, as `check_exposure` computes it. The cell
   computes it once and passes that figure to `admit_bounded`. When an
   envelope carries both blocks their `max_leverage` must be equal, refused at
   construction, so there is one claim about one ratio; `AuthorityBounds`'
   field documentation is corrected to say what is measured. Definition only:
   no value is chosen.
5. **The signer is the envelope's existing class.** Bounded envelopes are
   issued by `EnvelopeIssuer` under `ArtefactClass::Envelope` and countersigned
   through `ApprovalChain::grant` (a human who is not the requester, a fresh
   credential, two humans above the dual threshold). `CapitalRequest` gains
   the authority block beside `exposure`, so the payload comparison in
   `CentralPlane::issue_narrowed` covers it and an approval for a bounded grant
   cannot be read as one for the same money unbounded. Under ADR 0110 the
   signature becomes a KMS signature without this record changing. No new
   signer, no new class.
6. **A new switch arms the cell**, `CellConfig::with_authority_bounds_required`,
   off in the library, armed by `qip-edge-node`'s assemble only once the issuer
   states the block in every envelope it signs and the node can build the
   class map for every instrument it may trade. Arming earlier is the
   refuse-everything failure the register notes call out. Rollout order:
   issuer, then the node's map, then the switch.

### C. WORLD-9023 and RISK-5044: an edge is computed or it gates nothing

7. **`Signal` gains an optional expected edge**, `expected_edge_bp`, a statistic
   in basis points of notional, with `#[serde(default, skip_serializing_if =
   "Option::is_none")]` so a signal that states none encodes as it always did.
   It is produced by `qip-strategy`'s runtime from a rule expression typed
   `Statistic`, exactly as conviction is, and only for a `Rule` whose author
   declared one. A rule without one raises a signal that states none. Nothing
   derives an edge from conviction: conviction is a probability that the claim
   holds, a payoff is a different quantity, and a product of the two with a
   payoff nobody declared is the number nobody computed. The `f64` is checked
   finite at the cell, refused under its own gate beside
   `GATE_SIGNAL_CONVICTION`, because a `NaN` survives a clamp.
8. **The net-edge gate exists only for a signal that states an edge.** In the
   cell, after the intent is formed and before it joins a net, it compares the
   stated edge with the cost the cell has measured for that intent, which is
   `feasibility::fixed_cost_fraction` (fee floor plus gas floor over notional)
   and nothing more. Slippage and impact are not subtracted because the cell
   computes neither for a directional intent; that makes the gate optimistic,
   and the margin below exists for that reason. It is `CellConfig`-armed with a
   margin that has **no default**: unset, the gate is not built and behaviour
   is today's. It is a cell gate with its own constant, like
   `GATE_CONDUCT_SPOOFING`, and is **not** added to
   `qip_contracts::feasibility::EDGE_GATES`: a refusal in that array enters the
   window a venue is withdrawn on (ADR 0062), and a strategy's thin edge is not
   evidence about a venue. A refusal outside the array is counted under
   `other` at the centre and seated nowhere.
9. **A second switch decides whether an edgeless directional signal is
   refused.** Off: it passes, and a gauge counts such signals per pass so the
   desk can see how much trading the gate does not judge. On: it is refused
   under the net-edge gate, naming the strategy. Turning it on today would stop
   every strategy in the tree, so it is a deliberate act with a RISK value
   attached (below).
10. **A realised outcome does not return to the cell.** The cell sees a fill,
    which is a venue fact about quantity and price, and has no mark it trusts
    for every instrument (ADR 0126 says so of drawdown). The signal and its
    stated edge are journaled (`Decision::SignalRaised`, which gains the field
    additively); the centre joins the realised result to the signal through
    attribution lineage (ADR 0007) and scores forecast skill in LEARN. Cells
    decide alone (ADR 0008): a cell adapting to a centre-computed outcome is a
    centre input and the policy payload is its channel. A tally of conviction
    against fill is execution, not calibration, and stays unnamed as such.
    Outcome storage is the event log; no new store.

### D. REFLEX-5030: a belief prior does not enter a cell's confidence

11. **No cell reads a belief value, and none will until ADR 0088 is built and
    Brier-scored.** `belief_priors` and `opportunity_definitions` are read by no
    cell code (`grep -rn 'belief_priors' backend/crates/edge
    backend/crates/apps/qip-edge-node --include=*.rs | grep -v /tests/` prints
    nothing, 2026-10-19). A blend of a prior into `Conviction` is arithmetic
    ADR 0005 says is never assigned. The only admissible future use is a
    multiplier in `(0, 1]` compounded into the pass's caps by ADR 0125's rule,
    with a calibrated source and a schedule that its own ADR fixes. ADR 0133's
    phrase "as it reads `belief_priors`" is read as freshness only, which is
    what the cell consumes today (`PolicyItem::capability` to
    `Capability::BeliefState`).
12. **Which detector an opportunity id names** is not decided here: ADR 0149
    makes the manifest a narrowing filter over compiled `Enter` rules. This
    record only adds that the cell must keep ignoring the slot's values until
    that producer and reader both exist.

### E. EXEC-1068: requote layering is a measure, not a count

13. **Counting requotes convicts ordinary market making** (`conduct.rs`'s
    module doc, and the register notes). The layering measure is defined on
    different facts: requotes that move an order **away from the touch** and
    whose order is then withdrawn unfilled, keyed by venue, instrument and
    side as `ConductWatch` is, judged against an **opposite-side fill** inside
    the same window. A requote that stays at or inside the touch, or one the
    venue or a halt forced, never counts. The window, the minimum and the ratio
    are not chosen here and have no default; with none declared the gate is not
    built and the spoofing gate is unchanged. The desk's order path in
    `qip-execution-engine` is a separate surface and is not covered by this
    decision.

### F. RISK-4101: build identity is signed policy; a credential has no paper source

14. **Deployment identity** reaches the node as a set of acceptable build
    identities in a new signed policy item, added on ADR 0088's versioning
    path (a new `PolicyItem`, produced-only digest, consumer before producer).
    A set, because a rollout runs two builds at once. The node reads its own
    identity from a file the unit that starts it writes, not from the network,
    in line with "secrets and facts reach the process as files". A cell whose
    own identity is absent from a produced set takes no new exposure, under a
    new halt source `deployment`, written like `journal`: absent, not zero, on
    a cell built without the wire. It narrows only: cancels continue, and the
    release is a fresh payload whose set names the cell's build. An unproduced
    item changes nothing, which is today's behaviour.
15. **Credential expiry has no source on a paper platform and is not built.**
    The simulated gateway authenticates against references to requirements, no
    secret material (`SimulatedGateway::with_settings`), and the venue
    credential is readable nowhere (ADR 0003). The cell does not read Secret
    Manager. When a real-venue connection is accepted under ADR 0108, the shape
    is a per-venue `not_after` instant in signed policy, compared with the
    cell's clock, withholding that one venue the way `GATE_CLOCK_SKEW` does.
    Recorded as unreachable, not pending.

### G. CONTRACT-7073: the manifest is built at replay, from the settings

16. **Assumptions are derived from `ExchangeSettings`, never written as
    prose.** `SimulatedGateway` gains one function that renders
    `ReplayAssumptions` from the settings it was built with (clock, latency and
    jitter, queue, fill, impact, with "none" stated where the setting is zero).
    The `PassMarker` topic and its schema lock are untouched. A
    `ReplayManifest` is constructed by the replayer from a recorded marker and
    that function's output; nothing is recorded per pass. To make that safe,
    `config_digest` in the marker (a value, not a schema) is widened to cover
    the gateway's settings, so a manifest built under different settings
    refuses on a digest mismatch instead of reproducing under a fill model
    nobody can name. No topic is added, which keeps ADR 0100's six bindings.

### H. Four restatements

17. **COVERAGE-5103: `DexVenue` stays a model.** A digital-asset order uses the
    same simulated gateway as every class. `DexVenue::feasibility_model` feeds
    the cell's feasibility seam; the class record already says T+0 exchange
    settlement for `DigitalAsset`; ADR 0130's observed-settlement record is the
    mechanism. `qip-kernel` gains no `qip-brokers` edge: nothing in the kernel
    needs an adapter, and an adapter trait with `submit_order` in the kernel is
    a second place the paper boundary must hold. Any simulated finality depth
    for a chain venue is an operator declaration (below).
18. **COVERAGE-7163: Prediction and PhysicalProduct get no `InstrumentType`.**
    Physical goods are commerce units, not instruments; a prediction market is
    a venue class (`VenueClass::PredictionMarket`) with no catalogue record and
    no licensed price source. The composite closes on what is constructible:
    the acceptance suite assembles `FixedIncome`, `Credit` and `Derivative`
    with fixtures that carry a stated maturity or extension record, as
    **test inputs**, not catalogue data.
19. **RISK-4107: the committed universe stays equities.** No option, future,
    FX, rate, commodity, volatility or event-contract candidate is declared
    until a catalogue record carries a price printed on the simulated exchange
    and a sensitivity that is stated by the instrument's own record or computed
    from printed prices by an in-tree pricer. A vendor-supplied sensitivity is
    a data subscription and BILLED.
20. **EXEC-5071: a generic REST adapter declares no native instruction.**
    `native_extensions` defaults to empty and the `submit_native` door refuses
    what an adapter did not declare; `RestOrderEntryAdapter` inherits both. The
    clause "each adapter exposes its native extension" is met by the declaration
    being truthfully empty, pinned by a test, rather than by inventing a native
    feature for a wire that has none.

### I. GOV-6116 and GOV-6118: one operating register, a paper seed, fail closed

21. **`qip_compliance::eligibility::EligibilityRegister` is the one register of
    operating permission** for venue, product, strategy, jurisdiction and
    entity. The user registry and `ProductCatalogue` remain, as records of
    *whom* a product may be offered to; neither is asked whether the operator
    may trade a venue or deploy a strategy. The kernel holds no eligibility
    table of its own for that question and reads the compliance register.
22. **The strategy subject is the strategy family**, the key
    `Platform::strategy_family` already returns, because strategy ids are minted
    by the factory and a record per id could never be written ahead of time.
23. **The gate is fail-closed and consulted at three places**:
    `StrategyFactory::promote` and the learning promotion path
    `promote_strategies_with_evidence` call `admit_deployment`, and the
    composition root of `qip-edge-node` calls `admit_enablement` for every
    venue it passes to `CellConfig::with_venue`, refusing to start otherwise,
    which is the composition-root rule. A subject with no record is ineligible.
24. **A committed seed makes the gate admit the simulated venues and nothing
    else.** A file beside `data/datasets/universe.json` holds one record per
    simulated venue, per asset-class product the registry ships and per
    strategy family, in the single jurisdiction `"paper"`: the literal the node
    already records for its mode gate, for the reason it gives. The grantor
    names this record as the authority and each record has an expiry the
    committing operator states, because the register refuses a record that
    never expires. A test asserts every seed record's jurisdictions are
    exactly `{"paper"}`; a seed that names a real jurisdiction fails the build.
    No real-venue or real-jurisdiction eligibility is recorded, and a platform
    pointed at one is refused as it should be.

## Consequences

- Five gates become buildable without a refuse-everything first release:
  authority bounds, net edge (for edge-bearing signals), requote layering,
  deployment identity and eligibility. Each is unarmed until its value or
  producer exists, and none changes behaviour before then.
- The register's wording changes for every row (see "Rows unblocked"). Three
  gap texts are corrected by the code: GOV-6120 (a leverage measure and a
  function vocabulary already exist), COVERAGE-5103 (`DexVenue` already feeds
  the feasibility seam) and GOV-6116 (the "register" is three records answering
  three questions).
- Code slices: the class map, mode `as_str`, shared leverage figure and
  `envelope_product` gate with the issuer's authority block (GOV-6120); the
  signal field, rule expression and net-edge gate (WORLD-9023, RISK-5044); the
  deployment item and halt source (RISK-4101); the settings-derived
  assumptions (CONTRACT-7073); the REST and asset-class pins (EXEC-5071,
  COVERAGE-7163); the promotion and enablement gates with the seed (GOV-6116,
  GOV-6118). Every new test needs its mutation.

## What it costs

- `Signal`, `Rule`, `CapitalRequest`, `CellConfig`, `PolicyItem` and the
  gateway all grow. Each addition is optional and off the wire when empty, so
  no digest moves, but every reader of those types must honour the absence.
- A net-edge gate compared with fixed costs only will pass trades that lose to
  slippage. It is a floor, not a profitability test, and the margin is the
  only lever until the cell computes impact.
- The seed is a committed claim that simulated things are permitted. It is
  small and honest, and it is a file someone must renew; an expired seed stops
  every promotion at once, loudly, which is the fail-closed price.
- Widening `config_digest` changes its value for every node once, so replays
  recorded before the change refuse under the new digest.
- Edgeless signals go unjudged by design until the second switch is turned on.

## What would make this wrong

- If strategies in practice never declare an edge, the gate is dead code and
  the gauge of unjudged signals says so within a week of arming. Then
  conviction-only strategies are the platform's real population and the right
  answer is a payoff model owned by `qip-strategy`, not a guess here.
- If `exposure_verdict` and the authority leverage still disagree in a
  deployed grant after decision 4, the equality refusal was not applied at
  every issue site; the fix is the issuer.
- If the class map drifts from the catalogue (an instrument traded, mapped to a
  stale class), the armed cell trades a class its envelope does not authorise;
  the test that builds the map from the universe record is the guard.
- If a rollout ever runs three builds at once, a set of two identities is
  wrong and the item needs a version.

## Dissent

**Do not halt on build disagreement.** A cell decides alone (ADR 0008) and
holds a verified, signed payload; a centre that publishes the wrong identity,
or a stale one, would halt every healthy cell at once, converting a
publication bug into a fleet outage with no operator in the loop. Reading the
build set from the centre makes the centre a single point of stoppage. This
loses because the halt only narrows (cancels continue, nothing new opens) and
the failure it prevents is the opposite silent one: a cell running code the
centre never published, unnoticed, sizing and sending. A fleet that stops
opening exposure is recoverable by one signed payload; a fleet running an
unpublished build is recoverable only after someone notices. It would be
revived by a measured incident in which a mispublished set halted a fleet that
was in fact on the right build, with the halt's cost recorded in the log.

**Let a belief narrow a cell's confidence now.** A prior that says a subject is
doubtful is evidence a cell is wasting by not reading, and narrowing cannot
hurt. It loses because the prior is a point estimate (ADR 0088, fact 1), the
narrowing would need a schedule nobody has fitted (a number assigned, ADR
0005), and a cell that reads values makes the policy slot a second channel for
sizing beside the envelope. It would be revived by a Brier-scored, calibrated
belief slot and an ADR naming the multiplier schedule.

## Code it constrains

- `qip-contracts/src/capital.rs`: `AuthorityBounds`, `CapitalEnvelope::admit_bounded`,
  `check_exposure` (`grep -n 'pub struct AuthorityBounds\|pub fn admit_bounded\|pub fn check_exposure' backend/crates/libs/qip-contracts/src/capital.rs`).
- `qip-edge/src/cell.rs`: `exposure_verdict`, `CellConfig`, `Cell::send`'s
  `ModeGate` consult, `place_net` (`grep -n 'fn exposure_verdict\|pub fn with_exposure_bounds_required\|mode_gate' backend/crates/edge/qip-edge/src/cell.rs`).
- `qip-execution-engine/src/modes.rs`: `ExecutionMode`.
- `qip-capital/src/envelope.rs`: `EnvelopeIssuer`, `EnvelopeTerms`, `ExposureTerms`;
  `qip-compliance/src/approval.rs`: `CapitalRequest`, `ApprovalChain::grant`;
  `qip-kernel/src/central/plane.rs`: `CentralPlane::issue_narrowed`
  (`grep -rn 'fn issue_narrowed\|pub struct CapitalRequest' backend/crates`).
- `qip-contracts/src/signal.rs`: `Signal`; `qip-strategy/src/ir.rs`: `Rule`;
  `qip-strategy/src/runtime.rs` (the one production `Signal` constructor);
  `qip-edge/src/feasibility.rs`: `fixed_cost_fraction`;
  `qip-contracts/src/feasibility.rs`: `EDGE_GATES` (unchanged)
  (`grep -rn 'Signal {' backend/crates/edge/qip-strategy/src`).
- `qip-edge/src/conduct.rs`: `ConductPolicy`, `ConductWatch`; `Cell::spend_requote`.
- `qip-contracts/src/policy.rs`: `PolicyItem`, `PolicyPayload`, `slot_digests`;
  `qip-edge/src/telemetry.rs`: `halt` (`grep -n 'pub fn halt(' backend/crates/edge/qip-edge/src/telemetry.rs`).
- `qip-edge-node/src/gateway.rs`: `SimulatedGateway`; `main.rs`: `config_digest`;
  `qip-contracts/src/replay.rs`: `ReplayManifest::with_assumptions`.
- `qip-brokers/src/rest.rs`: `RestOrderEntryAdapter`; `adapter.rs`:
  `VenueAdapter::native_extensions`.
- `qip-compliance/src/eligibility.rs`: `EligibilityRegister::admit_deployment`,
  `admit_enablement`; `qip-kernel/src/central/factory.rs`: `promote`;
  `central/learning.rs`: `promote_strategies_with_evidence`;
  `qip-edge-node/src/lib.rs`: the mode-gate block.

## Rows unblocked

- R12-WORLD-9023: `Signal` states an expected edge only when its rule declares
  one; a realised outcome returns to the centre through the journal, not to the
  cell; no storage ADR needed.
- R12-RISK-5044: a net-edge gate exists for edge-bearing signals, unarmed, with
  no default margin, outside `EDGE_GATES`; the margin is RISK.
- R12-REFLEX-5030: no cell reads a belief value; pinned by a test; the only
  future form is a narrowing multiplier under its own ADR.
- R12-EXEC-1068: requote layering is defined as away-from-touch, withdrawn
  unfilled, against an opposite-side fill; no code until a bound is declared.
- R12-RISK-4101: deployment identity is a signed set and a `deployment` halt
  source; credential expiry is unreachable on paper and recorded so.
- R12-GOV-6120: product class, regulated function and leverage have one
  declared source each; the issuer states the block; a new switch arms the cell.
- R12-CONTRACT-7073: the manifest is built at replay from settings-derived
  assumptions; `config_digest` widened; no new topic.
- R12-COVERAGE-5103: `DexVenue` stays a model; digital assets use the
  simulated gateway; no kernel `qip-brokers` edge.
- R12-COVERAGE-7163: Prediction and PhysicalProduct have no instrument type by
  decision; three fixtures close the constructible rest.
- R12-RISK-4107: the universe stays equities; no guessed sensitivity.
- R12-EXEC-5071: a generic REST adapter declares none; pinned by a test.
- R12-GOV-6116: the compliance register is the one operating register; the
  other two answer other questions.
- R12-GOV-6118: fail-closed gate at promotion and venue enablement, with a
  committed seed for simulated subjects in jurisdiction `"paper"` only.

## Not decided here

- **RISK**: every value the structures above read. The authority block's
  `max_leverage` and permitted product classes per grant; the net-edge margin
  and whether the edgeless-signal switch is turned on; the requote-layering
  window, minimum and ratio; the credential lead window if one is ever built;
  the width of any narrowing multiplier for a belief. Another lane implements
  these; this record fixes only where each is read from and that its absence is
  inert or refusing as stated. (rows: R12-GOV-6120, R12-RISK-5044,
  R12-EXEC-1068, R12-RISK-4101, R12-REFLEX-5030)
- **PROTECTED**: stamping the build identity into the image and the file the
  unit writes needs the deploy workflow under `.github/**` and may touch the
  environment tfvars; the need is described, the edit is not decided.
  (row: R12-RISK-4101)
- **BILLED**: any vendor-supplied option, future or FX sensitivity, or price
  feed for the classes RISK-4107 leaves out. (row: R12-RISK-4107)
- **paper-boundary**: a real venue credential, a real-venue eligibility
  record, and any `VenueAdapter::submit_order` for a decentralised venue are
  not built or recorded; the seed is paper-only by test. (rows: R12-RISK-4101,
  R12-GOV-6118, R12-COVERAGE-5103)
- Any chain-finality depth for a simulated venue's settlement: an operator
  declaration that delays proceeds, therefore **RISK**. (row: R12-COVERAGE-5103)
