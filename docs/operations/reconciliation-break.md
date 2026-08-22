# The book and the venue disagree

A reconciliation break means the order manager recorded a fill the order itself
refused — an over-fill, a fill on an order already closed, a fill of zero. The
platform's positions may not be the venue's positions.

## Do this

1. **Halt.** Not because the platform is about to do something wrong, but
   because everything it decides next is sized off a book that may be wrong.
   ```sh
   curl -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" \
     -d '{"reason":"reconciliation break under investigation"}' \
     .../api/v1/kill-switch
   ```
2. Read the breaks. Each one names the venue, the order, the quantity and what
   the order said when it refused.
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/orders
   ```
3. Get the venue's own record for those order ids. The platform's number is the
   one to doubt first, but only one of the two can be right.
4. Correct the book from the venue's record, not the other way round.
5. Clear the halt once the two agree.

## Why it is surfaced rather than absorbed

The order state machine refuses a fill it cannot apply, and that refusal is a
`Result`. Discarding it would leave the platform quietly holding a position
different from the one the venue holds, and nothing downstream would notice:
attribution would reconcile against the wrong quantity, risk would check the
wrong exposure, and the next order would be sized off both.

So the refusal is recorded on the submission result and counted on the order
manager. A non-empty `reconciliation_breaks` is the one condition under which
none of the platform's own numbers should be believed.

## What it is not

It is not a venue rejection. A venue that refuses an order is an ordinary
outcome, recorded as `VenueRejected` with the reason, and the book stays
correct. A break is the case where the venue says something happened and the
platform cannot represent it.
