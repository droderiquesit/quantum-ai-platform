# ADR 0092: A shown growth rate is the trailing realised rate over a declared window, or none, and a projection is never displayed

- **Status**: Accepted on the authority the owner delegated on 2026-09-19
  (ADR 0081's status line records the delegation); the design is a
  specification for the lanes that own the crates it names, and **nothing
  is built by this record**. `qip-capital` and `qip-routing` are held by
  other lanes tonight, and §4 says which half belongs to which.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Extends ADR 0066's "deliberately does not do"
  list, which said withdrawal drag and minimum viable scale "need a forward
  growth rate the platform does not measure", by deciding what the platform
  may show in their place. That sentence stands; this record is what
  follows from it.
- **Related**: ADR 0066 (reinvestment is planned and never performed; the
  two §18.4 rows it refused to build), ADR 0021 (capital leaves this
  platform by no path — so no withdrawal exists to charge drag against),
  ADR 0005 (confidence is arithmetic, never assigned — the same rule
  applied to a rate), ADR 0007 (exact attribution, the source the series is
  built from), ADR 0050 and ADR 0072 (a number this repository invents and
  quotes back as a measurement is refused in their domains, and in this
  one), ADR 0085 (a sentence that travels with the figure from the API, the
  pattern the label follows).

## Context

Blueprint §18.4 has five rows. Three are built and reached through the
cadence fold (ADR 0066, and the register's row: `CompoundingPolicy`,
`ReinvestmentDecision`, `ThresholdLadder`, a bounded `FeeVolumeLedger`,
and since 2026-09-19 a fee schedule the simulated exchange charges at the
rung the account's own volume selects). Two are not, and the register and
ADR 0066 give the same reason for both: "Withdrawal drag — every withdrawal
resets compounding. The frontend should show that cost, not hide it" and
"Minimum viable scale — the capital level at which the platform becomes
economically sensible, computed and shown honestly" each need a growth rate
to project forward, and "a compounding cost projected from an assumed
return is a number that would read as a measurement".

That reason was re-verified in this tree on 2026-09-20 and it holds, but
the sentence it rests on is narrower than a reader takes it to be. The
platform does not measure a **forward** rate. It does measure a **trailing**
one, and has since the demotion monitor needed a series of live returns to
judge:

- `backend/crates/runtime/qip-kernel/src/central/realised.rs` keeps, per
  strategy at each cell, a session-by-session series of attributed P&L
  (`RealisedSeries`), where a session is a UTC day the cell reported on and
  the figure is what the centre's own attribution booked — deliberately not
  the cell's claim about its own loss, "because two claims about the same
  P&L will disagree". The working set is bounded at
  `REALISED_SESSIONS` (252, a trading year); the record is the event log.
- `RealisedCalendar` re-keys that corpus by strategy and day, summed
  across cells, and `RealisedCalendar::desk(day)` is "the desk's own book,
  which is the only series the platform holds that a stress window could be
  cut from". Each day is a `GrantedDay { pnl: Decimal, grant: Decimal }`,
  and `GrantedDay::fraction` is the one place the two figures cross into
  `f64` — "Money is `Decimal` up to this line." A day on which a grant was
  held and nothing settled is a return of zero; a day with no live grant is
  absent, not zero.
- It is built in production: `CentralPlane::realised_calendar(now)`
  (`grep -n 'pub fn realised_calendar' backend/crates/runtime/qip-kernel/src/central/plane.rs`)
  is read by the family-structure measurement
  (`grep -rn 'realised_calendar(' backend/crates --include=*.rs | grep -v '/tests/'`
  prints the definition and one caller in `central/structure.rs`).

So the platform has a realised return series over granted capital, on a
calendar of sessions, built from the attribution and read by a control that
already decides something. What it does not have is any rate about the
future, and nothing in the tree could produce one without assuming it.

Three more facts shape what follows, each verified rather than assumed:

- **No withdrawal exists in this build.** ADR 0021 decides that capital
  leaves this platform by no path. There is no withdrawal event to charge
  drag against, and a "cost of withdrawing" shown to a user is therefore a
  figure about a hypothetical act — a projection twice over.
- **The platform already has a floor for judging a return series.**
  `qip_simulation_engine::baseline::MINIMUM_OBSERVATIONS` is twenty, "the
  floor both `GaussianHmm::fit` and `deflated_sharpe` impose", and
  `realised.rs`'s own documentation says the demotion monitor needs "twenty
  live observations before decay is judged".
- **The compounding seam is the kernel's, and it reads a point, not a
  series.** `qip_kernel::adaptive_cadence::policy_for` builds the
  `CompoundingPolicy` from `platform.equity()` and the mandate
  (`grep -n 'pub fn policy_for' backend/crates/runtime/qip-kernel/src/adaptive_cadence.rs`).
  No route in `qip-api` and no page in the portal shows anything about
  compounding today (`grep -rn 'compound\|reinvest' backend/crates/apps/qip-api/src/*.rs`
  and `grep -rli 'withdrawal drag\|minimum viable\|growth rate\|compounding' frontend/portal/src`
  both print nothing).

## Decision

### 1. The only growth rate the platform may show is the trailing realised return on granted capital over a declared window of sessions

The figure is computed from `RealisedCalendar::desk` and from nothing else:
not a cell's `Utilisation` claim, not a backtest, not a fitted model, not
the mandate's target, not an operator's number. Its window is a count of
**sessions** — days on which the desk held a live grant, as the calendar
holds them — declared by the caller and carried on the figure, so that
"trailing" is never read as "recent" without a number attached. Its
denominator is the grant the day's P&L was made under, because that is the
platform's own definition of a daily return (`GrantedDay::fraction`) and a
second denominator would be a second claim about the same fact. It is
compounded over the window — one unit of granted capital grown session by
session, less one — because a growth rate is what compounding over a
window means; the arithmetic mean of daily fractions is rejected below.
Money stays `Decimal` until the crossing `GrantedDay::fraction` already
names, and the implementing lane states the crossing in a comment where it
happens, as the rule requires.

### 2. A window that holds fewer than twenty sessions is refused, not shown

Twenty is the floor the platform already imposes before it judges a return
series, and this record adopts it rather than choosing a second number for
the same question. A book younger than that shows **no rate** — not zero,
not a dash, not the rate over the sessions it has — and the refusal is an
`Error::invalid` whose message names the window, the sessions held, and the
floor. A rate over five days is a measurement of noise, and a page that
shows it will be read as a measurement of the book.

### 3. A projected rate is never displayed, and the two §18.4 rows are shown only in their trailing forms

- **Withdrawal drag.** Until a withdrawal path exists — ADR 0021's
  reversal, its own record — there is no withdrawal, and the page shows the
  trailing rate with its window and the sentence that capital leaves by no
  path. If a path is ever authorised, drag on an actual withdrawal is
  computed at the rate the book realised **since** the withdrawal, over the
  sessions that have elapsed, and never at a rate the book is assumed to
  realise after today. "What this withdrawal will cost you" is not a
  sentence this platform utters; "what the capital that left on that day
  would have realised at this book's rate since" is.
- **Minimum viable scale.** The capital at which the trailing realised rate
  over the window covers a declared cost per session: `declared cost ÷
  trailing rate`. The cost is an operator's declared figure and is labelled
  *declared*, not measured — §18.2's "~$1,015 per month" is the blueprint's
  number, the cost router bills only the model spend that ran, and no
  infrastructure invoice is in the tree. When the trailing rate is not
  positive, the figure is **refused** with the sentence that no scale is
  viable on the realised record — not a large number, not infinity, not a
  rate clamped to a floor. A book that has not realised a positive return
  over the window has no viable scale on the evidence, and that is the
  honest answer.

### 4. The label travels with the figure, and each half has an owner

The rate, its window in sessions, and the word *trailing* are one value
from the API, on the pattern ADR 0085 set with `inflow_posting`: the
browser renders what the platform decided and may not compute, annualise
or relabel it. The frontend rule already says the browser holds no trading
logic; a growth rate computed in a page is a growth rate nobody journaled.

Ownership, so the row can say which crate each half belongs to and no lane
manufactures a caller to flip it:

- **The series and its window** are the central plane's — `qip-kernel`,
  `central/realised.rs` and `CentralPlane::realised_calendar`, which exist
  and are reached.
- **The arithmetic** — the trailing rate, the refusal under twenty, the
  drag-since form, the scale form — is `qip_capital::compounding`'s, beside
  `CompoundingPolicy`, taking per-session `(pnl, grant)` pairs as `Decimal`
  so the crate never depends on the kernel. `qip-capital` is lane W4a's
  tonight, and this record hands it a design rather than a diff.
- **The surface** is a `qip-api` route answering the figure with its label,
  and a portal page rendering it beside `PAPER TRADING`. The kernel seam
  that would supply the pairs is beside `adaptive_cadence::policy_for`.
- **The row's other half is not this record's.** §18.4's fee-tier gap —
  `VenueProfile::listed` has no production constructor
  (`grep -rn 'VenueProfile::listed' backend/crates --include=*.rs` prints
  test files only) — is `qip-routing`'s and lane W5's, and nothing here
  touches it.

## Consequences

- Register row §18.4 keeps its verdict, `PARTIAL`, and says which crate
  each remaining half belongs to. Nothing moves in the tree on this
  record's account.
- No file under `backend/`, `frontend/` or `infrastructure/` changes. The
  paper-trading boundary is untouched at all three layers — Terraform's
  ceiling refusal, `AutonomyLevel::deployable` at the composition roots,
  and the type system in `qip-edge`'s `Cell` and `qip-cost-router`'s
  `Determinism::Required`. A rate is a number on a page; nothing here sizes,
  reserves, orders or moves capital, and ADR 0066's rule that reinvestment
  is planned and never performed is not loosened by a figure beside the
  plan.
- ADR 0066's sentence — the platform does not measure a forward growth rate
  — stands unchanged. This record does not measure one either.

## What it costs

**The blueprint asked for a forecast and gets a history.** "Every
withdrawal resets compounding. The frontend should show that cost" is a
cost before the act, and a cost before the act is a projection. This
record refuses to show it, and says so on the page rather than showing a
trailing figure where a reader expects a forward one. A user who wanted to
know what withdrawing tomorrow would cost is told what the book realised
until today, and that is less than they asked for.

**A young book shows nothing.** Twenty sessions is four trading weeks with
a grant held every day, and longer for a book granted intermittently,
because the window counts sessions and not calendar days. Until then the
page carries the refusal and no number. That is by design and it will be
read as a gap.

**The window moves the number, and so it is printed.** A twenty-session
rate and a sixty-session rate on the same book differ, and neither is "the"
rate. The figure carries its window so a reader cannot compare two without
seeing they are different questions, and that makes the page busier than a
single percentage would.

**The denominator is the grant, not the equity.** The figure is a return
on capital *committed*, which is what the platform measures, and not the
growth of the book's equity, which it does not measure as a series —
`platform.equity()` is a point and `CashBalance` is a state. A reader who
wants "how much has my money grown" is given "how much did what was put to
work return", and the label must say which.

**The cost input is declared, not billed.** Minimum viable scale divides a
measured rate by a number an operator typed. The rate half is evidence; the
cost half is an assertion labelled as one, and the figure is only as honest
as the label.

**A desk-level figure only.** The calendar exists at the centre, summed
across cells. No cell surface and no per-region page can show a rate,
because the series does not exist there.

## What would make this wrong

- **A withdrawal path is authorised.** ADR 0021's reversal, in its own
  record. Then §3's drag-since form gains its event, and the sentence that
  capital leaves by no path comes off the page. The trailing rate and the
  refusal to project are unchanged by that day.
- **The owner wants a projected figure.** Then a separate record decides
  what assumption it rests on and requires the assumption displayed beside
  the number, in the same size, every time — and that record argues with
  ADR 0005 and ADR 0050 in the open, because a rate assumed and shown is
  the shape both refuse.
- **An equity series is journaled per session.** If the ledger ever records
  the book's equity at each session close as a replayable series, the
  denominator question reopens: a growth rate on equity becomes measurable,
  and this record's choice of the grant is then a choice between two
  measured figures rather than the only one available.
- **A billed cost lands.** If infrastructure cost is ever read from an
  invoice into the tree the way model spend is billed by the cost router,
  *declared* becomes *billed* in §3 and the label changes with it.
- **Twenty proves wrong for this purpose.** The floor is borrowed from the
  HMM and the deflated Sharpe because one floor for "enough sessions to say
  anything" beats two. If a lane shows, on the realised record, that twenty
  sessions of desk returns are not enough to bound the trailing rate at a
  stated confidence, the floor moves — up, with the evidence, and in
  `qip-capital` where the arithmetic lives.

## Alternatives considered

**Assume a rate — from the backtest, the mandate's target, or a fitted
model — and project.** Rejected. It is the sentence ADR 0066 wrote and the
shape ADR 0050 and ADR 0072 refuse in their domains: a number this
repository would make up and then quote back as a measurement. A backtest
rate is a claim about a simulator; a mandate target is a wish; a fitted rate
is the model's, not the book's.

**Annualise the trailing rate.** Rejected. Scaling twenty sessions to a
year is extrapolation wearing a measurement's clothes, and it is the
single most common way a small book's noise is presented as an engine. The
window is printed, never scaled.

**The arithmetic mean of daily fractions.** Rejected. A book that loses
half and then gains half averages zero and is down a quarter; a growth rate
that cannot see that is not a growth rate. Compounding over the window is
the only form that answers "what did one unit of granted capital become".

**Use the cell's `Utilisation` claim as the series.** Rejected for
`realised.rs`'s own reason: the centre reads its attribution because two
claims about the same P&L will disagree, and a figure argued from the
cell's number would be contested by the centre's own books.

**Show nothing at all.** Rejected. §18.4 says "computed and shown honestly",
and the trailing figure *can* be shown honestly. A page that hides a
realised number because it cannot show a projected one hides evidence, and
the register already records what happens when a control that could fire
is left unwired.

**Compute it in the browser from the API's raw series.** Rejected. The
frontend rule keeps trading and risk logic out of the browser, and a rate
computed on a page is a rate nobody journaled and nobody can replay.

## Dependency-direction argument

Nothing in this record adds, removes or redirects an edge. The arithmetic
lands in `qip-capital`, a service, taking `Decimal` pairs and depending on
nothing new; the series stays in `qip-kernel`, the runtime, which already
depends on `qip-capital` and would call it, never the reverse; the route is
`qip-api`'s and the page is the portal's, both leaves. The two-dependency
rule is untouched: `scripts/check-dependencies.sh` stays at eleven.
