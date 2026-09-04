//! The producer of the shipping payload's slot 8 — blueprint §41.5's cycle
//! whitelist — as the central plane can honestly fill it today.
//!
//! # Where the whitelist comes from
//!
//! `CycleWhitelist` gained structured `conversions` and `start_sizes` so the
//! edge node could turn the slot into an arbitrage graph
//! (`qip-edge-node/src/arbitrage.rs`), and nothing in the centre produced
//! either: every payload shipped the slot unproduced, so the desk installed
//! in deployment never. This module is the producer, and it is worth saying
//! plainly what it derives from and what it does not.
//!
//! The centre has no pair list. `qip-optimization-engine` solves portfolio
//! problems and routes solvers; it holds no cycle, path or whitelist type.
//! The DISCOVER and REASON stages carry opportunities on single instruments,
//! not conversions between them. The venue fee schedule lives at the edge
//! (`qip-routing`), below nothing this crate may depend on. The only venue
//! fact the centre holds is the venue list on each grant it issued. So the
//! whitelist is built from an explicit, operator-supplied
//! [`ArbitragePolicy`] on [`super::CentralConfig`] — validated when the plane
//! assembles, refused rather than repaired — joined to the one fact the
//! centre does own: the live capital grant the desk's strategy holds at each
//! cell, whose per-order limit sizes what a cycle may commit from the
//! funding instrument. An unset policy emits an empty whitelist, which is the
//! fail-closed state: the cell's installer reads an empty whitelist as
//! `Installation::EmptyWhitelist` and installs no desk.
//!
//! # What this is not
//!
//! Policy, never an order. The whitelist names which books the desk may
//! price, on which side, at what proportional cost, and how much of each
//! start instrument it may commit. Whether any cycle is taken is the cell's
//! decision, made alone against its own books (ADR 0008). Nothing here
//! prices anything: the cell re-quotes every edge from its books before
//! every scan, and the centre's placeholder rate of one is a value no scan
//! reads.
//!
//! # What refuses, and where
//!
//! A market naming a venue the policy does not describe is refused when the
//! plane assembles, naming the market. A policy venue the desk's grant does
//! not permit is refused at emission, naming the venue, and nothing is
//! emitted for that cell — the cell would refuse the whole whitelist for the
//! same reason (`graph_from_whitelist` checks every edge against its own
//! venue list), and refusing at the producer names the entry before it
//! travels. A cost outside `[0, 1)`, a conversion of an instrument into
//! itself, an unsized start instrument and a non-positive size are refused
//! here for the same reason: each is a refusal the cell already makes, and a
//! refusal made at the centre is one an operator sees at the console instead
//! of in a cell's delta stream.
//!
//! # Why slot 8 is the only one this module produces
//!
//! The audit is recorded here because the next reader of this file is the
//! next person asked to fill another slot, and every one of the remaining
//! nine looks producible until its input is read.
//!
//! Three of the nine — belief priors, the episodic digest and the causal
//! digest — are the three `PolicyItem::capability` maps to a §6.2
//! capability, which means producing one *relaxes* the cell rather than
//! informing it: `DegradationState::sizing_multiplier` stops narrowing when
//! the belief and causal slots read fresh, and `pauses` stops pausing
//! situational-recognition strategies when the episodic slot does. The
//! kernel holds no belief engine, no episodic store, and no causal edge —
//! `WorldModel::claim_causal` is called only by its own demo seed — so a
//! producer for any of them would ship an empty or relabelled value and buy
//! a wider position with it. Their unproduced state is the platform saying
//! it has no such capability, and that sentence is load-bearing.
//!
//! Of the other six: a model manifest names each model by content digest and
//! `ModelCard` carries no digest of any artifact, because weights are never
//! materialised here; the compiled plan's digest must match bytes an edge
//! node reads from a file the centre is never handed; inventory targets are
//! exact quantities and the centre holds realised books, not targets, and
//! holds no marks to price them with; the feasibility constraints are per
//! venue while the only tick this platform states is per instrument, and a
//! catalogue that omits a tick is indistinguishable from one that states the
//! builder's default, so signing it would be signing a default as a venue's
//! grid; there is no adversary monitor at all. The regime is the closest —
//! the tape statistics are real and drive routing every cycle — but
//! `Platform::market_regime` is per subject and answers `Crisis` from this
//! process's own drawdown, so shipping it per cell would sign a house state
//! as a market state, and `RegimeState::confidence` has no source that is a
//! confidence rather than a classifier's input.
//!
//! An unproduced slot narrows the cell and says why. A produced one asserts
//! a fact, and a slot asserting a number nobody computed is worse than the
//! gap it closes.

