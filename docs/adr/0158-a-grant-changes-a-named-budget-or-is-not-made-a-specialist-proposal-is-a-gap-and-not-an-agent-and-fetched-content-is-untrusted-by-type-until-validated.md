# ADR 0158: A grant changes a named budget or is not made, a specialist proposal is a gap and not an agent, and fetched content is untrusted by type until validated

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises the in-tree allocation, measurement and
  type-boundary choices below. It does not authorise a TPU or other accelerator
  budget, a quota value for a billed training provider, or a change to any
  financial limit; those are listed under "Not decided here".
- **Date**: 2026-10-19
- **Extends**: [ADR 0069](0069-an-infrastructure-gate-is-proven-by-a-plan-and-capability-nothing-consumes-is-not-declared.md)
  (a capability nothing consumes is not declared), applied here to compute
  grants as ADR 0101 C11 applies it to compute spend, and
  [ADR 0134](0134-the-hybrid-solver-router-is-the-compute-router-searches-reach-it-asynchronously-through-the-kernel-and-feature-subset-selection-is-the-next-quantum-family.md)
  (a solver search is skipped, not queued).
- **Keeps**: ADR 0003, ADR 0006 (a classical baseline every time a QPU grant is
  charged), ADR 0083 (a trained model is promoted through its own held-out gate),
  ADR 0125 (an advisory stage only subtracts), ADR 0146 (a registered brain is
  what registration means; an ambient agent is not an organisation agent), and
  ADR 0157 (a source enters only as a recorded licence).

## Context

`MetaIntelligence::allocate_within` in `qip-expansion` prices bids on four
resources by marginal value per unit cost, charges them to per-resource budgets and
a per-agent compute ceiling, and returns grants with a rationale. Ten rows ask
what a grant *does*, and nine of them end in the same observation: nothing
consumes it, so it is a label. Reading the code settles most of them in the same
direction, and it contradicts the rows in three places.

**The only bids made are the research desk's items.** In `ResearchLoop::run_market`
every bid is built from `self.desk.queue().ranked()`, and `work_kind` maps the
desk's `TaskKind` to `WorkKind` as `Simulation`/`Campaign` to `Simulation`,
`DataAcquisition` to `DataSource`, `Experiment` to `Model`, `Label`/`ToolBuild` to
`Question`. `WorkKind::Agent` and `WorkKind::Solver` are accepted by the library
and produced by nothing outside tests. Rows AMBIENT-10094 and AMBIENT-3110 ask
what a granted agent or solver bid does; no such bid exists to be granted. The
consumers that do exist are real: a granted item is the one `carry_out` starts, a
unit granted is measured next sweep (`record_cell`, `MarginalValue::realised_bp`)
and moves that domain's standing, and `agent_budgets` bounds one agent's compute
across resources in a round. The `Tpu` budget is a literal `Decimal::ZERO` in
`run_market`, and no bid there is ever priced on `Tpu`.

**A model bid is already a research item.** A drifted model raises a
`GapTrigger::ModelDrift` gap; the desk maps a gap's class to a task kind
(`qip_expansion::desk::task_kind` sends the `Model`, `Tool`, `Specialist` and
compute-research classes to `Experiment`), and an `Experiment` item bids as
`WorkKind::Model`. What the row wants (a grant bringing `LearningDesk` forward)
would be a second route to a round that already has a cadence of its own:
`LearningDesk::due(cycle)` is a function of the cycle count alone.

**Class exposure does not exist, and the book's equity is the wrong number.**
`ResearchLoop` weights a belief class's residual by `platform.equity().abs()` for
every class. The book holds one `positions` lot per instrument, net of every
thesis that traded it; a thesis names its class and subject in
`RecordedPrediction::claim` (`ThesisClaim::class`, `subject`), which is the only
join from a class to an instrument. A `Platform` accessor mapping a class to
positions was never written, which is why the row's closure could not land in the
loop.

