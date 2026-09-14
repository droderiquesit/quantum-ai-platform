# ADR 0067: A quote is a priced intent with no path to a venue, and an originated market is gated by a type

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default and in fact), ADR 0005 (confidence as arithmetic), ADR 0016 (repository layout), ADR 0050 (what an option-quote source must satisfy), ADR 0052 (a deterministic gate must be able to approve), ADR 0055 and ADR 0063 (counterfactual evidence may only narrow)

## Context

Blueprint §29.1 "The Quote Loop" and §29.3 "Market Creation" were both
`ABSENT` in `docs/DELIVERY-STATUS.md`. §29.1 specifies a fair value, a half
spread built from a base, a volatility term and an adverse-selection term, an
inventory skew, a size, and a requote threshold, and its table names the
failure each component prevents. §29.3 specifies five gates that must be
cleared before the platform may create a market at all.

Closing §29.1 is the single most dangerous row in the register, and the reason
is not subtle: **in every real venue, quoting is order submission.** A quote
loop that produced anything a venue adapter could act on would be a live-order
path with a research name on it. `.claude/rules/01-security-and-safety.md`
names three layers that hold the paper-trading boundary and says that a task
appearing to require a live path "has never yet been legitimate".

Two further constraints shaped the design rather than merely bounding it.

1. **This workspace has a register of controls that read as protection and
   could never fire.** `RiskState::expected_shortfall` was always empty, so
   `MaxExpectedShortfall` shipped in every default limit set and could not
   trigger. §29.1 is unusually rich in opportunities to repeat that: an
   inventory skew that never binds, a toxic-flow widening no input triggers, a
   queue-position term fed by a constant.
2. **The platform's own record is bar-resolution.** §29.1's table is written in
   the vocabulary of tick-level microstructure.
   `qip_market::microstructure::MicrostructureMetrics` computes the
   realised-minus-effective decomposition properly, but it needs a *window* of
   quotes and trades, and `MarketSnapshot` holds one of each per instrument.

## Decision

### 1. A quote is a type that carries nothing a venue could act on

`qip_execution_engine::quoting::QuotePair` holds an object id, a fair value, a
bid, an ask, a size and the decomposition of the half spread. It carries **no
venue, no side, no client id and no time in force**, and no function in the
workspace turns one into an `Order`. Neither `quoting` nor `origination` names
a broker, an order manager or a venue in production code.

This is structural rather than procedural, deliberately: a guarantee the type
system holds beats one a runtime check holds. `QuotePair` is not a refusal
that could be edited away; it is a value with nothing on it to send.

The three layers of the paper-trading boundary are untouched. Nothing in this
lane reads or writes an autonomy ceiling (layer two), constructs a
`qip_edge::Cell` (layer three), or reaches Terraform (layer one).

### 2. The quote loop lives in the execution service and is composed in the kernel

The arithmetic is `qip-execution-engine`'s, because it is execution's domain
and it depends on nothing but `qip-core` and the origination gate beside it.
The composition — market view, order manager and equity read together — is
`qip-kernel`'s `quote_loop` module, for exactly the reason `valuation`,
`rule_review`, `venue_review`, `family_review` and `sizing_review` are kernel
modules: the kernel is the only place allowed to hold two domains at once.

A new crate was considered and rejected. The quoting arithmetic is forty lines
of decimal and basis points; a crate for it would add a `Cargo.toml`, a
workspace member and a boundary to reason about, and would put the quote loop
somewhere other than beside the order types it must never construct — which is
precisely where a reviewer needs to be standing to notice if it starts.

### 3. Every component of §29.1 is present with the input that makes it bind

Each is listed in `quoting.rs`'s header with what makes it act, and each of
those inputs is supplied by a test:

| Component | What makes it act |
|---|---|
| Inventory skew | inventory away from target; at the limit, quoting halts |
| Adverse-selection term | a non-zero reading widens the half spread |
| Volatility term | a non-zero reading widens it and shrinks the size |
| Requote threshold | `QuotePair::supersedes` is false inside it, true outside |
| Queue position value | a measured position with size ahead shrinks the size |
| Toxic flow | one-sided direction **and** a moving reference withholds |
| Belief weighting | scales the signal and the size; below the bar, withholds |

