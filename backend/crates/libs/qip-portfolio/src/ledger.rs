//! Double-entry posting logic for a paper fill (ADR 0100 §1; §2 names the
//! writer, `qip-ledgerd`, which is not here).
//!
//! Two steps, and the split between them is the point.
//!
//! 1. [`PaperFill::try_from`] reads a cell's P1 [`OutcomeRecord`] and refuses
//!    everything a posting cannot be built from honestly: a fill whose
//!    `simulated` flag is not `true` (ADR 0100 §9's fourth paper fence), a
//!    fill with no side or no quote unit (red-team B2), shares that do not
//!    sum to the fill, and any number that would need rounding to fit. It is
//!    the **only** constructor. A `PaperFill` has no `simulated` field to
//!    check, because its existence is the check: code holding one cannot be
//!    holding a live fill, and code that wants to book a live fill has no
//!    type to book it with.
//! 2. [`post`] turns a `PaperFill` into one [`LedgerEvent`], split exactly by
//!    the fill's shares. It is a pure function of the fill — no clock, no
//!    I/O, no market data — so a replay of the same journal books identical
//!    postings, byte for byte.
//!
//! For each share `(strategy, q)` of a fill of object `X` at price `p` in
//! quote unit `U` on venue `v` from cell `c`, with `t = trading:c/strategy`:
//!
//! ```text
//! bought (Ask):  Dr t          X q      Cr venue:v  X q
//!                Dr venue:v    U q*p    Cr t        U q*p
//! sold   (Bid):  Dr venue:v    X q      Cr t        X q
//!                Dr t          U q*p    Cr venue:v  U q*p
//! reported fee f, this share's part f*q/Q of it:
//!                Dr fees:v     U f*q/Q  Cr t        U f*q/Q
//! ```
//!
//! An unreported fee posts nothing and is marked
//! [`FeeReport::Unreported`]; it is never estimated, and never zero.

use qip_contracts::ledger::{
    Account, Direction, FeeReport, LedgerEvent, Posting, Settlement, SourceEvent,
};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{Decision, OutcomeRecord};
use qip_core::Decimal;
use qip_core::decimal::{SCALE, SCALE_DIGITS};
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// A fill the ledger may book: simulated, complete, and exact.
///
/// Private fields and one constructor, `TryFrom<&OutcomeRecord>`. See the
/// module documentation for what it refuses and why a type rather than a
/// flag check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaperFill {
    source: SourceEvent,
    venue: String,
    object: String,
    side: BookSide,
    quote_unit: String,
    quantity: Decimal,
    price: Decimal,
    fee: FeeReport,
    /// Keyed by strategy so the postings come out in one order on every
    /// run, whatever order the cell listed the shares in.
    shares: BTreeMap<String, Decimal>,
}

/// Parse a journal decimal exactly, or refuse.
///
/// `Decimal::parse` trims whitespace and rounds a tenth fractional digit
/// into the ninth. Either is a silent correction of a venue fact, and a
/// ledger that books the corrected figure cannot be reconciled against the
/// statement that carries the original.
fn exact(field: &str, text: &str) -> Result<Decimal> {
    let refused = || {
        Error::invalid(format!(
            "fill {field} `{text}` is not an exact decimal of at most {SCALE_DIGITS} \
             fractional digits; the ledger refuses it rather than rounding or trimming it"
        ))
    };
    if text.trim() != text {
        return Err(refused());
    }
    if let Some((_, fraction)) = text.split_once('.')
        && fraction.len() > SCALE_DIGITS as usize
    {
        return Err(refused());
    }
    Decimal::parse(text).ok_or_else(refused)
}

fn positive(field: &str, text: &str) -> Result<Decimal> {
    let value = exact(field, text)?;
    if !value.is_positive() {
        return Err(Error::invalid(format!(
            "fill {field} {value} is not positive; a fill of nothing, or of less than \
             nothing, has no economic effect to book"
        )));
    }
    Ok(value)
}

/// `quantity * price`, exactly, or refused.
///
/// `Decimal::checked_mul` rounds at the ninth digit. Summed over shares, a
/// rounded notional per share can differ from the rounded notional of the
/// fill, and the difference is exactly the thing a ledger must not invent.
/// So the product is taken on the raw integers and refused when it does not
/// land on the grid.
fn exact_product(quantity: Decimal, price: Decimal) -> Result<Decimal> {
    let raw = quantity.raw().checked_mul(price.raw()).ok_or_else(|| {
        Error::numeric(format!(
            "notional {quantity} x {price} overflows; the fill is refused rather than wrapped"
        ))
    })?;
    if raw % SCALE != 0 {
        return Err(Error::invalid(format!(
            "notional {quantity} x {price} has more than {SCALE_DIGITS} fractional digits; \
             the ledger refuses it rather than rounding the cash leg"
        )));
    }
    Ok(Decimal::from_raw(raw / SCALE))
}

