# ADR 0061: Rule regret is a proposal, a defence or a dormancy finding, and only a signed file moves a bound

- **Status**: Proposed
- **Date**: 2026-09-13
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0005 (confidence as arithmetic), ADR 0016 (the data domain), ADR 0055 (a counterfactual result narrows sizing and only narrows it)

## Context

Blueprint §12.3 says what a counterfactual result changes, and the table has
six rows — `docs/DELIVERY-STATUS.md` corrected the count from four on
2026-09-13. Three of the six are about the risk rules:

| Finding | Consequence the blueprint names |
|---|---|
| A rule vetoes mostly *profitable* paths | the rule is too tight and is recalibrated |
| A rule vetoes mostly *losing* paths | the rule is earning its place |
| A rule almost never fires | the rule is dead weight |

ADR 0055 built the sixth row's declined-path half — a sizing multiplier that
can only narrow — and left the three rule rows exactly as absent as it found
them, for a reason it stated: §12.4's guardrail reads "a veto rule may only be
loosened through the full approval path, never automatically from
counterfactual evidence", so the one automatic consequence the first row
invites is the one the blueprint forbids.

That was true and it was also incomplete, because the platform could not key
any of the three rows even if it had wanted to. A refusal was charged to the
*control*: `RefusalReason::RiskRejected` carried the sentence pre-trade risk
wrote and nothing else, the OMS dropped `check.post_trade_check` — the
checker's own record of which limits bound — and `gate_of` in the kernel
collapsed every risk refusal to `pre-trade-risk`. Every limit in the set was
one bar on one chart and one key in the twin's score. Parsing the rule back
out of the sentence would have worked until the sentence was reworded, and it
has been reworded before.

So the question this record settles has two halves. How is a refusal charged
to a rule, so that regret can accumulate per rule at all? And, once it can,
what may the first row's consequence be, given that the running process must
never loosen a control on its own?

## Decision

**A refusal is charged to the rule by the breach the checker wrote, never by a
word in the sentence. From the per-rule accumulation the LEARN stage journals
three findings — a defence, a dormancy record, and a recalibration
proposal — and the proposal is the only shape the first row may take. Two
operator signatures on a proposal produce a file: the running limit set with
exactly one bound replaced. The process that produced it keeps the limits it
booted with, and the only path by which a bound reaches any running process
is the file a composition root reads at start-up.**

In detail:

1. **Attribution by breach.** `RiskRejected` carries the blocking
   `LimitBreach`es beside its sentence, and `RefusalReason::rule_names` reads
   each limit's configured *name* — `order-notional`, never
   `max_order_notional`, because two limits share a kind and a tally that
   merged `sector-concentration` with `country-concentration` would propose
   loosening a bound that refused nothing. A feasibility veto attributes to
   its `GATE_*` constant; a posture refusal — halted, autonomy, venue —
   attributes to no rule; a risk refusal on an unevaluated figure carries no
   breach and is charged to nobody. The kernel counts
   `qip_rule_fired_total{rule}` at the same site as
   `qip_orders_refused_total{control}`, from the same refusal, and carries
   the names and readings on the declined path the twin scores.

2. **Two bars, reused.** A rule's scored refusals clear a finding at the same
   two thresholds sizing already uses — `COUNTERFACTUAL_SIZING_MIN_SAMPLE`
   (ten) and `COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION` (three in four) —
   by name rather than restated. The platform has one answer to "how many
   observations make an evidence-weighted finding trustworthy", and a second,
   differently-sized answer to the same question would be a number nobody
   could reconcile with the first.

3. **A defence is a record.** A rule whose sample clears the bar on the
   correctly-declined side is defended under `risk.rule_defended`, with the
   simulated loss it avoided — the magnitudes of the negative earnings over
   the `!regret` paths, summed, and still `Simulated`. Keyed on the newest
   scored order, so the same evidence reviewed on the next cycle is the same
   record; counted in `qip_rule_defended_total{rule}` only when the log took
   it. It changes nothing.