Two of these deserve their reasoning on the record.

**Toxic flow requires both conditions, never either.** One-sided flow in a
quiet market is ordinary trading, and a market that moves with balanced flow is
what the volatility term is priced for. A detector that fired on either alone
would halt quoting on every trending day: a control that reads as protection
and is an outage.

**Queue position value is `Unknown` or `Measured`, and `Unknown` is
pessimistic.** Queue position value is not derivable from a depth snapshot
alone — only from a snapshot plus a resting order of one's own. Rather than
average the two states into an invented number, they are distinguished, and
the unknown state sizes at half. A test asserts that the unknown arm really is
the smaller size, because a fail-closed default that is not actually smaller is
a comment rather than a control.

### 4. Widths and inventory are refused past their bounds, never clamped

A half spread the terms ask for beyond the policy's ceiling **withholds the
quote** rather than narrowing to the ceiling: a quote shown at the ceiling when
the terms said twice that is a quote priced for a market the platform does not
believe it is in, and it would fill. Inventory at the limit halts quoting
rather than skewing harder, which is the blueprint's own sentence.

Malformed *inputs* — an infinite volatility, an imbalance outside `[-1, 1]`, a
zero inventory limit — are `Error`s. A caller that computed one has a defect,
and a value silently corrected is a caller bug that survives.

`QuotePolicy::validate` additionally refuses any policy whose imbalance
coefficient, spread ceiling and skew could total one whole. That is what makes
a non-positive bid arithmetically impossible, so no arm of `quote` carries a
defensive branch against one — an unreachable defensive branch is
indistinguishable from a control.

### 5. §29.3's five gates are a type with one constructor and no `Deserialize`

`OriginationMandate` has private fields, one constructor
(`OriginationMandate::admit`) and derives `Serialize` but **not**
`Deserialize`. A mandate therefore cannot be decoded into existence out of a
config file, a policy frame or a cell report; the five refusals are the only
door. The gates:

1. **A defensible valuation with method and confidence** — method named,
   value positive, confidence at or above `ORIGINATION_MIN_VALUATION_CONFIDENCE`.
2. **A causal explanation for the absence of other participants** — an enum,
   not free text, because a gate that reads free text approves anything phrased
   confidently. `InformationWeLack` and `Unexplained` are refused, which is the
   blueprint's own sentence: if the reason is information you lack, you are the
   counterparty they are avoiding.
3. **An adverse-selection model for this instrument class** — fitted for
   *this* class and on at least `ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE`
   observations.
4. **Bounded maximum exposure, hard-coded** — `ORIGINATION_MAX_EXPOSURE` is a
   constant in `origination.rs` and not a configurable, because the blueprint's
   fourth gate is literally the word "hard-coded". A request above it is
   **refused, not lowered**: a desk that asked for ten million and silently
   received the maximum would believe something false about its own book.
5. **Human approval per instrument class** — a `ClassApproval` naming an
   operator and a digest, for the matching class. Nothing in this workspace
   constructs one from a model output, an agent finding or a config value, so
   until a desk hands one in every origination request is refused. That is the
   fail-closed direction.

Per ADR 0052, the gate can also **approve**: a test drives a complete request
through and asserts the mandate comes out, so this is not a control that
refuses everything.

### 6. The mandate is load-bearing on the quote loop, not a document filed once

`QuoteReference::Originated` anchors the fair value on the mandate's valuation
and bounds the size by the mandate's remaining headroom on **every pass**; at
the ceiling the quote is withheld. §29.3's fourth gate is therefore checked
continuously rather than at admission.

## Consequences

**What is now true.** The platform can price what it would quote in an
instrument it observes a two-sided market in, attribute the width to its three
terms, skew it against its own filled inventory, and say in one sentence which
instruments it declined to quote and why. It can refuse to create a market it
cannot defend a price for, cannot explain the absence of other participants in,
has no adverse-selection model for, wants an unbounded ceiling on, or has no
signed approval for.

**What is not true.** Nothing sends a quote, and nothing is built that could.
There is no message-rate budget, no mass cancel and no per-venue token bucket —
those are §29.2's remaining rows and belong at the edge, where `qip-routing`'s
`RepricePolicy` already holds the requote threshold for a *resting child
order*. `QuotePair::supersedes` is the quote-level question and
`Repricer::consider` is the order-level one; they are deliberately separate
mechanisms, and merging them would require the edge crate to depend on a
service, which the layering forbids.

