# Owner decisions, 2026-11-28

The owner wrote on 2026-11-28, answering a question about why no register
row had closed since 2026-11-23: "Just make the decisions for me to complete
this". The orchestrator made the decisions below under that instruction.
Each one answers a question that an earlier record left open for the
owner. Each is provisional. Any of them can be replaced by a later owner
ruling, through an ADR where the decision says it needs one.

## What this instruction does not cover

The instruction does not lift any of these, and nothing below relies on
lifting one:

- **No live-order path.** The platform submits no live order. All three
  paper-trading layers stay as they are: the Terraform refusal in
  `infrastructure/terraform/variables.tf`, `AutonomyLevel::deployable` at
  the composition roots, and the paper-only `Cell` type with the
  `Determinism::Required` arm.
- **No weakening of what exists.** No test, guard, scanner or existing
  limit value is weakened.
- **No new dependency without an ADR** (ADR 0109).
- **Decision 39 stands.** Billing stays paused and nothing is deployed. No
  agent dispatches `infra.yml` or `deploy.yml`. A decision to deploy or
  to spend is a separate instruction, and this one does not give it.

## Decisions

### 1. R12-WORLD-1181: which views can contradict, and what a contradiction does

- **Decision.** A contradiction is counted only between two members that
  claim the same kind of state:
  - two members reading the present; or
  - two members of the same scenario.

  When a scenario or counterfactual member (`stress-world`,
  `alternate-future`) opposes a member reading the present, the
  disagreement is recorded as dispersion, in the spread the review already
  journals. It is not recorded as a contradiction.
- **Why.** `alternate-future` is built as `1 - stress`. Its opposition to
  any present-state view therefore comes from how it is built, not from
  evidence. Counting it made every one of the 2,320 instrument
  arbitrations on the demonstration tape a contradiction, so the detector
  measured nothing.
- **Decision.** On a contradiction past `CONTRADICTION_MARGIN`, REASON
  abstains. Any thesis on the contradicted instrument is withheld for that
  pass, at any confidence. The information request naming the two views is
  still journaled. This replaces the round-85 cap, which admitted the
  thesis at confidence 0.5 and still approved it.
- **Why.** The row asks the platform to "decide to abstain". A thesis held
  to even odds and then approved has not abstained. Withholding is the
  more restrictive of the two.
- **Said out loud.** Under this rule, fewer theses are withheld than if
  scenario members counted. Counting them would have withheld every thesis
  on the tape.
- **How the margin is measured.** `CONTRADICTION_MARGIN` is measured on
  the feed a deployed fast brain reads, `Feed::synthetic`, and the
  distribution of view gaps is recorded. If that feed produces no
  contradiction between two present-state members, the record says so in
  those words.

### 2. R12-WORLD-3036: the blind-spot discount, the uncertainty reading and the boundary

- **Decision.** Evidence from a component in a blind-spot cell carries
  half the weight grading would give it: `0.5 × min(UNGRADED_WEIGHT,
  SelfModel::factor)`. A blind-spot cell is a (component, sector) cell with
  no graded resolved thesis.
- **Why.** The two ends are both ruled out:
  - Weight 0 starved the shipped review (`qip-fastbrain` `tests/tape.rs`).
  - The earlier cap, equal to the weight the origin already carries,
    changed no weight in any configuration.

  Half is the one discount that keeps the evidence and still says it is
  less known. It only ever lowers a weight, and it never touches sizing.
- **Decision.** The self-model reports an explicit uncertainty reading
  for each component:
  - the number of resolved theses behind its grade;
  - the share of the grade that comes from the prior,
    `PSEUDO_COUNTS / (PSEUDO_COUNTS + n)`;
  - whether `n` is below `MINIMUM_SAMPLE`.
- **Why.** The shrinkage is already applied, so this introduces no new
  figure. It makes the shrinkage readable instead of implicit.
- **Decision.** The capability boundary is drawn per instrument, not per
  asset class. The boundary is the set of instruments the feed serves with
  fresh bars, and REASON refuses a thesis on an instrument outside it.
- **Why.** The committed universe holds one asset class. A class boundary
  therefore fires only when the whole feed has stopped. An instrument
  boundary fires when one instrument's bars stop. It is the more
  restrictive of the two.

### 3. R12-WORLD-6015: a byte ceiling on the fast brain's in-memory log

- **Decision.** On memory storage, the fast brain's in-memory event log is
  bounded at one quarter of the committed container memory. For the 2 GiB
  in `infrastructure/terraform/catalogue.tf` that is 512 MiB. At the
  ceiling:
  - the oldest sealed records are released from memory as a whole range;
  - a checkpoint record carrying the released range's last hash and its
    record count is appended;
  - the chain still verifies from that checkpoint forward.

  Before the ceiling is applied, the bytes each cycle writes are measured,
  and duplicated payload is removed first.
- **Why.** This is the only option that needs no billed resource. A
  durable log would need storage outside the container, and Decision 39
  keeps spending the owner's. A Cloud Run container's own disk is memory,
  so a file log would exhaust the same 2 GiB.
- **Said out loud.** On memory storage, the fast brain's log no longer
  replays from genesis. It replays from the most recent checkpoint. That
  log never survived a restart anyway. No durable record changes: the deep
  brain's committed storage target is also `memory`.
- **ADR required.** The change to `replay_journal` is recorded in a
  Proposed ADR before it lands.

### 4. R12-MODEL-4021: whether a model trained on synthetic data may inform paper decisions

