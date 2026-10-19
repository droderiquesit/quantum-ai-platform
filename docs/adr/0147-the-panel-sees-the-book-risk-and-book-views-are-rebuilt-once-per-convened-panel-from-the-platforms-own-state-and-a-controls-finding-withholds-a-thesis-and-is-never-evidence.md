# ADR 0147: The panel sees the book: risk and book views are rebuilt once per convened panel from the platform's own state, and a control's finding withholds a thesis and is never evidence

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval ("I
  allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval lets this record settle the structure of the feed
  and of the control's effect. It does not move any limit value, admit any
  dependency or paid resource, or touch a paper-trading layer, and the last
  section lists what it leaves alone.
- **Date**: 2026-10-19
- **Extends**: ADR 0125 (an advisory stage has two powers, veto and narrow, and
  both only subtract), ADR 0005 (confidence is arithmetic over evidence).
- **Keeps**: ADR 0003 (paper only), ADR 0063 and ADR 0066 (caps compound by
  multiplication in `(0, 1]`), `Determinism::Required` (pre-trade checks never
  route to a model), the pre-trade gate as the one control that refuses an
  order.

## Context

The eighteen-agent panel reads the platform through a `Desk`
(`backend/crates/agents/qip-investment-agents/src/desk.rs`). Three of its
facilities are fed: `world`, `market` and `memory` are `Upstream` slots the
kernel writes between cycles. The other two that matter here are not. `Desk::new`
in `Platform::new` is handed a `BookView` (an empty `Portfolio` at the initial
equity) and a `RiskView` (`RiskState` with equity, cash and a survival ceiling
set to the whole equity) and nothing updates either. Locate them with
`grep -n 'Desk::new(' backend/crates/runtime/qip-kernel/src/platform.rs` and
`grep -n 'pub struct Upstream' backend/crates/libs/qip-agents/src/runtime.rs`.

Five register rows wait on this one choice (R12-RISK-3021, R12-CAPITAL-3020,
R12-ASSET-3018, R12-ASSET-3019, R12-REASON-3023), and three more are really
about what the panel is for (R12-REASON-3026, R12-AMBIENT-3024,
R12-WORLD-3025). The rows describe the feed as "RiskControl vetoes against the
real book". Reading the code, that sentence is wrong in a way that decides the
design.

**There is no veto.** `RiskControl::analyse` (`control.rs`) returns a finding
whose direction is `Negative` with conviction `1.0` when a limit blocks and
`Neutral` otherwise. The manifest holds `Capability::VetoProposal`, and nothing
in the kernel reads it: no code path turns that capability into a refusal
(`grep -rn 'VetoProposal' backend/crates/runtime` finds nothing). The finding
instead goes to `ReasoningEngine::reason` as evidence, and
`stance_of` (`qip-reasoning-engine/src/engine.rs`) classifies a finding against
the **claim's** sign: `Negative` contradicts an `Undervalued` claim and
**supports** an `Overvalued` one. The kernel builds `Overvalued` whenever the
anomaly's z-score is positive (`mechanism_for`).

Today this is invisible, because the empty book breaches nothing and
`RiskControl` is always `Neutral`. Once the feed is live and a limit blocks,
every bearish thesis gains a full-conviction supporting item from the risk
control: a book over its leverage limit would raise the confidence of a thesis
to sell. That is the exact opposite of what the register wants, and it is not
a hypothetical about some future edit; it is what the first live feed would do.
`ComplianceControl` has the same shape (`Negative` at conviction `1.0` on a
buy-blocked name) and carries the same defect.

Three further facts from the code bound the design:

- `reason_about_the_queue` works the head of the queue only, so the panel is
  convened at most once per cycle. A "per-pass" snapshot is a per-dispatch
  snapshot.
- The platform holds no marks. `Platform::positions_at_cost` and
  `Platform::cash` carry the book at average entry cost, and
  `TrackedCapital::equity` is "cash plus open positions at cost". A panel book
  at market would disagree with the equity every other figure divides by.
- `Upstream::update` takes a closure and releases the write lock before it
  returns, so a feed written before `dispatch` cannot be held across an agent
  read.

## Decision

1. **The two views are `Upstream`s written once per convened panel.** `RiskView`
   and `BookView` become `Upstream<RiskView>` and `Upstream<BookView>`, re-pointed
   at the desk by `.gate(required, label)` exactly as `world`, `market` and
   `memory` are. The kernel writes both inside `reason_about_the_queue`,
   immediately before `self.organisation.dispatch`, and only when the panel is
   actually convened. No agent holds the writing end (`Upstream` is not `Clone`
   and no `Gated` method yields it), so the panel reads the snapshot and cannot
   alter it.
2. **The risk view is `Platform::risk_state()` and the limits are
   `self.monitor.limits()`.** These are the figures the pre-trade gate and the
   monitor read, so the panel is never judged against a bound the desk is not
   under (principle 6: two claims about one fact disagree). The assembly-time
   seed (`survival_gross_ceiling: Some(initial_equity)`) is **not** carried
   forward. That seed existed so an empty book would not be vetoed for want of
   a ceiling; with a live state, a `None` ceiling means the survival kernel
   could read nothing, `MaxSurvivalGross` records a non-number, and the check
   refuses. That refusal is correct and stays: a desk that cannot establish its
   own ceiling takes no new risk, at REASON as at the gate.
3. **The book view is rebuilt from `positions_at_cost()` and `cash()`, valued at
   cost.** `BookView.marks` holds each lot's average price, and the `Portfolio`
   is opened with `cash + sum(lot cost)` and given one `apply_fill` per lot at
   its average price and zero costs, so after the rebuild `Portfolio` cash and
   equity equal `TrackedCapital`'s to the unit. The view says in its doc that it
   is realised-only and at cost. A lot whose instrument is not in the
   universe, or priced in another currency, makes `apply_fill` refuse; the
   snapshot then fails as a whole and **no panel is convened this cycle**, with
   the reason on the stage report. A stale or partial book is the failure this
   record exists to remove, so it is not substituted for a missing one.
4. **A control's finding is never evidence for a claim.** The kernel reads each
   agent's `AgentRole` from `self.organisation.roster()` and passes
   `SynthesisInput.findings` only the findings of agents that are not
   `AgentRole::Control`. Control findings stay on the `OrganisationReport`
   (they are facts and they count toward coverage and the cycle record); they
   are not stances. This is structural rather than a rule inside
   `stance_of`: the evidence set cannot contain a control's opinion because
   the kernel never hands it one.
5. **A blocking limit withholds a thesis that would add to the exposure it
   protects; it never withholds one that reduces it.** After review approves
   and before the thesis is queued, the kernel evaluates `limits.check` on the
   snapshot's state. For each blocking breach it asks whether the breach
   applies to the thesis: it does when `subject` is `None`, names the
   thesis's instrument, or names a bucket `exposure_axes_for` places that
   instrument in. If any applicable breach exists and the thesis is not
   reducing, no thesis is queued; the decision recorded is
   `DecisionTaken::WithheldOnLimit` (a new variant in
   `qip-ai/src/memory/episode.rs`) and the stage report names the limits.
   A thesis is *reducing* when its claim has an implied sign opposite to the
   held lot's sign. A claim with no implied sign (`RegimeShift`,
   `EventOccurs`) or a flat book is *not* reducing, so it is withheld: where
   the direction cannot be established the answer is the restrictive one. This
   mirrors the gate's own exemption (`state.sheds_gross()`), so the panel
   cannot withhold the cure a limit was written to allow. The prediction and
   the episode are recorded as for any other hypothesis (they precede this
   branch), so a withheld claim is still scored in LEARN.
