//! The ledger is the book of record; the desk's book is a working copy checked
//! against it (R12-LEDGER-5004, ADR 0653).
//!
//! Two books for one fact will disagree, and the louder one will be wrong, so
//! one of them is named the record: `qip-ledgerd`'s balances. The kernel's desk
//! book is what the desk trades against between ships, and this module is the
//! check run after each acknowledged ship. The rules are the skeptic's from the
//! owner's 2026-10-31 ruling:
//!
//! * the comparison runs only when the ledger's applied desk-chain position
//!   equals the acknowledged ship, because a ledger still behind the ship has
//!   not booked fills the working copy already holds;
//! * a trailing ledger **refuses new desk orders as lag** and lifts when it
//!   catches up, recording no break, because lag is not disagreement;
//! * a difference at an equal position **is** a break: new desk orders are
//!   refused until a person records a correction;
//! * neither side is ever overwritten to make them agree. A correction is a
//!   record that someone found which side was wrong and fixed it where the
//!   fix belongs.
//!
//! Pure: it reads two statements and holds a gate. Reading the ledger over its
//! API and calling [`AuthorityGate::permit_new_orders`] from the desk's order
//! path is the wiring this slice does not do (ADR 0653, "Still open").

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// What the ledger says the desk holds, and how far along the desk's chain it
/// has applied when it says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerStatement {
    /// The last desk-chain sequence the ledger has applied.
    pub applied_through: u64,
    /// Signed balance per unit; a unit absent is zero.
    pub balances: BTreeMap<String, Decimal>,
}

/// What the kernel's desk book says, as of the last acknowledged ship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingCopy {
    /// The last desk-chain sequence the fabric acknowledged.
    pub acknowledged_through: u64,
    pub balances: BTreeMap<String, Decimal>,
}

/// One unit the two books disagree on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub unit: String,
    pub ledger: Decimal,
    pub working: Decimal,
}

/// The result of one comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The ledger has not applied the acknowledged ship yet.
    Lagging { ledger: u64, acknowledged: u64 },
    /// The ledger has applied past the acknowledged ship, so the two are not
    /// at one position and nothing can be said. Not a break and not a refusal.
    Unaligned { ledger: u64, acknowledged: u64 },
    /// Equal positions, equal balances.
    Agrees { through: u64 },
    /// Equal positions, a difference.
    Disagrees {
        through: u64,
        differences: Vec<Difference>,
    },
}

/// Compare the working copy with the ledger, once they are at one position.
pub fn compare(working: &WorkingCopy, ledger: &LedgerStatement) -> Verdict {
    if ledger.applied_through < working.acknowledged_through {
        return Verdict::Lagging {
            ledger: ledger.applied_through,
            acknowledged: working.acknowledged_through,
        };
    }
    if ledger.applied_through > working.acknowledged_through {
        return Verdict::Unaligned {
            ledger: ledger.applied_through,
            acknowledged: working.acknowledged_through,
        };
    }
    let units: std::collections::BTreeSet<&String> = ledger
        .balances
        .keys()
        .chain(working.balances.keys())
        .collect();
    let differences: Vec<Difference> = units
        .into_iter()
        .filter_map(|unit| {
            let l = ledger.balances.get(unit).copied().unwrap_or(Decimal::ZERO);
            let w = working.balances.get(unit).copied().unwrap_or(Decimal::ZERO);
            (l != w).then(|| Difference {
                unit: unit.clone(),
                ledger: l,
                working: w,
            })
        })
        .collect();
    if differences.is_empty() {
        Verdict::Agrees {
            through: ledger.applied_through,
        }
    } else {
        Verdict::Disagrees {
            through: ledger.applied_through,
            differences,
        }
    }
}

/// A person's record that a break was resolved, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    pub operator: String,
    /// What was found and which side was fixed.
    pub note: String,
    /// The position of the break this resolves.
    pub through: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Open,
    Lagging { ledger: u64, acknowledged: u64 },
    Broken { through: u64, units: Vec<String> },
}

