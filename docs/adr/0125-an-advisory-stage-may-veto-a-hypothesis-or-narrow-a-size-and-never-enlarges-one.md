# ADR 0125: An advisory stage may veto a hypothesis or narrow a size, and never enlarges one

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It admits no dependency, no paid
  resource and no external action, and it changes no paper-trading layer.
- **Date**: 2026-10-18
- **Extends**: ADR 0063 (a sizing cap in `(0, 1]` from the fill record) and
  ADR 0066 (a regime narrows and never favours) to every advisory stage the
  cycle already runs.
- **Keeps**: ADR 0003 (paper only), ADR 0005 (confidence is arithmetic),
  ADR 0006 (a classical baseline), `Determinism::Required` (pre-trade checks
  stay deterministic and never route to a model).

## Context

Four stages run in every cycle, journal a record, and are kept off the order
path by a sentence in their own module documentation that names this
decision as missing:

- The World Model Federation (`qip-kernel/src/federation_stage.rs`, module
  doc section "Advisory, and structurally so"): "An arbitrated probability
  that moved a size would be a control nobody chose and no ADR admits."
  `FederationReview` carries every `Arbitration` with its disagreement and
  no merged number.
- The forecast lattice (`qip-world-model/src/lattice.rs`, same section):
  `staleness_multiplier` "states how a consumer *should* contract on a stale
  forecast; no order-path caller reads it, and making one would be a sizing
  decision for an ADR". It returns one when fresh, falls linearly to zero at
  expiry, and refuses after it
  (`grep -n 'pub fn staleness_multiplier' backend/crates/services/qip-world-model/src/lattice.rs`).
- The symbolic reasoners (`qip-kernel/src/symbolic_stage.rs`, module doc):
  `derive` runs after REASON has decided, and a rule pack that disagrees with
  the review is recorded as `agrees_with_review = false`, "never acted on.
  Promoting any part of this to a control is a sizing decision for an ADR".
  `Platform::reason_symbolically` takes the hypothesis by shared reference so
  the derivation has no path back into it.
- The tournament and attribution observers: `Platform::observe_hypothesis`'s
  doc ends "Neither module holds a path back into the thesis or its size", so
  a refutation (a `DataPoisoning` finding among them) holds the question out
  of the tournament and the thesis is sized anyway.

Locate the three platform seams with
`grep -n 'fn reason_symbolically\|fn observe_hypothesis\|fn construct_from' backend/crates/runtime/qip-kernel/src/platform.rs`.

Seven register rows are PARTIAL on exactly this sentence: R12-WORLD-1003 and
3037 (decisions still read the single `WorldModel`), R12-WORLD-9024 (capital
sizing contracts on staleness), R12-REASON-1004, 1024 and 3053 (a symbolic
verifier that can reject), and R12-REASON-3108 (an attempt to falsify the
leading explanation that can stop it).

The cycle already has the shape this decision needs. In
`Platform::construct_from`, DECIDE builds a per-instrument `caps` map; ADR
0063's fill-record cap and ADR 0066's regime stance both write into it,
"compounding ... by multiplication — both live in `(0, 1]`, so the order of
the two is immaterial and neither can raise a bound". `sizing_review::cap_multiplier`
has "no branch above one, which is how §12.4's guardrail is held
structurally rather than by care at the call site".

## Decision

1. **An advisory stage has exactly two powers, and both only subtract.** It
   may **veto** a formed hypothesis, so no proposal is constructed from it,
   or it may **narrow** an instrument's weight bound by a multiplier in
   `(0, 1]` written into DECIDE's existing `caps` map. It never raises a cap,
   a budget, a conviction or a probability that reaches a size, and it never
   creates a proposal. The type that carries a narrowing has no constructor
   above one, as `cap_multiplier` and `Stance::multiplier` have none today.
2. **The symbolic verifier vetoes** (REASON-1004, 1024, 3053). When the rule
   pack's verdict refuses a hypothesis the review approved
   (`agrees_with_review = false` on an approved review), the hypothesis forms
   no proposal and the cycle records the refusal with the derivation that
   produced it. The derivation is moved before approval in
   `reason_symbolically`'s caller, so the verdict is an input to approval
   rather than a comment on it. A reasoner that *refused its input*
   (`Step::Refused`) is not a veto: a malformed timeline is a finding about
   the evidence, not a rule firing, and vetoing on it would let a broken
   input silence every hypothesis.
