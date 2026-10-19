//! The Capital Bank's paper records: collateral posted and recalled, currency
//! converted, idle cash routed to yield.
//!
//! Three records, each the simulated form of something the blueprint asks the
//! bank to do with real money, and each built as ADR 0021 permits it: the
//! record and the refusal, never the movement. Nothing in this module calls a
//! venue, a custodian or a counterparty. The other side of each record is a
//! value the caller hands in — a venue's release confirmation, a
//! counterparty's rate, a custodian's shelf — and in this repository that
//! caller is a simulator.
//!
//! * [`PostingBook`] (CAPITAL-011). Collateral is encumbered from the instant
//!   the posting is recorded and stays encumbered until the venue's
//!   [`ReleaseConfirmation`] arrives. Asking for it back frees nothing: a
//!   recall is a request, and collateral counted as available on the strength
//!   of a request is collateral pledged twice the moment it is used.
//! * [`CurrencyBook`] (CAPITAL-013). A conversion debits one currency and
//!   credits the other in one step at a rate written into the record, so the
//!   two balances can be rebuilt from the journal alone.
//! * [`route_idle_cash`] (CAPITAL-009, ADR 0159). Capital the survival
//!   waterfall left unreleased this cycle goes only into the instrument
//!   classes the blueprint names, each placement is a paper hold in the
//!   [`ReservationLedger`] so the next construction sees exactly that much
//!   less free cash, and every routed amount is dated on the ladder at the
//!   instant it can be had back — because cash in a seven-day instrument that
//!   still reads as cash today is the floor being spent without anybody
//!   deciding to.

use crate::collateral::Pledge;
use crate::dated_ladder::{DatedLadder, EntryKind, LadderEntry};
use crate::reservation::{MAXIMUM_RESERVATION_VALIDITY, ReservationLedger};
use crate::survival::SurvivalReport;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a posting stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostingState {
    /// At the venue, backing its obligation.
    Posted,
    /// The bank has asked for it back. Still at the venue, still encumbered.
    RecallRequested {
        /// When the recall was requested.
        at: Timestamp,
    },
    /// The venue confirmed the release.
    Released {
        /// When the venue said so.
        at: Timestamp,
    },
}

/// One amount of one asset posted against one obligation at one venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    /// The asset posted.
    pub asset: String,
    /// The venue holding it.
    pub venue: VenueId,
    /// The obligation it backs.
    pub obligation: String,
    /// Face amount posted.
    pub amount: Decimal,
    /// When it was posted, and so when it became encumbered.
    pub posted_at: Timestamp,
    /// Where it stands now.
    pub state: PostingState,
}

impl Posting {
    /// Whether the amount is unavailable for any other use. Everything short
    /// of a confirmed release is.
    pub fn is_encumbered(&self) -> bool {
        !matches!(self.state, PostingState::Released { .. })
    }
}

/// A venue's statement that it has released a posting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseConfirmation {
    /// The posting released.
    pub posting: String,
    /// The venue confirming.
    pub venue: VenueId,
    /// The amount it says it released.
    pub amount: Decimal,
    /// When it confirmed.
    pub at: Timestamp,
}

/// What the bank holds of each asset, and how much of it is posted.
#[derive(Debug, Clone, Default)]
pub struct PostingBook {
    holdings: BTreeMap<String, Decimal>,
    postings: BTreeMap<String, Posting>,
}

impl PostingBook {
    /// Open a book over what is held. A negative holding is refused: it is a
    /// short, and a short is not something that can be posted.
    pub fn new(holdings: BTreeMap<String, Decimal>) -> Result<Self> {
        if let Some((asset, held)) = holdings.iter().find(|(_, held)| held.is_negative()) {
            return Err(Error::invalid(format!(
                "holding {asset} is {held}; a posting book holds what can be posted, so \
                 reconcile the balance before opening it"
            )));
        }
        Ok(Self {
            holdings,
            postings: BTreeMap::new(),
        })
    }

    /// Everything held of an asset, posted or not.
    pub fn held(&self, asset: &str) -> Decimal {
        self.holdings.get(asset).copied().unwrap_or(Decimal::ZERO)
    }

