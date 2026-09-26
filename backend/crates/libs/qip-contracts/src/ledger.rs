//! The ledger posting contract: one economic event, its double-entry
//! postings, and the settlement state it was booked under (ADR 0100 §8, §9).
//!
//! The posting *logic* lives in `qip-portfolio::ledger`, pure; the ledger
//! *writer* is `qip-ledgerd`. What lives here is the shape both of them and
//! every reader agree on, and the three invariants that shape refuses to
//! represent a violation of:
//!
//! * **Every event balances in every unit** (LEDGER-002, LEDGER-006). Debits
//!   equal credits per unit, exactly, in [`Decimal`]. An unbalanced set is
//!   refused by [`LedgerEvent::new`] — never plugged with a difference — and
//!   deserialisation goes through the same constructor, so an event read off
//!   the wire meets the same refusal as one built in code.
//! * **Every event is simulated** (ADR 0100 §9, the fourth paper fence).
//!   [`Settlement`] has exactly one variant. There is no field on which a
//!   live settlement could be written, so no ledger event can claim one.
//! * **Every event cites the observed fact that caused it** (LEDGER-019,
//!   LEDGER-021). [`SourceEvent`] names the cell, the session, the journal
//!   entry and its digest, and the order; a posting with no source is a guess
//!   the ledger would have no way to reproduce.
//!
//! **There is no update and no delete** (LEDGER-017). Fields are private and
//! the accessors hand out shared references; a correction is a new event that
//! references the original, which is the writer's business, not this type's.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The settlement state a ledger event was booked under.
///
/// **One variant, on purpose.** The platform is paper-trading only, and a
/// ledger whose settlement type could say `Settled` or `Live` is a ledger
/// that could book a live fill if anything upstream let one through. Adding a
/// variant here is widening the paper boundary, and the test
/// `settlement_has_no_variant_but_simulated` refuses it by the wire text a
/// second variant would admit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    /// Filled by the simulated venue. Nothing moved anywhere but here.
    Simulated,
}

/// Which side of an account a posting lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Debit,
    Credit,
}

/// An account in the ledger, by the subledger it belongs to.
///
/// Three kinds and no more: the trading subledger holds positions and cash
/// per cell and strategy (LEDGER-007), the venue account is the counterparty
/// a fill's two legs are exchanged with, and the fees subledger holds what a
/// venue charged (LEDGER-011). A structured enum rather than a string so a
/// reader cannot mistake one kind for another by a prefix; [`fmt::Display`]
/// renders the `kind:name` text form, which is unambiguous because
/// [`LedgerEvent::new`] refuses a `:` or `/` inside any name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Account {
    /// `trading:<cell>/<strategy>` — who owns the position and the cash.
    Trading { cell: String, strategy: String },
    /// `venue:<venue>` — the other side of every exchange of units.
    Venue { venue: String },
    /// `fees:<venue>` — what the venue reported charging, and nothing else.
    Fees { venue: String },
}

impl Account {
    fn names(&self) -> Vec<&str> {
        match self {
            Self::Trading { cell, strategy } => vec![cell, strategy],
            Self::Venue { venue } | Self::Fees { venue } => vec![venue],
        }
    }
}

impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trading { cell, strategy } => write!(f, "trading:{cell}/{strategy}"),
            Self::Venue { venue } => write!(f, "venue:{venue}"),
            Self::Fees { venue } => write!(f, "fees:{venue}"),
        }
    }
}

/// One leg of a ledger event: an amount of one unit, on one side of one
/// account. `amount` is strictly positive; the direction carries the sign,
/// so a negative amount cannot silently flip a debit into a credit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    pub account: Account,
    pub direction: Direction,
    /// The asset or currency the posting moves (LEDGER-002): the fill's
    /// object for a position leg, its quote unit for a cash or fee leg.
    pub unit: String,
    pub amount: Decimal,
}

/// What the venue said about the fee on the fill this event books.
///
/// Two states and no default. **`Unreported` is not zero** (LEDGER-019): a
/// venue that said nothing about a fee has not said it was free, and a
/// ledger that booked it as zero has invented a venue fact that no later
/// statement can be reconciled against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeReport {
    /// The venue reported this fee, in the fill's quote unit.
    Reported { amount: Decimal },
    /// The venue reported no fee. No fee posting exists, and none is implied.
    Unreported,
}

