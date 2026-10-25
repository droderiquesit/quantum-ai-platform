# ADR 0200: An order names evidence the sender can re-read, the send seam resolves it, and the order's record carries it

- **Status**: **Accepted**, 2026-10-24, decided under the owner's 2026-10-24
  delegation ("Make all decisions for me, I want the end result to have a full
  system", relayed by the parent lane). The delegation authorises the
  structural choices below. It does not authorise a live order, a financial
  limit or a weakened guard, and none is made: every control here refuses
  more than it did before and nothing is relaxed.
- **Date**: 2026-10-24
- **Extends**: ADR 0080 (a disposition unwind is an instruction of the signed
  policy), ADR 0084 (the release schedule's fields on `OrderSent`), ADR 0155
  (the cell admits against declared inputs).
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no dependency is
  added), the three paper-trading layers, and `Cell::send` as the one place
  a cell calls a `Placer`.

## Context

R12-GOV-6127 reads: "Public communications and market-facing actions cannot
be justified solely by expected profit or desired price impact", with the
unit "a justification of only profit and/or price impact is refused; one with
an evidence ground is admitted".

`qip_contracts::action::Justification` already exists, and `Justification::of`
refuses a set of grounds with no non-blank `Ground::Evidence`. Commit
`288e5887` made `Cell::send` take one and made the kernel queue a thesis only
beside one. An independent review then refused to grade the row complete,
for three reasons that are each correct:

1. **An evidence ground was a string nothing checked.** A cycle leg stood on
   `cycle:{id}` — the name of the trade, not a reason for it — and an offset
   on `cycle_broken:{id}`. The type proved that a reference had been named,
   not that it referred to anything. A caller could put any text there and
   the order would go.
2. **The chain did not say why an order went.** `OrderSent` and `Hedged`
   carried no grounds, so an auditor could not tell a grounded order from a
   profit-only one without re-deriving the reasons from the entries around
   it, which is the attribution the platform exists to make unnecessary.
3. **The desk's order had no justification of its own.** `Order::validate`
   refused an order with no non-blank hypothesis, which is the same rule, but
   no value of type `Justification` existed on the desk's order path, so the
   rule and the contract type were two statements of one fact.

The review also asked that "systematic edge strategies are out of scope" not
be graded as an owner decision. This record does not scope anything out: it
grounds every order the cell sends.

## Decision

### 1. Six forms of evidence reference, five of them checked

An evidence ground on an order a cell sends is a reference in one of six
forms. Five name something the cell holds and are resolved against the
cell's own state at `Cell::send`, after the posture checks (live venue,
execution mode) and before the fence, the risk gate and the order sequence:

| Reference | Resolves when |
|---|---|
| `journal:signal_raised:{sequence}` | a signal raised this pass was journaled at that sequence |
| `policy:{sequence}:disposition:{strategy}:{object}` | the signed policy in force has that sequence and instructed that lot unwound (ADR 0080) |
| `journal:feed_silent:{sequence}` | a venue was journaled silent at that sequence and is still silent |
| `journal:cycle_broken:{sequence}` | a cycle broke at that refusal and is being unwound now |
| `book:{venue}:{object}@{sequence}` | the cell holds that book, it is not stale, and it has applied that venue message |

The sixth, `feature:{name}@{revision}`, corroborates and is not checked: the
revision belongs to a contributor's feature graph, which the send seam does
not hold. It therefore never carries an order alone.

An order is refused, journaled under `GATE_JUSTIFICATION` (`justification`)
and counted on `qip_edge_refusals_total{gate="justification"}` when **no**
reference resolves, when **any** reference in a checked form does not
resolve, or when a reference is in no known form. A false citation beside a
true one is still a false record of why the order went, so one is enough to
refuse. `Cell::send` records that refusal directly, as it records
`GATE_LIVE_VENUE`, because it has no `WorkReport` to push one onto; the site
passes the `pub const`, which already went through `Cell::refuse`, so the
bound `qip-edge/src/telemetry.rs` states — every site passes a constant —
holds and the set of `gate` label values does not grow.

### 2. What each kind of order cites

- **A net intent** cites each contributor's signal of this pass, each feature
  revision a contributor reasoned from, and for a disposition unwind the
  policy's instruction (unchanged from `288e5887`; now resolved).
- **A cycle leg** cites the book of **every** leg of its cycle, by the venue
  sequence of the last message the cell applied to it. The cycle's motive is
  the profit the books imply; the evidence is the books themselves, which a
  reader can match against the `Ingested` entries the same chain holds. A
  leg whose books cannot be named is not sent, and the cycle breaks through
  `break_cycle`, as any other leg that cannot be sent does.
