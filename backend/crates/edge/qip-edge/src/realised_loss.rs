//! Realised profit and loss, booked from the position changes the cell itself
//! makes, so that a grant's drawdown limit can fire.
//!
//! [`qip_contracts::capital::CapitalEnvelope::admit`] refuses an order once
//! `Utilisation::realised_loss` reaches the grant's loss limit. Until this
//! module existed nothing in the cell wrote that field — the only writes to a
//! `Utilisation` were `gross_committed` and `orders_sent` — so the loss read
//! zero forever and every grant shipped a drawdown limit that could not fire.
//! That is the `MaxExpectedShortfall` class the risk rules prohibit: a control
//! that is checked on every order, reads as protection, and is not
//! (CAPITAL-026).
//!
//! # What is booked, and against what
//!
//! One average-cost position per owner, venue and instrument — keyed exactly
//! as the cell keys `strategy_positions`, so the working set is bounded by the
//! same thing that bounds that book — fed from both seams that move one: each
//! confirmed fill share at the fill price, and each leg of an internal cross
//! at the cross price. Both, because a cross moves a strategy's lot with no
//! venue fill at all, and a ledger fed from fills alone books the venue fill
//! that *closes* a crossed position as the trade that *opens* one: the loss is
//! understated and the limit fires late or never.
//!
//! P&L is realised only on the part of a leg that reduces its position, at the
//! difference between the leg's price and the position's average entry. A leg
//! that adds re-averages; a leg that crosses through flat realises the close
//! and opens the remainder at its own price — the arithmetic of the centre's
//! `StrategyLot::apply`. The loss an envelope reads is `max(0, −realised)` per
//! owner, the convention `central::realised` states, so the two planes'
//! figures are comparable. They are not identical: the centre also marks the
//! open lot at each trade, and the two agree exactly when the owner is flat.
//!
//! # What cannot be priced
//!
//! A leg is priced only in the unit its position was opened in, and a
//! realisation is added only to realised P&L held in the same unit. A leg in
//! any other unit — a stated unit against an unstated one included, since the
//! cell cannot tell whether those agree — is **unpriced**: its quantity still
//! moves the position, because the trade happened, and nothing is added to
//! realised P&L, because the only number the cell could add is one computed
//! across two units with no rate between them. The caller latches the owner on
//! it ([`RealisedLedger::latch`]). Booking nothing is not enough by itself:
//! `admit` reads only the number, so a loss nobody could price would read as
//! no loss and the next order would go out.
//!
//! Two units the placer never stated are treated as one. For one listing that
//! is the listing's own unit read twice. Across two listings it is an
//! assumption the cell cannot check — the one the centre's attribution also
//! makes — and a placer that states its units is what closes it.
//!
//! The owner rule has a consequence that is fail-closed and is not a design.
//! An owner that realises in two units is latched on its first realisation in
//! the second, and every triangle the arbitrage desk trades realises in at
//! least two — the production gateway states each listing's own currency, so
//! a USDT/BTC/ETH cycle unwinds in USDT and in BTC. Adding the two would be
//! arithmetic across units with no rate; a grant's single loss limit has no
//! reading in two units until somebody decides which unit it is stated in
//! and at what rate the others convert. That decision is the envelope's, not
//! this module's, and it is open.
//!
//! # When it clears
//!
//! Neither the realised P&L nor a latch clears on its own or on any unsigned
//! call. Each clears only when its owner receives a grant issued after the
//! fact it records — the last realisation, the latest unpriced leg — because
//! only a grant signed after a loss can have been signed knowing it, the rule
//! `Cell::apply_halt` keeps for a release. A strategy receives one by being
//! renewed or redeployed; the desk only by being renewed, since a cell never
//! installs a second desk. A grant issued before the fact, including the one
//! the owner already holds, keeps both. None of this survives a restart: the
//! ledger starts empty with the process, as every `Utilisation` does, and
//! nothing rebuilds it from the journal.

use qip_core::{Decimal, Timestamp};
use std::collections::BTreeMap;

/// The unit a price was quoted in, as the placer stated it. `None` is a unit
/// nobody stated, never a default somebody chose.
type Unit = Option<String>;