    /// The part of an asset standing behind an obligation.
    pub fn encumbered(&self, asset: &str) -> Decimal {
        self.postings
            .values()
            .filter(|posting| posting.asset == asset && posting.is_encumbered())
            .fold(Decimal::ZERO, |sum, posting| sum + posting.amount)
    }

    /// The part of an asset free for another use.
    pub fn available(&self, asset: &str) -> Decimal {
        self.held(asset) - self.encumbered(asset)
    }

    /// One posting, if it was ever recorded.
    pub fn posting(&self, id: &str) -> Option<&Posting> {
        self.postings.get(id)
    }

    /// Record a posting. Refused whole when it asks for more than is
    /// available, because the units it would take are already standing behind
    /// something else.
    pub fn post(
        &mut self,
        id: &str,
        asset: &str,
        venue: VenueId,
        obligation: &str,
        amount: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "posting {id} must be a positive amount, got {amount}"
            )));
        }
        if self.postings.contains_key(id) {
            return Err(Error::invalid(format!(
                "posting {id} is already recorded; use a new id for a new posting"
            )));
        }
        let available = self.available(asset);
        if amount > available {
            return Err(Error::denied(format!(
                "posting {id} asks for {amount} of {asset} against {obligation} and only \
                 {available} is unencumbered; recall a posting and wait for the venue to \
                 confirm it, or post less"
            )));
        }
        self.postings.insert(
            id.to_owned(),
            Posting {
                asset: asset.to_owned(),
                venue,
                obligation: obligation.to_owned(),
                amount,
                posted_at: at,
                state: PostingState::Posted,
            },
        );
        Ok(())
    }

    /// Ask for a posting back. Changes what the bank is waiting for and
    /// nothing about what is available.
    pub fn request_recall(&mut self, id: &str, at: Timestamp) -> Result<()> {
        let posting = self
            .postings
            .get_mut(id)
            .ok_or_else(|| Error::not_found(format!("no posting {id} to recall")))?;
        if posting.state != PostingState::Posted {
            return Err(Error::invalid(format!(
                "posting {id} is {:?}; only a posted amount can be recalled",
                posting.state
            )));
        }
        posting.state = PostingState::RecallRequested { at };
        Ok(())
    }

    /// Take the venue's word that a posting is released. The only step that
    /// makes collateral available again.
    ///
    /// Refused when the confirmation is for a posting nobody recalled, comes
    /// from another venue, or names another amount: each is a statement that
    /// does not match the record, and matching it anyway would free
    /// collateral on a claim about something else.
    pub fn confirm_release(&mut self, confirmation: &ReleaseConfirmation) -> Result<()> {
        let id = &confirmation.posting;
        let posting = self
            .postings
            .get_mut(id)
            .ok_or_else(|| Error::not_found(format!("no posting {id} to release")))?;
        let PostingState::RecallRequested { at: requested } = posting.state else {
            return Err(Error::invalid(format!(
                "posting {id} is {:?} and no recall is outstanding; request the recall first, \
                 and reconcile with the venue if it released collateral nobody asked for",
                posting.state
            )));
        };
        if confirmation.venue != posting.venue {
            return Err(Error::denied(format!(
                "posting {id} is held at {} and the release is confirmed by {}; only the \
                 venue holding the collateral can release it",
                posting.venue, confirmation.venue
            )));
        }
        if confirmation.amount != posting.amount {
            return Err(Error::invalid(format!(
                "posting {id} is for {} and the venue confirms {}; a partial release is a new \
                 posting for the remainder, so record it as one",
                posting.amount, confirmation.amount
            )));
        }
        if confirmation.at < requested {
            return Err(Error::invalid(format!(
                "posting {id} was recalled at {requested} and the confirmation is dated {}; a \
                 release cannot precede the request it answers",
                confirmation.at
            )));
        }
        posting.state = PostingState::Released {
            at: confirmation.at,
        };
        Ok(())
    }

    /// Every encumbered posting as the pledge set
    /// [`crate::collateral::CollateralGraph::build`] takes, face amounts
    /// summed per venue and asset. This is how a posting reaches the
    /// collateral model: the graph is built from what the book says is out.
    pub fn pledges(&self) -> Vec<Pledge> {
        let mut by_key: BTreeMap<(VenueId, String), Decimal> = BTreeMap::new();
        for posting in self.postings.values().filter(|p| p.is_encumbered()) {
            *by_key
                .entry((posting.venue.clone(), posting.asset.clone()))
                .or_insert(Decimal::ZERO) += posting.amount;
        }
        by_key
            .into_iter()
            .map(|((venue, asset), amount)| Pledge {
                asset,
                venue,
                amount,
            })
            .collect()
    }
}