/// `fee * share / total`, exactly, or refused.
///
/// A share of a fee that does not divide exactly has no honest allocation:
/// rounding each part and handing the remainder to one strategy is an
/// attribution the venue never made. Refused, and the refusal names why.
fn exact_part(fee: Decimal, share: Decimal, total: Decimal) -> Result<Decimal> {
    let raw = fee.raw().checked_mul(share.raw()).ok_or_else(|| {
        Error::numeric(format!(
            "fee {fee} x share {share} overflows; the fill is refused rather than wrapped"
        ))
    })?;
    if total.raw() == 0 || raw % total.raw() != 0 {
        return Err(Error::invalid(format!(
            "fee {fee} does not split exactly into a share of {share} of {total}; the ledger \
             refuses the fill rather than rounding one strategy's part of the fee"
        )));
    }
    Ok(Decimal::from_raw(raw / total.raw()))
}

impl TryFrom<&OutcomeRecord> for PaperFill {
    type Error = Error;

    fn try_from(outcome: &OutcomeRecord) -> Result<Self> {
        let Decision::Filled {
            order_id,
            venue,
            object,
            quantity,
            price,
            simulated,
            shares,
            side,
            quote_unit,
            fee,
        } = &outcome.entry.decision
        else {
            return Err(Error::invalid(format!(
                "outcome {}#{} is a `{}` decision, not a fill; only a fill is posted",
                outcome.cell,
                outcome.journal_sequence,
                outcome.entry.decision.kind()
            )));
        };

        // The fourth paper fence (ADR 0100 §9), checked before anything else
        // so a live fill is refused as live, whatever else is wrong with it.
        if !simulated {
            return Err(Error::denied(format!(
                "fill on order {order_id} at {venue} is not marked simulated; the ledger \
                 books paper fills only and a live fill has no representation in it"
            )));
        }

        // Two claims about where this fill sits on the chain must agree, or
        // the source the ledger cites is not the entry it posted from.
        if outcome.journal_sequence != outcome.entry.sequence
            || outcome.journal_digest != outcome.entry.digest
        {
            return Err(Error::invalid(format!(
                "outcome cites journal entry {} ({}) but carries entry {} ({}); the ledger \
                 refuses a fill whose source it cannot place on one chain",
                outcome.journal_sequence,
                outcome.journal_digest,
                outcome.entry.sequence,
                outcome.entry.digest
            )));
        }

        let Some(side) = *side else {
            return Err(Error::invalid(format!(
                "fill on order {order_id} carries no side; without it the ledger cannot tell a \
                 debit from a credit, and it will not guess"
            )));
        };
        let quote_unit = match quote_unit.as_deref() {
            Some(unit) if !unit.is_empty() => unit.to_string(),
            _ => {
                return Err(Error::invalid(format!(
                    "fill on order {order_id} carries no quote unit; its price is a number in \
                     no currency, and the ledger will not assume one"
                )));
            }
        };
        if quote_unit == *object {
            return Err(Error::invalid(format!(
                "fill on order {order_id} quotes {object} in itself; the two legs would \
                 offset in one unit and book nothing"
            )));
        }

        let quantity = positive("quantity", quantity)?;
        let price = positive("price", price)?;
        let fee = match fee {
            None => FeeReport::Unreported,
            Some(text) => {
                let amount = exact("fee", text)?;
                if amount.is_negative() {
                    return Err(Error::invalid(format!(
                        "fill on order {order_id} reports a negative fee {amount}; a rebate \
                         needs its own posting rule, not a negative fee"
                    )));
                }
                FeeReport::Reported { amount }
            }
        };

        if shares.is_empty() {
            return Err(Error::invalid(format!(
                "fill on order {order_id} is attributed to no strategy; a position nobody \
                 owns cannot be booked"
            )));
        }
        let mut split = BTreeMap::new();
        let mut attributed = Decimal::ZERO;
        for (strategy, share) in shares {
            let share = positive("share", share)?;
            attributed = attributed.checked_add(share).ok_or_else(|| {
                Error::numeric(format!(
                    "the shares of order {order_id} overflow when summed"
                ))
            })?;
            if split.insert(strategy.clone(), share).is_some() {
                return Err(Error::invalid(format!(
                    "fill on order {order_id} names strategy `{strategy}` twice; the ledger \
                     will not decide which share is the real one"
                )));
            }
        }
        if attributed != quantity {
            return Err(Error::invalid(format!(
                "the shares of order {order_id} sum to {attributed} but the fill is {quantity}; \
                 the ledger refuses the fill rather than re-splitting it"
            )));
        }

        Ok(Self {
            source: SourceEvent {
                cell: outcome.cell.clone(),
                session: outcome.session,
                journal_sequence: outcome.journal_sequence,
                journal_digest: outcome.journal_digest.clone(),
                order_id: order_id.clone(),
            },
            venue: venue.clone(),
            object: object.clone(),
            side,
            quote_unit,
            quantity,
            price,
            fee,
            shares: split,
        })
    }
}