/// One change to one owner's position.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Leg<'a> {
    /// The strategy or desk whose book moves.
    pub(crate) owner: &'a str,
    /// The owner, venue and instrument, keyed as `strategy_positions` is.
    pub(crate) position: &'a str,
    /// Bought positive, sold negative.
    pub(crate) signed: Decimal,
    pub(crate) price: Decimal,
    pub(crate) unit: Option<&'a str>,
}

/// What booking one leg did to its owner's realised P&L.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Booking {
    /// Whatever the leg realised, if anything, is in its owner's figure.
    Priced,
    /// Nothing was added to the owner's figure, and why. The position moved.
    Unpriced(String),
}

/// A leg the ledger could not price, as its owner's latch holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unpriced {
    /// What the leg was, in the words the journal names it by.
    pub(crate) leg: String,
    /// Why it could not be priced.
    pub(crate) why: String,
    /// When the cell booked it. A grant issued after this is what clears it.
    pub(crate) at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    /// Bought positive. Never zero: a position that goes flat is removed, so
    /// the next leg opens afresh at its own price and in its own unit.
    quantity: Decimal,
    /// The average entry of `quantity`, or `None` once a leg the ledger could
    /// not price has moved it: the quantity is still a fact, what it cost is
    /// not, and a reduction measured against a guessed entry would be a loss
    /// nobody computed.
    average: Option<Decimal>,
    unit: Unit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Owner {
    /// Realised P&L since the last reset, signed: a gain is positive.
    realised: Decimal,
    /// The unit `realised` is held in, once anything has been realised.
    unit: Option<Unit>,
    /// When `realised` last changed. A redeploy resets it only under a grant
    /// issued after this.
    realised_at: Option<Timestamp>,
    /// The latest leg this owner could not price, while the latch holds.
    unpriced: Option<Unpriced>,
}

/// What one leg did to one position, before the owner's figure is touched.
enum Step {
    /// Opened or added; nothing realised.
    Opened,
    /// Reduced, realising `amount` in `unit`.
    Realised {
        amount: Decimal,
        unit: Unit,
    },
    Unpriced(String),
}

/// Every owner's average-cost positions and realised P&L at this cell.
#[derive(Debug, Default)]
pub(crate) struct RealisedLedger {
    positions: BTreeMap<String, Position>,
    owners: BTreeMap<String, Owner>,
}

impl RealisedLedger {
    /// Book one position change, and add whatever it realised to its owner.
    ///
    /// The position moves whatever the answer, because the leg is a trade that
    /// happened; only the P&L is withheld from a leg that cannot be priced.
    pub(crate) fn book(&mut self, leg: Leg<'_>, at: Timestamp) -> Booking {
        if leg.signed.is_zero() {
            return Booking::Priced;
        }
        let held = self.positions.remove(leg.position);
        let (next, step) = step(held, leg);
        if let Some(next) = next {
            self.positions.insert(leg.position.to_string(), next);
        }
        match step {
            Step::Opened => Booking::Priced,
            Step::Unpriced(why) => Booking::Unpriced(why),
            Step::Realised { amount, unit } => self.realise(leg.owner, amount, unit, at),
        }
    }

    fn realise(&mut self, owner: &str, amount: Decimal, unit: Unit, at: Timestamp) -> Booking {
        let owner = self.owners.entry(owner.to_string()).or_default();
        if let Some(held) = &owner.unit
            && *held != unit
        {
            return Booking::Unpriced(format!(
                "it realised {amount} in {} and this owner's realised P&L is held in {}; the cell \
                 holds no rate to add one to the other",
                describe(&unit),
                describe(held)
            ));
        }
        let Some(total) = owner.realised.checked_add(amount) else {
            return Booking::Unpriced(format!(
                "adding the {amount} it realised to {} cannot be represented",
                owner.realised
            ));
        };
        owner.realised = total;
        owner.unit = Some(unit);
        owner.realised_at = Some(at);
        Booking::Priced
    }

