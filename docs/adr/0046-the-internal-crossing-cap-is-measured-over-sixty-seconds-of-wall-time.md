# ADR 0046 — The internal-crossing cap is measured over sixty seconds of wall time, not over passes

- **Status:** **proposed**
- **Date:** 2026-09-06
- **Numbering note:** drafted as 0043 and renumbered on discovering that a
  record written in parallel had taken that number and indexed it. This note
  used to end "the mis-numbered file at
  `docs/adr/0043-the-internal-crossing-cap-…` is a stub pointing here and
  should be deleted; this session had no shell and could not remove it."
  **The stub is gone** — checked 2026-09-06, `ls docs/adr/0043*` returns only
  `0043-the-cryptography-this-platform-has-and-the-three-gaps-no-crate-closes.md`
  — so that instruction is withdrawn rather than left standing, because a
  record that sends a maintainer to delete a file which does not exist costs
  them the same search whether or not the file is there. 0043 is the
  cryptography record and nothing else.
- **Decides:** DEC-D3 / PHASE-B13 — the interval §27.1's forty percent
  internal-crossing cap is measured over, which
  `CellConfig::crossing_interval` has held as `None` since the mechanism
  landed and which no composition root sets.
- **Relates to:** ADR 0008 (a cell decides alone, on what it was handed —
  the cap is one of the things it was handed), ADR 0003 and ADR 0021 (the
  paper boundary, untouched below), ADR 0035 (the one node in shadow mode,
  the only place this would first be observed), ADR 0024 (the execution
  node's shape, which is why the *pass* is the wrong clock).
- **Does not touch:** the paper-trading boundary. A cross reaches no venue
  by construction — `Cell::book_cross` moves two strategy books and
  deliberately leaves `positions`, the venue-facing aggregate, alone — so
  nothing here creates, eases or implies an order path. No crate is added.
  `qip-contracts` is unchanged.
- **Applies nothing.** No code changes under this record. The interval is a
  value a composition root must set, and setting it is an implementer's
  commit against this decision, not this decision.

## The failure the cap prevents, named from the code rather than from §27.1

§27.1 states the cap and its reason in one sentence
(`docs/architecture/algorik-blueprint-v10.1-source.md:2377`):

> Forty percent of gross intent per instrument per interval. Above that a
> persistent internal market forms whose marks drift from reality; below
> roughly twenty percent free netting value is left unclaimed

"Marks drift from reality" is the blueprint's phrase. What it means in this
tree is specific, and worth writing out because the specificity is what
makes the interval a safety parameter rather than a tuning knob.

`Cell::cross_internally` (`backend/crates/edge/qip-edge/src/cell.rs:3464`)
computes the matched size as `min(buy, sell)` over one net's contributors,
prices it at the book's prevailing mid read at that instant, and hands the
record to `Cell::settle_cross` (`:3648`), which calls `book_cross` (`:3682`).
`book_cross` moves exactly two things per side: `strategy_positions` and
`strategy_cash`. Its closing comment says what it does not move, and that is
the whole of the exposure:

> `positions`, the venue-facing aggregate, is left alone: the two lots sum
> to zero and the venue saw nothing.

So a cross is a transfer of inventory and cash between two of the platform's
own strategies, at a price the platform read off its own book, which no venue
ever confirmed and which the cell's venue-facing position never records. Two
consequences follow, and together they are the failure:

1. **A cross is structurally invisible to the reconciliation that would
   otherwise catch it.** The cell's break counter,
   `qip_edge_reconciliation_breaks_total`, is described as "disagreements
   between the cell's fills and the venue's own account"
   (`qip-edge/src/telemetry.rs:258-261`); the drop copy checks fills. A cross
   is not a fill, and the venue has no account of it. There is no second
   party anywhere to disagree with a crossed price. Every other price in the
   cell is eventually adjudicated by somebody outside it. A crossed price is
   adjudicated by nobody.
2. **The strategy books that carry crossed lots and crossed cash are the
   books the LEARN stage scores.** `strategy_cash` after a run of crosses is
   a profit-and-loss statement whose only price source is the platform's own
   read of a book it did not trade on. A strategy pair that crosses with each
   other repeatedly can therefore be scored, promoted and funded on
   performance that no venue ever validated — and the two sides' cash legs
   are equal and opposite, so in aggregate the cell looks flat while the two
   strategy books diverge.

That is the "persistent internal market". It is not a market that trades
badly; it is a market whose prices are unfalsifiable. The cap bounds how
much of a strategy set's gross intent may be settled in it.

**The cap refuses; it does not clamp.** `cross_internally` refuses the whole
cross above the cap rather than crossing the permitted two fifths, and the
comment at `:3419-3425` gives the reason — crossing to the cap every interval
and abandoning the rest "would build exactly that market, just more slowly".
That is settled and this record does not reopen it.

**What the counter counts, and what it does not.**
`qip_edge_internal_crosses_total{cell,region,venue}` is incremented once per
sealed cross in `Cell::record_cross` (`cell.rs:3884`), by venue. It is a
*count of crosses*, not a volume: nothing published carries the crossed
quantity or the window's gross. This matters below, under evidence.

## What the cap actually bounds today, and why that is not §27.1

With `crossing_interval` unset — the default at `CellConfig::new`
(`cell.rs:108`) and the value at every composition root, since
`grep -rn 'with_crossing_interval\|crossing_interval' backend/crates/apps`
returns nothing — the window is empty by construction (`crossing_window`
returns zero, zero, not-full when no interval is configured, `:3789-3793`)
and the cap is measured against the single net in front of it.

The arithmetic of that default is stated in the code (`:3427-3438`) and is
the reason a decision is owed here at all: the numerator is
`min(buy, sell)`, the denominator is `buy + sell`, so the ratio cannot exceed
one half and reaches one half exactly when the two sides cancel completely.
A forty percent cap therefore fires only in the narrow band above two fifths
— **and a net that cancels to zero is always refused.** §27.1's flagship
sentence is the case the default rejects:

> Strategies that disagree cost nothing to run together because their
> disagreement never reaches a venue.

Under the default that disagreement is never booked as a cross at all. The
two strategies still net, so nothing extra reaches the venue; what is lost is
the settlement — neither strategy receives the fill §27.1's attribution row
promises it ("Both strategies receive their full intended fill at the
crossing price. Neither is disadvantaged"), and the value the blueprint
prices at "below roughly twenty percent free netting value is left
unclaimed" is left entirely unclaimed.
`with_no_interval_the_same_two_passes_never_cross`
(`qip-edge/tests/crossing.rs:292`) holds that default in place deliberately.

The default is safe and is *less* than §27.1 asks for. It is not the rule.
Choosing the interval is what turns the default into the rule.

## The decision

Two parts, and the first is the load-bearing one.

### 1. The window is `CrossingInterval::Span`, never `CrossingInterval::Passes`

`CrossingInterval` offers both forms (`cell.rs:81-87`): `Passes(n)`, the last
*n* calls to `Cell::work`, and `Span(d)`, every net inside a trailing span of
wall time.

**`Passes(n)` must not be used in this deployment, because a pass is not an
interval this platform controls.** `qip-edge-node` has no scheduler. Its
serve loop runs one iteration per accepted TCP connection on the health port,
and `run_pass` is called inside that iteration
(`qip-edge-node/src/main.rs:714-786`); the loop's own comment says so at
`:749-754`:

> One exchange with the central plane per probe, for the same reason and with
> the same caveat as the flush above: this node has no scheduler, so the
> liveness probe is the only periodic event it has. A production cell runs
> this on a timer, because tying capital renewal to how often something asks
> whether the cell is alive is a compromise, not a design.

The same compromise governs the pass. The only committed periodic caller of
that port is the Ops Agent's Prometheus receiver, at thirty seconds
(`infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl:218`,
`scrape_interval: 30s`). So under `Passes(n)` the wall-clock length of the
window in which a safety control fires would be set by an observability
component's scrape interval, and would halve if a second scraper were added
or if a load-balancer health check were pointed at the same port. A risk
parameter whose value can be changed by editing a monitoring config, by
someone who has no idea they are changing it, is not a risk parameter. It is
an accident with a name.

`Span` is measured off the cell's own clock —
`sample.at >= now.saturating_sub(span)` (`cell.rs:3780`) — and is therefore
the same window whatever the probe rate. That is the form.

*The one thing `Passes` has that `Span` does not*, and it is real: `Passes`
does not stretch when the cell is idle, and `Cell::work` increments the pass
counter before the halt check precisely so a halted cell's window does not
stretch over more wall time the longer it is stopped (`cell.rs:1663-1666`).
Under `Span`, a cell that halts for ten minutes returns to an empty window
and its first net after the halt is judged per net — which is the *safe*
direction (a full cancellation is refused), so the property is preserved
where it matters and lost only where losing it costs nothing.

### 2. The span is sixty seconds

`CrossingInterval::Span(Duration::from_secs(60))`, set at the node's
composition root, applied per instrument by the key `cross_internally`
already uses (`crossing_key`, `venue/object/representation`, `:3762`).

Four independent reasons converge on sixty, and they bound it from both
sides.

**It is the fastest clock the cell already lives by.** The slots
`qip-edge` actually reads from a policy payload are `cycle_whitelist`,
`compiled_plan`, `capital_grants` and `feasibility_constraints`
(`grep -n 'payload()\.' backend/crates/edge/qip-edge/src/cell.rs`). Of those
the shortest time to live is the cycle whitelist's, sixty seconds
(`qip-contracts/src/policy.rs:128`), and `Cell::cycle_whitelist` reads it
fresh-only, returning `None` the instant it goes stale (`cell.rs:715-733`).
The whitelist is what decides which strategies may be running at all. A
crossing window longer than one whitelist lifetime would measure a ratio
across two different strategy populations — attributing one population's
crossed size to another population's gross intent — and the number it
produced would describe no set of strategies that ever existed together.
Sixty seconds is the longest window over which "the strategies that could be
crossing" is one answer.

**It is twelve times the book staleness the cell defaults to.**
`max_staleness` is five seconds (`CellConfig::new`, `cell.rs:105`): past that
a book's prices stop counting. A cross is priced at the mid at the netting
instant, so the window ought to span a stretch of market the cell would still
consider priceable, several times over, and no more. Sixty is several times
over. Six hundred would be measuring a ratio across a market the cell has
already declared unpriceable a hundred times.

**It is three orders of magnitude clear of the window's own refuse-all
bound.** `MAX_CROSSING_WINDOW_SAMPLES` is 1,024 per instrument (`cell.rs:97`),
and at that bound the cap stops measuring and refuses *every* cross until the
history drains, under the `internal_cross_window` gate (`cell.rs:3512-3526`)
— deliberately, because "a cap measured against part of its window is a cap
that fires late". A span long enough to hold 1,024 nets for one instrument
converts the cap from a ratio into an exhaustion refusal. At the one
committed pass rate (thirty seconds) a sixty-second span holds two or three
samples. Reaching 1,024 samples inside sixty seconds would need roughly
seventeen passes per second on a single instrument, which is two to three
orders of magnitude above anything this deployment does. An hour-long span,
by contrast, reaches the bound at about one pass every three and a half
seconds — well inside plausible.

**It lands the realised crossing ratio inside the blueprint's own band, by
arithmetic.** This is the argument that fixes the number rather than merely
bounding it, and it is derived here rather than measured; it is stated so it
can be checked and falsified.

At the committed thirty-second pass rate a sixty-second span holds the
current net plus the two before it — the same window
`over_a_three_pass_interval_a_repeated_full_cancellation_crosses_on_the_second_pass_at_the_mid`
(`qip-edge/tests/crossing.rs:209`) drives with `Passes(3)`. That test records
the trace: two strategies cancelling completely at 100 each, so gross 200 and
matched 100 per pass. Pass one has no window behind it, is judged per net,
and is refused. Pass two crosses (100 against a window gross of 400). Pass
three crosses. Pass four is refused, and the test says why it must be — "the
fourth pass crossed, so the window did not slide with the passes". Continuing
the same arithmetic past the test's end gives a period-three steady state of
admit, admit, refuse, and a realised crossed-over-gross ratio of 200 over 600:

    33.3%

which sits inside §27.1's stated band — under the forty percent above which
the internal market becomes persistent, and above the "roughly twenty
percent" below which free netting value is left unclaimed. The default
(per-net) sits at zero percent for this case. That is the whole distance the
decision covers.

**Unverified, and named as such.** The steady-state figure above is
arithmetic continued from a test trace, not the output of a test that was
run. No test in the tree asserts a realised ratio, and this session ran no
cargo command (see "What was not run"). The implementer taking this record
should write the assertion before setting the value; it is named under
"The tests that would prove it".

## Rejected alternatives

**Leave it unset (keep the per-net default).** This is the status quo and it
is genuinely safe: nothing crosses that should not, and a full cancellation
is refused by arithmetic. It is rejected because it is safe by refusing the
case §27.1 is written to enable, and because leaving it unset while calling
DEC-D3 answered would be the worse of the two failures this repository
guards against — the control reads as configured and measures something the
blueprint did not ask for. If this record is declined, the honest outcome is
not "unset by default" but "unset **as a decision**, with §27.1's
attribution row recorded as deliberately unimplemented in the traceability
matrix". A gap named is a different object from a gap left.

**`Passes(n)` for any n.** Rejected on the shape of the guarantee, not on the
number: the window's wall-clock length would be owned by whoever connects to
the health port. Rejected even though `Passes` has the better idle-cell
property, because that property is safe-direction-only under `Span` and the
prober problem is not.

**A long span — one hour, matching the `capital_grants` slot's own time to
live (3,600 s, `policy.rs:126`).** Superficially attractive, because it
matches the clock the cell's capital already runs on. Rejected on two counts:
the ratio would be measured across sixty whitelist lifetimes, so the
strategy population in the denominator is not the one in the numerator; and
the 1,024-sample bound is reachable at about one pass per three and a half
seconds, at which point the cap becomes a refuse-all that reads in the log as
a cap firing on its ratio when it is firing on its buffer.

**A very short span — five or ten seconds.** Rejected because at the
committed pass rate it holds one sample or none, which is the per-net default
wearing a span's clothes: it would be a configured value that changes
nothing, which is worse than the unset default because it looks decided.

**A per-instrument or per-venue interval table.** Rejected as a second
configuration surface with no evidence to fill it. §27.1's "per instrument
per interval" says the window is *keyed* per instrument — which
`crossing_key` already does — not that its *length* varies per instrument.
Nothing in the tree could supply a per-instrument length today, and a table
of guesses is worse than one argued number.

**Deriving the interval from the pass rate at runtime** (e.g. "the last three
passes, but at least thirty seconds"). Rejected: it makes the window a
function of two clocks, so an operator reading the configuration cannot say
what the window is without also knowing the probe rate. One clock.

## What it costs

- **Crossing starts happening.** The default books no cross for a full
  cancellation; with a span, a repeated cancellation is booked roughly two
  passes in three. Each booked cross moves `strategy_positions` and
  `strategy_cash` at a mid no venue confirmed, which is precisely the
  exposure the first section describes — bounded at a third rather than
  eliminated. This is a deliberate exchange of an unfalsifiable-price
  exposure, capped, for the netting value §27.1 says is otherwise unclaimed.
  It should be read as an exchange and not as an improvement.
- **Memory, bounded and small.** `observe_crossing` allocates nothing while
  the interval is unset (`cell.rs:3834`); with a span it holds one
  `CrossingSample` per net per instrument inside the window, capped at 1,024
  per instrument.
- **One more thing to configure at the node, and one more way to get it
  wrong.** `with_crossing_interval` refuses zero passes, a zero span and a
  pass count above the sample bound (`cell.rs:119-147`) — it refuses rather
  than clamps — so a bad value stops the node rather than silently becoming a
  different control. It does not refuse a span that is merely unwise.
- **The evidence to tune it does not exist yet** — see below. Setting this
  value without adding the series is setting a safety parameter that cannot
  be checked afterwards.

## The evidence that would revise this, and the gap that stops it

**The gap first, because it is the most important sentence in this record.**
The number that would confirm or refute sixty seconds is the realised
crossed-volume-over-window-gross ratio, per instrument. **No published series
carries it.** `qip_edge_internal_crosses_total` is a count of crosses by
venue (`cell.rs:3884`), not a volume. `qip_edge_netting_ratio` is gross
intent over *net order volume* (`WorkReport::netting_ratio`,
`cell.rs:183-187`), which is a different ratio with a different denominator.
`qip_edge_refusals_total{gate}` carries `internal_cross_cap` (proven at
`qip-edge/tests/telemetry.rs:489`) and so counts how often the cap fired, but
a refusal count with no volume underneath it cannot distinguish a cap firing
at 41% from one firing at 50%.

So this decision, if applied as it stands, is tunable only by refusal counts.
The implementer should treat "record the crossed size and the window gross"
as part of the same change, not as a follow-up: a safety parameter set
without the series that judges it is exactly the pattern
`.claude/rules/domains/observability.md` exists to stop.

With that series in place, three readings would revise this record:

- **The realised ratio sits below twenty percent.** §27.1's own lower bound:
  free netting value is being left unclaimed and the span is too short.
  Lengthen — and re-check the sample bound at the same time.
- **`qip_edge_refusals_total{gate="internal_cross_cap"}` tracks the pass
  count.** The window is not filling; the span is behaving as the per-net
  default and the value is decorative. Most likely cause: the pass rate is
  far lower than the scrape rate assumed here.
- **`qip_edge_refusals_total{gate="internal_cross_window"}` is ever
  non-zero.** The 1,024-sample bound is being reached inside sixty seconds,
  which means the pass rate is three orders of magnitude above what this
  record assumed, and every arithmetic argument above needs redoing.

And two facts about the deployment would revise it independently of any
series:

- **The node gains a scheduler.** The main-loop comment at `:749-754` says a
  production cell runs its periodic work on a timer. If that happens, the
  pass rate stops being the probe rate, and the "two or three samples per
  window" arithmetic — the argument that fixes sixty rather than merely
  bounding it — must be recomputed against the timer.
- **A second prober appears on the health port.** Under this record's `Span`
  form that changes the number of samples in the window but not its length,
  which is the whole reason `Span` was chosen; under `Passes` it would have
  silently halved the control. Worth stating so that a future reader can see
  the choice paying for itself.

## What would make this wrong

Outright, as distinct from the readings above that would merely revise the
number.

- **If a cross turns out to be reachable by the reconciliation path after
  all** — if a venue or a drop copy can adjudicate a crossed price — then the
  "unfalsifiable price" argument in section one collapses, the cap is a
  tuning parameter rather than a safety control, and the interval should be
  chosen for netting value alone, which argues for a longer window.
- **If §27.1's "interval" is meant as the *accounting* interval** (a
  regulatory reporting period — a day, a session) rather than a rolling risk
  window, then sixty seconds answers a different question from the one the
  blueprint asked, and this record is wrong in kind rather than in degree.
  The blueprint text quoted above does not say which it means; this record
  reads it as a rolling risk window because that is what the implemented
  mechanism is and because "above that a persistent internal market forms" is
  a statement about a live condition, not about a report. A reader who can
  establish the other reading should supersede this record rather than change
  the number in it.
- **If the strategy population turns over faster than the whitelist's sixty
  seconds** — if strategies are admitted and withdrawn inside one whitelist
  lifetime — then the "one strategy population per window" argument does not
  hold at sixty either, and the window must shorten to whatever the true
  turnover is, accepting that it collapses toward the per-net default.

## What changes, by crate, if accepted

| Crate | Layer | Change |
|---|---|---|
| `qip-edge` | edge | **None to the mechanism.** `CellConfig::with_crossing_interval`, `CrossingInterval::Span`, the window and the cap all exist and are tested. The crossed-size and window-gross series named under evidence would be added here, in `telemetry.rs` and at the two sites in `cell.rs` that already know both numbers |
| `qip-edge-node` | app | The composition root reads the interval and calls `with_crossing_interval`, refusing to start on a value the constructor refuses; the banner prints the window in force, as it prints `region_ceiling` and `region_bound`, so a node's log says what its cap is measured over |
| `infrastructure/terraform/modules/execution-node` | infra | The startup template writes the line, beside `QIP_VENUE_FEED`. Not applied by this record and not applied by anything: `execution_nodes = {}` in every environment |
| everything else | — | Nothing. No lib, no service, no contract, no dependency |

## The tests that would prove it

Each named as a property, premise asserted first, mutation-verified, and the
mutation named beside it. None of these exists today.

- `qip-edge/tests/crossing.rs::a_span_window_slides_with_the_clock_and_not_with_the_pass_count`
  — the same two cancelling strategies driven at two different pass rates
  over the same wall-clock span, asserting the same set of passes crosses.
  This is the test that distinguishes `Span` from `Passes` and is the point
  of the first half of the decision. Mutation: make `in_crossing_window`'s
  `Span` arm compare `sample.pass` instead of `sample.at`; the two rates must
  then disagree.
- `qip-edge/tests/crossing.rs::a_repeated_full_cancellation_settles_to_a_realised_crossing_ratio_inside_the_blueprints_band`
  — drive the cancelling cell for enough passes to reach steady state at the
  configured span and assert the crossed-over-gross ratio is above 0.20 and
  at most 0.40, asserting first that some cross was booked and some was
  refused (a ratio of zero or of one half both satisfy a badly written
  assertion). This is the test the "33.3%" arithmetic above owes, and until
  it runs that figure is a derivation and not evidence. Mutation: set the
  span to five seconds and watch the lower bound fail; set it to an hour and
  watch the upper bound fail.
- `qip-edge/tests/crossing.rs::a_span_that_would_fill_the_sample_bound_refuses_under_the_window_gate_and_not_the_cap`
  — the two refusals are different findings and must not be read as one.
  Mutation: let `crossing_window` report `full` as false at the bound.
- `qip-edge-node/tests/pass.rs::a_node_configured_with_an_interval_measures_the_cap_over_it_and_a_node_without_one_measures_per_net`
  — the composition root actually installs it; a value read and dropped is
  the commonest way a configured control becomes decorative. Mutation: parse
  the value and never call `with_crossing_interval`.
- `qip-edge-node/tests/…::a_node_given_an_interval_the_constructor_refuses_does_not_start`
  — refuse, do not clamp, at the root. Mutation: fall back to `None` on a
  parse failure, which is the specific bug that would turn a typo into a
  silently different safety control.
- `qip-acceptance/tests/architecture.rs` — the existing
  `a_library_never_depends_on_a_service_or_an_application` and
  `no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`,
  unchanged and still passing: the evidence that this moved no boundary.

## Dependency-direction argument

The graph is unchanged, and can be stated exhaustively because the change is
so small.

`CrossingInterval` and `CellConfig` are `qip-edge`'s own types, declared in
`qip-edge/src/cell.rs`. The window, the cap and the sample history are
private fields of `Cell` in the same crate. Nothing new is named across a
crate boundary by the decision itself.

The interval's *value* is read in `qip-edge-node`, an app, which is where
`.claude/rules/architecture/00-boundaries.md` requires configuration to be
read and the only layer permitted `std::env`. `qip-edge` receives it as an
argument to a builder it owns — the same shape as `Cell::with_metrics`, which
takes the registry it records into rather than reaching for one, and the same
shape as `Cell::with_unfunded_region`. No lib comes to depend on a service;
no service on the runtime; nothing on an app; `qip-edge` gains no dependency
at all, and in particular gains nothing from `qip-kernel`, `qip-capital` or
`qip-mesh`.

The optional telemetry addition names `qip-observability` from `qip-edge`, an
edge crate depending on a lib that holds a `BTreeMap` behind a mutex and
performs no I/O — an edge already present and argued in
`.claude/rules/domains/observability.md`.

The infrastructure half writes an environment line in a startup template. It
reaches no crate.

## What was not run

No cargo gate ran for this record, and none applies: this record changes no
code, no `Cargo.toml`, no test and no Terraform. `cargo fmt`, `cargo clippy`,
`cargo test`, `terraform fmt -check` and `terraform validate` were **not
run**, and are named here as not run rather than omitted.
**`./scripts/check-secrets.sh` was also not run**: this session had no shell
tool available. The record adds no credential material — it names no key, no
token, no account identifier and no hostname — but that is an argument, not
the scan, and the scan is owed before this file is committed.

The one substantive claim in this record that rests on arithmetic rather than
on a command is the 33.3% steady state, and it is labelled as such where it
appears.