use qip_contracts::CapitalEnvelope;
use qip_contracts::message::BookSide;
use qip_contracts::policy::{CycleWhitelist, WhitelistedConversion};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most markets one policy may name.
///
/// Every market becomes two conversions, and the cell walks at most 256
/// conversions per pass (`qip-edge-node`'s `MAX_CONVERSIONS`). The cell's
/// bound governs; this one only surfaces the refusal where the policy is
/// written rather than where it is applied.
pub const MAX_MARKETS: usize = 128;

/// One venue the desk may trade at, as the policy describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhitelistedVenue {
    /// What the venue is, for the planner's settlement assumptions.
    pub class: VenueClass,
    /// Proportional cost of taking a conversion here, as a fraction in
    /// `[0, 1)`. Exact, because it is charged against money.
    pub taker_cost: Decimal,
}

/// One book the desk may price, named by its own instrument id at its venue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhitelistedMarket {
    /// A key of [`ArbitragePolicy::venues`].
    pub venue: String,
    /// The book's own instrument id — not either instrument on it, since a
    /// venue quoting one against several has several books.
    pub market: String,
    /// The instrument the book is priced in units of.
    pub base: String,
    /// The instrument the book is priced against.
    pub quote: String,
}

/// The operator's statement of what the arbitrage desk may price.
///
/// Set on [`super::CentralConfig::arbitrage`]; `None` there emits an empty
/// whitelist. Held as data rather than derived so that what the desk was
/// permitted is reproducible from the configuration and the journal alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitragePolicy {
    /// The desk's strategy, whose grant funds it. The same name the edge
    /// node's installer is configured with; a grant for any other strategy
    /// funds no desk there.
    pub strategy: StrategyId,
    /// The instrument the grant is denominated in. Its start size is the
    /// grant's per-order limit, never a policy value: the grant is the one
    /// authority on how much may be committed, and a second number here would
    /// be a second claim about the same fact.
    pub funding_instrument: String,
    /// Every venue a market may name, keyed by venue id.
    pub venues: BTreeMap<String, WhitelistedVenue>,
    /// The books, in the order the desk's conversions will be listed.
    pub markets: Vec<WhitelistedMarket>,
    /// How much of each non-funding instrument a cycle may commit, in that
    /// instrument's own units. The centre holds no price to convert a grant
    /// into these, so they are stated.
    pub start_sizes: BTreeMap<String, Decimal>,
}