    /// The loss an envelope reads for `owner`: `max(0, −realised)`.
    ///
    /// A figure too large to negate is read as the largest loss there is,
    /// never as none.
    pub(crate) fn realised_loss(&self, owner: &str) -> Decimal {
        self.owners.get(owner).map_or(Decimal::ZERO, |owner| {
            Decimal::ZERO
                .checked_sub(owner.realised)
                .map_or(Decimal::MAX, |loss| loss.max(Decimal::ZERO))
        })
    }

    /// Stop `owner` on a leg the ledger could not price.
    ///
    /// The latest such leg is kept, because a grant must postdate every one
    /// of them to clear the latch and the latest is the one that decides.
    pub(crate) fn latch(&mut self, owner: &str, unpriced: Unpriced) {
        let owner = self.owners.entry(owner.to_string()).or_default();
        let keep_held = owner
            .unpriced
            .as_ref()
            .is_some_and(|held| held.at > unpriced.at);
        if !keep_held {
            owner.unpriced = Some(unpriced);
        }
    }

    /// The leg `owner` is latched on, if it is.
    pub(crate) fn latched(&self, owner: &str) -> Option<&Unpriced> {
        self.owners
            .get(owner)
            .and_then(|owner| owner.unpriced.as_ref())
    }

    /// `owner` has been redeployed or renewed under a grant;
    /// `granted_after(t)` answers whether that grant was issued after the
    /// instant `t`.
    ///
    /// The realised P&L resets only if the grant postdates the last
    /// realisation, and the latch clears only if it postdates the leg that set
    /// it. Everything else carries: a strategy redeployed under the grant it
    /// lost money under — the path a plan that changes one rule takes — keeps
    /// the loss, because a drawdown cleared by an unsigned redeploy is a limit
    /// anybody holding the deployment call can reset.
    pub(crate) fn redeployed(&mut self, owner: &str, granted_after: impl Fn(Timestamp) -> bool) {
        let Some(owner) = self.owners.get_mut(owner) else {
            return;
        };
        if owner.realised_at.is_some_and(&granted_after) {
            owner.realised = Decimal::ZERO;
            owner.unit = None;
            owner.realised_at = None;
        }
        if owner
            .unpriced
            .as_ref()
            .is_some_and(|unpriced| granted_after(unpriced.at))
        {
            owner.unpriced = None;
        }
    }

    /// Signed realised P&L for `owner`, for the tests beside this module.
    #[cfg(test)]
    fn realised(&self, owner: &str) -> Decimal {
        self.owners
            .get(owner)
            .map_or(Decimal::ZERO, |owner| owner.realised)
    }
}

