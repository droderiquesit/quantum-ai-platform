//! Custody truth against the ledger of record (ADR 0130 decision 4, ADR
//! 0442, R12-LEDGER-8139).
//!
//! The cell reconciles the venue's drop copy against its own order state and
//! halts on a break; nothing compared what the venue says it holds for a
//! cell with what `qip-ledgerd` has booked for it. A fill the ledger booked
//! twice, a fill the venue booked and the ledger never saw, or a refused
//! posting released by hand onto the wrong book would each leave the ledger
//! and the custodian disagreeing while the cell kept trading on top of the
//! difference — and an unreconciled break compounds with every fill.
//!
//! This module is the pure half: the statement the simulated venue sends,
//! the comparison, and the deterministic policy that turns a break into a
//! halt. Pure so the same statement against the same balances yields the
//! same breaks and the same unsigned [`HaltCommand`] in every process, which
//! is what lets a replay say why a cell stopped.
//!
//! # What is compared, and what is not
//!
//! * **Units, not cash.** The statement names the instruments the venue
//!   holds; every unit the statement lists in [`CustodyStatement::cash_units`]
//!   is left out on both sides. Cash custody is the venue's cash balance and
//!   is not compared here — that is a named gap, not a zero.
//! * **At the statement's own chain position.** A statement says what the
//!   venue held when the cell's journal stood at `(session, sequence)`. The
//!   ledger side is the cell's fills at or before that position and nothing
//!   after it, so a fill in flight on the P1 stream is never read as a
//!   break, and a fill booked after the statement is never read as one
//!   either.
//! * **Exactly.** A difference of one unit in the ninth place is a break.
//!   There is no tolerance: the venue and the ledger both count in
//!   [`Decimal`], and a tolerance would be a limit that cannot fire on the
//!   small, repeated error that is the commonest real break.

use crate::policy::HaltCommand;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One custody statement as the simulated venue sent it: what it holds for
/// one cell, as of the cell's chain position at the instant it was taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyStatement {
    pub cell: String,
    /// The custodian. On a paper platform the venue is also the custodian
    /// (ADR 0130 decision 4), and the statement says which venue.
    pub venue: String,
    /// The cell's journal session when the statement was taken.
    pub session: u64,
    /// The last journal sequence the cell had shipped when the statement
    /// was taken. Fills at or before it are the ledger side.
    pub sequence: u64,
    pub as_of: Timestamp,
    /// The paper fence (ADR 0130 decision 6): a statement not marked
    /// simulated is refused, exactly as a live fill is.
    pub simulated: bool,
    /// Signed units held per instrument; negative is short. A unit absent
    /// here is a statement that the venue holds none of it.
    pub holdings: BTreeMap<String, Decimal>,
    /// Units this statement does not state custody of — the venue's cash
    /// currencies. Left out on both sides of the comparison.
    pub cash_units: BTreeSet<String>,
}

impl CustodyStatement {
    /// Refuses a blank cell or venue: a statement nobody can attribute
    /// cannot be compared with anybody's books.
    pub fn new(
        cell: impl Into<String>,
        venue: impl Into<String>,
        session: u64,
        sequence: u64,
        as_of: Timestamp,
        simulated: bool,
    ) -> Result<Self> {
        let cell = cell.into();
        let venue = venue.into();
        if cell.trim().is_empty() || venue.trim().is_empty() {
            return Err(Error::invalid(
                "a custody statement must name the cell and the venue it is for; an \
                 unattributed statement reconciles against nobody's books",
            ));
        }
        Ok(Self {
            cell,
            venue,
            session,
            sequence,
            as_of,
            simulated,
            holdings: BTreeMap::new(),
            cash_units: BTreeSet::new(),
        })
    }

    /// State the units held of one instrument. Refuses a unit the statement
    /// already names as cash: one unit cannot be both compared and excluded.
    pub fn holding(mut self, unit: impl Into<String>, quantity: Decimal) -> Result<Self> {
        let unit = unit.into();
        if self.cash_units.contains(&unit) {
            return Err(Error::invalid(format!(
                "{unit} is already named as a cash unit of this statement; state it as a \
                 holding or as cash, not both"
            )));
        }
        self.holdings.insert(unit, quantity);
        Ok(self)
    }

    /// Name a cash unit to leave out of the comparison on both sides.
    pub fn cash(mut self, unit: impl Into<String>) -> Result<Self> {
        let unit = unit.into();
        if self.holdings.contains_key(&unit) {
            return Err(Error::invalid(format!(
                "{unit} is already stated as a holding of this statement; it cannot also be \
                 left out as cash"
            )));
        }
        self.cash_units.insert(unit);
        Ok(self)
    }

    /// Whether the statement's chain position is at or before `(session,
    /// sequence)`. Sessions order first: a later session's sequence restarts.
    pub fn is_at_or_before(&self, session: u64, sequence: u64) -> bool {
        (self.session, self.sequence) <= (session, sequence)
    }
}

/// Which side holds more. A bounded label: these two values and no other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyDirection {
    /// The ledger books more than the venue says it holds.
    LedgerOverVenue,
    /// The venue holds more than the ledger has booked.
    VenueOverLedger,
}

