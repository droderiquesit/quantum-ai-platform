# ADR 0069: A collateral graph refuses the arrangements it cannot cost, and a physical rate carries its source

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default and in fact), ADR 0002 and ADR 0009 (two dependencies), ADR 0021 (no route by which capital leaves the platform), ADR 0007 (exact attribution)

## Context

Two blueprint sections scored `ABSENT` in `docs/DELIVERY-STATUS.md`, and both
are sections where the honest deliverable is smaller than the section and the
dishonest one is easy to write.

**§25.6, the cross-margin model**, wants six things: a collateral graph with
haircuts per margin regime, portfolio versus isolated margin per venue,
rehypothecation, correlated collapse, cross-venue collateral, and a
liquidation cascade modelled before it happens. What existed was
`qip_capital::margin::MarginModel` — one requirement against one collateral
balance for one book, with no caller — and the status row said so.

A single scalar requirement is not a weaker version of §25.6; it is an
*assumption* of §25.6's most dangerous row. A book reported as one requirement
against one collateral balance is a book assumed to be cross-margined
everywhere, which is the rarest arrangement in the market.

**§17.4, physical**, wants a logistics, customs and spoilage cost model for
physical commodities and retail arbitrage, plus an auction engine. Its own
status column says the first two rows are "reachable with a logistics engine"
and calls the second "a genuinely different cost model".

The trap in both is the same and this repository has a name for it. A
rehypothecation limit nothing computes a chain for, a liquidation-cascade
threshold no book can cross, a spoilage rate nobody measured: each reads as
protection, each is the `MaxExpectedShortfall` shape, and each makes a
delivery row go green while changing nothing a desk could act on.

## Decision

**One. The graph refuses every arrangement that would make the collateral
balance larger than the collateral.** `qip_capital::collateral::CollateralGraph`
has one constructor, no setter, and refuses: an asset pledged beyond its mark
across every domain; a haircut outside `[0, 1)`; maintenance above initial; a
pledge naming an asset or domain the graph does not hold; a re-pledge out of a
domain that forbids rehypothecation, into an isolated domain, or beyond what
is posted at the source. Each refusal names what to do instead, because each
is a caller's model being wrong rather than a transient condition.

**Two. Isolated margin and forbidden rehypothecation are the `Default`.**
There is no constructor producing the other arm implicitly. Portfolio margin
and permission to reuse are terms of specific agreements with specific
counterparties; a model that assumes either produces a collateral pool larger
than the assets. Fail closed, as the working agreement's third principle
requires.

**Three. A re-pledge chain deeper than one link is refused, and this is the
decision most likely to be reopened, so here is the argument.** A venue that
is the destination of one re-pledge may not be the source of another. The
cycle A → B → A falls out of the same rule and is obviously wrong: one asset
standing behind itself. The chain A → B → C is not obviously wrong and is
refused anyway. What B re-pledges onward comes out of a pool holding A's
collateral beside every other client's, and *which* of it travelled to C is a
fact B knows and this platform does not. Modelling it means choosing an
attribution rule — pro rata, first in, A's alone — and every one of those is a
number this repository would invent and then quote back as a measurement. A
desk that has genuinely signed a chain finds out at construction rather than
reading a coverage figure derived from an assumption nobody made.

**Four. A cascade terminates by construction, not by a cap.** A domain is
closed out at most once, so `CollateralGraph::cascade` runs at most `domains`
times. An iteration cap would make a genuinely deep cascade indistinguishable
from a bug.

**Five. An unobserved account is never reported as an empty one.**
`qip_kernel::cross_margin::review` builds a margin domain only for a venue
that has a statement. A venue carrying exposure with no statement leaves by a
different door, as `UnobservedExposure`. This is the load-bearing decision in
the kernel module: a domain with no pledges has zero cover against a positive
maintenance, which is arithmetically identical to an account somebody looked
at and found empty, and the two call for opposite actions. It is the same
discipline `Platform::reconcile_wallet` already follows for the wallet.

**Six. Rehypothecation reuse and a multi-step cascade are library-only, and
the delivery row must say so.** A statement is keyed by venue *and* asset, so
nothing this platform records says one holding stands behind two obligations.
There is therefore no reuse to find and every cascade built from platform
state is one step long. Those are not fields on `CrossMarginReview`: a field
structurally empty on every production path is exactly the control that reads
as protection and cannot fire, and an `Option` that is always `None` is the
same thing wearing a type. What would populate them is a declared cross-venue
or rehypothecation agreement — a term somebody signs, which this platform has
never been given.

**Seven. A physical cost model states its rates and defaults none of them.**
`qip_financial::physical::LogisticsTerms` has no `Default`. Nothing in this
platform ingests a freight rate, a duty schedule, a spoilage rate or a
marketplace fee table, and a default freight rate would be a number this
repository invented.

**Eight. A spoilage rate carries its source, structurally.**
`qip_financial::physical::Spoilage::new` refuses a blank source. Freight has
an invoice and duty has a schedule; shrinkage has whatever somebody remembers.
A figure carried with its source is a figure the next reader can check; the
same figure alone is quoted forward for ever by people who assume somebody
measured it. Making it a field rather than a comment is the difference between
a discipline and a hope.

**Nine. Spoilage compounds and every quantity-dependent charge reads the
surviving quantity.** Linear loss — `quantity * (1 - rate * days)` — reports a
negative delivered quantity past `1 / rate` days, and a cost per unit computed
off it is a negative number presented as a cost. Duty is charged on what
reaches the border and storage on what is still on hand, because a consignment
that has lost a third of itself is not paying to store the third that is gone.