/// One conversion, both legs and the rate that joins them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversion {
    /// Stable identity.
    pub id: String,
    /// Who took the other side.
    pub counterparty: String,
    /// Currency debited.
    pub sold: Currency,
    /// Amount debited.
    pub sold_amount: Decimal,
    /// Currency credited.
    pub bought: Currency,
    /// Amount credited: `sold_amount` at `rate`.
    pub bought_amount: Decimal,
    /// Units of `bought` per unit of `sold`, as the counterparty quoted it.
    pub rate: Decimal,
    /// When it was converted.
    pub at: Timestamp,
}

/// Cash by currency, and every conversion between them.
#[derive(Debug, Clone, Default)]
pub struct CurrencyBook {
    balances: BTreeMap<Currency, Decimal>,
    journal: Vec<Conversion>,
}

impl CurrencyBook {
    /// Empty book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Credit an opening or funded balance.
    pub fn fund(&mut self, currency: Currency, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a funding of {currency} must be positive, got {amount}"
            )));
        }
        *self.balances.entry(currency).or_insert(Decimal::ZERO) += amount;
        Ok(())
    }

    /// The balance held in one currency.
    pub fn balance(&self, currency: Currency) -> Decimal {
        self.balances
            .get(&currency)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// Every conversion, in order, for the event log.
    pub fn journal(&self) -> &[Conversion] {
        &self.journal
    }

    /// Convert `sold_amount` of `sold` into `bought` at `rate`, debiting one
    /// balance and crediting the other together or not at all.
    ///
    /// Refused, not reduced, when the balance does not cover the amount: a
    /// conversion cut to fit funds a placement in a currency with less than
    /// the placement was told it had.
    pub fn convert(
        &mut self,
        id: &str,
        counterparty: &str,
        sold: Currency,
        sold_amount: Decimal,
        bought: Currency,
        rate: Decimal,
        at: Timestamp,
    ) -> Result<Conversion> {
        if sold == bought {
            return Err(Error::invalid(format!(
                "conversion {id} sells and buys {sold}; a conversion crosses two currencies"
            )));
        }
        if !sold_amount.is_positive() || !rate.is_positive() {
            return Err(Error::invalid(format!(
                "conversion {id} needs a positive amount and rate, got {sold_amount} at {rate}"
            )));
        }
        if self.journal.iter().any(|done| done.id == id) {
            return Err(Error::invalid(format!(
                "conversion {id} is already recorded; use a new id"
            )));
        }
        let held = self.balance(sold);
        if sold_amount > held {
            return Err(Error::denied(format!(
                "conversion {id} sells {sold_amount} {sold} and the book holds {held}; fund \
                 the balance or convert at most that"
            )));
        }
        let bought_amount = sold_amount
            .checked_mul(rate)
            .filter(|amount| amount.is_positive())
            .ok_or_else(|| {
                Error::numeric(format!(
                    "conversion {id} of {sold_amount} {sold} at {rate} does not yield a \
                     positive, representable amount of {bought}; check the quoted rate"
                ))
            })?;
        *self.balances.entry(sold).or_insert(Decimal::ZERO) -= sold_amount;
        *self.balances.entry(bought).or_insert(Decimal::ZERO) += bought_amount;
        let conversion = Conversion {
            id: id.to_owned(),
            counterparty: counterparty.to_owned(),
            sold,
            sold_amount,
            bought,
            bought_amount,
            rate,
            at,
        };
        self.journal.push(conversion.clone());
        Ok(conversion)
    }
}