**Three requests are two types.** The federation emits
`qip_contracts::control::InformationRequest`: validated, in the account currency,
with `allowed_sources`, a `budget` and a `deadline`. The research loop ranks
`qip_contracts::ambient::InfoRequest` on basis points. `FederationStage::set_stakes`
has no production caller and `FederationReview::information_requests` has no
reader outside tests, so on the running path no request is issued, which the row's
gap says and the code confirms. `InfoRequest::record_acquisition` shrinks the
remaining uncertainty reduction by the *cost* in basis points ("a proxy by cost; the
information actually obtained is not fed back"), and its own comment notes the
proxy "can only make a request halt sooner".

**Quotas exist and are optional.** `QueueQuotas` is enforced by
`LocalTrainingProvider::submit` and the Vertex provider when quotas are present; both
providers construct with `quotas: None`, which bounds nothing. No composition root
constructs either: the deep brain's learning desk fits in-process
(`LearningDesk::learn`), not through a provider, so there is no queue to give a
quota. The cost router's `ComputeLedger` sums each rung's *planned* `TierCharge`
(the crate reads no clock), so "cost and latency per decision" are expectations.

**`ToolRegistry::propose` and `AgentBrief` take plain strings.** The existing guard
for untrusted content is the dependency graph
(`untrusted_content.rs::nothing_that_fetches_untrusted_content_can_reach_a_tool_an_order_or_capital`),
which holds because the fetching crates cannot name the agents. The kernel is where
the two meet, and what crosses there is `String`.

Locate them with
`grep -n 'fn run_market\|const fn work_kind\|fn cheapest_viable\|fn launch_ranked' backend/crates/apps/qip-deepbrain/src/research.rs`,
`grep -rn 'WorkKind::Agent\|WorkKind::Solver' --include=*.rs backend/crates | grep -v '/tests/'`,
`grep -n 'pub fn allocate_within\|pub fn propose_specialists' backend/crates/services/qip-expansion/src/meta.rs`,
`grep -rn 'set_stakes\|information_requests' --include=*.rs backend/crates/runtime | grep -v '/tests/'`,
`grep -n 'quotas: None' backend/crates/services/qip-training/src/vertex.rs backend/crates/services/qip-training/src/job.rs` and
`grep -n 'pub fn propose' backend/crates/libs/qip-agents/src/tools.rs`.

## Decision

### A. What a grant is

1. **A named budget is one of four things, and a grant lands on exactly one.**
   (i) the per-resource sweep budget `allocate_within` charges (`Cpu`, `Gpu`, `Qpu`,
   `Tpu`); (ii) the per-agent compute ceiling `ResearchConfig::agent_budgets`;
   (iii) the desk's start gate: an item granted this sweep starts, one not granted
   does not; (iv) the sweep's information-acquisition budget
   (`acquisition_budget_bp`), charged by `launch_ranked`. A bid kind is made only if
   its grant changes one of these. The loop's bid builder produces `Question`,
   `Simulation`, `DataSource` and `Model` and nothing else, and a test pins the set.
2. **No grant raises a ceiling.** A grant is permission to spend within a ceiling.
   `qip_agents::budget::Budget` is a validated ceiling declared in an agent's
   manifest, and it has no setter on purpose; an agent's reputation sets how it
   bids, so a bid whose grant lifted its own ceiling would make the leash a
   function of the thing leashed. An "agent bid", should a producer ever exist, is
   permission to start one `AgentHost::run`; the run's in-run `Budget` stays the
   manifest's and the lesser of the two bounds applies. Row 10094 therefore closes
   as: agent compute is bounded by `agent_budgets` in the market and by the manifest
   `Budget` in the run, and there is no agent bid to consume.
3. **A model grant starts its item and never moves a cadence.** `LearningDesk` runs
   on `due(cycle)`, a cycle count, so a replay refits at the same cycles. Bringing a
   round forward on a grant makes the training window depend on reputation arithmetic
   that earlier grants wrote, and it sidesteps the promotion gate's assumption of a
   fixed rhythm (ADR 0083). Drift already reaches the desk as a gap; the grant that
   follows runs the `Experiment`, which is the whole of what a model bid does.
4. **A solver bid is skipped, not queued.** ADR 0134 decides this already for
   searches; there is no pending-QUBO queue and none is built.
5. **A TPU budget has no consumer and none is granted.** The `Tpu` entry stays the
   literal zero and no bid is priced on it. Giving it a budget would be a billed
   commitment with nothing to spend it on (see "Not decided here").
6. **A detector states an economic value only if it can compute money from what it
   observed.** Fill-residual gaps can; the others state none, and the attention
   router drains them on severity. Inventing a figure so the drain looks
   value-ordered is a number nobody computed. The router's policy (materiality,
   activations per window, window, held capacity) is read from `ResearchConfig`
   instead of a literal in `ResearchLoop::new`, with defaults equal to today's
   literals, so an acceptance test can drive two differently valued held signals.
   This moves where a number is read from and changes no value.