6. **The snapshot is journaled with the cycle.** `CycleJournalEntry` gains
   `desk_feed: Option<DeskFeedJournal>` with `#[serde(default)]`, taken (not
   cloned) in `journal_cycle` as the counterfactual journal is. It carries the
   as-of instant, equity, cash, the instruments and signed quantities of the
   book, the names of the blocking limits, and a `sha256` of that canonical
   text. `RiskState` is not `Serialize`, and this record does not make it so:
   the digest and the breach names are what a replay checks the verdict
   against, and the state itself is a pure function of sealed fills.
7. **Why the direction is restrictive, stated so it can fail.** Relative to the
   assembly-time desk, which breached nothing, the feed can only (a) turn
   `RiskControl` from `Neutral` to `Negative`, which by decision 4 changes no
   evidence, (b) withhold a thesis, which by decision 5 only removes a
   proposal, (c) add facts and coverage. Nothing in it can raise a confidence,
   a cap or a size, so ADR 0125's first decision holds without a new
   multiplier. Because no number is introduced, this is a structural decision
   and not a limit decision.
8. **No capital specialist is added** (R12-CAPITAL-3020). A second `Control`
   agent over the same `RiskState` would be a second finding from one origin,
   which the engine already treats as correlated, and would restate
   `RiskControl`. The capital reading is added to `RiskControl` as computed
   facts from the state it already holds (`cash`, `funding_buffer`,
   `margin_utilisation`, `survival_gross_ceiling`, each only where present) and
   a new `coverage::Scope::CapitalPosition` evidenced by those labels, so
   removing the control still drops the scope. "Grants" are per-cell and are
   not on the desk; no desk facility for them is created here.