/// Apply one leg to one position, average-cost.
fn step(held: Option<Position>, leg: Leg<'_>) -> (Option<Position>, Step) {
    let unit: Unit = leg.unit.map(str::to_string);
    let priceable = leg.price.is_positive();
    let Some(held) = held else {
        return if priceable {
            (
                Some(Position {
                    quantity: leg.signed,
                    average: Some(leg.price),
                    unit,
                }),
                Step::Opened,
            )
        } else {
            (
                Some(Position {
                    quantity: leg.signed,
                    average: None,
                    unit,
                }),
                Step::Unpriced(format!(
                    "its price {} is not positive, so what it opened has no entry price",
                    leg.price
                )),
            )
        };
    };
    let Some(after) = held.quantity.checked_add(leg.signed) else {
        return (
            Some(Position {
                average: None,
                ..held
            }),
            Step::Unpriced(format!(
                "a position of {} moved by {} cannot be represented",
                held.quantity, leg.signed
            )),
        );
    };
    let same_unit = held.unit == unit;

    if held.quantity.signum() == leg.signed.signum() {
        // Adding in the held direction re-averages, and realises nothing.
        let (average, step) = match held.average {
            // Already unknown, and the latch that made it so was set by the
            // leg that did. Adding to it realises nothing to withhold.
            None => (None, Step::Opened),
            Some(_) if !same_unit => (
                None,
                Step::Unpriced(format!(
                    "it is quoted in {} and its position is held in {}; an average across two \
                     units is not a price",
                    describe(&unit),
                    describe(&held.unit)
                )),
            ),
            Some(_) if !priceable => (
                None,
                Step::Unpriced(format!("its price {} is not positive", leg.price)),
            ),
            Some(average) => match weighted(held.quantity, average, leg.signed, leg.price) {
                Some(average) => (Some(average), Step::Opened),
                None => (
                    None,
                    Step::Unpriced(format!(
                        "the average of {} at {average} and {} at {} cannot be represented",
                        held.quantity, leg.signed, leg.price
                    )),
                ),
            },
        };
        return (
            Some(Position {
                quantity: after,
                average,
                unit: held.unit,
            }),
            step,
        );
    }

    // Reducing. Only the part that closes realises; the rest, if the leg
    // crosses through flat, opens afresh at the leg's own price and unit.
    let closed = held.quantity.abs().min(leg.signed.abs());
    let step = match held.average {
        None => Step::Unpriced(
            "its position's entry price is unknown, because a leg the cell could not price \
             moved it earlier"
                .to_string(),
        ),
        Some(_) if !same_unit => Step::Unpriced(format!(
            "it is quoted in {} and closes a position held in {}",
            describe(&unit),
            describe(&held.unit)
        )),
        Some(_) if !priceable => Step::Unpriced(format!("its price {} is not positive", leg.price)),
        Some(average) => {
            // Long: sold above the entry is a gain. Short: bought below it is.
            let per_unit = if held.quantity.is_positive() {
                leg.price.checked_sub(average)
            } else {
                average.checked_sub(leg.price)
            };
            match per_unit.and_then(|per_unit| per_unit.checked_mul(closed)) {
                Some(amount) => Step::Realised {
                    amount,
                    unit: held.unit.clone(),
                },
                None => Step::Unpriced(format!(
                    "closing {closed} entered at {average} at {} cannot be represented",
                    leg.price
                )),
            }
        }
    };
    let next = if after.is_zero() {
        None
    } else if after.signum() == held.quantity.signum() {
        // A partial close leaves the rest at its own entry, whatever this leg
        // was: the part that stayed was never repriced.
        Some(Position {
            quantity: after,
            ..held
        })
    } else {
        Some(Position {
            quantity: after,
            average: priceable.then_some(leg.price),
            unit,
        })
    };
    (next, step)
}

/// `(|q₀|·a + |s|·p) / (|q₀| + |s|)`, checked at every step.
fn weighted(held: Decimal, average: Decimal, signed: Decimal, price: Decimal) -> Option<Decimal> {
    let held = held.abs();
    let added = signed.abs();
    let cost = held
        .checked_mul(average)?
        .checked_add(added.checked_mul(price)?)?;
    cost.checked_div(held.checked_add(added)?)
}