/// Whether the desk may take a new order.
#[derive(Clone, Debug)]
pub struct AuthorityGate {
    state: State,
    corrections: Vec<Correction>,
    breaks_seen: u64,
}

impl Default for AuthorityGate {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthorityGate {
    pub fn new() -> Self {
        Self {
            state: State::Open,
            corrections: Vec::new(),
            breaks_seen: 0,
        }
    }

    /// Take a comparison's verdict. A break holds until a correction; lag lifts
    /// by itself on the next agreeing verdict; a verdict that agrees does **not**
    /// lift a break, because an agreement nobody explained is the books having
    /// been moved to match, which is the overwrite this design refuses.
    pub fn observe(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Lagging {
                ledger,
                acknowledged,
            } => {
                if !matches!(self.state, State::Broken { .. }) {
                    self.state = State::Lagging {
                        ledger: *ledger,
                        acknowledged: *acknowledged,
                    };
                }
            }
            Verdict::Unaligned { .. } => {}
            Verdict::Agrees { .. } => {
                if matches!(self.state, State::Lagging { .. }) {
                    self.state = State::Open;
                }
            }
            Verdict::Disagrees {
                through,
                differences,
            } => {
                if !matches!(self.state, State::Broken { .. }) {
                    self.breaks_seen = self.breaks_seen.saturating_add(1);
                }
                self.state = State::Broken {
                    through: *through,
                    units: differences.iter().map(|d| d.unit.clone()).collect(),
                };
            }
        }
    }

    /// Whether a new desk order may be taken. A refusal says why and what lifts
    /// it.
    pub fn permit_new_orders(&self) -> Result<()> {
        match &self.state {
            State::Open => Ok(()),
            State::Lagging {
                ledger,
                acknowledged,
            } => Err(Error::denied(format!(
                "the ledger has applied the desk chain through {ledger} and the desk's \
                 acknowledged ship is {acknowledged}; new desk orders wait for the ledger to \
                 catch up, which lifts this without a correction"
            ))),
            State::Broken { through, units } => Err(Error::denied(format!(
                "the desk's book disagrees with the ledger at position {through} in {}; new \
                 desk orders are refused until a person records a correction",
                units.join(", ")
            ))),
        }
    }

    /// Record the correction that lifts a break. Refused unless a break is
    /// open, the correction names it by position, and names who and what.
    pub fn record_correction(&mut self, correction: Correction) -> Result<()> {
        let State::Broken { through, .. } = &self.state else {
            return Err(Error::invalid(
                "there is no open break to correct; a correction is not recorded in advance",
            ));
        };
        if correction.operator.trim().is_empty() || correction.note.trim().is_empty() {
            return Err(Error::invalid(
                "a correction names the operator and says what was found and which side was fixed",
            ));
        }
        if correction.through != *through {
            return Err(Error::invalid(format!(
                "the open break is at position {through}; a correction for {} resolves a \
                 different one",
                correction.through
            )));
        }
        self.corrections.push(correction);
        self.state = State::Open;
        Ok(())
    }

    pub fn corrections(&self) -> &[Correction] {
        &self.corrections
    }