9. **Private holdings reach the desk as a read-only view, and the private-markets
   agent comes second** (R12-ASSET-3018). The kernel writes a `PrivateView`
   (the `CommitmentBook`, `private_forecasts`, and `illiquid_unmarkable` that
   `private_holdings_of` produced) into an `Upstream` in the same pre-dispatch
   write, gated by the existing `Capability::ReadPortfolio` (no new
   capability). The agent that follows reports commitments outstanding, the
   next scheduled call and the unmarkable assets as facts, direction
   `Neutral`: it cannot mark a private asset, so it has no direction to give.
10. **The kernel becomes the writer of `approved_conviction`** (R12-ASSET-3019).
    The feature name is the one `PortfolioConstruction` already reads, so no
    feature is invented. At the point a thesis is queued, the kernel defines the
    feature if absent (staleness bounded to the thesis horizon) and records the
    signed `thesis.conviction` with `effective_from = now` and `known_at = now`.
    `PortfolioConstruction` emits `Direction::Neutral`, so this changes its
    facts and no confidence. Its "divestment" reading is added as one fact (a
    held lot with no live `approved_conviction`); "causal-driver" findings are
    not built in the agent, because per-factor and per-driver exposure is
    already charged in `RiskState` by `shared_cause` and enforced by limits.
11. **The two missing probes are context, never evidence** (R12-REASON-3023).
    `SelfExploitationProbe` reads a bounded own-activity window written in the
    same pre-dispatch write under `Capability::ReadExecutionHistory` (the most
    recent `PROPOSAL_HISTORY` fills, newest first; `OrderManager::fills` is
    unbounded and must not reach the desk): it reports the platform's signed
    own volume on the instrument inside the anomaly's window as a fraction of
    the bars' volume, and refuses to report when the fraction is not finite or
    exceeds one. `RegimeBreakProbe` reads a snapshot of
    `Platform::experience(now)` under `ReadResearchMemory`: it reports the
    resolved episodes the platform holds in the current regime and whether the
    cell is a blind spot. Both are `Direction::Ambiguous` like the poisoning
    detector, so by `stance_of` they are `Contextual` and move no
    confidence. Each adds a `coverage::Scope`.
12. **No meta-arbitration agent and no second dispatch phase** (R12-AMBIENT-3024).
    A contested thesis is held small by arithmetic
    (`Hypothesis::confidence_for_sizing` is `effective_confidence` times
    `1 - k*d` with `k` below one, which "can only narrow") and the engine already
    scales each evidence origin by what the self-model measured of it
    (`origin_factors`, built from `SelfModel`). An arbiter that read the panel
    and resolved the disagreement would convert a contested thesis into an
    unopposed one, which sizes larger; that is the one thing ADR 0125 and ADR 0005
    forbid. The register's memory gap is also stale: `desk.memory` is fed
    (`write_research_memory`). Per-regime reliability exists in neither
    `SelfModel` (keyed by component, not regime) nor `ResearchMemory` (episodes
    carry no regime); it exists only in the tournament scoreboard
    (`tournament.rs`, per forecaster per regime) and the `qip-ai` episode store.
    Feeding it to the engine is a separate record when a regime-keyed origin
    factor is wanted, and it must be a factor in `(0, 1]`.