/// The observed fact a ledger event was booked from (LEDGER-019, LEDGER-021).
///
/// The journal entry's sequence and digest place the fill on the chain it
/// came from; `order_id` is the next link back toward the intent. A cell and
/// a session with no entry would cite nothing a replay could find.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEvent {
    pub cell: String,
    /// The persisted, monotonic session counter (ADR 0100 §4).
    pub session: u64,
    pub journal_sequence: u64,
    pub journal_digest: String,
    pub order_id: String,
}

/// One immutable, balanced, simulated economic event (CONTRACT-016).
///
/// The only constructor is [`LedgerEvent::new`], and deserialisation is
/// routed through it. Nothing mutates an event once built.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LedgerEventWire")]
pub struct LedgerEvent {
    source: SourceEvent,
    settlement: Settlement,
    fee: FeeReport,
    postings: Vec<Posting>,
}

/// The deserialised shape, so an event read from a store or a stream meets
/// the refusals [`LedgerEvent::new`] applies to one built in code. Without
/// it the balance invariant would hold for every event this process wrote
/// and for none it read.
#[derive(Deserialize)]
struct LedgerEventWire {
    source: SourceEvent,
    settlement: Settlement,
    fee: FeeReport,
    postings: Vec<Posting>,
}

impl TryFrom<LedgerEventWire> for LedgerEvent {
    type Error = Error;

    fn try_from(wire: LedgerEventWire) -> Result<Self> {
        Self::new(wire.source, wire.settlement, wire.fee, wire.postings)
    }
}

fn refuse_unnamed(what: &str, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::invalid(format!(
            "a ledger event needs a non-empty {what}; an unattributed posting cannot be traced \
             to the fact that caused it"
        )));
    }
    Ok(())
}