**The quote policy is constants rather than configuration**, in
`qip_kernel::quote_loop::default_policy`. These numbers have never priced
anything a venue saw, and a configurable is a promise that a deployment may
tune it. When the loop has a consumer beyond the cycle report, this becomes a
config block and the default stays exactly these numbers.

**Two honest limits on provenance**, stated here so they are not discovered
later as claims that were never true:

* **Adverse selection is a bar-resolution proxy.** It is the mean absolute
  one-bar return, which measures how far the reference moves over the horizon a
  quote is exposed for. It is *not* the realised-minus-effective spread
  decomposition. `MicrostructureMetrics` computes that properly and needs a
  window of quotes and trades the snapshot does not hold.
* **The `Measured` queue arm is unreached by today's cycle.**
  `order_type_for` returns market, time-weighted, volume-weighted or
  participation and never `Limit`, so every pass of a cycle takes the `Unknown`
  arm. The measured arm is reached through `Platform::submit_order` with a
  limit order — a public production door, exercised by a test — and it exists
  because the edge cell does rest limit orders and this is the seam their queue
  position would arrive through. Saying so is the alternative to shipping a
  branch whose reachability is only described.

**Reversal.** Deleting `quote_loop.rs`, the two execution-engine modules and
the acceptance suite removes everything this record decides. No other module
depends on any of it; the one production seam is a single line in `stage_act`.

## What it costs

**A cycle stage does arithmetic nothing consumes.** The quote loop prices up to
`QUOTE_PASS_LIMIT` instruments per ACT stage and the only reader of the result
is the cycle report. That is a real cost paid for a real thing — the platform
can now say what it would have quoted and why it declined — but it is not a
trade, and calling it one would be the overclaim this register exists to stop.

**Two more public modules in the highest-consequence crate.** Every future
reviewer of `qip-execution-engine` now has to hold in mind that two of its
modules must never grow a seam the rest of the crate has by design. The
acceptance suite is the mitigation and it is a source-text scan, which is a
weaker instrument than a type: it can be defeated by a type alias nobody spells
out. The behavioural test beside it — a real platform pass that asserts no
order and no fill was created — is what catches that case, and it is the one to
keep working if the two ever disagree.

**Numbers that have never been calibrated against anything.** The policy widths
and the two equity fractions are reasoned from this workspace's own fixtures
(a listed name quoted at three basis points) and from nothing else. They have
priced no real flow. Presenting a spread derived from them as a measurement
would be false.

**A bar-resolution adverse-selection reading carried under a
microstructure name.** The field is documented as a proxy at both ends, but
the blueprint's vocabulary and the platform's evidence do not match here, and a
reader who skips the documentation will assume the stronger thing.

## What would make this wrong

**Anything that gives a `QuotePair` a destination.** A venue, a side, a client
id or a time in force on that type, or any function that converts one into an
`Order`, means this record has been reversed in substance whether or not it was
reversed in text. The acceptance suite is written to fail first.

**A second writer of an `OriginationMandate`.** If a mandate can be produced
anywhere other than `admit` — a `Deserialize` derive, a `from_parts`
constructor, a `Default` — the five gates stop being the only door and §29.3's
protection becomes a formality. This is the `MaxExpectedShortfall` shape in a
new place.

**`ORIGINATION_MAX_EXPOSURE` becoming configurable.** The blueprint's fourth
gate is the word "hard-coded". A deployment that can raise the ceiling is a
ceiling an incident can raise.

**Evidence that the toxic-flow conjunction is the wrong shape.** If a period
arrives in which the platform is picked off while flow is *balanced* and the
reference moves, the two-condition gate will not fire and the volatility term
alone will be carrying it. That is a measurable claim and it has not been
measured; the honest response would be a third condition, not a loosening of
the two.

**A consumer appearing for the quotes.** The moment anything reads a
`QuotePair` to make a decision, the policy stops being a set of constants and
becomes configuration with a reviewed default, and the calibration cost above
stops being acceptable.