13. **No symbolic agent** (R12-REASON-3026). The kernel's `reason_symbolically`
    is the symbolic seat; ADR 0125 decision 2 moves it before approval as a
    verifier that can refuse. An agent wrapping it would run the same
    derivation over the same hypothesis a second time, as a second origin for
    one source, and needs the `qip-investment-agents -> qip-reasoning-engine`
    edge the acceptance anchor keeps dev-only. Not built; that edge stays
    dev-only.
14. **The federation is a kernel stage, not a Brain** (R12-WORLD-3025).
    `FederationStage` is a field of `Platform`, runs in `understand_federation`
    each UNDERSTAND pass, journals its review, and since ADR 0125 decision 4
    narrows a size when arbitration carries a disagreement. "Constructed in the
    production organisation" is satisfied by that; a Desk Brain would be a
    second consumer of one record. The coverage analogue of "drops its scope
    when removed" is `FederationReview::uncovered` against `REQUIRED_FAMILIES`.

## Consequences

- Once the feed is live, `forecast_value_at_risk`, `stress_loss_3sd_day` and
  `recommended_gross_reduction` are emitted in a running binary for the first
  time, and the coverage scopes the register has counted as covered only in
  tests become true at runtime.
- The reviewer's "the book already holds this" objection (`review.rs`) starts to
  fire on a re-argued position. That objection lowers confidence by the
  engine's arithmetic and cannot raise it.
- A panel can no longer approve, in REASON, a thesis whose first order the
  gate would refuse for a limit that already blocks. The gate stays the only
  control that refuses an order.
- Code slices (each needs its mutation): the two `Upstream`s and the
  pre-dispatch write in `Platform`; the control-role partition in the call to
  `synthesise`; the withholding branch and `DecisionTaken::WithheldOnLimit`;
  `DeskFeedJournal` on `CycleJournalEntry`; `RiskControl` capital facts and
  `Scope::CapitalPosition`; `PrivateView`; the `approved_conviction` writer;
  the two probes.

## What it costs

- **Research stops when the book cannot be read.** Decision 3 convenes no panel
  on a book that fails to rebuild, and decision 2 lets a missing survival
  ceiling block everything. Both are the intended direction of error, and both
  mean a defect in the book silences the research record until it is fixed.
- **Control findings stop counting as dissent.** The confidence governor's
  `observe_dissent` counts `Contradicts` stances, and a control no longer
  produces one. A breached book therefore narrows through withholding, not
  through dissent arithmetic; the first is a hard stop for applicable theses
  and no effect on the rest.
- **A snapshot copy per convened panel.** `risk_state()` runs the liquidity
  ladder and the tail statistics; it already runs per order and per monitor
  pass, and now once more per cycle that convenes a panel.
- **The book is at cost.** A position deep underwater at market looks unchanged
  to the panel. That is the platform's stated limit ("the platform holds no
  marks"), not a new one.
- `WithheldOnLimit` adds an enum variant to a serialised type; an older reader
  of the episode store cannot decode a record written after it.

## What would make this wrong

- If theses withheld under decision 5 were later shown, by the declined-path
  accounting, to have beaten standing aside more often than not (measured as
  withheld episodes whose resolved `realised_move_bps` agrees with the claim),
  the withholding is too broad and should narrow by the thesis's bucket only.
- If the engine ever gains a role-aware stance (a control finding marked as a
  constraint rather than a view), decision 4's partition becomes redundant and
  should be deleted in favour of that type, which would be the stronger
  guarantee.
- If a restart during a breach showed a different withholding verdict from the
  journaled digest, the snapshot is not a pure function of sealed state, and
  decision 6's claim of replayability is false.

## Dissent

The strongest counter-position is that the panel should stay blind to the
book. A research claim about an instrument is a statement about the
instrument; the same claim at the same time should be formed and scored the
same whatever the desk happens to hold, or the episode store mixes the
platform's research quality with its position state and no cohort is
comparable to another. The pre-trade gate already sees the book and refuses;
duplicating its verdict a stage earlier only adds a second place the same
fact can disagree with itself.