impl LedgerEvent {
    /// Build an event, refusing anything the ledger must not hold.
    ///
    /// Refused, never adjusted:
    ///
    /// * fewer than two postings, or a posting of zero or negative amount —
    ///   a one-legged or empty entry is not double-entry, and a signed amount
    ///   would let a sign error flip a side silently;
    /// * any unit whose debits and credits differ by any amount at all, or
    ///   whose sums overflow — the difference is reported, never posted to a
    ///   suspense account to make the set close;
    /// * postings to a fees account that do not equal the reported fee, or
    ///   any fees posting when the fee is unreported — a fee the venue did
    ///   not report is not a fee the ledger may book;
    /// * an empty name, unit or source field, or a `:` or `/` inside an
    ///   account name, which would make two different accounts render to the
    ///   same text.
    pub fn new(
        source: SourceEvent,
        settlement: Settlement,
        fee: FeeReport,
        postings: Vec<Posting>,
    ) -> Result<Self> {
        refuse_unnamed("source cell", &source.cell)?;
        refuse_unnamed("source journal digest", &source.journal_digest)?;
        refuse_unnamed("source order id", &source.order_id)?;
        if postings.len() < 2 {
            return Err(Error::invalid(format!(
                "a ledger event needs at least two postings to be double-entry; got {}",
                postings.len()
            )));
        }

        // Per unit: (debits, credits). BTreeMap so the first imbalance named
        // is the same on every run, and a replayed refusal reads identically.
        let mut sums: BTreeMap<&str, (Decimal, Decimal)> = BTreeMap::new();
        let mut fees_debited = Decimal::ZERO;
        let mut fees_credited = Decimal::ZERO;
        let mut touches_fees = false;
        for posting in &postings {
            refuse_unnamed("posting unit", &posting.unit)?;
            for name in posting.account.names() {
                refuse_unnamed("account name", name)?;
                if name.contains(':') || name.contains('/') {
                    return Err(Error::invalid(format!(
                        "account name `{name}` contains `:` or `/`, which would make `{}` \
                         ambiguous as text; rename the cell, strategy or venue",
                        posting.account
                    )));
                }
            }
            if !posting.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "posting to {} of {} {} is not positive; the direction carries the \
                     sign, and a zero leg records nothing that happened",
                    posting.account, posting.amount, posting.unit
                )));
            }
            let overflow = || {
                Error::numeric(format!(
                    "the {} postings overflow when summed; the event is refused rather \
                     than booked with a wrapped total",
                    posting.unit
                ))
            };
            let entry = sums
                .entry(posting.unit.as_str())
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            match posting.direction {
                Direction::Debit => {
                    entry.0 = entry.0.checked_add(posting.amount).ok_or_else(overflow)?
                }
                Direction::Credit => {
                    entry.1 = entry.1.checked_add(posting.amount).ok_or_else(overflow)?
                }
            }
            if let Account::Fees { .. } = posting.account {
                touches_fees = true;
                match posting.direction {
                    Direction::Debit => {
                        fees_debited = fees_debited
                            .checked_add(posting.amount)
                            .ok_or_else(overflow)?;
                    }
                    Direction::Credit => {
                        fees_credited = fees_credited
                            .checked_add(posting.amount)
                            .ok_or_else(overflow)?;
                    }
                }
            }
        }
        for (unit, (debits, credits)) in &sums {
            if debits != credits {
                return Err(Error::invalid(format!(
                    "unbalanced in {unit}: debits {debits}, credits {credits}; the ledger \
                     refuses the set rather than plugging the difference"
                )));
            }
        }

        match fee {
            FeeReport::Unreported if touches_fees => {
                return Err(Error::invalid(
                    "the fee is unreported but the event posts to a fees account; a fee the \
                     venue did not report is not a fee the ledger may book",
                ));
            }
            FeeReport::Unreported => {}
            FeeReport::Reported { amount } => {
                if amount.is_negative() {
                    return Err(Error::invalid(format!(
                        "reported fee {amount} is negative; a rebate needs its own posting \
                         rule, not a negative fee"
                    )));
                }
                let net = fees_debited
                    .checked_sub(fees_credited)
                    .ok_or_else(|| Error::numeric("the fees postings overflow when netted"))?;
                if net != amount {
                    return Err(Error::invalid(format!(
                        "the fees postings net to {net} but the venue reported {amount}; \
                         the ledger books the reported fee, not another figure"
                    )));
                }
            }
        }

        Ok(Self {
            source,
            settlement,
            fee,
            postings,
        })
    }

    pub fn source(&self) -> &SourceEvent {
        &self.source
    }

    pub fn settlement(&self) -> Settlement {
        self.settlement
    }

    pub fn fee(&self) -> FeeReport {
        self.fee
    }

    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceEvent {
        SourceEvent {
            cell: "cell-a".to_string(),
            session: 1,
            journal_sequence: 7,
            journal_digest: "d".to_string(),
            order_id: "o-1".to_string(),
        }
    }

    fn leg(account: Account, direction: Direction, unit: &str, amount: &str) -> Posting {
        Posting {
            account,
            direction,
            unit: unit.to_string(),
            amount: Decimal::parse(amount).expect("test literal"),
        }
    }

    fn trading() -> Account {
        Account::Trading {
            cell: "cell-a".to_string(),
            strategy: "alpha".to_string(),
        }
    }

    fn venue() -> Account {
        Account::Venue {
            venue: "sim".to_string(),
        }
    }

    #[test]
    fn a_set_unbalanced_by_one_nano_unit_is_refused_and_not_plugged() {
        // The failure this prevents: a ledger that closes a rounding
        // difference by posting it somewhere, so a real break later reads as
        // accumulated noise. Premise first: the balanced twin is accepted.
        let balanced = vec![
            leg(trading(), Direction::Debit, "USD", "10.5"),
            leg(venue(), Direction::Credit, "USD", "10.5"),
        ];
        assert!(
            LedgerEvent::new(
                source(),
                Settlement::Simulated,
                FeeReport::Unreported,
                balanced
            )
            .is_ok(),
            "premise: a balanced pair is accepted"
        );
        let off_by_one = vec![
            leg(trading(), Direction::Debit, "USD", "10.5"),
            leg(venue(), Direction::Credit, "USD", "10.500000001"),
        ];
        let refused = LedgerEvent::new(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            off_by_one,
        )
        .expect_err("an unbalanced set must be refused");
        assert!(
            refused.to_string().contains("unbalanced in USD"),
            "refused for the wrong reason: {refused}"
        );
    }

    #[test]
    fn an_event_read_off_the_wire_meets_the_same_balance_refusal() {
        // Deserialisation is routed through `new`; a derived `Deserialize`
        // would have admitted this bytes-for-bytes.
        let good = LedgerEvent::new(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            vec![
                leg(trading(), Direction::Debit, "USD", "3"),
                leg(venue(), Direction::Credit, "USD", "3"),
            ],
        )
        .expect("balanced");
        let text = serde_json::to_string(&good).expect("serialises");
        let back: LedgerEvent = serde_json::from_str(&text).expect("premise: round-trips");
        assert_eq!(back, good);
        let tampered = text.replacen("\"amount\":\"3\"", "\"amount\":\"4\"", 1);
        assert_ne!(tampered, text, "premise: the tamper changed the bytes");
        let refused = serde_json::from_str::<LedgerEvent>(&tampered)
            .expect_err("an unbalanced event on the wire must not deserialise");
        assert!(
            refused.to_string().contains("unbalanced in USD"),
            "refused for the wrong reason: {refused}"
        );
    }
}
