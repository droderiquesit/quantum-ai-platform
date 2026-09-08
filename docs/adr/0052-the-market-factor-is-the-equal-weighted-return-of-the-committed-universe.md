# ADR 0052: The market factor is the equal-weighted return of the committed universe

- **Status**: Proposed
- **Date**: 2026-09-08
- **Supersedes**: nothing
- **Related**: ADR 0007 (exact attribution), ADR 0016 (the data domain)

## Context

`qip_risk::metrics::beta` estimates a beta from a return series against a
benchmark. It is built, tested, and has no production caller:
`grep -rn '::beta(' backend/crates --include=*.rs | grep -v qaoa` finds nothing
outside tests. The consequence is not local to that function.

`factor_betas` is therefore constructed empty at both places the kernel builds
a position period — `platform.rs` and `central/plane.rs` — and the platform has
no factor model at all. Three capabilities read that empty map and so measure
nothing:

- **§19.1**, whose effective-breadth measure (`RiskDecomposition::effective_bets`)
  is the inverse Herfindahl of asset risk contributions. With no contributions
  it reports zero.
- **§23.7**, whose `StressTester::apply` walks each exposure's betas and pushes
  every position with none onto its `unmodelled` list. A stress run against the
  current book would execute, complete, and model nothing — the shape this
  repository names by example in `MaxExpectedShortfall`, and the reason
  `unmodelled` is a separate output rather than a silent zero.
- **The factor half of `qip-learning-engine`'s attribution**, which reads the
  same map.

Each of the three has been recorded in `docs/DELIVERY-STATUS.md` as its own gap
with its own "every caller is a test" evidence. They are one gap seen three
times, and the wire that closes all three needs one thing this repository has
not decided: **what the benchmark is.**

`beta(returns, benchmark)` is a ratio of covariance to benchmark variance. The
benchmark is not an implementation detail of that arithmetic — it is the thing
every factor number the platform ever reports is measured against. Choosing it
inside a patch would settle it by accident.

## Decision

**The market factor's return series is the equal-weighted arithmetic mean of
the per-period returns of the instruments in the committed universe, computed
from the platform's own price tape.**

Precisely:

- The tape is `Platform::price_history()` — closes per instrument, the series
  the platform accumulated by observing its own feed. No external index is
  fetched, and none may be: an index the platform cannot reproduce from its own
  records is a number a replay cannot check.
- A period's factor return is the mean of the simple returns of every
  instrument that has a close at both ends of that period. An instrument
  missing either end is excluded from that period rather than imputed.
- Equal weight, not capitalisation weight. The platform holds no share counts
  and no free float, so a capitalisation weight would have to be estimated from
  something it does not observe; an equal weight is computable from what it
  does.
- An instrument's beta is `metrics::beta(instrument_returns, factor_returns)`
  over the overlapping window, and is reported only where that window meets the
  minimum sample the risk crate already applies. Below it, **no beta is
  reported** — the position stays `unmodelled` rather than carrying a number
  estimated from four observations.

The factor is named `market` in `factor_betas` and `factor_returns`, matching
the name `StressTester`'s shock table and the standard scenario library already
use for equity-style shocks.

## What this decision is not

**It is not a claim that one factor is enough.** A single equal-weighted market
factor is the smallest honest model the platform's own data supports. §22.2's
table asks for more, and rates and credit shocks in the standard scenario
library will keep reporting `unmodelled` for instruments whose exposure to them
this factor cannot express — which is correct, and visible, rather than
silently folded into the market beta.

**It is not a benchmark for performance.** Alpha and tracking error take the
same argument and are a separate question; this ADR fixes the factor used for
*risk decomposition and stress*, and a performance benchmark may reasonably
differ. Nothing here authorises reusing it as one without saying so.

**It does not make the universe a market.** The committed universe is five
synthetic instruments today. An equal-weighted mean of five names is a factor
in the arithmetic sense and is not a market proxy anybody should trade against,
and the effective-breadth number it produces should be read as "how
concentrated is this book in the thing these five instruments have in common",
not as a statement about diversification in the world.

## What it costs

**A factor that is wrong in a knowable way.** An equal-weighted mean of the
instruments on the tape is not the market; it is the platform's own book's
common movement. Betas measured against it are betas against that, and a reader
who takes them for betas against an index will overstate what they mean. The
name `market` is the honest label for what it is used *for* — matching the
shocks that consume it — and not a claim about what it *is*.

**Work on every attribution.** The factor is re-estimated whenever the
attribution runs, over the whole tape, rather than cached. That is deliberate
while the universe is five instruments and the tape is thousands of closes; at
a universe of hundreds it becomes a cost worth measuring, and the place to fix
it is a cached estimate keyed on the tape's length, not a shortcut in the
arithmetic.

**A number people will over-read.** `effective_bets` will begin reporting
values, and a book of five instruments that all load on one factor will report
an effective breadth near one — correctly, and alarmingly, and for a universe
this small that is arithmetic rather than a finding about diversification.

**Two floors that will hide things.** An instrument below `MINIMUM_OVERLAP`
observations and a factor below the variance floor both produce *no* betas.
That is the safe direction, and it means a freshly started process reports
nothing rather than something — a silence that looks like breakage and is not.

## What would make this wrong

**A capitalisation-weighted benchmark becoming computable.** If the platform
ever observes share counts and free float, an equal weight stops being the only
honest choice and this decision should be revisited rather than inherited.

**A second factor arriving.** The moment rates or credit exposure is estimated
from data rather than left `unmodelled`, `factor_betas` carries more than one
key and the single-factor framing here is a special case of something larger.
Nothing in this decision forbids that; it just does not do it.

**Evidence that the equal-weighted mean is degenerate for this universe.** If
the instruments on the tape turn out to be near-identical — which for five
synthetic names driven by shared factors is plausible — then every beta
approaches one and the decomposition explains nothing while appearing to work.
The check is whether `effective_bets` over a genuinely mixed book still
discriminates; if it does not, the factor is not earning its place.

**Anyone substituting zero for a missing beta.** That would not make the
decision wrong so much as void it: the whole value here is the difference
between unmodelled and immune, and a caller that erases it has taken the cost
of this decision without the benefit.

## Consequences

**What becomes measurable.** Positions acquire a market beta from the
platform's own tape; `effective_bets` reports an inverse Herfindahl over real
contributions; a stress scenario with an equity-style shock produces a P&L
attribution per position rather than an `unmodelled` list; and the attribution's
factor decomposition has a factor.

**What stays honest about its limits.** An instrument with too short an overlap
carries no beta and every consumer treats it as unmodelled. A scenario shocking
`rates` or `credit` still models nothing, because this decision adds no such
factor — and the `unmodelled` list is what says so.

**What must not follow from this.** No caller may substitute zero for a missing
beta. The whole reason `StressTester` separates `unmodelled` from a zero
contribution is that a position nobody could model and a position genuinely
insensitive to the shock are different facts, and a stress report that conflates
them understates the book.

**Reversibility.** The factor is computed, not stored: nothing is written to the
event log that a later decision would have to migrate. Replacing this factor
with a multi-factor model changes what `factor_betas` contains and breaks no
record, because the map is rebuilt every time it is read.