It lost for two reasons. The panel is already not blind: the reviewer reads
the book for its novelty objection, `PortfolioConstruction` reads it for
turnover, and `RiskControl` is on the roster to evidence three coverage scopes
that are empty in a running binary, which is a claim of coverage without
information. And decision 5 keeps the research record intact: the claim is
still formed, predicted and scored before it is withheld, so the cohort
comparison survives; only the proposal is dropped. What would revive the
dissent is the measured condition above: withheld claims that keep winning.

A second, narrower dissent holds that a control's finding should remain
evidence and simply be made to contradict regardless of claim sign. It lost
because that makes a breach narrow a thesis that reduces the breached
exposure, working against the cure the limits exempt.

## Code it constrains

- `Desk::new` and its callers: `grep -n 'Desk::new(' backend/crates/runtime/qip-kernel/src/platform.rs`.
- `Upstream`, `Gated`: `grep -n 'pub struct Upstream' backend/crates/libs/qip-agents/src/runtime.rs`.
- `RiskControl::analyse`: `grep -n 'impl Agent for RiskControl' backend/crates/agents/qip-investment-agents/src/control.rs`.
- `stance_of`: `grep -n 'fn stance_of' backend/crates/services/qip-reasoning-engine/src/engine.rs`.
- The panel call and synthesis: `grep -n 'self.organisation.dispatch\|fn synthesise\|fn reason_about_the_queue' backend/crates/runtime/qip-kernel/src/platform.rs`.
- The book at cost: `grep -n 'fn positions_at_cost\|fn risk_state\b\|fn risk_state_from' backend/crates/runtime/qip-kernel/src/platform.rs`.
- The journal entry: `grep -n 'pub struct CycleJournalEntry\|fn journal_cycle' backend/crates/runtime/qip-kernel/src/platform.rs`.
- `DecisionTaken`: `grep -n 'pub enum DecisionTaken' backend/crates/libs/qip-ai/src/memory/episode.rs`.
- `approved_conviction`: `grep -rn '"approved_conviction"' backend/crates --include=*.rs`.
- Coverage scopes: `grep -n 'pub enum Scope' backend/crates/agents/qip-investment-agents/src/coverage.rs`.
- The federation stage: `grep -n 'fn understand_federation\|pub const REQUIRED_FAMILIES' backend/crates/runtime/qip-kernel/src/platform.rs backend/crates/runtime/qip-kernel/src/federation_stage.rs`.

## Rows unblocked

- **R12-RISK-3021**: decided; the feed is decisions 1 to 7, and the row's "veto" is restated as withholding plus a control that is not evidence.
- **R12-CAPITAL-3020**: decided; feed plus capital facts and `Scope::CapitalPosition` on `RiskControl`; no capital specialist agent.
- **R12-ASSET-3018**: decided; `PrivateView` under `ReadPortfolio`, then a `Neutral` facts-only private-markets agent.
- **R12-ASSET-3019**: decided; the kernel writes `approved_conviction` at the queueing seam; a divestment fact is added; causal-driver findings are not built.
- **R12-REASON-3023**: decided; two context-only probes, each with its own scope, over bounded desk facilities.
- **R12-REASON-3026**: decided; no symbolic agent, the architecture anchor stands; register wording restated.
- **R12-AMBIENT-3024**: decided; no meta-arbitration agent and no second phase; stale memory text corrected; register wording restated.
- **R12-WORLD-3025**: decided; the kernel stage satisfies the row; "recorded, never acted on" is stale since ADR 0125 decision 4.

## Not decided here

- **RISK**: no limit value is added, changed or loosened. Decision 5 applies
  the limits already installed earlier in the cycle, with no new number. It
  is a judgment that this is structure and not a risk limit; if the owner
  reads "a conduct bound that refuses" to include withholding a thesis, that
  reading removes decision 5 and leaves decisions 1 to 4, which are unaffected.
  The capacity of the own-activity window (`PROPOSAL_HISTORY` fills) is an
  operational bound, chosen and not measured.
- **PROTECTED**: no change to `.claude/**` is made. Several comments in
  `platform.rs` and `.claude/rules/domains/observability.md` will need
  correcting when this lands; the rule file is protected and is described, not
  edited, here.
- **BILLED**: nothing. No new resource, subscription or compute budget.
- **PAPER-BOUNDARY**: nothing. The feed only reads; the withholding only drops a
  proposal; no order path, venue or autonomy level is touched.