### B. Class exposure and what a proposal creates

7. **A class's exposure is the gross notional, at cost, of the instruments only that
   class holds an open thesis on.** A new `Platform::class_exposure(now)` returns the
   exposure per belief class and an `unattributed` figure. For each instrument in
   `positions_at_cost()` it reads the classes of the open claims
   (`RecordedPrediction::is_open`) on that subject: exactly one class gets
   `|quantity| x average cost`; two or more, or none, add the same amount to
   `unattributed`. Positions net across classes inside one lot, so splitting a shared
   lot between classes would be a split nobody measured; it is counted, not spread.
   The figures are at cost and labelled so, since the only book accessor is
   `positions_at_cost`. The invariant `sum(by_class) + unattributed == gross book at
   cost` is tested. A class with no attributed exposure has weight zero and proposes
   nothing, which is the restrictive direction and also the correct economics: a
   class whose theses were never traded costs the desk no money.
8. **A specialist proposal creates a research gap and never an agent.** What
   `propose_specialists` returns stays an `Observation` with `Wanting::Specialist`.
   Registering a `BrainSpec` is registering code that is woken to act (ADR 0146); one
   minted from a residual statistic would be an unreviewed population. A specialist
   enters through the Cognitive Compiler's promotion path or a reviewed spec, with the
   gap as its evidence. `specialist_threshold` is not touched.

### C. Information requests

9. **The federation's request is canonical for what is asked; the loop's request is
   its ranking view.** `control::InformationRequest` is the contract (money, sources,
   deadline, refused when ill-formed). `ambient::InfoRequest` is derived from it in
   `qip-deepbrain`, never the reverse. The conversion has four inputs and each has a
   source or the conversion refuses: economic significance is the expected
   information value on the loop's existing money-to-basis-points scale
   (`economic_scale_bp`); cost is `budget / expected_information_value` in basis
   points (exactly the stated share, refused above 10,000); urgency is `urgency_bp`
   from the request's deadline; uncertainty reduction is the arbitration's spread,
   which the request does not carry and which is therefore forwarded beside it. A
   source id in `allowed_sources` that has no admitted licence record (ADR 0157) is
   dropped, and a request left with none is not forwarded and is counted as
   `no_admitted_source`.
10. **Stakes are stated once per pass, from the book.** `Platform::understand_federation`
    calls `FederationStage::set_stakes` before the pass with each instrument's gross
    notional at cost from `positions_at_cost()`, the same measure as decision 7. A
    subject not held has no stake and issues no request. `REQUEST_BUDGET_SHARE_BP`
    and `REQUEST_DEADLINE` are unchanged: they bound an information spend, not an
    exposure, and this record moves neither.
11. **Realised information gain is measured against the reading the request was
    raised on, and it can only shrink what is left.** At launch the loop stores the
    uncertainty reading (`Granted::uncertainty` already keeps it for the market); the
    next sweep measures the same reading again. Realised gain is the fall in basis
    points of the launch reading, and the request's remaining uncertainty reduction
    becomes `min(proxy_remaining, launch_reduction - realised)`. A reading that rose
    is a gain of zero, and a request whose purchase measurably bought nothing is
    halted as `no_gain` and not repeated. A request is re-acquired at most once per
    sweep, charged to `acquisition_budget_bp`, until `should_halt`. Absent a
    measurement a request completes, as it does today. The cost proxy stays as a
    floor on shrinkage, because feeding gain back without it would let a purchase
    that bought nothing make buying more look better.

### D. Training queues and the cost router

12. **A billed provider with no declared quota refuses.** `VertexTrainingProvider`
    constructed without `with_quotas` refuses every submit, as `QueueQuotas::admit`
    already refuses a queue it holds no quota for. The in-process
    `LocalTrainingProvider` keeps `None` as unbounded because it spends no money. No
    composition root constructs a provider today and none is added by this record;
    the quota *values* for a billed provider are not decided.