4. **Dormancy is a record.** A rule that has not fired for
   `RULE_DORMANCY_CYCLES` (one hundred) cycles across
   `RULE_DORMANCY_MIN_ORDERS` (one hundred) accepted orders is recorded under
   `risk.rule_dormant` and `qip_rule_dormant{rule}` reads one until it fires.
   Both bars together: a rule idle on a platform that submitted nothing is a
   rule nothing asked, and a rule idle across a hundred orders in three
   cycles has not been asked across enough market states. Every limit name
   has an activity row from assembly, so a rule that never fires has somewhere
   for its silence to be measured; a fire ends the episode and a second
   silence is a second record. It changes nothing.

5. **Recalibration is a proposal, and `RecalibrationProposal::new` is the
   only constructor.** It refuses thin evidence, a non-finite bound, and any
   bound that does not loosen — a ceiling must rise and a floor must fall,
   decided by `LimitKind::is_minimum`. The proposed bound is the observation
   farthest past the bound among the paths the twin regrets: the bound at
   which every one of them would have been admitted. Journaled under
   `risk.rule_recalibration` (Decide group, permanently retained), withdrawn
   when the evidence stops clearing the bar, resumed from the log at
   assembly so a restart does not forget what it proposed.

6. **Enactment is an artefact.** `Platform::approve_recalibration` is cloned
   from `approve_promotion`: a fresh credential, two distinct people, a first
   signature that goes stale after a day, the approver taken from the session
   and never from a body. The second signature produces
   `RecalibrationApprovalEntry::artefact` — `LimitSet::rebound`, the running
   set with one bound replaced — journals the proposal as enacted, and closes
   it. `self.orders`, `self.monitor` and the desk are not written.

7. **The file is the only door.** `QIP_RISK_LIMITS_PATH` names a committed
   JSON `LimitSet` under `data/risk-limits/`, mounted by `risk_limits_file`
   on all three central roots — api, fastbrain and deepbrain each assemble
   their own `Platform` on their own set, and a bound moved on one brain and
   not the other would be two desks with one name. Unset, a root runs the
   shipped `conservative_default`. Set, the file is read once at boot,
   validated against the shipped set, and never read again. **A file may
   move a bound and never remove a control**: `LimitSet::validate` refuses
   one that does not carry every limit the shipped set carries, along with
   an empty set, a duplicated or unexplained limit, a bound that is not a
   finite positive number, a threshold outside its range. It deliberately
   does not refuse a looser bound — loosening through the file *is* the
   governed path.

8. **No setter exists, and the acceptance suite says so.**
   `no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration`
   walks every shipped `impl` of `LimitSet`, `PreTradeChecker`,
   `RiskMonitor`, `OrderManager` and `Platform` and refuses a `&mut self`
   method naming a limit, and refuses a `conservative_default()` call from
   shipped code outside a reviewed list — which is how `qip-api`'s risk page
   came to be found rendering the shipped set while the platform ran another.

## Why the route can only loosen, and why nothing tightens automatically

Four facts, none of them a call site's care:

- The bound comes from a platform-generated proposal, and the proposal's one
  constructor refuses a bound that does not loosen.
- The route body carries a rationale and nothing else.
  `RecalibrationApprovalRequest` refuses every other key by position, and in
  particular refuses a bound, because a bound a caller could name would be a
  request to loosen a control rather than a signature on the evidence for
  one.
- The defence finding has no bound field and no route. Evidence that a rule
  is earning its place is a record, not a proposal to tighten it.
- The only path by which a bound changes in a running process is the file
  read at boot. A tightening is therefore a reviewed commit to that file —
  the path a desk already has, and the same review any other configuration
  change gets.

So the automatic direction the guardrail forbids is not merely unimplemented;
there is no code that could take it. What the platform does automatically is
*say* that a rule is too tight, on evidence it names, to two people who can
disagree.

