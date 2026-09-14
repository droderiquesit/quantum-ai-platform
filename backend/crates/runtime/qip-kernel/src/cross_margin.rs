//! Blueprint §25.6's first two rows against the platform's own state: what
//! collateralises what, per margin domain, and which exposure has no cover
//! anybody has seen.
//!
//! [`qip_capital::collateral`] holds the graph and its arithmetic. This
//! module is the only thing that builds one out of what this process
//! actually knows, and the interesting part is which two facts it is willing
//! to join.
//!
//! **The requirement side** comes from the risk aggregate's counterparty
//! axis — [`qip_risk::limits::COUNTERPARTY_AXIS`], the running gross per
//! counterparty that `MaxCounterpartyExposure` already reads. Every fill
//! moves it: a desk fill under the broker's name, a cell's fill under the
//! venue its own report named. So the platform knows, per venue, how much
//! exposure it is carrying there.
//!
//! **The cover side** comes from [`Platform::observe_statement`]'s
//! holdings — a balance somebody read at a venue and handed in, keyed by
//! venue and asset. That is the one statement of *what is held where* this
//! process can honestly attest; the kernel holds no read-only key and no
//! watch-only address, so there is no other channel it could come through.
//!
//! The two are joined on the venue name, which both sides already use.
//!
//! # The three things this produces, and what makes each of them fire
//!
//! * **Exposure nobody has seen the cover for.** A venue carrying gross on
//!   the counterparty axis for which no statement exists is not a
//!   well-collateralised venue and it is not an under-collateralised one: it
//!   is an unobserved one, and it is reported as its own category. It fires
//!   on any book that has taken a fill and holds no statement for that
//!   counterparty, which is every deployment today, and it stops firing one
//!   venue at a time as statements arrive. That is the shape a gap should
//!   have.
//! * **A margin call on observed cover.** A venue whose statements, after
//!   haircut, come to less than the maintenance its own exposure requires.
//!   It fires when a statement is handed in and the numbers do not work,
//!   which is what a statement is for.
//! * **A debit balance charged as an obligation.** A statement may show a
//!   negative balance — a margin account in debit is a real balance, and
//!   `HoldingObservation` deliberately does not refuse the sign. A debit is
//!   not collateral with a minus sign in front of it; it is money owed, and
//!   it is added to that venue's requirement. Treating it as negative cover
//!   would net it against the securities in the same account and understate
//!   the call by exactly twice the debit.
//!
//! # What this deliberately does not produce, and why
//!
//! [`qip_capital::collateral::CollateralGraph`] can find collateral counted
//! by two domains and can follow a forced close-out from one venue to the
//! next. **Neither can happen in a graph built here, and the reason is a
//! property of the input rather than a limitation of the arithmetic**: a
//! statement is keyed by venue *and* asset, so an asset at one venue and the
//! same asset at another are two observations of two holdings, and nothing
//! this platform records says one holding stands behind two obligations. So
//! there is no reuse to find and every cascade is one step long.
//!
//! They are therefore not fields on [`CrossMarginReview`]. A field that is
//! structurally empty on every production path is the control that reads as
//! protection and cannot fire, and an `Option` that is always `None` is the
//! same thing wearing a type. What would populate them is a declared
//! cross-venue or rehypothecation agreement, which is a term somebody signs
//! and which this platform has never been given; the graph refuses to invent
//! one, and so does this module. The arithmetic is exercised by
//! `qip-capital`'s own tests and by
//! `qip-acceptance/tests/cross_margin.rs`, and it is honest to say it is
//! library-only until an arrangement exists to feed it.
//!
//! # Money and statistics
//!
//! Every balance, haircut and requirement is [`Decimal`]. There is no `f64`
//! in this module at all: the rates come from
//! [`qip_capital::margin::MarginModel`], which states them as `Decimal`, and
//! the only ratios anyone reads are the ones `DomainCoverage` computes and
//! marks. Iteration is over [`BTreeMap`], so two reviews of one book name
//! the venues in one order.

