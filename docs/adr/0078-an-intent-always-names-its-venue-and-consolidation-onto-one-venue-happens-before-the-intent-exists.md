# 0078 — An intent always names its venue, and consolidation onto one venue happens before the intent exists

**Status:** *proposed*, 2026-09-19. Decided by the architecture lane; awaiting
implementation, which is a `qip-edge` change and nothing else.

**Relates to:** blueprint §27.2 (second row, "same instrument, different
venues"), §28 (strategy-level limits are checked *before* netting), §56.2 rule
22; ADR 0007 (exact attribution), ADR 0067 (the quote loop names no venue),
ADR 0073 (a venue's region is configuration).

**Does not touch:** `qip-contracts::intent` — neither `Intent`, `NettingKey`,
`net` nor `NetIntent` changes shape; no journaled net is re-derived. Nothing
here approves, places or routes an order it would not already have placed; it
changes *which venue* a cell reasons an intent at. The paper-trading boundary
stands at all three layers and is not on this path.

---

## Context

`docs/DELIVERY-STATUS.md` scores §27.2 `PARTIAL` with one of its five rows
absent: "the router consolidating unspecified-venue intents onto the best
venue". This lane was briefed that a `Consolidator` in `qip-routing`, keyed on
a reserved `UNSPECIFIED_VENUE` identifier seen only in its own tests, would be
a strict no-op if wired — a control that cannot fire — because
`Intent::venue` is a `VenueId` and `Cell::intent_for` resolves a concrete venue
before any intent exists.

**Two of those premises hold and one does not, in the tree this record was
written against.** `Intent::venue` is a `VenueId`
(`grep -n 'pub venue: VenueId' backend/crates/libs/qip-contracts/src/intent.rs`)
and `intent_for` resolves the venue first
(`grep -n 'fn intent_for\|fn venue_for' backend/crates/edge/qip-edge/src/cell.rs`).
But `grep -rn 'consolidat\|UNSPECIFIED_VENUE' --include=*.rs backend/crates/edge/qip-routing/`
prints nothing, and the register's own §27.2 row says the same. There is no
consolidator to be a no-op. This record therefore decides the shape against
the *type*, so that it holds for any consolidator that later lands, and it
refuses the sentinel approach by name whether or not one exists.

The row has two clauses and they are answered by two different facts:

- *"Not netted by default — different executions at different prices."* Held
  by `NettingKey`, which carries the venue: two intents on one instrument at
  two venues are two groups and two orders
  (`grep -n 'struct NettingKey' -A 12 backend/crates/libs/qip-contracts/src/intent.rs`).
- *"The router may consolidate onto the best venue if strategies did not
  specify one."* The condition is the finding. **In this platform a strategy
  cannot specify a venue.** `Signal` has no venue field
  (`grep -n 'pub struct Signal {' -A 16 backend/crates/libs/qip-contracts/src/signal.rs`)
  and the intent module's own doc says a signal is "the strategy's own view
  before any venue is chosen". So *every* intent is in the "did not specify"
  case, and the venue every one of them carries was chosen by the cell, per
  signal, in `Cell::venue_for`, before the intent existed. Within one pass
  `venue_for` is deterministic on the cell's own books, so N strategies'
  signals on one instrument resolve to one venue, land on one netting key and
  become one order. That is the row's behaviour. It has been the platform's
  only behaviour since intents existed.

What the row asks for and the cell does not do is the word **best**.
`venue_for` takes the *first* venue in the cell's configured order whose book
for the instrument is not `Unreachable`. First is not best; it is
deterministic, which is the property that matters more, and it is the property
any replacement must keep.

## Decision

**One. `Intent::venue` stays `VenueId`. An intent that names no venue is
inexpressible, and that is the fail-closed shape rather than a gap.**

§28 puts every per-strategy gate *before* netting — "a strategy that has
exhausted its budget must not contribute to a net intent at all" — and
`intent_for` runs them in that order: expiry, venue selection, book presence,
staleness, venue status, price, pricing policy, degradation multiplier,
envelope. Five of those gates are questions about *a book*, a book is at *a
venue*, and so an intent whose gates have run has a venue by construction. An
`Option<VenueId>` would be a type that can spell "gated, but at nothing",
which is a contradiction the compiler would let through and every consumer of
`NetIntent::venue` — `place_net`, `cross_internally`, `would_self_trade`, the
journal's refusal text — would then have to handle with an arm that means
"this cannot happen".

**Two. Consolidation is the venue choice in `Cell::venue_for`, before the
intent exists, and "best" is a change to that function's criterion — not to
the contracts crate and not a pass after `net()`.**

The criterion an implementer is to write, in `qip-edge` only: among the
venues the cell is configured for whose book for the instrument is usable at
the pass instant — present, not stale, accepting orders, serving a mid —
choose by one stated, exact, deterministic comparison, and journal the
choice with every candidate and the figure it was compared on, so the pick is
reproducible from the journal alone. The first criterion is the tightest
quoted top-of-book spread in `Decimal`, ties broken by configured venue
order, because the spread is the one cost the cell measures itself on every
pass; fee floor and tick from policy slot 11 may join later as refusals or
tie-breaks and may never override a staleness refusal. No `f64` on this path.
No call to the centre, ever: the choice is on the hot path and ADR 0008 puts
nothing there that waits on the centre.

The comparison is per instrument per pass, so every strategy's signal on that
instrument in that pass resolves to the same venue. That is what makes the
consolidation hold *at the existing netting key*: the key does not change,
the intents simply all carry the venue that won.

**Three. No reserved venue identifier, now or later.** A sentinel inside a
`VenueId` is a magic string that `place_net` would send an order to. The
failure it invites is an order routed to a venue named "unspecified", and it
is exactly the kind of control that cannot fire, since nothing outside a test
would ever construct one. A consolidator that needs a sentinel to find its
input is a consolidator placed after the wrong seam.

**Four. The extension path is named so it is not reinvented.** The day a
`Signal` gains `venue: Option<VenueId>` — a strategy that specifies its venue —
`venue_for` honours it: a specified venue with a usable book is taken as
given, and a specified venue without one is a `venue_selection` refusal, never
a substitution. Row 2's first clause then holds for specified venues through
the key that already carries the venue, and the second clause holds for the
rest through the same `venue_for`. That is why the key carried the venue from
the start, and it is the same argument `Representation` shipped on.

## Consequences

- **Crate boundaries.** `qip-edge` alone, in `venue_for` and the journal
  record its choice writes. `qip-contracts` is untouched; `qip-routing` is
  untouched; no journaled `NetIntent` is re-derived because no key changed.
  The dependency direction is unchanged: `qip-edge` already depends on
  `qip-contracts` and `qip-routing` (edge → libs); nothing in a lib gains a
  dependency and no service depends on the runtime.
- **The netting guarantee is strengthened, not traded.** Before, two signals
  on one instrument netted only if the first configured venue happened to be
  usable for both; after, they net at whichever venue is best, and they always
  net together because the comparison is per instrument per pass.
- **Rule 22 is not decided here, and this record says so rather than
  implying it.** Consolidation puts one net at one venue, so
  latency-equalised dispatch does not arise on the netting path. Where it
  does arise — the legs of a cross-venue cycle, which `place_cycle` sends one
  after another in the plan's order — it turns on ADR 0001/0011's
  no-async-runtime decision and on `unsafe_code = "forbid"`, and it remains a
  policy question for the owner. §56.2's row keeps rule 22 absent.
- **The market-making row of §27.2 is not this record's.** ADR 0067 decided
  that a quote names no venue and no side, and the register's row says why
  the quote loop skews off the held position rather than off the net.

## What it costs

- **Two strategies' signals in different passes may land at different
  venues and not net.** That is row 2's first clause behaving correctly — a
  later pass is a different execution at a different price — and not a
  defect; but a reader expecting "same instrument always nets" will see two
  orders in the journal and should find this sentence.
- **Best by spread is not best by cost.** Until slot 11's fee floor joins the
  comparison, a venue with the tightest spread and the highest fee wins. The
  order of adoption is spread first because it is measured locally; fees
  arrive on a wire the cell must already be treating as subtract-only.
- **The register's §27.2 stays `PARTIAL` until `venue_for` compares.** This
  record decides the shape; it does not move the row.

## What would make this wrong

- A `Signal` that names a venue and a `venue_for` that substitutes another.
  Check: `grep -n 'venue' backend/crates/libs/qip-contracts/src/signal.rs`
  prints nothing today; the day it prints a field, read `venue_for` for a
  substitution.
- A consolidation pass placed **after** `net()` — any code that rewrites
  `NetIntent::venue`. `grep -rn '\.venue = ' --include=*.rs backend/crates/edge/`
  should find no assignment to a net's venue.
- `Intent::venue` becoming `Option<VenueId>`, or any `VenueId` constant
  whose name means "none".
- A criterion that reads anything but the cell's own books, or one in `f64`.
- A choice not journaled with its candidates: a pick a replay cannot verify
  is a pick nobody can audit.

## Alternatives considered

**(a) `Intent::venue: Option<VenueId>`, with `net`'s key and every consumer
updated.** Rejected. It makes a required fact optional on a type whose every
consumer needs the fact; it separates the venue choice from the gates that
depend on it, so consolidation after netting would either re-run
per-strategy gates on the *net* — the wrong level, which §28 forbids — or skip
them for the `None` group, a gate that cannot fire; and a `None` in the key is
a key change, which re-derives every journaled net.

**(b) Move the cell's venue choice out of `intent_for` into a consolidation
step after netting.** Rejected for the same gate-ordering reason, and for a
second: the net's `reference_price` is the mid the largest contributor was
reasoned at, which is a price *at a venue*. A net with no venue has no
reference price, and a consolidation step that then picks a venue would be
pricing the net a second time after the strategies had already been sized
against the first price.

**(c) Record that the blueprint's "unspecified venue" row is not a behaviour
this platform wants.** Rejected because it is false: the row is the only
behaviour this platform has, since no strategy can specify a venue. What it
lacks is "best", and refusing the row would leave "first configured" as an
undocumented cost model.

**(d) A reserved `UNSPECIFIED_VENUE` identifier for a consolidator to find
its input by.** Rejected for the reason in Decision three.

## For the implementer

Tests worth writing first, each mutation-verified:

- Two strategies, one instrument, two configured venues, the first configured
  with the wider spread: both intents carry the second venue and net into one
  order. Mutation: reverse the comparison, and the test must fail on the venue
  the order names, not on the count.
- The best venue's book stale: the next usable venue is chosen; no usable
  book anywhere: the existing `venue_selection` refusal, unchanged.
- The same books twice: the same venue twice, and the journal record names
  every candidate and the spread compared. A test that asserts the journal
  record *contains* the winner passes when the winner is also a candidate;
  assert the delimited winner field.
