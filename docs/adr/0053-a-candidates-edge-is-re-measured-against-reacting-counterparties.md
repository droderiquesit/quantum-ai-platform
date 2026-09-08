# ADR 0053: A candidate's edge is re-measured against reacting counterparties, and may not yet refuse one

- **Status**: Proposed
- **Date**: 2026-09-08
- **Supersedes**: nothing
- **Related**: ADR 0006 (a classical baseline always), ADR 0052 (the market factor)

## Context

`qip-simulation-engine` builds five counterparty behaviours — passive, informed,
momentum, competitor, market maker — and a market that reacts to them:
`MarketSimulator::with_agents` attaches them, and the books every fill is priced
against then carry their flow. All of it is built, calibrated in shape, and
tested. Every caller is a test:
`grep -rn 'with_agents(' backend/crates --include=*.rs` finds only
`qip-simulation-engine/tests/agents.rs`.

Meanwhile the deep brain's evolution loop judges each candidate strategy on a
tape where **nothing else is trading**. The tape is a recording; a recording has
no counterparties that respond. So a candidate whose apparent edge consists
entirely of being the only participant taking one side scores exactly as well as
one whose edge would survive company, and the loop cannot tell them apart. That
is blueprint §15.3's "crowding stress" and "capacity discovery", and it is the
gap that makes a backtest most flattering.

The machinery to close it exists. What has not been decided is two things: what
the counterparty panel *is*, and what its verdict is allowed to do.

## Decision

**After a candidate produces holdout evidence, the same tape is replayed twice
through `MarketSimulator`, and the difference between the two runs is recorded
as the candidate's crowding measurement. It is reported. It does not refuse a
candidate.**

Precisely:

- The two runs are a controlled comparison. Same bars, same book shapes, same
  seed, same cost model, same strategy program. The **only** difference is
  whether the standard counterparty panel is attached. Anything else varying
  between them would make the difference attributable to something other than
  the counterparties, which is the whole measurement.
- The replay is `MarketSimulator`'s historical constructor over the candidate's
  own tape, not a fresh synthetic path. A candidate's edge is specific to the
  series it was found on; measuring it against a different random walk would
  report its disappearance and blame the counterparties for it.
- The panel's parameters are stated relative to **the volume the tape itself
  recorded**, so a panel attached to a thinly traded name is not the same panel
  as one attached to a liquid one — and, more importantly, so the panel trades
  against a book built from the same number it was sized by. Not the liquidity
  profile's `average_daily_volume`: `MarketSimulator::replay` fills the book
  from the bars' own volume, "because the whole reason to replay real history is
  that its liquidity is real", so sizing the panel off a profile's *claim* while
  the book is built from the *record* is two claims about one fact. The panel is
  the louder one. This was written the wrong way round first and found by a
  fixture whose profile overclaimed by ten times: the crowded run lost half the
  capital, which reads as a devastating crowding finding and was an arithmetic
  error about which number to trust.
- **The measurement may not gate promotion.** See below.

## What this decision is not

**It is not a claim that the panel is realistic.** It is not calibrated, and the
crate already says so in one place: `FlowCalibration` has exactly one arm,
`NotCalibrated`, and every run carrying agent flow carries that sentence. The
panel's sizes, horizons and thresholds are chosen to be *plausible and
proportionate*, not measured, because the platform has observed no counterparty
flow to measure them from.

**It is not a capacity number.** A crowding difference says the edge changed
when others were present at these sizes. It does not say at what size the edge
disappears, which is what capacity discovery means and which would need the
panel swept rather than run once.

## Why it may not refuse

This is the load-bearing half of the decision, and it cuts against the instinct
that a measurement nobody acts on is a measurement wasted.

Refusing a candidate on this number would be a promotion decision made on
parameters nobody measured. The repository's standing complaint about controls
is `MaxExpectedShortfall` — a limit that shipped in every default set and could
never fire — and the fix for that class of defect is to make controls able to
fire on real inputs. **The opposite defect is a control that fires confidently
on invented ones**, and it is worse, because the first fails open and visibly
while the second fails closed and silently: strategies would be discarded, the
round would report a refusal with a reason, and nothing in the output would
reveal that the reason was arithmetic over numbers chosen by whoever wrote the
panel.

