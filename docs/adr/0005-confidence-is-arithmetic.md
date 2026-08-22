# 0005 — Confidence is computed, never assigned

**Status:** accepted

## Decision

`Hypothesis` has no constructor that accepts a confidence. `Hypothesis::form`
computes it from the evidence via a Bayesian update in log-odds, and
`Hypothesis::validate` rejects a hypothesis whose stated confidence has drifted
from what its evidence implies.

The only way to a higher confidence is more or better evidence.

## Why

A confidence somebody chose is a number that will be chosen to suit the
conclusion. Not dishonestly — by the ordinary process of writing up an argument
one already believes.

Making it arithmetic has three consequences worth having. The same evidence
gives the same confidence every time. A change in confidence can always be
traced to a change in evidence. And a language model, which could otherwise
produce a very persuasive 0.85, has no way to affect it.

## The correction that made it work

The first implementation multiplied the evidence-driven posterior by the causal
chain's confidence. That was wrong in a way the tests caught: with a prior of
0.25 and a long mechanism it returned 0.22, asserting the claim was *less*
likely than its own base rate purely because its explanation was long. With the
review floor set against that scale, the red team then rejected every thesis
including sound ones.

The correction is to attenuate toward the prior rather than toward zero: a weak
mechanism means the evidence tells you *less*, not that the claim is less likely
than the base rate. `bayes::attenuate` blends in log-odds space, which is
monotone in both inputs, returns the posterior at full weight and the prior at
zero.

## What it costs

Evidence has to be modelled properly — kind, reliability, diagnosticity, origin
— which is more work than writing down a number. And the arithmetic has to be
calibrated: the review policy's floor is set against the scale the formula
actually produces, and changing one without the other breaks the red team.

## What would make this wrong

Nothing about the approach. But the specific parameters — the per-item
log-likelihood cap, the independence discount, the minimum chain weight — are
judgement calls, and if calibration data ever showed them systematically
wrong, they should change. They are constants with comments for exactly that
reason.
