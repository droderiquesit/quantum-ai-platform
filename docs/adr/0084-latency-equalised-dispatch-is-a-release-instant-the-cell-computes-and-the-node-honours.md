# ADR 0084: Latency-equalised dispatch is a release instant the cell computes and the node honours, so it is an engineering task and not a policy exception

- **Status**: Accepted, under the authority the owner delegated to the
  policy lane on 2026-09-19 over legal, business and policy decisions. The
  design below is a specification for an implementing lane; nothing is built
  by this record.
- **Date**: 2026-09-19
- **Supersedes**: nothing. Corrects the reading, carried by three lanes and
  by `qip-edge/src/dispersion.rs`'s module doc, that the mechanism needs
  "a timer wheel on a dispatch thread, and this process has neither" and is
  therefore out of reach.
- **Related**: ADR 0001 and ADR 0011 (no async runtime), ADR 0008 (a cell
  decides alone), ADR 0012 (`tokio` refused; threads are not), ADR 0068
  (path 2 is "latency-equalised parallel dispatch from pinned I/O slots"
  inside one process), ADR 0069 (arrival dispersion after equalisation has
  no recording site — this design creates one), ADR 0082 (the node has one
  thread by design, and where a second one would sit)

## Context

Blueprint §32.1 names five mechanisms against fill-time dispersion and calls
latency-equalised dispatch "the single largest effect": "timer wheel on the
dispatch thread and a rolling per-venue latency percentile. Five to tenfold
reduction." Its own diagram is the whole algorithm — `EQUALISED send at
(max - own) -> all arrive at 8 ms; dispersion collapses to jitter`. Rule 22
in §56.2 makes it mandatory: "Dispatch is latency-equalised wherever more
than one venue is involved." §33.1's venue profile says the latency it
equalises on is "measured, not declared", and §49.1 asks for "arrival
spread per venue combination before and after equalisation".

Three lanes concluded it could not be built here, and the register's §32.1
row says so: it "needs a dispatch thread this process does not have". The
brief asked whether that is a policy obstacle or an engineering one. Checked
against the tree on 2026-09-19:

- **A timer wheel needs a thread, not an async runtime.** `std::thread` is
  permitted — `qip-transport/src/retry.rs` already sleeps on one, and ADR
  0012 refuses `tokio` for a workload that "does not need" concurrency, not
  threads as such. The boundaries rule forbids a *new async runtime*. So the
  "no async runtime" decision is not the obstacle.
- **The real obstacle is replay.** `Cell` reads no clock: every method takes
  `now: Timestamp` (`grep -c 'now: Timestamp' backend/crates/edge/qip-edge/src/cell.rs`
  is in the dozens), and `qip-edge-node` reads the clock once per turn of
  its loop (`grep -n 'clock.now()' backend/crates/apps/qip-edge-node/src/main.rs`).
  `dispersion.rs` says why: "nothing here reads a clock: the two timestamps
  arrive from the pass. So a replay of the same reports produces the same
  verdicts." A dispatch thread inside the cell that fires on wall time would
  make the orders a function of the machine's clock rather than of the pass,
  and the journal could no longer be replayed to the same orders.
- **The measurement already exists and is the cell's own.** `FillTimes`
  keeps a window of fill times per venue, from the instant the cell sent an
  order to the instant the venue's execution report was confirmed
  (`grep -n 'fill_times.observe\|let taken = fill.at.since' backend/crates/edge/qip-edge/src/cell.rs`),
  and exposes the per-venue **median** (`grep -n 'pub fn median' backend/crates/edge/qip-edge/src/dispersion.rs`)
  — the "rolling per-venue latency percentile" the blueprint asks for, with
  the median chosen over the mean for a reason the module states. Its
  `assess` already refuses a cycle whose legs would arrive too far apart.
  `qip-routing`'s `VenueHealth` keeps a separate EWMA of acknowledgement
  latency; it is not the figure to equalise on (see alternatives).
- **The node's only arm is the simulator.** `run_pass` is typed to
  `SimulatedGateway` (`grep -n 'pub fn run_pass' -A 4 backend/crates/apps/qip-edge-node/src/pass.rs`)
  and a live gateway is refused at start-up. The simulated gateway matches
  in-process with no latency model
  (`grep -n latency backend/crates/apps/qip-edge-node/src/gateway.rs`
  finds one comment). So every simulated venue fills in the same pass it is
  sent in, every median is the same, and every equalisation offset is zero.
- **`Placer::place` already takes `at: Timestamp`**, and the cell passes
  `now` (`grep -n 'gateway.place(' backend/crates/edge/qip-edge/src/cell.rs`).
  The seam through which a release instant would travel exists; only its
  meaning is missing.

So the question is whether there is a design in which the cell computes a
**scheduled release instant** as a pure function of the pass — replayable —
and the node's loop honours it. There is, and it is small.

## Decision

**The policy question dissolves. Latency-equalised dispatch is an
engineering task under the existing rules, with the design below, and no
exception to "no async runtime", "the cell reads no clock" or the
two-dependency rule is granted or needed.**

### 1. The cell computes a release schedule, and it is a pure function of the pass

In `qip-edge` (`dispersion.rs`, or a sibling `release.rs`):

```rust
/// When each leg of a multi-venue cycle is released, relative to the pass.
/// Offsets are `max_median - median(venue)` over the cycle's venues, so the
/// slowest venue is released first and every leg is expected to arrive
/// together. A pure function of `FillTimes`, which is a pure function of
/// the reports the pass was handed — so a replay computes the same schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSchedule {
    /// Venue → delay after the decision instant. `BTreeMap` because the
    /// order reaches the journal.
    offsets: BTreeMap<VenueId, Duration>,
    /// False when any venue in the cycle is unmeasured; then every offset is
    /// zero and the journal entry says the cycle was sent unequalised. An
    /// unmeasured venue does not refuse, for the reason `FillTimes` gives:
    /// a venue has no fill times until it fills something.
    equalised: bool,
    unmeasured: Vec<VenueId>,
}