/// What kind of instrument a custodian offers for idle cash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum InstrumentClass {
    /// Cash and overnight deposits.
    Cash,
    /// Stable-value assets.
    StableValue,
    /// Short-duration instruments.
    ShortDuration,
    /// Anything else on the shelf. Never a home for idle cash.
    Other,
}

impl InstrumentClass {
    /// Whether idle cash may be routed into this class. The three the
    /// blueprint names and no fourth: a class nobody listed is refused by
    /// being absent here, not by a rule somebody has to remember to write.
    pub fn takes_idle_cash(self) -> bool {
        matches!(self, Self::Cash | Self::StableValue | Self::ShortDuration)
    }
}

/// One instrument on a custodian's shelf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YieldInstrument {
    /// Stable identity.
    pub id: String,
    /// Its class, which decides whether idle cash may go into it.
    pub class: InstrumentClass,
    /// How long after placing the cash can be had back.
    pub realisation: Duration,
    /// The most the custodian will take.
    pub capacity: Decimal,
    /// Yield per annum, as a fraction. Orders the shelf; multiplies nothing.
    pub yield_rate: Decimal,
}

/// The instruments an operator has declared, validated.
///
/// There is no default shelf and no licensed source behind one (ADR 0159
/// decision 4): [`Shelf::empty`] is what an absent declaration means, and an
/// empty shelf routes nothing. Construction is the only validation point, so a
/// router holding a `Shelf` never meets an instrument it would have to
/// second-guess.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shelf {
    instruments: Vec<YieldInstrument>,
}

impl Shelf {
    /// A shelf that routes nothing: the meaning of an undeclared shelf.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Validate a declaration. Refuses, naming the instrument:
    ///
    /// * a blank or duplicated id (the hold and the ladder entry are keyed on
    ///   it, so a duplicate would be two claims about one hold);
    /// * a non-positive capacity, or a negative yield;
    /// * for a class that takes idle cash, a realisation that is not positive
    ///   or exceeds [`MAXIMUM_RESERVATION_VALIDITY`]. Such an instrument
    ///   cannot be held honestly, and truncating its realisation would date
    ///   cash earlier than it is available. [`InstrumentClass::Other`] may be
    ///   listed with any realisation because it is never routed.
    pub fn new(instruments: Vec<YieldInstrument>) -> Result<Self> {
        let mut seen = std::collections::BTreeSet::new();
        for instrument in &instruments {
            let id = &instrument.id;
            if id.trim().is_empty() || id.contains(char::is_whitespace) {
                return Err(Error::invalid(
                    "a shelf instrument needs an id with no whitespace; its hold and its ladder \
                     entry are keyed on it",
                ));
            }
            if !seen.insert(id.as_str()) {
                return Err(Error::invalid(format!(
                    "shelf instrument {id} is declared twice; one id is one hold, so rename or \
                     remove one"
                )));
            }
            if !instrument.capacity.is_positive() {
                return Err(Error::invalid(format!(
                    "shelf instrument {id} declares a capacity of {}; the custodian takes a \
                     positive amount or the instrument is not on the shelf",
                    instrument.capacity
                )));
            }
            if instrument.yield_rate.is_negative() {
                return Err(Error::invalid(format!(
                    "shelf instrument {id} declares a yield of {}; the shelf is ordered by \
                     yield and a negative one is a typo for a cost",
                    instrument.yield_rate
                )));
            }
            if instrument.class.takes_idle_cash() {
                if instrument.realisation.as_nanos() <= 0 {
                    return Err(Error::invalid(format!(
                        "shelf instrument {id} is realised in {:?}; cash placed there is had \
                         back after a positive time or it is not a placement",
                        instrument.realisation
                    )));
                }
                if instrument.realisation > MAXIMUM_RESERVATION_VALIDITY {
                    return Err(Error::denied(format!(
                        "shelf instrument {id} is realised in {:.1} hour(s), past the {:.1} \
                         hour(s) a reservation may hold capital; a placement is held in the \
                         reservation ledger, so an instrument that takes longer cannot be held \
                         honestly and its realisation is not truncated to fit",
                        instrument.realisation.as_secs_f64() / 3600.0,
                        MAXIMUM_RESERVATION_VALIDITY.as_secs_f64() / 3600.0
                    )));
                }
            }
        }
        Ok(Self { instruments })
    }