impl PaperFill {
    pub fn source(&self) -> &SourceEvent {
        &self.source
    }

    pub fn side(&self) -> BookSide {
        self.side
    }

    pub fn quantity(&self) -> Decimal {
        self.quantity
    }

    pub fn price(&self) -> Decimal {
        self.price
    }

    pub fn fee(&self) -> FeeReport {
        self.fee
    }

    pub fn shares(&self) -> &BTreeMap<String, Decimal> {
        &self.shares
    }
}

fn leg(account: &Account, direction: Direction, unit: &str, amount: Decimal) -> Posting {
    Posting {
        account: account.clone(),
        direction,
        unit: unit.to_string(),
        amount,
    }
}

/// Book one paper fill as one balanced [`LedgerEvent`].
///
/// Pure: the same `PaperFill` yields the same event on every call, in every
/// process. Refuses — never rounds — a notional or a fee part that does not
/// land exactly on the nine-digit grid, and [`LedgerEvent::new`] refuses any
/// set that does not balance per unit, so an error here is always a named
/// refusal and never a plugged difference.
pub fn post(fill: &PaperFill) -> Result<LedgerEvent> {
    let venue = Account::Venue {
        venue: fill.venue.clone(),
    };
    let fees = Account::Fees {
        venue: fill.venue.clone(),
    };
    let object = fill.object.as_str();
    let unit = fill.quote_unit.as_str();
    let mut postings = Vec::with_capacity(fill.shares.len() * 6);
    for (strategy, &share) in &fill.shares {
        let trading = Account::Trading {
            cell: fill.source.cell.clone(),
            strategy: strategy.clone(),
        };
        let cash = exact_product(share, fill.price)?;
        match fill.side {
            BookSide::Ask => {
                postings.push(leg(&trading, Direction::Debit, object, share));
                postings.push(leg(&venue, Direction::Credit, object, share));
                postings.push(leg(&venue, Direction::Debit, unit, cash));
                postings.push(leg(&trading, Direction::Credit, unit, cash));
            }
            BookSide::Bid => {
                postings.push(leg(&venue, Direction::Debit, object, share));
                postings.push(leg(&trading, Direction::Credit, object, share));
                postings.push(leg(&trading, Direction::Debit, unit, cash));
                postings.push(leg(&venue, Direction::Credit, unit, cash));
            }
        }
        if let FeeReport::Reported { amount } = fill.fee
            && amount.is_positive()
        {
            let part = exact_part(amount, share, fill.quantity)?;
            postings.push(leg(&fees, Direction::Debit, unit, part));
            postings.push(leg(&trading, Direction::Credit, unit, part));
        }
    }
    LedgerEvent::new(
        fill.source.clone(),
        Settlement::Simulated,
        fill.fee,
        postings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::message::BookSide;
    use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry};
    use qip_core::Timestamp;

    fn make_outcome_record(simulated: bool, quantity: &str, price: &str) -> OutcomeRecord {
        let entry = JournalEntry {
            sequence: 1,
            at: Timestamp::from_secs(1_000_000),
            decision: Decision::Filled {
                order_id: "test-order".to_string(),
                venue: "test-venue".to_string(),
                object: "TEST".to_string(),
                quantity: quantity.to_string(),
                price: price.to_string(),
                simulated,
                shares: vec![("strategy-1".to_string(), quantity.to_string())],
                side: Some(BookSide::Ask),
                quote_unit: Some("USD".to_string()),
                fee: None,
            },
            digest: "test-digest".to_string(),
            version: ChainVersion::V2,
        };
        OutcomeRecord {
            cell: "test-cell".to_string(),
            session: 1,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry,
        }
    }

    #[test]
    fn a_simulated_paper_fill_can_be_posted_as_one_balanced_event() {
        // Premise: a valid outcome record with all required fields is provided.
        let outcome = make_outcome_record(true, "100.5", "50.25");

        // PaperFill::try_from should succeed for a valid simulated fill.
        let fill =
            PaperFill::try_from(&outcome).expect("premise: a valid simulated fill is accepted");

        // post() should succeed and return a balanced ledger event.
        let event = post(&fill).expect("a valid fill posts");

        // Verify the event is marked as simulated settlement.
        assert_eq!(event.settlement(), Settlement::Simulated);

        // Verify postings are present and balanced in the quote unit.
        let postings = event.postings();
        assert!(!postings.is_empty(), "an event has postings");

        let mut debits = Decimal::ZERO;
        let mut credits = Decimal::ZERO;
        for p in postings {
            if p.unit == "USD" {
                match p.direction {
                    Direction::Debit => debits += p.amount,
                    Direction::Credit => credits += p.amount,
                }
            }
        }
        assert!(debits.is_positive(), "debits exist in quote unit");
        assert_eq!(debits, credits, "debits equal credits in quote unit");
    }

    #[test]
    fn a_live_fill_is_refused_and_not_posted() {
        // Premise: the same fill marked simulated is accepted.
        let outcome_simulated = make_outcome_record(true, "50", "100");
        let fill_simulated =
            PaperFill::try_from(&outcome_simulated).expect("premise: a simulated fill is accepted");
        assert!(
            post(&fill_simulated).is_ok(),
            "premise: simulated fill posts"
        );

        // A live fill (simulated=false) should be refused at construction.
        let outcome_live = make_outcome_record(false, "50", "100");
        let result = PaperFill::try_from(&outcome_live);

        assert!(result.is_err(), "live fill is refused");
        if let Err(e) = result {
            assert!(
                e.to_string().contains("not marked simulated"),
                "refusal names the paper-trading boundary: {}",
                e
            );
        }
    }

    #[test]
    fn a_fill_with_no_side_is_refused() {
        // Premise: a fill with a side is accepted.
        let outcome_with_side = make_outcome_record(true, "50", "100");
        assert!(
            PaperFill::try_from(&outcome_with_side).is_ok(),
            "premise: a fill with side is accepted"
        );

        // A fill with no side should be refused.
        let entry = JournalEntry {
            sequence: 1,
            at: Timestamp::from_secs(1_000_000),
            decision: Decision::Filled {
                order_id: "test".to_string(),
                venue: "test".to_string(),
                object: "TEST".to_string(),
                quantity: "50".to_string(),
                price: "100".to_string(),
                simulated: true,
                shares: vec![("s1".to_string(), "50".to_string())],
                side: None,
                quote_unit: Some("USD".to_string()),
                fee: None,
            },
            digest: "d".to_string(),
            version: ChainVersion::V2,
        };
        let outcome_no_side = OutcomeRecord {
            cell: "c".to_string(),
            session: 1,
            journal_sequence: 1,
            journal_digest: "d".to_string(),
            entry,
        };

        let result = PaperFill::try_from(&outcome_no_side);
        assert!(result.is_err(), "fill with no side is refused");
    }

    #[test]
    fn a_paper_fill_posting_is_pure_and_deterministic() {
        // Premise: a valid fill is created.
        let outcome = make_outcome_record(true, "10.5", "25.5");
        let fill = PaperFill::try_from(&outcome).expect("premise: fill is created");

        // post() should produce identical results on replay.
        let event1 = post(&fill).expect("first post succeeds");
        let event2 = post(&fill).expect("replay succeeds");

        // Both events should be equal (same postings in same order).
        assert_eq!(
            event1.postings(),
            event2.postings(),
            "replaying the same fill produces identical postings"
        );
    }

    #[test]
    fn a_figure_that_would_need_rounding_is_refused_rather_than_rounded() {
        // `Decimal::parse` would round the tenth digit and `checked_mul`
        // would round the notional; either would book a figure no venue
        // reported. Premise first: a figure on the grid is accepted as is.
        assert_eq!(
            exact("price", "1.000000001").map(|d| d.raw()),
            Ok(1_000_000_001),
            "premise: nine fractional digits are exact"
        );
        assert!(
            exact("price", "1.0000000005").is_err(),
            "a tenth fractional digit was rounded instead of refused"
        );
        assert!(
            exact("price", " 1.5").is_err(),
            "surrounding whitespace was trimmed instead of refused"
        );
        let on_grid = exact_product(
            Decimal::parse("0.5").expect("literal"),
            Decimal::parse("0.000000002").expect("literal"),
        );
        assert_eq!(
            on_grid.map(|d| d.raw()),
            Ok(1),
            "premise: a product on the grid is exact"
        );
        assert!(
            exact_product(
                Decimal::parse("0.5").expect("literal"),
                Decimal::parse("0.000000001").expect("literal"),
            )
            .is_err(),
            "a notional of half a nano-unit was rounded instead of refused"
        );
    }
}