impl FillTimes {
    pub fn release_schedule(&self, venues: &[VenueId]) -> ReleaseSchedule { /* … */ }
}
```

A single-venue cycle has one offset of zero and `equalised: true` — rule 22
says "wherever more than one venue is involved", and a one-venue schedule
is trivially equalised rather than exempt.

### 2. The release instant travels through `Placer::place`, whose `at` becomes "release no earlier than"

`Cell::work` sends each leg with `at = now.saturating_add(offset)`. The
`Placer` doc changes from "when it was sent" to "the instant before which
the gateway must not release it", and `OpenOrder` gains `release_at` beside
`sent_at`. The journal's `Decision::OrderSent` gains `release_at` and
`equalised`, so an operator reading the chain sees that a leg was held for
four milliseconds on purpose and not that the node was slow.

**The fill time is measured from `release_at`, not from `sent_at`.** This
is the one detail that decides whether the mechanism works: measured from
the decision instant, a held leg's fill time would include its own hold, the
median would grow by the offset, the next schedule would shrink it, and the
equaliser would chase its own tail. `Cell::confirm`'s
`let taken = fill.at.since(sent_at)` becomes `since(release_at)`.

### 3. The simulated gateway honours the instant deterministically, and that is the only arm this binary runs

`SimulatedGateway` treats `at` as the arrival instant: an order with
`at > now` is held in the gateway and matched on the first pass whose `now
>= at`, in `BTreeMap<Timestamp, Vec<_>>` order. No thread, no clock read —
the gateway is driven by the same `now` the pass is handed, so a replay of
the same passes releases the same orders in the same order. With no latency
model every offset is zero and nothing is held, which is the honest state:
the mechanism is built, reached on the production pass, and inert until a
venue takes measurably different times to fill — exactly the state of the
live-venue refusal in `Cell::send`, and stated the same way.

### 4. Where a real gateway's release thread sits, when one exists

No real venue gateway exists and none may reach this binary today. When one
does, the release lands in `qip-edge-node` (an `apps/` crate, which may read
a clock), not in `qip-edge`:

- one `std::thread` owning a `BTreeMap<Timestamp, Vec<PendingRelease>>` —
  the timer wheel at this scale, a handful of legs per pass; a wheel proper
  is a performance change to make when a measurement asks for it;
- it sleeps with `std::thread::sleep` until the earliest key, releases what
  is due through the venue adapter, and records
  `qip_edge_release_lag_nanos{venue}` — the actual release instant minus the
  scheduled one, which is §49.1's "arrival dispersion after equalisation"
  descriptor that ADR 0069 found had no recording site;
- a leg whose scheduled instant is already more than a bounded lag in the
  past is **withdrawn, not sent late** — refusing rather than guessing — and
  the withdrawal is journaled under its own gate literal, a `pub const`, so
  the `gate` label stays bounded;
- the thread's clock is the composition root's `Clock`, read there and only
  there. The cell never sees the thread; it sees the fills the venue reports,
  as it does now.

That thread is the second thread ADR 0082 names as the point at which
per-thread pinning becomes a live question. It is not this record's to
authorise; this record says where it goes so that nobody puts it in the cell.

### 5. What replay means here, stated so it can be checked

The cell's journal records the schedule it computed and the reports it was
handed. Replaying the same passes with the same reports reproduces the same
schedule and the same orders. The node's actual release lag is telemetry —
an observation about the machine, like the network's own latency — and is
never an input to a decision. The one input the cell takes from the world is
the fill-time window, and it took that already. A test in the implementing
lane drives two cells through identical passes and asserts identical
`OrderSent` entries including `release_at`; a mutation that reads a clock
anywhere in `qip-edge` must fail it.

## Consequences

- §32.1 moves in the register from "needs a dispatch thread this process
  does not have" to "decided in ADR 0084: an engineering task with a design;
  not yet built". Verdict stays `PARTIAL`. Rule 22 in §56.2 likewise.
- `dispersion.rs`'s module doc sentence — "latency-equalised dispatch needs
  a timer wheel on a dispatch thread, and this process has neither" — is
  corrected by the implementing lane in the same change that adds
  `release_schedule`; it was true of the design as understood and is not
  true of the design here.
- `Placer::place`'s `at` changes meaning. Every implementor in the workspace
  is found by `grep -rn 'impl Placer for' backend/crates --include=*.rs` and
  each is read, not assumed: a gateway that ignores `at` today is correct
  today and wrong after this lands.
- `Decision::OrderSent` gains two fields. The journal is hash-chained; a new
  field is a schema addition that old entries lack, and the reader must
  treat an absent `release_at` as "sent at `sent_at`, unequalised" rather
  than as zero.
- `qip_edge_release_lag_nanos` is described in `qip-observability` only when
  its recording site lands, per the observability rules — a registered
  constant nothing calls would satisfy the acceptance test and page nobody.

## What it costs

**Nothing observable changes on the simulated arm.** Every offset is zero
because the simulator fills everything in the same pass. The mechanism will
be built, tested and reached and will move no number until a venue with a
latency exists, and this record says so rather than letting §32.1 read as
delivered. A latency model in the simulator would make the offsets non-zero
in tests; it is worth adding for the tests and is not evidence about any
venue.

**The equaliser is only as good as the median.** A venue whose fill time is
bimodal — fast on acceptance, slow when it rests — has a median that
describes neither mode, and a leg released on it arrives early or late. The
window is 64 fills; a regime change takes 32 fills to move the median. The
blueprint's "rolling percentile" has the same property and does not say
which percentile; this record chooses the median because `FillTimes` already
argued for it and a second statistic would be a second claim.

**A held leg is exposure the cell has decided on and not yet placed.** For
the offset's duration the cell holds a decision the book may move under. The
dispersion gate bounds that by refusing a cycle whose spread exceeds
`DispersionPolicy`'s bound (a quarter of a second by default), so the hold
is bounded by the same number; a deployment that measures its venues sets a
tighter one.

**A second thread, later.** Decision 4's thread is the first concurrency in
the edge node and brings the class of bug the one-thread design avoided —
a release racing a halt. The design answers it (the thread releases only
what the cell handed it before the halt, and a mass cancel on halt withdraws
the pending map first), but the answer has to be built and mutation-tested,
and it is not built here.

## What would make this wrong

- **A venue whose latency is not a property of the venue.** If measured
  fill times vary more by order size, time of day or book state than by
  venue, a per-venue median equalises nothing and the schedule should be
  keyed on what actually predicts arrival. That is a finding from
  `qip_edge_release_lag_nanos` and the per-venue `summary`, and it revises
  decision 1's key, not the shape.
- **A measured need for a real timer wheel.** If a real gateway ever has
  enough legs per pass that a `BTreeMap` walk is on the critical path, the
  wheel replaces the map inside decision 4's thread. It is a data-structure
  change inside one function and needs no ADR.
- **Replay diverging.** If the test in decision 5 ever fails because a
  schedule depended on something other than the pass, the mechanism has
  reached for a clock and must be reverted to the last replayable commit;
  the property is the decision.
- **A blueprint amendment naming a percentile other than the median.** Then
  `FillTimes` grows that statistic beside the median and `release_schedule`
  takes it; the median stays for the dispersion gate, which argued for it on
  its own grounds.

## Alternatives considered

**A timer wheel on a dispatch thread inside the cell, as the blueprint
draws it.** Rejected: the cell would read a clock, and a replay of the
journal would no longer reproduce the orders. The blueprint's node is a
different process from this one (ADR 0082); its wheel is where its threads
are, and this platform's replay guarantee is worth more than its diagram.

**An async runtime.** Rejected, and not needed: a sleep-until-key loop is
one thread and one `BTreeMap`. ADR 0012 refuses `tokio` for exactly this
reason, and the boundaries rule records it as a decision.

**Equalise on `VenueHealth::observed_latency`, the acknowledgement EWMA.**
Rejected. An acknowledgement is not a fill; the exposure §32.1 is about
opens at the fill, not at the ack. And an EWMA is a mean, which one slow
sample drags — the argument `dispersion.rs` already makes for the median.
The ack latency stays what it is: `qip-routing`'s health signal.

**Retire the mechanism.** Rejected: the design exists, it takes no
dependency, it keeps every guarantee, and rule 22 is mandatory in the
blueprint. Retiring a mandatory mechanism that can be built would be the
register telling the next lane not to try, which is the failure ADR 0083
corrected in §21.2.

## Dependency-direction argument

`qip-edge` (edge) gains a type and a method beside `FillTimes`, depending on
`qip-core` and `qip-contracts` as it already does. `qip-edge-node` (app)
holds the simulated gateway's ordered map now and the release thread later,
depending inward on `qip-edge` and the libs; nothing depends on it. The
clock is read in the app and only in the app, which is the property this
record exists to keep. No lib depends on a service, no service on the
runtime, and `qip-routing` is unchanged.