So the measurement is attached to the round and reported, and the promotion path
does not read it.

**What would change that.** `FlowCalibration` growing a second arm — a panel
whose parameters were fitted to observed flow, with the observation named. The
enum exists precisely so that adding the arm is the moment somebody has to
produce the data. When it has one, this ADR should be revisited and the gate
argued for on the strength of that calibration, not on this paragraph.

## What it costs

**Two extra simulator runs per candidate.** Each replay walks the tape once and
prices every step's book. On the committed tape and the round's candidate budget
this is small; a search over hundreds of candidates on years of bars is not, and
the place to fix it is running the check on the shortlist that produced evidence
rather than on every candidate generated — which is what this decision does, and
is why the check sits after evaluation rather than inside it.

**A number that invites over-reading.** A candidate reported as losing half its
P&L under crowding will look like a finding. It is a statement about an
uncalibrated panel, and the round's own record says so by carrying
`NotCalibrated` alongside it. The first sentence of the report is the
calibration statement for that reason.

**An asymmetry that is not a bug.** The panel can only ever make a candidate
look *worse* or leave it alone in expectation, because adding participants to
the other side of a trade adds adversity. A candidate that improves under
crowding has almost certainly found an artefact of the flow generation rather
than a genuine benefit, and that is worth looking at rather than celebrating.

## What would make this wrong

**The panel being so small it changes nothing — and on today's tape it is.**
If every candidate's crowded and bare runs agree to within rounding, the check
costs two runs and reports a constant, which is a measurement in name only.

**This is the observed state, not a hypothetical, and it was found by wiring the
check rather than by reasoning about it.** On the committed synthetic tape the
panel places thousands of orders per round and every comparison comes back with
a cost of zero *to the last digit*: the book's displayed size at the touch —
`InstrumentSpec::level_size`, taken from the instrument's recorded top-of-book
depth — is far larger than the panel's clips, so the takers sweep some depth and
the strategy still arrives to the same quote. The fills are identical, so the
P&L is identical.

That is a real physical outcome and it is not a fault. What it is *not* is
evidence that the edge survives company, and those two readings are
indistinguishable in the cost alone. So the round counts them apart:
`crowding_unmoved` beside `crowding_uncontested`, one for a panel that did not
show up and one for a panel whose takers did not outlast the depth.
`every_registered_candidate_has_its_edge_re_measured_against_company` asserts
the unmoved count *equals* the measured count, so if the panel ever starts to
bite on this tape a test fails and somebody has to say why.

**The response is to count it, not to enlarge the panel.** Sizing the agents
against the book's displayed depth rather than the tape's volume would make the
number move on demand, which is tuning an uncalibrated model until it says
something — the same defect as gating on it, arriving by a friendlier route.

**The panel being so large it swamps everything.** The symmetric failure: a
panel sized at a large fraction of daily volume moves the book so far that every
candidate's edge disappears, and the check reports that every strategy is
crowded out — indistinguishable from a check that always says no.

**Someone wiring it to the gate before the calibration exists.** That is the
failure this decision exists to forbid, and it would not look like a mistake: it
would look like tightening a standard.

## Consequences

**What becomes measurable.** Whether a candidate's edge survives company, on the
same tape it was found on, with everything but the counterparties held fixed.
The five agent behaviours acquire a caller outside a test, and the evolution
round's record carries a number that was structurally absent from it.

**What stays honest about its limits.** The panel is `NotCalibrated` and every
report says so. The measurement does not refuse anything, and the round's
accounting keeps the crowding figure separate from the gate refusals so a reader
cannot mistake one for the other.

**Reversibility.** Nothing is stored that a later decision would migrate: the
figure is computed per round and reported with it. Replacing the panel, or
letting a calibrated one gate, changes what future rounds report and invalidates
no record.
