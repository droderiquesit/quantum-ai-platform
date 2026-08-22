# 0007 — Attribution must reconcile exactly

**Status:** accepted

## Decision

`Attribution::residual` must be zero, and `Attribution::validate` refuses an
attribution where it is not. There is no "other" or "unexplained" bucket.

## Why

An attribution that nearly adds up tells you nothing. The missing piece is
precisely where the thing nobody understood is hiding, and a bucket labelled
"other" is a place for it to sit undisturbed for years.

Making the reconciliation a hard constraint means a discrepancy surfaces as a
failure at the moment it appears, in the code that produced it, rather than as
a slowly growing line in a report.

## How it closes

Every explicit component — commission, spread, impact, implementation
shortfall, carry, financing, factor — is computed from its own inputs. The
position's idiosyncratic move is then whatever is left.

Making the *price move* the balancing term is the right way round: a price
change is the one quantity always exactly measurable from the books, so
defining it residually loses nothing. Defining any of the cost components
residually would lose a great deal.

## What it costs

Every input has to be present and correct. A missing commission figure does not
produce a slightly wrong attribution; it produces a failure. In practice that
is the behaviour worth having, but it does mean the attribution stage is
sensitive to upstream data quality in a way a tolerant one would not be.

## What would make this wrong

Nothing yet encountered. If a genuinely unmeasurable component appeared —
something that affects P&L and cannot be computed from the books — the honest
response would be a named component with a stated measurement method, not a
relaxation of the constraint.