- **A feed-loss hedge** cites the `FeedSilent` entry it answers. Its reason is
  risk reduction, not profit.
- **A cycle offset** cites the `arbitrage_cycle_broken` refusal that broke the
  cycle. `break_cycle` registers that sequence for the length of one
  `unwind_cycle` call and removes it after, so an offset can cite the break
  it answers and nothing else can cite one.

### 3. The order's record carries its grounds

`Decision::OrderSent` and `Decision::Hedged` gain `grounds: Vec<String>`,
written by `Justification::rendered` — `expected_profit` and
`desired_price_impact` by name and each evidence ground as its reference. The
field is `#[serde(default, skip_serializing_if = "Vec::is_empty")]`: entries
sealed before it existed re-serialise byte for byte, so every chain already
sealed still verifies, the rule `release_at` and `grants` already follow.

### 4. The desk's order derives its justification

`qip_execution_engine::order::Order::justification` returns the order's
`Justification`: `expected_profit` and one `hypothesis:{id}` evidence ground
per non-blank hypothesis. `Order::validate` calls it, so the rule has one
statement; the order manager and the simulated broker both validate, so
neither seam a desk order reaches a venue through admits one without a
justification. A hypothesis is the independent ground because on the desk it
is what REASON synthesised from evidence and review found actionable, and the
thesis built on it was queued only beside a `Justification` naming the
supporting evidence (`JustifiedThesis`). `Platform::submit_order` and
`order_from` keep their signatures: the order they handle carries its
justification as a function of its own fields, and changing two hundred and
seventy call sites to pass a value derivable from the order would add a
second statement of the same fact.

## Consequences

- Every order a cell sends names evidence the cell checked at the moment it
  sent it, and the chain says what that evidence was.
- An order that cites a signal nobody raised, a book the cell does not hold,
  a message a book never applied, or text in no known form never reaches a
  venue and burns no order number.
- A reader of the chain can audit R12-GOV-6127 from `OrderSent` and `Hedged`
  alone.

## What it costs

- No money, no infrastructure and no dependency: the change is in-process
  code in `qip-edge`, `qip-contracts` and `qip-execution-engine`.
- One string parse and one map lookup per evidence reference on every order
  a cell sends, on the send seam, which already runs the risk gate's
  arithmetic; the reference count per order is bounded by the contributors
  of one net or the legs of one cycle.
- Each `OrderSent` and `Hedged` entry grows by its rendered grounds, a few
  short strings, on a journal that is already bounded and trimmed on ship.
- An order whose citation does not resolve is now refused where it was
  sent before. That is the point, and it is a cost: a producer that builds
  a reference in a new form must add the form here first.

## What would make this wrong

- **A legitimate order whose evidence is none of the five forms.** Then the
  form belongs in this table and in `Cell::resolves`, with a test; it does not
  belong in an exception.
- **A book reference read as proof the cycle was profitable.** It proves the
  books existed as cited. Whether the scanner priced them correctly is the
  scanner's own tests' business, and this record does not claim otherwise.
- **The desk's hypotheses being minted without evidence.** The desk's
  ground is only as good as REASON's refusal to queue an unsupported thesis;
  `justify_thesis` is that refusal and its tests are its proof.

## Known limits, from the independent review of 2026-10-24

The review that read this change refused to grade R12-GOV-6127 complete,
and its three findings stand as the remaining work, recorded here so that
nobody reads this record as closing the row:

1. **A requote was not re-justified** — closed after the review. A
   requote moves an order's price and nothing else, so a replacement stands
   on the grounds `Cell::send` resolved when the original went out, kept on
   the working order. `Cell::admit_replacement` refuses under
   `justification`, journaled and counted, when the order it replaces holds
   no recorded ground beyond profit, price impact and features. They are not
   resolved again, because a pass's signals are cleared with it and a ground
   re-resolved a pass later would refuse every honest requote.
2. **A book reference may be read as the profit opportunity itself.** This
   record reads the books a cycle was found over as the evidence and the
   profit as the motive. The review reads the book as the price discrepancy,
   so a relabelled profit ground. Closing it means a leg also cites a ground
   that is not price — a signal, a policy instruction, or a signed
   cycle-discovery record — or an owner-approved amendment of the
   requirement's reading.
3. **A desk hypothesis is checked for presence, not resolved.** `Order` can
   see only its own fields, so `hypothesis:{id}` is not resolved against the
   hypotheses REASON actually queued. Closing it means a registry of queued
   hypotheses the order manager resolves against.

## Not decided here

No financial limit, no venue, no strategy, no deployment, and nothing about
public communications beyond the agency guard's existing conduct gate, which
this record does not touch.