impl ArbitragePolicy {
    /// Refuse a policy the cell would refuse, or one that says less than it
    /// appears to.
    pub fn validate(&self) -> Result<()> {
        if self.strategy.as_str().is_empty() {
            return Err(Error::invalid(
                "CentralConfig::arbitrage names no strategy; the desk's grant is looked up by \
                 it, so name the strategy the edge node installs the desk under",
            ));
        }
        if self.funding_instrument.is_empty() {
            return Err(Error::invalid(
                "CentralConfig::arbitrage names no funding instrument; the grant's order limit \
                 sizes it, so name the instrument the grant is denominated in",
            ));
        }
        if self.markets.is_empty() {
            return Err(Error::invalid(
                "CentralConfig::arbitrage names no market; a policy that permits nothing should \
                 be left unset, which emits an empty whitelist and installs no desk",
            ));
        }
        if self.markets.len() > MAX_MARKETS {
            return Err(Error::invalid(format!(
                "CentralConfig::arbitrage names {} markets and a cell walks at most {} \
                 conversions per pass, two per market",
                self.markets.len(),
                MAX_MARKETS * 2
            )));
        }
        for (venue, terms) in &self.venues {
            if venue.is_empty() {
                return Err(Error::invalid(
                    "CentralConfig::arbitrage describes a venue with an empty id",
                ));
            }
            if terms.taker_cost.is_negative() || terms.taker_cost >= Decimal::ONE {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage gives venue {venue} a taker cost of {}, outside \
                     [0, 1); a cost is a fraction of notional",
                    terms.taker_cost
                )));
            }
        }

        let mut books = BTreeSet::new();
        let mut instruments = BTreeSet::new();
        let mut venues_used = BTreeSet::new();
        for (position, market) in self.markets.iter().enumerate() {
            if market.market.is_empty() || market.base.is_empty() || market.quote.is_empty() {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage market {position} leaves its market, base or \
                     quote empty"
                )));
            }
            if !self.venues.contains_key(&market.venue) {
                return Err(Error::denied(format!(
                    "CentralConfig::arbitrage market {position} ({} at {}) names a venue the \
                     policy does not describe; describe it under `venues` with its class and \
                     taker cost, or drop the market",
                    market.market, market.venue
                )));
            }
            if market.base == market.quote {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage market {position} ({} at {}) converts {} into \
                     itself",
                    market.market, market.venue, market.base
                )));
            }
            if !books.insert((market.venue.clone(), market.market.clone())) {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage lists {} at {} twice; a book is one edge pair, \
                     and a second listing would be a second claim about the same book",
                    market.market, market.venue
                )));
            }
            venues_used.insert(market.venue.clone());
            instruments.insert(market.base.clone());
            instruments.insert(market.quote.clone());
        }
        if let Some(unused) = self
            .venues
            .keys()
            .find(|venue| !venues_used.contains(*venue))
        {
            return Err(Error::invalid(format!(
                "CentralConfig::arbitrage describes venue {unused} and no market names it; a \
                 venue nothing trades at is a permission nobody checks, so drop it"
            )));
        }
        if !instruments.contains(&self.funding_instrument) {
            return Err(Error::invalid(format!(
                "CentralConfig::arbitrage funds the desk in {} and no market trades it; a \
                 cycle cannot start from an instrument no edge leaves",
                self.funding_instrument
            )));
        }
        if self.start_sizes.contains_key(&self.funding_instrument) {
            return Err(Error::invalid(format!(
                "CentralConfig::arbitrage sizes {} itself; the funding instrument is sized by \
                 the grant's order limit, and a second size here would disagree with it",
                self.funding_instrument
            )));
        }
        for (instrument, size) in &self.start_sizes {
            if !size.is_positive() {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage sizes {instrument} at {size}; a start size must \
                     be positive, or the cell refuses every cycle from it"
                )));
            }
            if !instruments.contains(instrument) {
                return Err(Error::invalid(format!(
                    "CentralConfig::arbitrage sizes {instrument} and no market trades it; a \
                     size nothing can commit is a number nobody computed"
                )));
            }
        }
        if let Some(missing) = instruments.iter().find(|instrument| {
            **instrument != self.funding_instrument && !self.start_sizes.contains_key(*instrument)
        }) {
            return Err(Error::invalid(format!(
                "CentralConfig::arbitrage trades {missing} and gives it no start size; every \
                 conversion leaves an instrument, so every instrument is a start, and the cell \
                 refuses a whitelist that leaves one unsized"
            )));
        }
        Ok(())
    }

    /// Every venue the policy trades at, in id order.
    pub fn venue_ids(&self) -> impl Iterator<Item = VenueId> + '_ {
        self.venues.keys().map(|venue| VenueId::new(venue.as_str()))
    }

    /// The whitelist this policy describes, sized against the desk's grant.
    ///
    /// Refuses a grant for another strategy, a grant that does not permit a
    /// policy venue, and a grant whose order limit is not positive, because
    /// each would produce a whitelist the cell refuses whole and the refusal
    /// belongs where the entry is made.
    pub fn whitelist_for(&self, envelope: &CapitalEnvelope) -> Result<CycleWhitelist> {
        if envelope.strategy() != &self.strategy {
            return Err(Error::guard(format!(
                "the arbitrage desk {} cannot be sized against a grant for {}",
                self.strategy,
                envelope.strategy()
            )));
        }
        if !envelope.order_limit().is_positive() {
            return Err(Error::invalid(format!(
                "the grant for {} at {} permits no order, so the desk has no start size in {} \
                 and no whitelist is emitted",
                self.strategy,
                envelope.cell(),
                self.funding_instrument
            )));
        }
        let mut conversions = Vec::with_capacity(self.markets.len() * 2);
        for market in &self.markets {
            let venue = VenueId::new(market.venue.as_str());
            if !envelope.permits_venue(&venue) {
                return Err(Error::denied(format!(
                    "CentralConfig::arbitrage trades {} at {} and the grant for {} at {} does \
                     not permit that venue; the cell would refuse the whole whitelist, so none \
                     is emitted",
                    market.market,
                    market.venue,
                    self.strategy,
                    envelope.cell()
                )));
            }
            let terms = self.venues.get(&market.venue).ok_or_else(|| {
                Error::guard(format!(
                    "CentralConfig::arbitrage market {} names venue {} the validated policy \
                     does not describe",
                    market.market, market.venue
                ))
            })?;
            // A book yields two edges: buying the base consumes the asks and
            // is a conversion out of the quote; selling it consumes the bids
            // and is a conversion out of the base. `WhitelistedConversion`
            // documents the same convention: `Ask` buys `to`, `Bid` sells
            // `from`.
            conversions.push(WhitelistedConversion {
                venue: market.venue.clone(),
                venue_class: terms.class,
                market: market.market.clone(),
                from: market.quote.clone(),
                to: market.base.clone(),
                side: BookSide::Ask,
                cost_fraction: terms.taker_cost,
            });
            conversions.push(WhitelistedConversion {
                venue: market.venue.clone(),
                venue_class: terms.class,
                market: market.market.clone(),
                from: market.base.clone(),
                to: market.quote.clone(),
                side: BookSide::Bid,
                cost_fraction: terms.taker_cost,
            });
        }
        let mut start_sizes = self.start_sizes.clone();
        start_sizes.insert(self.funding_instrument.clone(), envelope.order_limit());
        Ok(CycleWhitelist {
            // No cycle ids and no path assignment: the centre enumerates no
            // cycles — the cell's candidate index does, from these edges —
            // and nothing at the cell reads this map. A name invented here
            // would be a value computed and ignored.
            cycles: BTreeMap::new(),
            conversions,
            start_sizes,
        })
    }
}