impl CustodyDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LedgerOverVenue => "ledger_over_venue",
            Self::VenueOverLedger => "venue_over_ledger",
        }
    }
}

/// One instrument on which the ledger and the venue disagree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyBreak {
    pub unit: String,
    pub ledger: Decimal,
    pub venue: Decimal,
    pub direction: CustodyDirection,
}

/// Compare the ledger's holdings with the statement's, per unit, over the
/// union of both sides less the statement's cash units. A unit absent on
/// one side reads as zero there, and a zero on both sides is no break.
///
/// Ordered by unit, so the same inputs give the same breaks in the same
/// order in every process.
pub fn compare(
    ledger: &BTreeMap<String, Decimal>,
    statement: &CustodyStatement,
) -> Vec<CustodyBreak> {
    let units: BTreeSet<&String> = ledger
        .keys()
        .chain(statement.holdings.keys())
        .filter(|unit| !statement.cash_units.contains(*unit))
        .collect();
    units
        .into_iter()
        .filter_map(|unit| {
            let booked = ledger.get(unit).copied().unwrap_or(Decimal::ZERO);
            let held = statement
                .holdings
                .get(unit)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let direction = if booked > held {
                CustodyDirection::LedgerOverVenue
            } else if held > booked {
                CustodyDirection::VenueOverLedger
            } else {
                return None;
            };
            Some(CustodyBreak {
                unit: unit.clone(),
                ledger: booked,
                venue: held,
                direction,
            })
        })
        .collect()
}

/// The deterministic policy (ADR 0442 decision 3): any break halts the whole
/// cell, unsigned, issued at the statement's own instant.
///
/// Whole-cell rather than narrowed: the break says the cell's position is
/// not what anybody thinks it is, and every gate that sizes against a
/// position — exposure, the drawdown stop, the region share — is sizing
/// against the wrong number. Narrowing would keep trading on it.
///
/// Issued at `as_of`, not at the instant somebody ran the comparison, so the
/// same statement always yields the same command, and a policy payload
/// issued before the statement — in flight while the break was found —
/// cannot release it (`HaltCommand::issued_at` is the release barrier).
/// Unsigned: signing is the centre's, under the halt key.
pub fn halt_for(statement: &CustodyStatement, breaks: &[CustodyBreak]) -> Option<HaltCommand> {
    if breaks.is_empty() {
        return None;
    }
    let detail = breaks
        .iter()
        .map(|b| {
            format!(
                "{} ledger {} venue {} ({})",
                b.unit,
                b.ledger,
                b.venue,
                b.direction.as_str()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(HaltCommand::new(
        statement.cell.clone(),
        statement.as_of,
        format!(
            "custody reconciliation break at {} for session {} sequence {}: {detail}",
            statement.venue, statement.session, statement.sequence
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).expect("test literal")
    }

    fn statement() -> CustodyStatement {
        CustodyStatement::new("cell-a", "sim", 1, 9, Timestamp::from_secs(100), true)
            .expect("named")
            .cash("USD")
            .expect("cash")
    }

    /// A ledger and a venue that agree produce no break and no halt; a unit
    /// the venue does not state, a unit the ledger has not booked, and a
    /// smaller holding each produce one break of the right direction, and
    /// the cash unit is never compared.
    ///
    /// Mutation: drop the cash filter in `compare` — the `USD` balance reads
    /// as a ledger-over-venue break and the agreeing assertion fails; swap
    /// the two direction arms — the direction assertions fail.
    #[test]
    fn every_unit_difference_is_a_break_of_its_own_direction_and_cash_is_never_compared() {
        let ledger: BTreeMap<String, Decimal> = [
            ("ACME".to_string(), d("10")),
            ("USD".to_string(), d("-1010")),
        ]
        .into();
        let agrees = statement().holding("ACME", d("10")).expect("holding");
        assert!(compare(&ledger, &agrees).is_empty(), "premise: they agree");
        assert!(halt_for(&agrees, &[]).is_none());

        let short = statement().holding("ACME", d("9.999999999")).expect("h");
        let found = compare(&ledger, &short);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].direction, CustodyDirection::LedgerOverVenue);

        let extra = statement()
            .holding("ACME", d("10"))
            .expect("h")
            .holding("BETA", d("3"))
            .expect("h");
        let found = compare(&ledger, &extra);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].unit, "BETA");
        assert_eq!(found[0].direction, CustodyDirection::VenueOverLedger);

        let halt = halt_for(&extra, &found).expect("a break halts");
        assert_eq!(halt.cell, "cell-a");
        assert_eq!(halt.issued_at, Timestamp::from_secs(100));
        assert!(halt.reason.contains("venue_over_ledger"), "{}", halt.reason);
    }

    #[test]
    fn a_unit_cannot_be_both_held_and_excluded_and_a_statement_must_be_attributed() {
        assert!(statement().holding("USD", d("1")).is_err());
        assert!(
            statement()
                .holding("ACME", d("1"))
                .expect("h")
                .cash("ACME")
                .is_err()
        );
        assert!(CustodyStatement::new(" ", "sim", 1, 1, Timestamp::from_secs(1), true).is_err());
    }
}
