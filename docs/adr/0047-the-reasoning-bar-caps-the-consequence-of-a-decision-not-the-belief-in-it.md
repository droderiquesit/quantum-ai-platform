# 0047 — The reasoning bar caps the consequence of a decision, not the platform's belief in it

**Status:** *proposed*, 2026-09-06. **The change this record describes is
already in the tree.** `PlatformConfig::reasoning_confidence_bar` is read at
`backend/crates/runtime/qip-kernel/src/platform.rs:5157` and two kernel tests
hold it there. So this is a ratification, in the shape
[ADR 0044](0044-adr-0039s-four-open-decisions-on-cross-process-region-shares-are-taken.md)
used for three of its four decisions: a record written after a commit decided
something, which is worse than writing it before and better than never writing
it. The commit itself flagged the question — *did this need a decision record?*
— as open. This record answers it: **half of it did, and the half that did is
not the half a reader would guess.**

**Answers:** the open question left by the wave that wired the field. There is
no register row for it in `../plan/PROJECT-PLAN.md`; adding one would change
that file's row count and its re-tallied arithmetic, which is the register
owner's edit and not this record's.

**Relates to:** [ADR 0005](0005-confidence-is-arithmetic.md) (confidence is
computed from evidence and never assigned — the property that makes the
distinction below meaningful), [ADR 0004](0004-capability-gated-agents.md)
(the panel is the reasoner being placed), [ADR 0007](0007-exact-attribution.md)
(what a recorded routing has to be able to reconcile),
[ADR 0023](0023-real-trading-is-the-destination-and-the-opening-is-gated.md)
(`qip-cost-router`'s `Determinism::Required` arm, which this record does not go
near).

**Does not touch:** the paper-trading boundary's layers; the determinism rule;
any dependency; any deployed configuration. No environment sets this field, so
every deployment runs the default.

---

## Context: one number, three documents, and no agreement between them

Before the wiring the tree said three different things about
`reasoning_confidence_bar`, and they could not all be true.

**One.** The field's own documentation described a control an operator sets:

> The default sits at the resolving power the adversarial panel is documented
> to reach, which is what makes the panel the cheapest rung capable of
> answering a consequential question rather than the rung the stage reaches for
> by habit. Lowering it is a deliberate statement that a single agent's answer
> is good enough for this deployment, and the router will then route below the
> panel and the panel will not convene.
> — `qip-kernel/src/config.rs:322-327`

**Two.** The one site that builds the router's requirement passed a different
number entirely — `opportunity.rank.confidence` — under a call-site comment
arguing *for* that choice on the grounds that reusing the detectors' own figure
kept the platform from investigating something it had already judged not
credible enough to look at. The field, its default and its builder existed and
nothing in the kernel read any of them.

**Three.** A test in another binary asserted the field governed a stage it has
never touched. The correction is now in that test's own comment:

> What stood here said the DECIDE stage "holds a thesis to
> `PlatformConfig::reasoning_confidence_bar`". It does not and never did:
> `stage_decide` sizes whatever REASON approved and compares nothing to a bar,
> and when that sentence was written nothing in `qip-kernel` read the field at
> all. The assertion under it — `best_confidence < 0.90` against panel
> confidences near 0.5 — was near-vacuous, and it named a control that did not
> exist as the cause of a number it did not cause.
> — `qip-fastbrain/tests/tape.rs:532-542`

A configured control that nothing reads is the shape
`.claude/rules/domains/risk-and-execution.md` names by example — the
`MaxExpectedShortfall` limit that shipped in every default set and could never
fire. Repairing that shape is not a decision. **Choosing which number replaces
it is**, and that choice was made in a commit and argued in a call-site
comment, which is where the boundaries rule says an architectural argument must
not live.

---

## What actually changed, read off the code rather than off the summary

Four things, and the fourth is the one that matters.

1. **The field is read.** `platform.rs:5157`, in `reason_decision_context`,
   the one place that builds the `qip_cost_router::DecisionContext`.
2. **The requirement is `rank.importance`, capped by the bar**, where it was
   `rank.confidence`: `opportunity.rank.importance.min(bar)` (`:5170`).
3. **A bar outside `(0, 1]` stops the decision and names the field**
   (`:5158-5164`) rather than being silently absorbed by `min`.
4. **The rung the router places the decision on moves, and the rationale loses
   a clause.** On the kernel test's deterministic tape the placement goes from
   a rung below the panel to `multi_agent_reasoning`
   (`qip-kernel/tests/kernel.rs:1249-1260`), and with it the appended sentence
   the kernel writes when it convenes above the placement disappears:

   > `{placed_rationale}; convened at {} regardless, the only rung this
   > platform implements`
   > — `platform.rs:5561-5568`

## What did **not** change, stated because the obvious reading is wrong

**The money.** The natural reading of the diff is that the platform now spends
three orders of magnitude more per reasoning decision:
`IntelligenceTier::cost` is 400 000 micro-units at `tiny_model` and
300 000 000 at `multi_agent_reasoning` (`qip-cost-router/src/tier.rs:113,116`),
a 750-fold difference, and the placement moved from the first to the second.
That reading is false, and a record that let it stand would be worse than none.