13. **The router's benefit is a report, not an input.** The join of cost, latency and
    accuracy is a read-only report keyed by rung and condition: the rung's charged
    cost and latency from `ComputeLedger` (stated as planned, since the ledger reads no
    clock), and the reputation book's resolved accuracy with its sample count, shown
    as *unmeasured* below the book's own minimum rather than as zero. No ratio is
    computed or fed to rung selection. A selector that divides accuracy by cost
    chooses the cheaper rung wherever accuracy is unmeasured, and the accuracy it
    would read was produced by the rungs it chose before. That needs its own record
    with a held-out comparison.

### E. Three doors that stay shut

14. **The in-process agent host stays synchronous (AGENCY-10063).**
    `AgentHost::run` is called by `Organisation::convene` in the cycle and by the
    poisoning detector. A backlog queue would make a finding arrive in a later cycle
    than its `as_of`, and its retention bound and consumer are undefined. Work the
    cycle cannot convene is not convened and is counted; the two doors that do queue
    (the ambient agency's `Intake::Deferred` and the edge node's reasoning outage)
    are proven on their own paths.
15. **`CapabilityRegistry` is consulted against nothing and is not populated
    (EXPAND-6062).** The sweep's `UnsupportedRequest` arm keys on asset class, venue
    and geography, never a verb, so there is no input to look up. Populating the ten
    verbs from configured venues would assert `Trade`, `Purchase` and `Transfer`
    capabilities on a platform that has none beyond a simulator. Its one legitimate
    future consumer is a promoted capability declaring its verb at registration, to
    refuse a duplicate and to record `eligibility` as shadow or simulated.

### F. Untrusted content

16. **Fetched content is `Untrusted<T>` until a validator narrows it.** A new
    `qip_core::untrusted` module (qip-core links into every crate that fetches, runs
    agents or composes them, and performs no I/O) holds `Untrusted<T>`: private
    fields, a recorded origin, a `Debug` that prints origin and length and never the
    content, and no `Deref`, `AsRef`, `Display`, `Into<String>` or `Serialize` of the
    content. The ways out are two and both are named: `validate(self, f)`, which
    returns whatever narrower type `f` builds from the content (an enum, a number, an
    id from a closed charset) or refuses, and `inspect(&self) -> &T`, documented for
    display, hashing and storage only. A digest of the content is available without
    either.
17. **A tool input is never an `Untrusted<_>` or a string derived from one by
    `inspect`.** `ToolRegistry::propose` and `register` take a `ToolName`, a
    validated newtype over a closed charset (`[a-z0-9_.-]`, bounded), not `&str`; a
    name derived from a research question is built by `validate` or not at all.
    `AgentBrief::new` keeps accepting platform-authored text and now cannot accept an
    `Untrusted<String>` without an explicit `inspect`, which is greppable.
18. **A model that read untrusted content produces untrusted content.**
    `LanguageModel::complete` over a request containing an untrusted part returns its
    output as `Untrusted`: the type cannot stop a model from being persuaded, so it
    refuses to let the persuasion launder itself into a trusted value. The model's
    structured output reaches a tool only through `validate` into a closed form.
19. **What this does and does not prove.** The type makes the *direct* path (fetched
    bytes into a tool argument or a brief) a compile error and the *deliberate* path
    a visible `inspect`. It proves nothing about what a validator accepts, and it
    does not make a model trustworthy. An acceptance test counts `inspect` call
    sites outside a short allow-list (evidence journaling, hashing, display); a
    source scan is the weaker kind of test, and the dependency-graph test remains
    beside it as the coarser net.

## Consequences

- A bid that has no consumer cannot be added silently: the set of kinds the loop
  makes is pinned and a new kind needs a consumer in the same change.
- The specialist weight starts to mean what the register says, "the money each class's
  error is applied to", and shrinks to nothing for classes the desk never traded,
  so fewer gaps are raised.
- Active sensing finally has an issuing path and a ranking path on the running node,
  both bounded by one spend budget, and a purchase that measurably bought nothing
  stops being repeated.
- Code slices left, each with a mutated test: the bid-kind pin (10094), the cadence
  pin (3110), `class_exposure` (3113), realised gain and re-acquisition (3107), router
  policy from config (7202), stakes and the request conversion (10093), the Vertex
  refusal (10139), and `Untrusted<T>` with `ToolName` and the model taint (9065).

## What it costs