**Ten. Nothing in either module reaches a venue, a broker or an order.** Both
are reads, in the shape `qip_capital::margin` already is. A requirement, a
coverage and a landed cost are facts about a book, and what a desk does about
them is a decision with a person in it.

## Consequences

The platform now refuses to build a collateral arrangement whose arithmetic
would exceed its assets, and it now distinguishes exposure at a venue nobody
has read a statement for from exposure at a venue observed and found short.
The second is the finding that fires today: every deployment has counterparty
exposure and no statements, and the finding closes one venue at a time as
statements arrive through `Platform::observe_statement`.

§25.6 is `PARTIAL`, not `DELIVERED`, and the six rows divide as follows.
Built and reachable from platform state: what collateralises what, portfolio
versus isolated margin. Built and exercised only in tests, because no
production input can populate them: rehypothecation, correlated collapse,
cross-venue collateral, liquidation cascade. The rehypothecation and
cross-venue rows would be populated by a declared agreement; the
correlated-collapse row by a statement of what moves a venue's own obligation,
which the causal graph could supply and does not today.

§17.4 is `PARTIAL`. Rows one and two have a cost model with no ingestion
behind it. Row three — auction bidding, winner's-curse adjustment, reserve
estimation — is untouched: a winner's-curse adjustment needs a distribution of
rival bids, and nothing here has one. Building it from an assumed distribution
would be decision six's mistake in a new place.

## Dependencies

None added. `qip-capital` already depended on `qip-core`, `qip-contracts`,
`qip-financial` and `qip-portfolio`; `qip-financial` on `qip-core`,
`qip-events` and the two permitted crates; `qip-kernel` on every crate the new
module names. `./scripts/check-dependencies.sh` reports 11 third-party
packages, all permitted, unchanged by this record. ADR 0002 and ADR 0009 are
untouched.

## What it costs

**A desk with a genuine two-step rehypothecation agreement cannot record it.**
That is decision three's intended cost. The way back is an ADR arguing for a
specific attribution rule with a specific counterparty's documentation behind
it — not a default chosen in a hurry to make a chain build.

**Four of §25.6's six rows are library code with no production input.** The
platform carries the arithmetic and the tests for reuse, correlated collapse,
cross-venue collateral and a propagating cascade, and no deployment can
exercise any of them. That is dead weight until an arrangement exists, and it
is worth carrying only because the refusals around it are what stop a later
lane inventing the input.

**A haircut and a spoilage rate are declared numbers a reader must argue
with.** `NON_CASH_HAIRCUT` and every rate on `LogisticsTerms` are stated
assumptions, not measurements. A desk that adopts them without replacing them
is running on figures this repository chose. The mitigation is that they are
findable, named, and — for spoilage — impossible to state without a source
beside them; the cost is that findable is not the same as correct.

**The kernel review is a read that refuses nothing.** It does not stop an
order, size a position or withdraw a venue. A venue the platform has never
seen a statement for keeps taking fills. Giving it teeth means a pre-trade
gate on a figure derived from an operator-supplied statement, which is a
different decision with a different failure mode — a stale statement halting a
book — and it is not taken here.

**`qip-capital` gained a module and the crate documentation gained a bullet.**
Its head now says "eight of the modules in brief" rather than a count that was
already two short of the module list.

## What would make this wrong

**A declared cross-venue or rehypothecation agreement arriving.** The moment a
desk records one, decisions two and six are the binding constraint rather than
a description of the evidence, and the review gains fields it deliberately
lacks today. Revisit this record then, not before.

**Evidence that the one-link depth limit refuses a common arrangement.** The
argument for it is that attribution through a mixed pool is unknowable here.
If a counterparty's documentation actually states the attribution — some do —
the refusal is refusing something costable, and decision three should become a
rule keyed on whether that statement exists.

**A second writer of the coverage walk.** `CollateralGraph::coverage` and
`cascade` both go through `coverage_excluding`. A second derivation of the
same figure would be the `MaxExpectedShortfall` shape in a new place: two
answers to one question, one cycle away from disagreeing, with a control
reading the wrong one. `grep -n 'fn coverage' backend/crates/services/qip-capital/src/collateral.rs`
should find exactly the public entry point and the one private walk.

**A production caller for `qip_financial::physical`.** If an ingestion path
for carrier tariffs or a customs schedule lands, decision seven's "no
`Default`" becomes a friction rather than a guard, and the right shape is a
declared-terms record loaded from the catalogue with the same provenance
discipline `Spoilage` already enforces.

**The unobserved finding never closing in practice.** If statements are never
handed in, the finding is an alarm that fires for ever and an operator learns
to ignore it, which is worse than no alarm. That is a fact about operations
rather than about this code, and it is the thing to watch first.

## Paper trading

Untouched, and each of the three layers checked rather than assumed.
Terraform's `infrastructure/terraform/variables.tf` still refuses the three
live ceilings at plan time; `AutonomyLevel::deployable` still stops the three
central composition roots on one; `qip-edge`'s `Cell` still has no constructor
taking a ceiling other than paper trading and `qip-cost-router`'s
`Determinism::Required` arm still returns a type that cannot name a model
rung. Nothing in this change touches a composition root, an autonomy value, a
broker, a venue adapter or an order path.
`qip-acceptance/tests/cross_margin.rs` asserts the last of those rather than
stating it: a platform driven to a real fill, reviewed twice, has the fills it
had and none of them live, and the kernel module's production source names no
broker, no placer and no order manager.