    /// How many distinct breaks the gate has taken.
    pub const fn breaks_seen(&self) -> u64 {
        self.breaks_seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(through: u64, units: &[(&str, i64)]) -> (u64, BTreeMap<String, Decimal>) {
        (
            through,
            units
                .iter()
                .map(|(u, q)| ((*u).to_string(), Decimal::from_int(*q)))
                .collect(),
        )
    }

    fn pair(
        ack: u64,
        working: &[(&str, i64)],
        applied: u64,
        ledger: &[(&str, i64)],
    ) -> (WorkingCopy, LedgerStatement) {
        let (_, w) = book(ack, working);
        let (_, l) = book(applied, ledger);
        (
            WorkingCopy {
                acknowledged_through: ack,
                balances: w,
            },
            LedgerStatement {
                applied_through: applied,
                balances: l,
            },
        )
    }

    #[test]
    fn a_trailing_ledger_is_lag_not_a_break_and_refuses_until_it_catches_up() {
        // The failure this prevents: reading a ledger that has not yet booked a
        // shipped fill as a disagreement, which would halt the desk on every ship.
        let mut gate = AuthorityGate::new();
        let (w, l) = pair(10, &[("X", 5)], 8, &[("X", 3)]);
        let verdict = compare(&w, &l);
        assert!(matches!(verdict, Verdict::Lagging { .. }), "{verdict:?}");
        gate.observe(&verdict);
        assert!(gate.permit_new_orders().is_err());
        assert_eq!(gate.breaks_seen(), 0, "lag records no break");
        let (w, l) = pair(10, &[("X", 5)], 10, &[("X", 5)]);
        gate.observe(&compare(&w, &l));
        assert!(gate.permit_new_orders().is_ok(), "lag lifts on catch-up");
    }

    #[test]
    fn a_difference_at_an_equal_position_refuses_until_a_correction_names_it() {
        let mut gate = AuthorityGate::new();
        let (w, l) = pair(10, &[("X", 5)], 10, &[("X", 4)]);
        let verdict = compare(&w, &l);
        assert_eq!(
            verdict,
            Verdict::Disagrees {
                through: 10,
                differences: vec![Difference {
                    unit: "X".to_string(),
                    ledger: Decimal::from_int(4),
                    working: Decimal::from_int(5),
                }],
            }
        );
        gate.observe(&verdict);
        assert!(gate.permit_new_orders().is_err());
        assert_eq!(gate.breaks_seen(), 1);

        // Agreement alone does not lift it: nobody said which side was wrong.
        let (w, l) = pair(11, &[("X", 4)], 11, &[("X", 4)]);
        gate.observe(&compare(&w, &l));
        assert!(gate.permit_new_orders().is_err());

        let wrong_position = Correction {
            operator: "op-1".to_string(),
            note: "desk book was double-counting a fill; fixed there".to_string(),
            through: 9,
        };
        assert!(gate.record_correction(wrong_position).is_err());
        let blank = Correction {
            operator: " ".to_string(),
            note: String::new(),
            through: 10,
        };
        assert!(gate.record_correction(blank).is_err());
        gate.record_correction(Correction {
            operator: "op-1".to_string(),
            note: "desk book was double-counting a fill; fixed there".to_string(),
            through: 10,
        })
        .unwrap();
        assert!(gate.permit_new_orders().is_ok());
        assert_eq!(gate.corrections().len(), 1);
    }

    #[test]
    fn a_unit_absent_on_one_side_reads_as_zero_and_equal_books_agree() {
        let (w, l) = pair(3, &[("X", 5), ("Y", 0)], 3, &[("X", 5)]);
        assert_eq!(compare(&w, &l), Verdict::Agrees { through: 3 });
        let (w, l) = pair(3, &[("X", 5)], 3, &[("X", 5), ("Y", 2)]);
        assert!(matches!(compare(&w, &l), Verdict::Disagrees { .. }));
    }

    #[test]
    fn a_ledger_ahead_of_the_ship_is_not_comparable_and_changes_nothing() {
        let mut gate = AuthorityGate::new();
        let (w, l) = pair(5, &[("X", 1)], 7, &[("X", 2)]);
        let verdict = compare(&w, &l);
        assert!(matches!(verdict, Verdict::Unaligned { .. }));
        gate.observe(&verdict);
        assert!(gate.permit_new_orders().is_ok());
        assert_eq!(gate.breaks_seen(), 0);
    }

    #[test]
    fn a_correction_with_no_open_break_is_refused() {
        let mut gate = AuthorityGate::new();
        assert!(
            gate.record_correction(Correction {
                operator: "op".to_string(),
                note: "n".to_string(),
                through: 1,
            })
            .is_err()
        );
    }
}