use qip_capital::collateral::{
    CollateralAsset, CollateralGraph, DomainCoverage, MarginDomain, MarginRegime, Pledge,
    Rehypothecation,
};
use qip_capital::margin::MarginModel;
use qip_capital_fabric::wallet::{HoldingObservation, VenueAsset};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Currency, Decimal};
use qip_events::{EventBody, Topic};
use qip_risk::aggregate::AggregateFigures;
use qip_risk::limits::COUNTERPARTY_AXIS;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The haircut applied to an observed holding that is not the account's own
/// settlement cash.
///
/// **A declared figure, not a measurement.** Nothing in this platform
/// measures what a counterparty would lend against a given security: that is
/// a term of a margin agreement, and this repository has never been shown
/// one. It is stated here the way
/// [`qip_capital::margin::MarginModel::default`]'s Reg-T-like rates are
/// stated — as a conservative standing assumption a reader can find, argue
/// with and replace — rather than being computed from something and
/// presented as a result. Thirty percent is on the severe side of a listed
/// equity haircut, which is the right side to be on for a default: a
/// haircut set too high reports a call that is not there, and a haircut set
/// too low reports cover that is not there.
pub const NON_CASH_HAIRCUT: Decimal = Decimal::from_raw(300_000_000);

/// The driver label a non-cash holding carries into the graph.
///
/// Every security observed at a venue is labelled with this one driver, and
/// no margin domain built here declares any driver, so nothing built by this
/// module is ever *correlated* in the graph's sense. That is deliberate and
/// it is the honest arm: the platform records no statement of what moves a
/// venue's own obligation, and labelling every security "market" while
/// labelling the domain the same thing would make the correlation finding
/// fire on every domain in every book, which is the same as it never firing.
/// The label exists so that a caller supplying real drivers has somewhere to
/// put them. See [`qip_capital::collateral::DomainCoverage::correlated`].
pub const HOLDING_DRIVER: &str = "security";

/// Exposure at a venue nobody has read a statement for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnobservedExposure {
    pub venue: VenueId,
    /// Gross the counterparty axis carries at this venue.
    pub gross: Decimal,
    /// What must be maintained there, on the model's rates.
    pub maintenance: Decimal,
}

/// What the cross-margin read found this cycle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossMarginReview {
    /// One entry per venue that has both a statement and, where it has
    /// exposure, a requirement — in venue order.
    pub coverage: Vec<DomainCoverage>,
    /// Venues carrying exposure and no observed cover, in venue order.
    pub unobserved: Vec<UnobservedExposure>,
    /// Venues whose observed cover is below their own maintenance.
    pub calls: Vec<VenueId>,
    /// Maintenance required across every venue, observed or not.
    pub maintenance_required: Decimal,
    /// Post-haircut cover observed across every venue that has a statement.
    pub cover_observed: Decimal,
}

impl CrossMarginReview {
    /// Gross carried at venues with no statement.
    pub fn unobserved_gross(&self) -> Decimal {
        self.unobserved.iter().map(|gap| gap.gross).sum()
    }

    /// Maintenance required at venues with no statement.
    ///
    /// The number a desk should be asked about: this much must be posted
    /// somewhere nobody has looked.
    pub fn unobserved_maintenance(&self) -> Decimal {
        self.unobserved.iter().map(|gap| gap.maintenance).sum()
    }

    /// Whether there is anything to say. A book that has traded nowhere and
    /// holds no statement produces no finding, because there is no fact in
    /// it — rather than producing a reassuring zero.
    pub fn is_finding(&self) -> bool {
        !self.unobserved.is_empty() || !self.calls.is_empty()
    }