- **Decision.** Yes, on paper and with a label:
  - The model card records training provenance `synthetic`.
  - Every proposal such a model shapes carries that provenance in its
    journaled detail, so the desk can always tell.
- **Decision.** The deep brain's synthetic arm states a signal strength.
  It is the smallest value at which a fit clears the existing skill bar,
  measured in the 400-cycle probe and recorded with the measurement. The
  skill bar is unchanged, and so is every promotion test, the adversarial
  attack and `PromotionClearance`.
- **Why.** The platform is paper-only, so a model that informs a paper
  decision moves no capital. Keeping synthetic-trained models out entirely
  would leave the promotion path unreachable on committed configuration.
  Every safeguard on that path would then be a control that cannot fire.
  The label keeps the result attributable, which is the platform's purpose.
- **Not covered.** A synthetic-trained model never informs anything
  outside paper trading. No such path exists, and this decision creates
  none.

### 5. R12-EXPAND-6033: whether a counterfactual twin score is an episode

- **Decision.** A counterfactual twin score is not a `SyntheticEpisode`.
  It scores a real decision against an alternative, and it is already
  journaled in `CounterfactualJournal` and replays from there.
  `SyntheticEpisode` covers generated worlds:
  - scenario generation;
  - agent simulations, including the capacity probe's seeded
    `MarketSimulator::synthetic` run;
  - Monte Carlo tail worlds drawn beyond history.

  This is a reading of ADR 0149 decision 18, not an amendment to it.
- **Why.** A second record of the same score would be a second source of
  truth for a fact the journal already holds.
- **What remains code.** Two pieces of work are left:
  - journal `MonteCarlo` episodes with their seeds, from the tail worlds
    and the capacity probe;
  - add a test that fails when the Conditional arm's push is deleted.

### 6. R12-MODEL-4025: the shortfall measure for the counterfactual hold

- **Decision.** The provisional hold figure stands: hold a model when the
  mean counterfactual shortfall exceeds 0 bps over at least 100 scored
  fills (owner-decisions-2026-10-31). The shortfall of one scored fill is
  the alternative's outcome minus the realised outcome, per unit of the
  fill's notional, in basis points. It is computed from the prices the
  score already carries.
- **Not covered.** If the score carries no prices, the measure is not
  invented from counts. The record says the score needs prices.
- **The hold only restricts.** It holds or rolls back a model. It never
  promotes or reinstates one.

### 7. R12-MODEL-6074: the tolerance for a candidate miscalibrated under stress or regime shift

ADR 0392, which is accepted, leaves this calibration tolerance to the owner.
It is judged only once the owner gives one. The owner gave it on 2026-11-28
through the delegation quoted at the top of this file, and this decision
supplies it. It does not amend ADR 0392.

- **Decision: the tolerance.** A regression candidate is miscalibrated in a
  regime when its empirical coverage of the `CONFORMAL_ALPHA` interval on
  that regime's holdout rows falls below the nominal `1 - CONFORMAL_ALPHA`
  by more than `1.645 x sqrt(alpha x (1 - alpha) / n)`, where `n` is the
  number of those rows. The figure 1.645 is the one-sided 95% binomial
  sampling tolerance. Coverage is measured on rows later than the ones the
  interval was calibrated on, because coverage on the calibration rows
  always passes.
- **Decision: the row floor.** It is 25 rows: the smallest `n` at which
  that tolerance is below the nominal miss rate `alpha` (0.1) itself.
  - **At entry**, a candidate with fewer than 25 rows in the regime it
    claims is refused.
  - **In a shadow round**, a stressed or shifted round with fewer than 25
    rows is lost, not scored. A round cannot be won on rows too few to
    measure.
- **Decision: what counts as a regime shift.** A shift is any holdout
  regime that differs from the regime the candidate's training window was
  judged in, not only a regime labelled stressed. Shifted rows face the
  same tolerance and the same floor as stressed ones.
- **Why.** Each figure follows from the sampling arithmetic and from
  `CONFORMAL_ALPHA`, which the platform already states. Each one only
  refuses or loses a candidate. None of them lowers the skill bar or any
  existing promotion test.

### 8. R12-TICK-2074: a publication delay on the synthetic feed, and the probe's time scale

- **Decision: the publication delay.** The synthetic feed stamps every
  tick and bar with a publication delay of one fast-brain cadence:
  `capture_time = at + DEFAULT_CYCLE_INTERVAL`, which is 100 ms,
  `qip-fastbrain` `config.rs`. This is synthetic data, labelled synthetic.
- **Why.** A delay of zero makes the delayed-feed condition vacuous on the
  feed a deployed node reads. One cadence is the smallest delay that a
  running node can observe: the value lands one cycle after it was true.
- **Decision: the capacity probe's time scale.** The probe's step is the
  lower edge of the latency band calibrated from `run_cycle`'s measured
  samples, with a floor of 1 microsecond and at most 32 steps. Per-step
  volatility is rescaled from the daily estimate by the square root of the
  step's share of a trading day. A series with no bars is read as daily.
- **Why.** Each arrival must cross a step, or a measured latency moves
  nothing. The square-root rescaling keeps per-step volatility consistent
  with the daily figure it comes from. This is simulation arithmetic, not
  a risk limit or a sizing figure.

## Not decided here

- **R12-RISK-3022: the central plane's gated halt through `qip-api`'s
  mesh.** The mesh would name edge cells, and no cell exists: every
  environment has `execution_nodes = {}` or `execution_nodes_enabled =
  false`. A mesh configured to cells that do not exist would be vacuous.
  Enabling a cell is a deployment, and Decision 39 keeps deployment the
  owner's.