Two facts in the code make it false:

- **The panel convenes regardless of where the router places the decision**,
  provided the panel is affordable and fast enough for that decision. The
  kernel places, then asks `assess(PANEL, &context)` and convenes unless the
  answer is unusable (`platform.rs:5526-5556`). The comment there says why: the
  organisation is the only reasoner this platform implements, and a gate that
  refused every decision placed below the panel "would decline nearly all of
  them, and the REASON stage would go silently dead while every rationale read
  as a deliberate saving".
- **The ledger bills what ran, not what was placed.** `ReasonRouting::charges`
  returns `self.convened.map(TierCharge::of)` (`platform.rs:1352-1354`) — the
  rung that actually ran — and `charge_cycle` charges from exactly that
  (`:4828-4830`).

So a decision that was placed at `tiny_model` and convened the panel anyway
cost the panel's price before the change and costs the panel's price after it.
The affordability veto has not moved either: it is the same `assess` on the
same context, run inside `Router::judged` (`router.rs:470-511`) instead of at
the kernel's second question, and `assess` answers affordability *before*
capability precisely so that a rung nothing can afford is refused rather than
climbed past (`router.rs:404-431`).

**What changed is the record, not the spend.** That is a smaller claim than the
diff suggests and a more consequential one than it sounds, for the reason
below.

---

## Decision

### 1. The routing requirement is the decision's consequence, not the platform's belief in it

`rank.importance` — how much the observation matters if it is real — is the
requirement. `rank.confidence` — how likely it is to be real — is not.

Two arguments, and the first is arithmetic rather than taste. Credibility is
**already** inside the value at stake: `share = importance * confidence`
(`platform.rs:5105`), so a flimsy observation already buys a smaller budget.
Using `confidence` again as the requirement counted one fact twice in the same
direction — lowering the budget *and* lowering the bar — while `importance`,
the thing the router is being asked to be sure enough about, was never used at
all.

The second is about which verdict does what. `TierVerdict::Incapable` is the
only verdict that climbs (`router.rs:103-116`); affordability and latency are
terminal. A requirement therefore only ever *widens* the ladder downward — it
cannot decline an observation the platform does not believe. The old call-site
comment claimed exactly that job for `confidence`, and no number in that
position can do it. What declines an incredible observation is
`value_at_stake`, which already carries the credibility, and
`DecisionContext::validate`, which refuses a non-positive one rather than
clamping it.

### 2. The default stays 0.90, and it is a statement rather than a tuning constant

0.90 is `IntelligenceTier::MultiAgentReasoning::resolving_power_f64`
(`tier.rs:157`) — the panel's own documented resolving power, and the panel is
the only reasoner implemented. Setting the ceiling at the panel's own figure is
the statement that a consequential decision is worth an argument. A deployment
that disagrees changes a number in a diff somebody reviews; no environment sets
it today, so every deployment runs 0.90.

### 3. A bar that is not a probability stops the decision and names the field

Refused, not clamped (`platform.rs:5158-5164`). `min` would swallow a bar above
one silently, because `importance` is at most one, so the invalid setting would
never bind and the operator would never learn the configuration was nonsense.
This is the house rule — refuse rather than guess — applied to a configuration
value rather than to an input.

### 4. The half that needed a record, and the half that did not

**No record was needed to make a documented control readable.** Repairing the
`MaxExpectedShortfall` shape restores what the tree already claimed; it takes
no new position.

**A record was needed for what the change does to a measurement**, and this is
the part nobody would find in a diff. `ReasonRouting` documents the gap between
the rung placed and the rung run as evidence for a future build:

> The gap between this and [`Self::tier`] is the measured case for building the
> cheaper rung.
> — `platform.rs:1332-1334`

Capping the requirement at the panel's own resolving power narrows the
population in which that gap can appear to opportunities whose `importance`
is *below* the bar. Before, the gap appeared whenever the detectors'
`confidence` was below 0.90, which is most observations most of the time. The
series still carries it — `place_reason_routing` labels `tier` with the
placement and `outcome` with whether the panel convened (`:5386-5399`), so
`tier=statistical_model, outcome=convened` is exactly "a cheaper rung would
have sufficed and the panel ran anyway" — but the sample it is measured over
is smaller by construction, and the platform's own case for building a cheaper
reasoner is weaker for it. **That is a decision about evidence, and it is the
reason this record exists.**

---

## The alternatives, and why they were not taken

**(a) Write nothing; the code comments say it.** The status quo the commit
left, and the cheapest option. Rejected: three documents in this tree disagreed
about one number, which is the signature of a choice that was never recorded
anywhere durable, and the resolution now lives in a seventeen-line call-site
comment. `.claude/rules/architecture/00-boundaries.md` is explicit that an
architectural choice explained in a comment needed a record. Beyond form: the
consequence in decision 4 is invisible at the call site, because the call site
cannot see the metric it degrades.