/// Why a cell's whitelist carries what it carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhitelistOutcome {
    /// `CentralConfig::arbitrage` is unset, so the whitelist is empty.
    NoPolicy,
    /// The desk's strategy holds no live grant at this cell, so nothing
    /// sizes the funding instrument and the whitelist is empty. The cell's
    /// installer would decline with no envelope regardless.
    NoLiveGrant { strategy: StrategyId },
    /// Emitted, with this many trade edges, sized against the grant whose
    /// signature this is.
    Emitted { edges: usize, sized_against: String },
}

/// One cell's slot 8, and why — the record the journal keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhitelistIssue {
    pub cell: String,
    pub issued_at: Timestamp,
    pub whitelist: CycleWhitelist,
    pub outcome: WhitelistOutcome,
}

impl WhitelistIssue {
    /// Whether the cell will install nothing from this.
    pub fn is_empty(&self) -> bool {
        self.whitelist.conversions.is_empty()
    }

    /// The line an operator reads.
    pub fn describe(&self) -> String {
        match &self.outcome {
            WhitelistOutcome::NoPolicy => format!(
                "cycle whitelist for {}: empty, CentralConfig::arbitrage is unset",
                self.cell
            ),
            WhitelistOutcome::NoLiveGrant { strategy } => format!(
                "cycle whitelist for {}: empty, {strategy} holds no live grant there",
                self.cell
            ),
            WhitelistOutcome::Emitted {
                edges,
                sized_against,
            } => format!(
                "cycle whitelist for {}: {edges} trade edge(s) across {} venue(s), sized \
                 against grant {sized_against}",
                self.cell,
                self.whitelist
                    .conversions
                    .iter()
                    .map(|conversion| conversion.venue.as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
            ),
        }
    }
}

impl EventBody for WhitelistIssue {
    // What the centre distributed as policy, recorded whether or not it was
    // anything: an empty whitelist shipped every few minutes is a fact an
    // operator asking "why does the desk never install" needs to find.
    const TOPIC: Topic = Topic::PolicyDistributed;
    const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_core::time::Duration;

    fn policy() -> ArbitragePolicy {
        ArbitragePolicy {
            strategy: StrategyId::new("arb-desk"),
            funding_instrument: "USD".to_string(),
            venues: BTreeMap::from([
                (
                    "XNYS".to_string(),
                    WhitelistedVenue {
                        class: VenueClass::Exchange,
                        taker_cost: dec!("0.0005"),
                    },
                ),
                (
                    "XLON".to_string(),
                    WhitelistedVenue {
                        class: VenueClass::Exchange,
                        taker_cost: dec!("0.001"),
                    },
                ),
            ]),
            markets: vec![
                WhitelistedMarket {
                    venue: "XNYS".to_string(),
                    market: "AAA-USD@XNYS".to_string(),
                    base: "AAA".to_string(),
                    quote: "USD".to_string(),
                },
                WhitelistedMarket {
                    venue: "XLON".to_string(),
                    market: "AAA-USD@XLON".to_string(),
                    base: "AAA".to_string(),
                    quote: "USD".to_string(),
                },
            ],
            start_sizes: BTreeMap::from([("AAA".to_string(), dec!("100"))]),
        }
    }