## The `Decimal` crossing

Every money figure in a declined path's score is `Simulated<Decimal>` and
stays that way; the defence's loss avoided is a sum of those and is still
`Simulated`. The readings a proposal reasons about are `f64` — copied from
`LimitBreach::observed` and `::bound`, which already crossed out of `Decimal`
where the limit engine compares. `LimitKind::with_bound` is the one crossing
back: for the two money-bounded kinds (`MaxOrderNotional`,
`MaxPositionNotional`) it goes through `Decimal::from_f64`, which rounds to
the nine decimal places the type carries. A bound proposed from a breach
reading came out of a `Decimal` through `to_f64` in the first place, so the
round trip at nine places is exact for any notional a desk would write down.
The rounding is stated because it exists, not because it is expected to bite.

## What it costs

- A `qip_rule_fired_total{rule}` series per limit name plus four feasibility
  constants, bounded by the boot-frozen set; a `qip_rule_dormant{rule}` gauge
  per limit written to zero at assembly, so `qip-api`'s scrape test now
  counts two assembly gauges rather than one.
- Three topics on the backbone, all permanently retained. A defence record
  per rule per body of evidence — bounded by scoring cadence, not by cycle
  count, because of the idempotency key.
- A deployment step. A signed recalibration reaches a process only after a
  person commits the artefact and sets `risk_limits_file`, and the runbook
  (`docs/operations/recalibrating-a-limit.md`) says so. The convenience this
  refuses — "apply it to the running process too" — is exactly the door the
  acceptance scan holds shut.
- `qip-cli` is untouched (ADR 0010): the operator's local tool prints and
  demos the shipped set, and is outside the deployment.

## What would make this wrong

- **The dormancy constants.** One hundred cycles and one hundred orders are
  stated bars, not measured ones. A desk whose cycle is a day will read the
  cycle bar as a quarter; a desk that submits a thousand orders a cycle will
  cross the order bar in the first hour. If either reads as wrong for a real
  cadence, the fix is a measured figure in a new record, not a quiet edit —
  and the finding is a record, so a wrong bar produces a wrong sentence in
  the log rather than a wrong control.
- **The admitting bound.** Proposing the observation farthest past the bound
  admits every regretted path and is therefore the loosest defensible
  proposal, not the tightest. A desk that wants the median, or a bound that
  admits three in four, has a different proposal rule; this one is chosen so
  the two signers read a number that would have changed every outcome in the
  evidence rather than some.
- **A rule refused by two rules.** A path both `order-notional` and
  `position-weight` refused is in both samples. If loosening one would not
  have admitted the path because the other still binds, the regret is
  over-attributed; the evidence lists the scored orders so a signer can see
  it. A joint model is a later record.
- **The file replacing the set wholesale.** `validate` holds the shipped
  names present and says nothing about a limit the file adds. An added
  limit is a control nobody argued for in an ADR, and it will pass. If that
  turns out to matter, the check is the mirror of the coverage loop.
- **`from_f64` at nine places.** If a desk ever proposes a bound with more
  than nine decimal places of meaning — it would have to come from a breach
  reading with that precision, which no `Decimal` produces — the crossing
  rounds it, and the comment on `with_bound` is where to look first.

## Consequences

- §12.3's three rule rows are built as findings: R1 as a proposal with a
  governed manual enactment, R2 and R3 as records. R6 is as ADR 0055 left it.
  R4 (a venue dropped) and R5 (an allocator objective revised) stay absent
  for the reasons ADR 0055 gives. The section stays `PARTIAL`.
- §12.4's "never loosened automatically" now has something to hold against:
  a governed manual path exists, and the automatic direction has no code
  path — the acceptance suite scans for one.
- `Platform::limits()` is the one place a page or a route reads the running
  set; `qip-api`'s risk page reads it.
- Every environment leaves `risk_limits_file` null and says why.