3. **A refutation vetoes** (REASON-3108). `observe_hypothesis` returns a
   typed refusal when the tournament's verifier society refutes the leading
   explanation (a `DataPoisoning` finding among them), and
   `reason_about_the_queue` honours it: the refuted thesis is not sized. The
   question stays out of the tournament as it does today.
4. **The federation narrows** (WORLD-1003, 3037). For an instrument whose
   arbitration carries a disagreement, DECIDE applies one multiplier from a
   fixed, journaled schedule keyed on the disagreement band — the schedule's
   top value is one, so an agreeing population changes nothing. An uncovered
   scope or a silent member does not narrow: absence is not disagreement,
   and the federation module already names it as uncovered rather than
   computing a coverage nobody measured.
5. **Staleness narrows** (WORLD-9024). A thesis sized against a lattice
   forecast is capped by that forecast's `staleness_multiplier`; a forecast
   past expiry, which the function refuses, vetoes the thesis rather than
   being read as zero or one.
6. **Every narrowing compounds by multiplication** with ADR 0063's and ADR
   0066's, in the same map, so the order of application is immaterial and
   the record of each multiplier is separate. The cycle report names every
   multiplier that moved a bound and which stage wrote it.
7. **Nothing here touches a pre-trade check.** Limits, the risk engine and
   `Determinism::Required` decide after these stages, unchanged; a veto
   removes a proposal before they see it and a narrowing only shrinks what
   they see.

## Consequences

- The four stages stop being decorative: a disagreeing federation, a stale
  forecast, a refusing rule pack and a refuted explanation each change what
  the platform does, and each change is on the log as its cause.
- The code slices left per row: the veto arm in `reason_symbolically`'s
  caller and a kernel test that a rule-refused hypothesis forms no proposal
  (REASON-1004, 1024, 3053); `observe_hypothesis` returning a refusal and a
  kernel test that a `DataPoisoning` refutation yields no proposal
  (REASON-3108); the federation schedule and a test that an agreeing
  population leaves every cap at one while a disagreeing one narrows
  (WORLD-1003, 3037); the staleness cap and a test that an expired forecast
  vetoes (WORLD-9024). Each needs its mutation.
- Each module's "Advisory, and structurally so" section must be corrected in
  the same commit that wires it, naming this record, or it becomes the stale
  sentence this register keeps paying for.

## What it costs

- Fewer proposals and smaller sizes. A rule pack that is too strict now
  costs trades rather than a disagreement in the log; a federation that
  disagrees about everything halves the book's sizes for as long as it
  does. That is the intended direction of error, and it is a cost.
- The rule packs become controls, so changing one is changing what trades.
  A rule-pack edit needs the review a limit change gets, which it did not
  need while advisory.
- The federation schedule is a new set of numbers nobody fitted. They are
  declared, journaled and bounded by one, and they will be wrong in an
  unknown direction until LEARN scores them.
- Moving `derive` before approval puts the symbolic reasoners on REASON's
  latency for every hypothesis; they were off it.

## What would make this wrong

- If a stage's record turned out to be a better predictor of value than the
  review it advises, capping it to subtraction would discard edge, and the
  right step would be a new record making that stage a forecaster with a
  calibrated weight under ADR 0005 — not widening this one.
- If vetoes from the symbolic verifier were dominated by rule-pack defects
  rather than bad hypotheses (measured as declined theses whose
  counterfactual score is positive), decision 2 is too strong and should
  narrow instead of veto.
- If any narrowing were found to raise a bound through an interaction with
  another cap (for example by renormalising weights after a cap), decision 1
  would have been broken in code, and the fix is the code, not this record.

## Rows

R12-WORLD-1003, R12-WORLD-3037, R12-WORLD-9024, R12-REASON-1004,
R12-REASON-1024, R12-REASON-3053, R12-REASON-3108. Each stays PARTIAL until its
code slice above runs on the cycle's default path and a mutated test proves
it.
