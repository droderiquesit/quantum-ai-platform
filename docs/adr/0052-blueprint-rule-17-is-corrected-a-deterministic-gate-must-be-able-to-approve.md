# 0052 — Blueprint rule 17 is corrected: a deterministic gate must be able to approve, and a computed reduction is not a violation of it

**Status:** *proposed*, 2026-09-12.

**Relates to:** [ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(the risk gate is one of the structures built without the live-capital path
the blueprint assumes), [ADR 0051](0051-the-two-unbound-custody-attestations-are-bound-to-a-content-digest-and-a-policy-fingerprint.md)
(the transfer gate's own `Admitted`/`Vetoed` shape, the other of the "both
gates" rule 17 names), `.claude/rules/domains/risk-and-execution.md` (the
`MaxExpectedShortfall` precedent this record measures itself against).

**Does not touch:** any Rust source. This record changes
`docs/architecture/algorik-blueprint-v10.1-source.md` and
`docs/DELIVERY-STATUS.md` only.
`backend/crates/services/qip-risk-engine/src/pretrade.rs` is read here, not
written to, and no composition root's use of `PreTradeChecker` is changed by
this record either.

---

## Context

`docs/DELIVERY-STATUS.md` recorded, under "Where the blueprint and the code
disagree", that §56.2 rule 17 —

> Both gates return veto or silence. Neither ever returns approval. Errors and
> timeouts are vetoes.
> — `docs/architecture/algorik-blueprint-v10.1-source.md:5150`

— is contradicted by `qip-risk-engine::pretrade::PreTradeDecision`, which has
an `Approved` arm and a `Reduced` arm that resizes an order rather than
refusing it:

```
grep -n -A16 'pub enum PreTradeDecision' backend/crates/services/qip-risk-engine/src/pretrade.rs
```

```rust
pub enum PreTradeDecision {
    /// The order may proceed.
    Approved,
    /// The order may proceed at a reduced size.
    Reduced {
        permitted_quantity: Decimal,
        limiting_constraint: String,
    },
    /// The order is refused.
    Rejected { reasons: Vec<String> },
}
```

Read on its own, that looks like a straightforward case of the code doing
something the specification forbids. It is not, for three reasons found by
reading `pretrade.rs` itself rather than just its enum declaration.

**First, a risk gate that can never approve cannot do its job.** The whole
point of a deterministic pre-trade check is to let a compliant order through
and stop a breaching one. An order that breaches no limit has to produce some
outcome other than a veto, and "silence" — the framing the blueprint uses at
§33 ("It returns veto or silence, never approval. Silence is permission")
— *is* an approval; it is only not named one. `PreTradeDecision::Approved` is
the named version of exactly that state, and naming it is the stronger
choice under this repository's own principle that "a guarantee the type
system holds beats one a runtime check holds" (`CLAUDE.md`, principle 4): an
exhaustive `match` on `PreTradeDecision` cannot forget the silence arm the way
a function that just returns without vetoing can be edited to forget to veto.

**Second, the blueprint's own §33 already contains a resizing action.** The
Unified Risk Gate's check table has a row for belief freshness whose "on
failure" column reads "Reduce to conservative multiplier"
(`docs/architecture/algorik-blueprint-v10.1-source.md:2755-2757`), sitting in
the same table as five rows whose "on failure" is "Veto". Rule 17's "neither
ever returns approval" was already in tension with the section that states
the gate's own design, independent of anything in the tree.

**Third, `Reduced` is not a guess and it is not the default.** Read in full:

```
sed -n '139,398p' backend/crates/services/qip-risk-engine/src/pretrade.rs
```

- `allow_reduction` is a field on `PreTradeChecker`, **off unless a caller
  calls `.allowing_reduction()`**, with a doc comment naming why: "Silently
  resizing an order means the executed trade is not the one that was
  reviewed, and the difference is invisible unless someone reads the fills."
  The one production composition root never calls it —
  `grep -n 'PreTradeChecker::new(limits.clone())' backend/crates/runtime/qip-kernel/src/platform.rs`
  constructs the checker with reduction off, and no other line in that file
  calls `.allowing_reduction()`. `Reduced` is built, tested, and **dormant in
  the one path that runs in production**; every non-test call site that does
  turn it on is a test fixture (`grep -rn 'allowing_reduction()' backend/crates`).
- The permitted quantity is computed, not approximated: `largest_permissible`
  bisects entirely in `Decimal`'s underlying scaled `i128`, with the module's
  own comment recording a fixed defect this replaced — an earlier version
  crossed into `f64` mid-bisection and could report a quantity a hair *above*
  the limit it was supposed to respect. The version in the tree cannot
  overshoot, by construction of the halving arithmetic.
- A figure the risk state could not evaluate refuses outright, **never**
  reduces, whatever `allow_reduction` says — the function's own doc comment
  states the reason: "no size makes an uncomputed figure computable." This is
  the same defect class `.claude/rules/domains/risk-and-execution.md` names
  for `MaxExpectedShortfall` — a control that cannot fire reads as protection
  and is not — applied here in the opposite direction: a control that has not
  run must not be allowed to look like one that passed.
- An error from `check()` — including `order.validate()`'s refusal of a
  zero-quantity order, a non-positive reference price, or an empty kill-switch
  scope — is treated as a refusal by its one caller:
  `grep -n -A12 'match self.checker.check' backend/crates/services/qip-execution-engine/src/oms.rs`
  shows the `Err` arm building a `RefusalReason::Malformed` and returning,
  never approving by default. This is rule 17's own "errors and timeouts are
  vetoes" clause, held exactly as written.

So the code is not the side that is wrong. `PreTradeDecision::Approved` is the
structural form of the "silence is permission" rule the blueprint already
states elsewhere, `Reduced` is a capital-protecting refinement of the
"veto"/"reduce to conservative multiplier" spectrum §33 already contains,
computed exactly and gated off by default, and every uncomputed or errored
path refuses rather than approves. Rule 17's flat sentence — "neither ever
returns approval" — is the text that does not survive contact with the
section it is meant to summarise.

## Decision

**Rule 17 is corrected in the blueprint source, and §56.2's row in
`docs/DELIVERY-STATUS.md` moves from "contradicted" to "held".**

The corrected rule, replacing the text at
`docs/architecture/algorik-blueprint-v10.1-source.md:5150`:

> Both gates refuse or admit deterministically; permission is never inferred,
> only computed. An order or movement that breaches nothing is admitted. A
> breach may be resized to the largest quantity that still clears every
> check — computed exactly, never approximated, and only where a composition
> root has explicitly enabled resizing — and a figure the state could not
> evaluate, or any error or timeout, refuses outright rather than approving by
> default.

This keeps everything rule 17 was actually protecting — no gate approves by
judgement, an unevaluated control never reads as a passed one, an error is
never silently generous — and drops the one clause the tree could never have
satisfied: that a deterministic check which exists to let compliant orders
through must never say so.

## Alternatives considered and rejected

**Leave rule 17 as written and score `PreTradeDecision` as a defect to fix in
code.** Rejected. There is no version of a pre-trade risk gate that both (a)
runs on every order and (b) never lets a clean one proceed; the two
requirements are inconsistent for a gate whose blocking arm is real. Filing a
task to remove `Approved` would either make every order un-executable or
would just rename `Approved` to something that still means the same thing —
the second is what "silence" already is.

**Read "neither ever returns approval" as describing only the `Reduced` arm
(i.e. the objection is to resizing, not to `Approved`).** Rejected on the
text: rule 17 says "neither ever returns approval", not "neither ever resizes
an order", and §33's own belief-freshness row already contains a resize
action, so the section this rule summarises does not support reading it that
narrowly.

**Delete the `Reduced` arm and keep only `Approved`/`Rejected`.** Rejected as
a Rust-code change this record does not have standing to make (see "Does not
touch"), and rejected on the merits regardless: `Reduced` is off by default,
computed exactly, and gives a desk the option to shrink an order to the
largest safe size instead of losing the trade entirely — a strictly more
capital-protective menu of outcomes than refuse-or-approve alone, not a
weaker one.

**Leave the disagreement recorded as open in `DELIVERY-STATUS.md` rather than
resolving it.** Rejected: the document's own charter is to record a verdict a
reader can act on, and "somebody has to decide which is wrong" was the
standing instruction under which this section exists. The evidence above is
sufficient to decide it now.

## What it costs

**One blueprint line changes meaning slightly.** A future reader who quotes
rule 17 from memory as "never returns approval" will be quoting the
superseded text. The correction is recorded here and at the line itself so
the citation trail survives.

**The dormant-in-production fact is now written down.** `Reduced` existing but
never being enabled by the one composition root that matters is not a defect
this record fixes — enabling it is a desk decision about whether an operator
wants shrink-or-refuse behaviour, and that decision is explicitly left open.
Recording it here means a future reader cannot mistake "the type supports
reduction" for "reduction happens".

## What would make this wrong

- **A composition root turning on `.allowing_reduction()` in a way that
  changes what actually executes without a corresponding review of whether a
  resized order should count as "the trade that was reviewed".** The doc
  comment on `allow_reduction` names exactly this risk; this record does not
  relax it.
- **A future `PreTradeDecision` arm that approves on an unevaluated or errored
  figure.** The whole argument above depends on `unevaluated` and `Err` both
  refusing unconditionally; if either ever approved by default, rule 17's
  corrected text would need to read differently, and would need to say the
  gate can silently pass a control that never ran.
- **The transfer gate (`qip_capital_fabric::journal::GateVerdict`) growing a
  resize-like arm.** It has none today — `Admitted`/`Vetoed` only — and
  nothing here proposes adding one; a movement is a different shape of
  decision from an order's size.
