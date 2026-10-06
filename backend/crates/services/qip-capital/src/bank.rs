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
//! * [`route_idle_cash`] (CAPITAL-009). Cash above the uncommitted floor goes
//!   only into the instrument classes the blueprint names, and every routed
//!   amount is dated on the ladder at the instant it can be had back —
//!   because cash in a seven-day instrument that still reads as cash today is
//!   the floor being spent without anybody deciding to.

use crate::collateral::Pledge;
use crate::dated_ladder::{DatedLadder, EntryKind, LadderEntry};
use crate::treasury::{CapitalBook, Use};
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

/// The commitment and ladder id a placement into `instrument` is filed under.
pub fn yield_entry_id(instrument: &str) -> String {
    format!("yield:{instrument}")
}

/// Route the cash above `book`'s uncommitted floor into the permitted
/// instruments on `shelf`, best yield first, and date each routed amount on
/// `ladder` at its realisation horizon.
///
/// The floor is never routed: only `uncommitted - floor` is idle. Each
/// placement is a [`Use::Yield`] commitment on the book, so the book's own
/// floor check stands behind this one, and a ladder entry at `now` plus the
/// instrument's realisation time.
pub fn route_idle_cash(
    book: &mut CapitalBook,
    ladder: &mut DatedLadder,
    shelf: &[YieldInstrument],
    now: Timestamp,
) -> Result<Vec<YieldPlacement>> {
    let mut permitted: Vec<&YieldInstrument> = shelf
        .iter()
        .filter(|instrument| instrument.class.takes_idle_cash())
        .collect();
    permitted.sort_by(|a, b| b.yield_rate.cmp(&a.yield_rate).then(a.id.cmp(&b.id)));

    let mut placements = Vec::new();
    for instrument in permitted {
        let idle = book.uncommitted() - book.floor();
        // Sizing, not correcting: the placement is whichever is smaller of
        // what is idle and what the custodian will take.
        let amount = idle.min(instrument.capacity);
        if !amount.is_positive() {
            continue;
        }
        let id = yield_entry_id(&instrument.id);
        let available_at = now.saturating_add(instrument.realisation);
        book.commit(&id, Use::Yield, amount)?;
        let dated = ladder.place(LadderEntry {
            id: id.clone(),
            kind: EntryKind::Balance,
            amount,
            at: available_at,
        });
        if let Err(refusal) = dated {
            // A placement the ladder will not date is not made: routed cash
            // missing from the ladder is exactly the failure this prevents.
            book.release(&id)?;
            return Err(refusal);
        }
        placements.push(YieldPlacement {
            instrument: instrument.id.clone(),
            amount,
            available_at,
        });
    }
    Ok(placements)
}