**(b) Keep `rank.confidence` as the requirement.** Rejected on the
double-counting argument above. It is worth naming that this alternative had a
written case in the tree and the case was wrong rather than merely different —
which is why the correction is stated in the code as well as here.

**(c) Use `rank.importance` uncapped.** Rejected, and it is the alternative
that looks most principled. Importance saturates at one and the ladder's top
rung reaches 0.99 (`config.rs:315-320`), so an uncapped requirement of 1.0 is
unroutable: `judged` would exhaust the ladder and return a refusal
(`router.rs:503-510`), and the platform would decline to reason about exactly
the observations it rated most consequential. The cap is load-bearing, not
decorative.

**(d) Clamp an out-of-range bar instead of refusing it.** Rejected: see
decision 3. A control that silently never binds is the shape this whole record
is about.

**(e) Delete the field.** The field's own documentation offers this — "Say
where a number is read, or delete it" (`config.rs:340`) — and it was the
smaller change. Rejected because *some* constant must sit in that position:
(c) shows the requirement has to be capped, so deleting the field replaces a
named, defaulted, reviewable control with a literal. A number an operator can
move in a diff somebody reviews is better than the same number spelled inline,
and it is only better while something reads it, which is what the wiring fixed.

**(f) Make the bar govern whether a thesis is sized, which is what one test
believed it did.** Rejected as a category error and named because a reader who
finds `tape.rs`'s old sentence in a stale checkout will propose it: sizing is
`stage_decide`'s and the review policy's floor, and a routing requirement has
nothing to say about whether an answer is acted on. Conflating them would put
a cost-routing knob on the risk path, which is the thing
`Determinism::Required` exists to make structurally impossible.

---

## Where this sits in the layering

No edge moves. `qip-kernel` (runtime) already depends on `qip-cost-router`
(service) and on its own `PlatformConfig`; the field is read in the runtime and
the requirement is passed *down* into the router as a value. No lib gains I/O,
no service gains a dependency on the runtime, nothing depends on an app, and
the configuration is still read at a composition root and handed in. Direction:
app → runtime → service → lib. Inward.

---

## What it costs

- **The cheaper-rung build case is measured over a smaller sample.** Decision
  4. The platform will accumulate less evidence for the one thing the cost
  router exists to justify, and it will do so quietly.
- **A ratification is weaker than a decision.** This record was written after
  the commit, so it can only ratify or ask for a revert; the reviewer who would
  have argued (b) or (e) at the time no longer has a cheap moment to do it.
- **0.90 is a judgement dressed as a citation.** It is the panel's documented
  resolving power, and that documentation is this platform's own — an internal
  constant agreeing with an internal constant is consistency, not calibration.
  Nothing here measures whether the panel actually resolves at 0.90.
- **A new way for a cycle to stop.** Decision 3 adds a refusal on a
  configuration value. That is the intended direction, and it is still one more
  configuration mistake that halts a REASON stage rather than degrading it.
- **The record now says the spend did not change.** If someone later makes the
  placement bill the cycle — a reasonable-looking change, since `JudgedRouting`
  carries its own `charges` — this record's central factual claim becomes
  false and the 750-fold difference becomes real.

## What would make this wrong

- **A second reasoner being implemented at a rung below the panel.** The moment
  something can actually run at `tiny_model` or `statistical_model`, the
  placement stops being a record and becomes a dispatch, the panel stops
  convening regardless, and every "what did not change" claim above needs
  re-reading. That is the day to re-take this decision, not to cite it.
- **The ledger being charged from `routing.tier()` rather than from
  `convened`.** It would make the placement a bill, and a bar that moves the
  placement would then move the money. `ReasonRouting::charges` is the line to
  watch.
- **A deployment setting the bar.** None does. The first one that does is
  making the statement decision 2 describes, and it should say so in its tfvars
  comment rather than leaving a number.
- **`importance` ceasing to mean the consequence.** The requirement is only
  the right number while `rank.importance` is "how much this matters if it is
  real". If a detector ever writes importance as a blend that includes its own
  credibility, the double counting comes back through the other door.
- **This record being cited to put any other configured number on the risk
  path.** The determinism rule is untouched here and is not weakened by the
  existence of a routing knob.

## Applied by this record

**Nothing.** The wiring, its refusal and its two tests are already in the tree
and this document changes no file. What it adds is the argument and the named
consequence; what it asks of whoever accepts it is a decision on the register
row — this platform has no row for a change that has already landed, and either
the register gains one or it records that ratifications do not get rows.

**Gates run for this record: none.** No Rust, TOML, Terraform or TypeScript
file was modified by it, so `cargo fmt`, `cargo clippy`, `cargo test`,
`terraform validate` and the frontend gates have nothing to judge. The
documentation acceptance suite is the one gate a new file under `docs/adr/`
does reach; whoever accepts this record runs
`cargo test -p qip-acceptance --test documentation` and quotes its
`test result:` line.