- **Fewer proposals, and a blind spot by design.** A class that shares every
  instrument with another is `unattributed` and cannot propose a specialist however
  wrong it is. The count is reported so the blind spot is visible, but it is a blind
  spot, and a desk that trades one instrument under five theses will see none.
- **The agent market stays inert.** Nothing makes `WorkKind::Agent` useful, and the
  library keeps code for a bid nobody makes. Deleting it is a separate, smaller
  decision; this record leaves it because removing an accepted kind is a contract
  change.
- **Exposure at cost is not exposure.** A position that has moved is weighted at what
  it was bought for. Marking it needs a price per instrument at the sweep, and that
  is a second accessor the desk would then own.
- **`Untrusted<T>` touches many call sites.** The evidence statement, news and
  alternative-data text, the brief, the language-model request and the tool registry
  all change signature, in a workspace where several lanes edit those files at once.
  It is the largest slice here and the likeliest to be done twice.
- **A second acquisition is a second spend.** Decision 11 lets a request buy again
  when the first measurably helped. The budget caps it; it does not make it free.

## What would make this wrong

- A measured case where a drifted model waited a full cadence while its item sat
  granted and unstarted and the shortfall was attributable. That argues for a
  *gap-driven* refit signal consumed by `due`, not for a grant moving the cadence,
  and it is the condition to revive decision 3.
- A producer that genuinely needs an agent bid: a workload where running an agent
  is the thing being priced and the manifest `Budget` is too coarse. It must name the
  run it starts (decision 2).
- Observed: `unattributed` exposure larger than the attributed total for most
  sweeps. Decision 7's exclusive attribution would then be measuring almost nothing
  and a different attribution (by contribution, from the exact attribution of ADR
  0007) would be needed.
- A validator that accepts hostile input. The type is only as strong as the narrowing
  it forces; if one ships, the remedy is a review of validators, which is a list a
  person can read.

## Dissent

The strongest counter-position is that the market is the point and the restraint
defeats it. The compute market prices work by marginal value per unit cost so
that scarce compute goes where it pays; a market whose grants may only start a
desk item is a priority queue with extra steps. A model bid should be able to bring
a refit forward, an agent bid should be able to earn a longer leash from a record
of delivering, a TPU budget should exist, and a specialist proposal should mint the
specialist: otherwise the expansion loop ranks everything and acts on one thing.
The register agrees, in that it calls every grant without a consumer "a label".

It lost because each of those moves makes a ceiling, a cadence or a population an
*output* of a statistic that earlier grants wrote, which is how a control stops being
one. A leash that lengthens with the leashed party's own reputation is circular; a
refit rhythm that follows the market makes a replay's training windows depend on
arithmetic over history instead of the cycle count; a population minted from a
residual is unreviewed code that acts. Nothing is lost by waiting: the ranking is
already used (it picks what starts), and each refused move has a conservative
route (a gap, a reviewed spec, a gap-driven flag) that does the work with a person
or a test in the loop. It is revived by decision 3's and decision 2's conditions
above, argued in a record that names the consumer.

## Code it constrains

- `qip-deepbrain/src/research.rs`: `ResearchLoop::run_market`, `work_kind`,
  `ResearchLoop::new` (router policy), `cheapest_viable`, `launch_ranked`,
  `ResearchConfig`, the specialist block and `economic_scale_bp` -
  `grep -n 'fn run_market\|const fn work_kind\|RoutingPolicy::standard\|pub struct ResearchConfig\|fn economic_scale_bp\|propose_specialists' backend/crates/apps/qip-deepbrain/src/research.rs`.
- `qip-expansion/src/meta.rs`: `MetaIntelligence::allocate_within`, `propose_specialists`,
  `WorkKind` - `grep -n 'pub fn allocate_within\|pub fn propose_specialists\|pub enum WorkKind' backend/crates/services/qip-expansion/src/meta.rs`.
- `qip-deepbrain/src/learning.rs`: `LearningDesk::due` -
  `grep -n 'pub const fn due' backend/crates/apps/qip-deepbrain/src/learning.rs`.
- `qip-contracts/src/ambient.rs`: `InfoRequest::record_acquisition`, `should_halt`,
  `AcquisitionCriteria`, `RoutingPolicy::standard`; `control.rs`: `InformationRequest` -
  `grep -n 'pub fn record_acquisition\|pub fn should_halt\|pub struct AcquisitionCriteria\|pub fn standard' backend/crates/libs/qip-contracts/src/ambient.rs`.