    /// Whether anything is declared.
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    /// The declared instruments, in declaration order.
    pub fn instruments(&self) -> &[YieldInstrument] {
        &self.instruments
    }
}

/// The two limits that arm routing. Both are the RISK lane's to set and
/// neither has a default (ADR 0159 decisions 6 and 7): a router built without
/// them does not exist, so "unarmed" is a state the caller holds as `None`,
/// never a value that routes everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingTerms {
    max_share: Decimal,
    horizons: Vec<Duration>,
}

impl RoutingTerms {
    /// Validate the terms. The share is a fraction in `(0, 1]` of the
    /// unreleased capital that may be routed in one cycle; the horizons are the
    /// rungs the ladder dates placements on. Both are refused rather than
    /// corrected: a share of `1.5` is a typo that would route more than there
    /// is, and unordered horizons are a ladder nobody configured.
    pub fn new(max_share: Decimal, horizons: Vec<Duration>) -> Result<Self> {
        if !max_share.is_positive() || max_share > Decimal::ONE {
            return Err(Error::invalid(format!(
                "the routed share is {max_share}; it is the fraction of unreleased capital one \
                 cycle may route, in (0, 1]"
            )));
        }
        // Validates the horizons by the ladder's own rule rather than a copy
        // of it, so the two cannot disagree about what a rung list may be.
        DatedLadder::new(Timestamp::from_nanos(0), &horizons)?;
        Ok(Self {
            max_share,
            horizons,
        })
    }

    /// The most of the unreleased capital one cycle may route.
    pub fn max_share(&self) -> Decimal {
        self.max_share
    }

    /// The ladder rungs, shortest first.
    pub fn horizons(&self) -> &[Duration] {
        &self.horizons
    }
}

/// The capital the cycle declined to release, and so the only cash the router
/// may move (ADR 0159 decision 5).
///
/// It has no public constructor but [`IdleCash::from_waterfall`], so a caller
/// cannot hand the router a number the waterfall did not leave over. The router
/// therefore cannot touch what the cycle released: that capital is the
/// confidence governor's decision that a thesis may take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleCash {
    amount: Decimal,
}

impl IdleCash {
    /// The part of `report`'s opportunity capital that `released` did not
    /// release. Refused when `released` is negative or exceeds the opportunity
    /// capital: the governor only ever contracts a budget, so a release above
    /// what the waterfall left is a disagreement between two claims about one
    /// balance, and the router is not the place to resolve it.
    pub fn from_waterfall(report: &SurvivalReport, released: Decimal) -> Result<Self> {
        if released.is_negative() || released > report.opportunity_capital {
            return Err(Error::invalid(format!(
                "the cycle released {released} against {} of opportunity capital; the \
                 waterfall only contracts, so nothing is routed until the two agree",
                report.opportunity_capital
            )));
        }
        let amount = report
            .opportunity_capital
            .checked_sub(released)
            .ok_or_else(|| Error::numeric("the unreleased capital overflows"))?;
        Ok(Self { amount })
    }

    /// The unreleased amount.
    pub fn amount(&self) -> Decimal {
        self.amount
    }
}

/// One amount of idle cash routed into one instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YieldPlacement {
    /// The instrument it went into.
    pub instrument: String,
    /// The amount routed.
    pub amount: Decimal,
    /// When it can be had back, and so where it sits on the ladder.
    pub available_at: Timestamp,
}

/// An instrument the router did not use, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRefusal {
    /// The instrument.
    pub instrument: String,
    /// What stopped it, naming the hold or the notice.
    pub reason: String,
}