    pub fn describe(&self) -> String {
        if !self.is_finding() {
            return format!(
                "every venue carrying exposure has a statement, and each covers its own \
                 maintenance: {} observed against {} required",
                self.cover_observed, self.maintenance_required
            );
        }
        let mut parts = Vec::new();
        if !self.unobserved.is_empty() {
            parts.push(format!(
                "{} of gross at {} venue(s) with no statement, needing {} posted where nobody \
                 has looked ({})",
                self.unobserved_gross(),
                self.unobserved.len(),
                self.unobserved_maintenance(),
                self.unobserved
                    .iter()
                    .map(|gap| gap.venue.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.calls.is_empty() {
            parts.push(format!(
                "{} venue(s) below maintenance on observed cover ({})",
                self.calls.len(),
                self.calls
                    .iter()
                    .map(VenueId::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join("; ")
    }
}

/// Build the collateral graph this process can honestly attest, and read it.
///
/// `holdings` is [`Platform::observe_statement`]'s map; `figures` is the risk
/// aggregate; `model` supplies the rates. Nothing here reads a clock, opens
/// anything, or reaches for state of its own.
///
/// **A venue with a statement becomes a margin domain; a venue without one
/// does not.** That is the same discipline `Platform::reconcile_wallet`
/// follows and it is the load-bearing decision in this function: building a
/// domain with no pledged assets would give it zero cover against a positive
/// maintenance, which reads identically to an account that has been observed
/// and found empty. An unobserved account must never read as an empty one,
/// so the unobserved venues leave by a different door with their own name on
/// it.
///
/// Every domain is [`MarginRegime::Isolated`] and every one forbids
/// rehypothecation, because no cross-venue or re-pledge agreement has ever
/// been recorded in this platform. Those are the graph's own defaults; they
/// are passed explicitly here so that a future caller with a real
/// arrangement has to change a line rather than discover one.
pub fn review(
    holdings: &BTreeMap<VenueAsset, HoldingObservation>,
    figures: &impl AggregateFigures,
    model: &MarginModel,
    settlement: Currency,
) -> Result<CrossMarginReview> {
    let gross_by_venue: BTreeMap<VenueId, Decimal> = figures
        .axis_exposures()
        .get(COUNTERPARTY_AXIS)
        .into_iter()
        .flatten()
        .filter(|(_, gross)| gross.is_positive())
        .map(|(counterparty, gross)| (VenueId::new(counterparty.clone()), *gross))
        .collect();

    let observed_venues: BTreeSet<VenueId> = holdings.keys().map(|key| key.venue.clone()).collect();

    let mut assets = Vec::new();
    let mut pledges = Vec::new();
    // Debits per venue, to be charged as obligations rather than netted
    // against the securities sitting in the same account.
    let mut debits: BTreeMap<VenueId, Decimal> = BTreeMap::new();
    for (key, observation) in holdings {
        if observation.observed.is_negative() {
            *debits.entry(key.venue.clone()).or_insert(Decimal::ZERO) += observation.observed.abs();
            continue;
        }
        if !observation.observed.is_positive() {
            // A flat balance is not a holding. The graph refuses a
            // non-positive mark, and this is not a correction of a bad input:
            // an account observed at zero is an account with nothing in it.
            continue;
        }
        let id = key.to_string();
        // Cash is the asset whose name *is* the settlement currency's, which
        // is the key `Platform::reconcile_wallet` already books the desk's
        // balance under. Everything else takes the haircut.
        //
        // The obvious alternative is a trap and was written first:
        // `Currency::parse` admits any two-to-four alphanumeric code, so the
        // ticker `AAA` parses as a currency and would have been handed a
        // haircut of zero. That is the wrong direction to be wrong in — a
        // security counted at full mark is cover the account does not have —
        // and it would have been invisible, because the figure would look
        // like a perfectly ordinary coverage number.
        let is_cash = key.asset.as_str() == settlement.as_str();
        assets.push(CollateralAsset {
            id: id.clone(),
            value: observation.observed,
            haircut: if is_cash {
                Decimal::ZERO
            } else {
                NON_CASH_HAIRCUT
            },
            driver: if is_cash {
                None
            } else {
                Some(HOLDING_DRIVER.to_string())
            },
        });
        pledges.push(Pledge {
            asset: id,
            venue: key.venue.clone(),
            amount: observation.observed,
        });
    }

    let mut domains = Vec::new();
    for venue in &observed_venues {
        let gross = gross_by_venue.get(venue).copied().unwrap_or(Decimal::ZERO);
        let debit = debits.get(venue).copied().unwrap_or(Decimal::ZERO);
        domains.push(MarginDomain {
            venue: venue.clone(),
            regime: MarginRegime::Isolated,
            rehypothecation: Rehypothecation::Forbidden,
            initial: requirement(gross, model.initial_rate, debit)?,
            maintenance: requirement(gross, model.maintenance_rate, debit)?,
            drivers: BTreeSet::new(),
        });
    }

    let graph = CollateralGraph::build(assets, domains, pledges, Vec::new())?;
    let coverage = graph.coverage()?;

    let mut unobserved = Vec::new();
    let mut maintenance_required = Decimal::ZERO;
    for (venue, gross) in &gross_by_venue {
        let maintenance = requirement(*gross, model.maintenance_rate, Decimal::ZERO)?;
        maintenance_required += maintenance;
        if !observed_venues.contains(venue) {
            unobserved.push(UnobservedExposure {
                venue: venue.clone(),
                gross: *gross,
                maintenance,
            });
        }
    }
    // A debit at an observed venue is part of what must be maintained and is
    // carried on no counterparty bucket, so it is added here too. Without
    // this line a book whose only obligation was a margin loan would report
    // nothing required while a domain below maintenance sat in `calls`, and
    // the two halves of one read would disagree.
    maintenance_required += debits.values().copied().sum::<Decimal>();

    let calls = coverage
        .values()
        .filter(|cover| cover.is_call())
        .map(|cover| cover.venue.clone())
        .collect();
    let cover_observed = coverage.values().map(DomainCoverage::effective).sum();

    Ok(CrossMarginReview {
        coverage: coverage.into_values().collect(),
        unobserved,
        calls,
        maintenance_required,
        cover_observed,
    })
}

/// A rate against gross, plus what is owed outright.
///
/// The debit is added rather than rated: money borrowed is owed in full,
/// where a position is owed a fraction of its notional.
fn requirement(gross: Decimal, rate: Decimal, debit: Decimal) -> Result<Decimal> {
    let rated = gross.checked_mul(rate).ok_or_else(|| {
        qip_core::error::Error::numeric(
            "a margin requirement overflowed against the configured rate",
        )
    })?;
    rated.checked_add(debit).ok_or_else(|| {
        qip_core::error::Error::numeric("a margin requirement overflowed against a debit balance")
    })
}

/// The cross-margin read, as the log carries it.
///
/// Journaled when [`CrossMarginReview::is_finding`] holds, and not otherwise:
/// a record every cycle that says "nothing" is a record nobody reads, and the
/// absence of this entry already means the same thing. Filed under
/// [`Topic::RiskEvaluated`] because a coverage read is a risk evaluation and
/// not a decision — nothing in this platform refuses an order on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossMarginFinding {
    /// Venues carrying exposure with no statement, in venue order.
    pub unobserved_venues: Vec<String>,
    pub unobserved_gross: Decimal,
    pub unobserved_maintenance: Decimal,
    /// Venues below maintenance on cover somebody actually read.
    pub called_venues: Vec<String>,
    pub maintenance_required: Decimal,
    pub cover_observed: Decimal,
    pub cycle: u64,
    pub at: Timestamp,
}

impl CrossMarginFinding {
    /// The record for a review, or `None` where there is nothing to record.
    pub fn of(review: &CrossMarginReview, cycle: u64, at: Timestamp) -> Option<Self> {
        review.is_finding().then(|| Self {
            unobserved_venues: review
                .unobserved
                .iter()
                .map(|gap| gap.venue.to_string())
                .collect(),
            unobserved_gross: review.unobserved_gross(),
            unobserved_maintenance: review.unobserved_maintenance(),
            called_venues: review.calls.iter().map(VenueId::to_string).collect(),
            maintenance_required: review.maintenance_required,
            cover_observed: review.cover_observed,
            cycle,
            at,
        })
    }
}

impl EventBody for CrossMarginFinding {
    const TOPIC: Topic = Topic::RiskEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("cross-margin:{}", self.cycle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_capital_fabric::wallet::{Asset, Provenance};
    use qip_core::dec;
    use qip_risk::aggregate::RiskAggregates;

    fn aggregates(book: &[(&str, &str, &str)]) -> RiskAggregates {
        let mut aggregates =
            RiskAggregates::new(dec!("1000000"), dec!("1000000")).expect("open the book");
        for (instrument, counterparty, notional) in book {
            let mut axes = BTreeMap::new();
            axes.insert(COUNTERPARTY_AXIS.to_string(), (*counterparty).to_string());
            aggregates
                .apply_fill(
                    "desk",
                    instrument,
                    &axes,
                    Decimal::parse(notional).unwrap_or(Decimal::ZERO),
                )
                .expect("apply the fill");
        }
        aggregates
    }

    fn statement(venue: &str, asset: &str, observed: &str) -> (VenueAsset, HoldingObservation) {
        let asset = Asset::new(asset).expect("asset");
        let venue = VenueId::new(venue);
        let observation = HoldingObservation::new(
            venue.clone(),
            asset.clone(),
            Decimal::parse(observed).unwrap_or(Decimal::ZERO),
            Timestamp::from_secs(1_760_000_000),
            Provenance::Statement,
        );
        (VenueAsset { venue, asset }, observation)
    }

    #[test]
    fn exposure_at_a_venue_with_no_statement_is_named_and_is_not_called_a_shortfall() {
        // The finding that fires on every deployment today: the book has
        // traded at a counterparty and nobody has read a statement there.
        // It must not arrive as a margin call. An account nobody has looked
        // at reads identically to an empty one if it is given a domain with
        // no pledges, and "we are under-collateralised at XLON" is a
        // different instruction to a desk from "we have never seen XLON's
        // statement".
        let figures = aggregates(&[("AAA", "XNYS", "400000"), ("BBB", "XLON", "200000")]);
        assert_eq!(
            figures.axis_exposures()[COUNTERPARTY_AXIS].len(),
            2,
            "the premise is two counterparties on the axis"
        );
        let holdings = BTreeMap::new();
        let found = super::review(&holdings, &figures, &MarginModel::default(), Currency::USD)
            .expect("review");

        assert!(found.calls.is_empty(), "an unobserved venue was called");
        assert_eq!(found.coverage.len(), 0);
        assert_eq!(
            found
                .unobserved
                .iter()
                .map(|gap| gap.venue.to_string())
                .collect::<Vec<_>>(),
            vec!["XLON".to_string(), "XNYS".to_string()]
        );
        assert_eq!(found.unobserved_gross(), dec!("600000"));
        // A quarter of gross maintained, per the shipped model.
        assert_eq!(found.unobserved_maintenance(), dec!("150000"));
        assert!(found.is_finding());
        assert!(
            found.describe().contains("no statement"),
            "{}",
            found.describe()
        );
    }

    #[test]
    fn a_statement_covering_its_own_maintenance_leaves_the_venue_off_both_lists() {
        // The admitting half. Without it the unobserved finding would be a
        // gate that fires on every venue for ever, and a statement handed in
        // would change nothing a reader could see.
        let figures = aggregates(&[("AAA", "XNYS", "400000")]);
        let holdings: BTreeMap<_, _> = [statement("XNYS", "USD", "200000")].into_iter().collect();
        let found = super::review(&holdings, &figures, &MarginModel::default(), Currency::USD)
            .expect("review");

        assert!(
            found.unobserved.is_empty(),
            "a venue with a statement was reported unobserved"
        );
        assert!(found.calls.is_empty(), "{}", found.describe());
        assert_eq!(found.coverage.len(), 1);
        // Cash takes no haircut, so the whole 200,000 stands against the
        // 100,000 maintained.
        assert_eq!(found.cover_observed, dec!("200000"));
        assert_eq!(found.maintenance_required, dec!("100000"));
        assert!(!found.is_finding());
    }

    #[test]
    fn a_haircut_on_securities_can_turn_cover_that_looks_sufficient_into_a_call() {
        // 130,000 of securities against 100,000 of maintenance reads as
        // covered on notional and is a call after the haircut. This is the
        // whole reason cover is not notional, and a review that reported
        // notional would say the venue was fine.
        let figures = aggregates(&[("AAA", "XNYS", "400000")]);
        let holdings: BTreeMap<_, _> = [statement("XNYS", "AAA", "130000")].into_iter().collect();
        let found = super::review(&holdings, &figures, &MarginModel::default(), Currency::USD)
            .expect("review");

        assert_eq!(
            found.coverage.len(),
            1,
            "the premise is one observed domain"
        );
        assert_eq!(found.coverage[0].maintenance, dec!("100000"));
        assert_eq!(found.cover_observed, dec!("91000"));
        assert_eq!(found.calls, vec![VenueId::new("XNYS")]);

        // And the same securities at a mark that clears the haircut are not
        // a call, so the haircut is a haircut and not a refusal of every
        // non-cash holding.
        let generous: BTreeMap<_, _> = [statement("XNYS", "AAA", "150000")].into_iter().collect();
        let clear = super::review(&generous, &figures, &MarginModel::default(), Currency::USD)
            .expect("review");
        assert_eq!(clear.cover_observed, dec!("105000"));
        assert!(clear.calls.is_empty());
    }

    #[test]
    fn a_debit_balance_is_charged_as_an_obligation_and_never_netted_against_the_securities() {
        // A statement may be negative — a margin account in debit is a real
        // balance. Netting it against the securities in the same account
        // would understate the call by exactly twice the debit: the debit
        // would come off the cover instead of going onto the requirement.
        // 150,000 of securities is 105,000 after haircut; a 50,000 debit
        // makes the requirement 150,000, so the venue is called. Netted, the
        // cover would have read 55,000 against 100,000 — also a call, but
        // 45,000 short instead of 45,000 short of a larger number, and the
        // requirement a desk is told to post against would be wrong.
        let figures = aggregates(&[("AAA", "XNYS", "400000")]);
        let holdings: BTreeMap<_, _> = [
            statement("XNYS", "AAA", "150000"),
            statement("XNYS", "USD", "-50000"),
        ]
        .into_iter()
        .collect();
        let found = super::review(&holdings, &figures, &MarginModel::default(), Currency::USD)
            .expect("review");

        assert_eq!(found.coverage.len(), 1);
        assert_eq!(
            found.coverage[0].maintenance,
            dec!("150000"),
            "the debit did not reach the requirement"
        );
        assert_eq!(
            found.cover_observed,
            dec!("105000"),
            "the debit was netted against the cover"
        );
        assert_eq!(found.calls, vec![VenueId::new("XNYS")]);
        assert_eq!(found.maintenance_required, dec!("150000"));
    }

    #[test]
    fn a_book_that_has_traded_nowhere_and_holds_no_statement_produces_no_finding() {
        // A review of nothing must say nothing rather than a reassuring
        // zero. The counterpart to the unobserved finding: it fires because
        // there is exposure, not because the map is empty.
        let figures = RiskAggregates::new(dec!("1000000"), dec!("1000000")).expect("open");
        assert!(
            figures.axis_exposures().is_empty(),
            "the premise is a book with no fills"
        );
        let found = super::review(
            &BTreeMap::new(),
            &figures,
            &MarginModel::default(),
            Currency::USD,
        )
        .expect("review");
        assert!(!found.is_finding());
        assert_eq!(found.maintenance_required, Decimal::ZERO);
        assert_eq!(
            CrossMarginFinding::of(&found, 7, Timestamp::from_secs(1)),
            None
        );
    }

    #[test]
    fn the_journal_record_carries_the_venues_rather_than_only_the_totals() {
        // A record saying "600,000 unobserved" tells an operator to go
        // looking; a record naming XLON and XNYS tells them where. The
        // totals alone would make the entry unactionable from the log, which
        // is the one place a replay has.
        let figures = aggregates(&[("AAA", "XNYS", "400000"), ("BBB", "XLON", "200000")]);
        let found = super::review(
            &BTreeMap::new(),
            &figures,
            &MarginModel::default(),
            Currency::USD,
        )
        .expect("review");
        let finding = CrossMarginFinding::of(&found, 11, Timestamp::from_secs(1_760_000_000))
            .expect("a finding");
        assert_eq!(
            finding.unobserved_venues,
            vec!["XLON".to_string(), "XNYS".to_string()]
        );
        assert_eq!(finding.unobserved_gross, dec!("600000"));
        assert_eq!(
            finding.idempotency_key().as_deref(),
            Some("cross-margin:11")
        );
    }
}