fn describe(unit: &Unit) -> &str {
    unit.as_deref().unwrap_or("no stated unit")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).expect("a decimal literal")
    }

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    const OWNER: &str = "alpha";
    const POSITION: &str = "alpha/XLON/obj-ACME";

    fn leg(signed: &str, price: &str, unit: Option<&'static str>) -> Leg<'static> {
        Leg {
            owner: OWNER,
            position: POSITION,
            signed: d(signed),
            price: d(price),
            unit,
        }
    }

    #[test]
    fn a_partial_close_realises_only_the_closed_quantity_at_the_average_entry() {
        let mut ledger = RealisedLedger::default();
        // Two buys average to 100: 10 at 95 and 10 at 105.
        assert_eq!(ledger.book(leg("10", "95", None), at(1)), Booking::Priced);
        assert_eq!(ledger.book(leg("10", "105", None), at(2)), Booking::Priced);
        assert_eq!(
            ledger.realised(OWNER),
            Decimal::ZERO,
            "the premise is that adding realises nothing"
        );
        // Selling 5 of the 20 at 90 realises (90 − 100) × 5, and only that.
        assert_eq!(ledger.book(leg("-5", "90", None), at(3)), Booking::Priced);
        assert_eq!(ledger.realised(OWNER), d("-50"));
        assert_eq!(ledger.realised_loss(OWNER), d("50"));
        // The 15 left are still carried at 100: closing them at 100 is flat.
        assert_eq!(ledger.book(leg("-15", "100", None), at(4)), Booking::Priced);
        assert_eq!(ledger.realised(OWNER), d("-50"));
    }

    #[test]
    fn a_fill_that_flips_the_position_realises_the_close_and_opens_the_remainder_at_the_fill_price()
    {
        let mut ledger = RealisedLedger::default();
        assert_eq!(ledger.book(leg("10", "100", None), at(1)), Booking::Priced);
        // Selling 25 at 90 closes the 10 long at a loss of 100 and leaves 15
        // short entered at 90.
        assert_eq!(ledger.book(leg("-25", "90", None), at(2)), Booking::Priced);
        assert_eq!(ledger.realised(OWNER), d("-100"));
        // Buying the 15 back at 80 realises (90 − 80) × 15 on the short: the
        // remainder's entry is the flip's price, not the old long's.
        assert_eq!(ledger.book(leg("15", "80", None), at(3)), Booking::Priced);
        assert_eq!(ledger.realised(OWNER), d("50"));
        assert_eq!(
            ledger.realised_loss(OWNER),
            Decimal::ZERO,
            "a net gain is no loss, and never a negative one"
        );
    }

    #[test]
    fn a_leg_in_a_unit_other_than_its_positions_is_unpriced_and_books_no_loss() {
        let mut ledger = RealisedLedger::default();
        assert_eq!(
            ledger.book(leg("10", "100", Some("GBP")), at(1)),
            Booking::Priced
        );
        // Closing in dollars a position opened in pounds: the cell has no rate.
        let booking = ledger.book(leg("-10", "50", Some("USD")), at(2));
        let Booking::Unpriced(why) = booking else {
            panic!("a close in a second unit was priced: {booking:?}");
        };
        assert!(why.contains("USD") && why.contains("GBP"), "{why}");
        assert_eq!(
            ledger.realised(OWNER),
            Decimal::ZERO,
            "a loss computed across two units was booked as if it were one"
        );
    }

    #[test]
    fn a_realisation_in_a_second_stated_unit_is_not_added_to_the_first() {
        let mut ledger = RealisedLedger::default();
        let other = "alpha/XNYS/obj-ACME";
        let in_other = |signed: &str, price: &str| Leg {
            owner: OWNER,
            position: other,
            signed: d(signed),
            price: d(price),
            unit: Some("USD"),
        };
        ledger.book(leg("10", "100", Some("GBP")), at(1));
        ledger.book(leg("-10", "90", Some("GBP")), at(2));
        assert_eq!(ledger.realised(OWNER), d("-100"), "the premise");
        // A second listing, consistently in dollars, is priced within itself —
        // and its realisation still cannot join a figure held in pounds.
        assert_eq!(ledger.book(in_other("10", "100"), at(3)), Booking::Priced);
        let booking = ledger.book(in_other("-10", "50"), at(4));
        assert!(
            matches!(booking, Booking::Unpriced(_)),
            "a dollar loss was added to a pound figure: {booking:?}"
        );
        assert_eq!(ledger.realised(OWNER), d("-100"));
    }

    #[test]
    fn a_redeploy_resets_the_loss_only_under_a_grant_issued_after_it_and_clears_a_latch_likewise() {
        let mut ledger = RealisedLedger::default();
        ledger.book(leg("10", "100", None), at(1));
        ledger.book(leg("-10", "80", None), at(10));
        ledger.latch(
            OWNER,
            Unpriced {
                leg: "fill on order o-1".to_string(),
                why: "a second unit".to_string(),
                at: at(20),
            },
        );
        assert_eq!(ledger.realised_loss(OWNER), d("200"), "the premise");

        // The grant it lost under, or any issued before the loss: kept.
        ledger.redeployed(OWNER, |instant| instant < at(0));
        assert_eq!(ledger.realised_loss(OWNER), d("200"));
        assert!(ledger.latched(OWNER).is_some());

        // Issued after the loss and before the unpriced leg: the loss resets,
        // the latch holds.
        ledger.redeployed(OWNER, |instant| instant < at(15));
        assert_eq!(ledger.realised_loss(OWNER), Decimal::ZERO);
        assert!(ledger.latched(OWNER).is_some());

        // Issued after both: the latch clears too.
        ledger.redeployed(OWNER, |instant| instant < at(25));
        assert_eq!(ledger.latched(OWNER), None);
    }
}