    fn grant(venues: &[&str], order_limit: Decimal) -> CapitalEnvelope {
        let at = Timestamp::from_secs(1_760_000_000);
        CapitalEnvelope::new(
            StrategyId::new("arb-desk"),
            "cell-lon-1",
            dec!("500000"),
            order_limit,
            dec!("50000"),
            venues.iter().map(|venue| VenueId::new(*venue)).collect(),
            at,
            at.saturating_add(Duration::from_hours(8)),
            "alice.chen",
            "sig-arb",
        )
        .expect("a well-formed grant")
    }

    #[test]
    fn a_market_yields_one_conversion_out_of_each_of_its_instruments() {
        let policy = policy();
        policy.validate().expect("the fixture policy is valid");
        let whitelist = policy
            .whitelist_for(&grant(&["XNYS", "XLON"], dec!("25000")))
            .expect("two permitted venues emit");
        assert_eq!(whitelist.conversions.len(), 4);
        let first = &whitelist.conversions[0];
        assert_eq!((first.from.as_str(), first.to.as_str()), ("USD", "AAA"));
        assert_eq!(first.side, BookSide::Ask);
        let second = &whitelist.conversions[1];
        assert_eq!((second.from.as_str(), second.to.as_str()), ("AAA", "USD"));
        assert_eq!(second.side, BookSide::Bid);
        assert_eq!(whitelist.conversions[2].cost_fraction, dec!("0.001"));
        // The funding size is the grant's, not the policy's.
        assert_eq!(whitelist.start_sizes.get("USD"), Some(&dec!("25000")));
        assert_eq!(whitelist.start_sizes.get("AAA"), Some(&dec!("100")));
        assert!(whitelist.cycles.is_empty());
    }

    #[test]
    fn a_policy_that_sizes_its_own_funding_instrument_is_refused() {
        let mut policy = policy();
        policy.start_sizes.insert("USD".to_string(), dec!("1"));
        let error = policy.validate().expect_err("two claims about one size");
        assert!(error.to_string().contains("order limit"), "{error}");
    }

    #[test]
    fn an_unsized_instrument_is_refused_before_the_cell_sees_it() {
        let mut policy = policy();
        policy.start_sizes.clear();
        let error = policy.validate().expect_err("AAA has no size");
        assert!(error.to_string().contains("AAA"), "{error}");
    }

    #[test]
    fn a_cost_of_one_or_more_is_refused() {
        let mut policy = policy();
        if let Some(venue) = policy.venues.get_mut("XNYS") {
            venue.taker_cost = Decimal::ONE;
        }
        let error = policy.validate().expect_err("a whole-notional cost");
        assert!(error.to_string().contains("[0, 1)"), "{error}");
    }

    #[test]
    fn a_venue_nothing_trades_at_is_refused() {
        let mut policy = policy();
        policy.venues.insert(
            "XPAR".to_string(),
            WhitelistedVenue {
                class: VenueClass::Exchange,
                taker_cost: Decimal::ZERO,
            },
        );
        let error = policy.validate().expect_err("XPAR has no market");
        assert!(error.to_string().contains("XPAR"), "{error}");
    }

    #[test]
    fn a_grant_for_another_strategy_cannot_size_the_desk() {
        let policy = policy();
        let at = Timestamp::from_secs(1_760_000_000);
        let other = CapitalEnvelope::new(
            StrategyId::new("momentum"),
            "cell-lon-1",
            dec!("500000"),
            dec!("25000"),
            dec!("50000"),
            vec![VenueId::new("XNYS"), VenueId::new("XLON")],
            at,
            at.saturating_add(Duration::from_hours(8)),
            "alice.chen",
            "sig-other",
        )
        .expect("a well-formed grant");
        let error = policy
            .whitelist_for(&other)
            .expect_err("a momentum grant funds no desk");
        assert!(error.to_string().contains("momentum"), "{error}");
    }
}