/// What one routing pass did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoutingOutcome {
    /// Placements made, best yield first.
    pub placements: Vec<YieldPlacement>,
    /// Instruments skipped, in the order they were considered.
    pub refused: Vec<RoutingRefusal>,
}

/// The commitment, hold and ladder id a placement into `instrument` is filed
/// under.
pub fn yield_entry_id(instrument: &str) -> String {
    format!("yield:{instrument}")
}

/// Route `idle` into the permitted instruments on `shelf`, best yield first,
/// as paper holds.
///
/// * At most `terms.max_share()` of `idle` is routed in one pass, and never
///   more than the reservation ledger has free. Sizing, not correcting: each
///   placement is the smaller of what remains and what the custodian takes.
/// * Each placement is a [`ReservationLedger`] hold valid for the instrument's
///   realisation, so `Platform::deployable_capital` sees it with no
///   reconciliation step, and a [`DatedLadder`] entry at `now` plus that
///   realisation. A placement the ladder will not date is released and the
///   pass fails: routed cash missing from the ladder is exactly the failure
///   the ladder exists to prevent.
/// * An instrument that cannot be had back before the ladder's earliest
///   outflow is refused, naming it (ADR 0159 decision 10). That is cash parked
///   where it cannot be had back before it is owed.
/// * An instrument still held from an earlier pass is skipped, not topped up:
///   one id is one hold.
///
/// Nothing leaves the process. A placement is a hold on paper capital, not a
/// venue order and not a custodian call (ADR 0003, ADR 0021).
pub fn route_idle_cash(
    idle: IdleCash,
    shelf: &Shelf,
    terms: &RoutingTerms,
    reservations: &mut ReservationLedger,
    ladder: &mut DatedLadder,
    now: Timestamp,
) -> Result<RoutingOutcome> {
    let mut permitted: Vec<&YieldInstrument> = shelf
        .instruments()
        .iter()
        .filter(|instrument| instrument.class.takes_idle_cash())
        .collect();
    permitted.sort_by(|a, b| b.yield_rate.cmp(&a.yield_rate).then(a.id.cmp(&b.id)));

    let mut remaining = idle
        .amount()
        .checked_mul(terms.max_share())
        .ok_or_else(|| Error::numeric("the routable share of idle cash overflows"))?;
    let mut outcome = RoutingOutcome::default();
    for instrument in permitted {
        let id = yield_entry_id(&instrument.id);
        let free = reservations.free(now);
        let available_at = now.saturating_add(instrument.realisation);
        if let Some(held) = reservations.reservation(&id) {
            outcome.refused.push(RoutingRefusal {
                instrument: instrument.id.clone(),
                reason: format!(
                    "{id} is still held until {}; one id is one hold, so it is not topped up",
                    held.expires_at
                ),
            });
            continue;
        }
        if let Some(outflow) = ladder.earliest_outflow()
            && available_at > outflow.at
        {
            outcome.refused.push(RoutingRefusal {
                instrument: instrument.id.clone(),
                reason: format!(
                    "{} is realised at {available_at} and {} of {} falls due at {} ({}); cash \
                     parked there cannot be had back before it is owed",
                    instrument.id, outflow.id, outflow.amount, outflow.at, outflow.id
                ),
            });
            continue;
        }
        // Sizing, not correcting: whichever is smallest of what the share
        // still allows, what the custodian takes and what is free to hold.
        let amount = remaining.min(instrument.capacity).min(free);
        if !amount.is_positive() {
            continue;
        }
        reservations.reserve(id.clone(), amount, now, instrument.realisation)?;
        let dated = ladder.place(LadderEntry {
            id: id.clone(),
            kind: EntryKind::Balance,
            amount,
            at: available_at,
        });
        if let Err(refusal) = dated {
            // A placement the ladder will not date is not made.
            reservations.release(&id, now)?;
            return Err(refusal);
        }
        remaining = remaining
            .checked_sub(amount)
            .ok_or_else(|| Error::numeric("the routable share of idle cash underflows"))?;
        outcome.placements.push(YieldPlacement {
            instrument: instrument.id.clone(),
            amount,
            available_at,
        });
    }
    Ok(outcome)
}