- `qip-kernel/src/platform.rs`: `Platform::positions_at_cost`, `RecordedPrediction`,
  `understand_federation`; `federation_stage.rs`: `FederationStage::set_stakes`,
  `REQUEST_BUDGET_SHARE_BP` -
  `grep -n 'pub fn positions_at_cost\|pub struct RecordedPrediction\|fn understand_federation' backend/crates/runtime/qip-kernel/src/platform.rs`.
- `qip-world-model/src/federation.rs`: `Federation::information_request`.
- `qip-training/src/vertex.rs` and `job.rs`: `with_quotas`, `QueueQuotas::admit` -
  `grep -n 'fn with_quotas\|quotas: None\|pub fn admit' backend/crates/services/qip-training/src/vertex.rs backend/crates/services/qip-training/src/job.rs`.
- `qip-cost-router/src/ledger.rs`: `ComputeLedger`, `TierCharge`; `reputation.rs`:
  `ReputationBook`.
- `qip-agents/src/tools.rs`: `ToolRegistry::propose`, `register`; `runtime.rs`:
  `AgentHost::run`; `finding.rs`: `AgentBrief`; `registries.rs`: `CapabilityRegistry`.
- `qip-ai/src/language.rs`: `LanguageModel::complete`.
- `qip-core/src/lib.rs`: the new `untrusted` module;
  `qip-acceptance/tests/untrusted_content.rs`.

## Rows unblocked

- **R12-AMBIENT-10094** - decided: no grant raises a ceiling; no agent bid exists;
  the bid-kind set is pinned. Register: restate.
- **R12-AMBIENT-3110** - decided: a model grant starts its `Experiment` item and
  never moves `LearningDesk` cadence; a solver bid is skipped, not queued.
- **R12-AMBIENT-3113** - decided, code: `Platform::class_exposure`, gross, at cost,
  exclusive attribution, `unattributed` counted; a proposal is a gap.
- **R12-AMBIENT-3107** - decided, code: realised gain defined and bounded by the
  proxy; re-acquisition once per sweep under the acquisition budget.
- **R12-AMBIENT-7202** - decided, code, also BILLED for any TPU budget: detectors
  state value only when computable; router policy read from config; TPU stays zero.
- **R12-AMBIENT-10093** - decided, code: the control request is canonical; conversion
  and stakes defined; allowed sources filtered to admitted ones.
- **R12-FINOPS-10139** - decided, code, also BILLED for quota values: an unquota'd
  billed provider refuses; benefit is a report with unmeasured shown as unmeasured.
- **R12-AGENCY-10063** - decided: `AgentHost` stays synchronous; no queue. Restate.
- **R12-EXPAND-6062** - decided: the registry is consulted against nothing and stays
  empty; restate.
- **R12-SEC-9065** - decided, code: `Untrusted<T>`, `ToolName`, model taint, with the
  limits of the guarantee stated.

## Not decided here

- **BILLED** - any TPU or other accelerator budget (AMBIENT-7202, AMBIENT-10094,
  AMBIENT-3110; ADR 0101 C11 records that no owner ceiling exists and the budget is
  zero cloud spend); the queue quota values for a billed training provider
  (FINOPS-10139); the values of `compute_per_sweep`, `qpu_per_sweep` and
  `acquisition_budget_bp` in any deployment, where they would cause spend.
- **RISK** - none created, changed or loosened. `specialist_threshold`,
  `REQUEST_BUDGET_SHARE_BP`, the router's materiality and `MIN_VIABLE_BP` are
  untouched; the exposure figures decided here are inputs to a ranking and gate no
  order.
- **PROTECTED** - none. The need for a TPU or accelerator budget, if it ever arises,
  would be a `variables.tf` and tfvars change and is not made here.
- **PAPER-BOUNDARY** - none touched. No grant, request or registry entry reaches an
  order, and decision 15 refuses to assert a `Trade`, `Purchase` or `Transfer`
  capability the platform does not have.
- **Deleting** the unused `WorkKind::Agent` and `WorkKind::Solver` arms, and a
  benefit-driven rung selector (decision 13): each needs its own record.
