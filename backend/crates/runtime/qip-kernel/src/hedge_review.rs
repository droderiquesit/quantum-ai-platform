//! The production caller of `qip_risk::hedge`: blueprint §31.4's hedge sizing,
//! run once per cycle in DECIDE against the book the platform actually holds.
//!
//! `qip-risk`'s hedge engine is the arithmetic; this is the only thing that
//! feeds it, for the reason [`crate::shared_cause`] gives about its own level:
//! the inputs live here. The catalogue is the market view's, the exposures are
//! the risk state's, the price is the snapshot's, and none of the three
//! reaches `qip-risk`, which is a library with nothing beneath it.
//!
//! # What this composes, and what it refuses to invent
//!
//! The engine needs four things a policy does not carry. Each is taken from
//! the one place the platform already holds it, and where that place is empty
//! the survey **refuses** rather than filling in:
//!
//! * **Exposures.** [`exposures_of`] builds them from
//!   `RiskState::position_notionals` — signed notional per instrument, the
//!   same figures the limit checks and the liquidity ladder read — classified
//!   through the catalogue onto the axes a policy may name. Nothing is priced
//!   here: a notional is already money, so the axis figures need no marks and
//!   inherit none of `Platform`'s realised-only caveat beyond the one the
//!   aggregate already carries.
//! * **The hedge instrument's mechanics.** The contract multiplier and the lot
//!   size come from the catalogue record, never from configuration. Principle
//!   6: two independent claims about one fact disagree, and a futures contract
//!   whose configured multiplier drifted from its record would be hedged by
//!   that factor wrong — under-sized, which reads as a working hedge. So a
//!   declaration names the instrument by object id and nothing else about it,
//!   and an instrument the platform was not assembled over is
//!   [`HedgeRefusal::UnknownInstrument`], not a default of one.
//! * **The price.** `InstrumentState::reference_price` — a trade if there was
//!   one, else a mid, and never fabricated from the other. The catalogue's
//!   own `price` field is deliberately not a fallback: it is whatever the
//!   record said when it was loaded, and sizing against it would be the
//!   guessed price the engine's `UnusablePrice` arm exists to refuse.
//! * **The limits.** The live [`LimitSet`] and [`RiskState`], so the hedge is
//!   projected against the same bounds an order faces.
//!
//! # Why the terminus is the log and the cycle report
//!
//! A proposal produced here is **not** submitted, and no code path from this
//! module reaches a broker: it is journaled under [`Topic::RiskEvaluated`] and
//! reported on the DECIDE stage, which is where a person acts on it. That is
//! the whole of §31.4's "refusals that keep a hedge from becoming a position"
//! held structurally — the road from a hedge to a market is the proposal →
//! approval → governed-submit path every other order takes, behind pre-trade
//! risk, and this module has no way onto it. Wiring a hedge straight to a
//! venue would not be a shortcut; it would be an order nobody approved.
//!
//! # The empty case is a statement, not silence
//!
//! A deployment that declares no hedge policy hedges nothing, and
//! [`HedgeReview::summary`] says so on every cycle rather than falling silent.
//! The two states an operator must be able to tell apart are "no policy is
//! declared" and "policies are declared and the book is inside their
//! thresholds"; a stage that printed nothing for both would be the liquidity
//! floor's fail-open in a new place.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::Universe;
use qip_market::snapshot::MarketSnapshot;
use qip_portfolio::exposure::ExposureBreakdown;
use qip_risk::hedge::{
    HedgeAxis, HedgeExposures, HedgeInstrument, HedgeOutcome, HedgePolicy, HedgeProposal,
    HedgeRefusal, propose_hedge,
};
use qip_risk::limits::{LimitSet, RiskState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The bucket an instrument with no catalogue record is charged to, matching
/// `Portfolio::exposures`' own rule: an unclassified position still counts
/// toward the book, because dropping it would understate exposure.
const UNCLASSIFIED: &str = "unknown";

/// One hedge policy as a deployment commits it.
///
/// Deliberately **not** `qip_risk::hedge::HedgePolicy`: that type embeds a
/// fully specified [`HedgeInstrument`], multiplier and lot included, and a
/// configuration that restated those would be a second source of truth for
/// two numbers the catalogue already holds. What a deployment declares here
/// is judgement — which exposure, hedged with which instrument, at what beta,
/// toward what target — and [`HedgePolicyDeclaration::resolve`] fetches the
/// mechanics.
///
/// `beta` is declared and never estimated, for the argument
/// `qip_risk::hedge`'s header makes: an estimated beta becomes a doubled
/// position the day the correlation it was fitted on flips sign, and nobody
/// can audit a number no person wrote down. Nothing in this module reads a
/// price series.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgePolicyDeclaration {
    pub name: String,
    pub axis: HedgeAxis,
    /// The bucket on that axis. On [`HedgeAxis::Instrument`] this is an object
    /// id, because that is what the aggregate keys positions by; on the
    /// others it is the classification the catalogue states — a sector name,
    /// an ISO country code, a currency code, an issuer.
    pub bucket: String,
    /// The hedge instrument, by object id, resolved against the catalogue.
    pub instrument: ObjectId,
    /// Units of hedge notional that offset one unit of the named exposure.
    /// **Declared by a person.**
    pub beta: Decimal,
    #[serde(default)]
    pub target_net: Decimal,
    #[serde(default)]
    pub de_minimis: Decimal,
    #[serde(default)]
    pub rationale: String,
}

impl HedgePolicyDeclaration {
    pub fn new(
        name: impl Into<String>,
        axis: HedgeAxis,
        bucket: impl Into<String>,
        instrument: ObjectId,
        beta: Decimal,
    ) -> Self {
        Self {
            name: name.into(),
            axis,
            bucket: bucket.into(),
            instrument,
            beta,
            target_net: Decimal::ZERO,
            de_minimis: Decimal::ZERO,
            rationale: String::new(),
        }
    }

    pub fn with_target(mut self, target_net: Decimal) -> Self {
        self.target_net = target_net;
        self
    }

    pub fn with_de_minimis(mut self, de_minimis: Decimal) -> Self {
        self.de_minimis = de_minimis;
        self
    }

    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = rationale.into();
        self
    }

    /// The engine's policy, with the instrument's mechanics from the record.
    ///
    /// `Err` where the catalogue does not hold the instrument. Refusing is the
    /// only honest arm: a hedge sized at a multiplier of one in a contract
    /// whose record says fifty is under-sized fiftyfold, and an under-sized
    /// hedge reads exactly like a working one on every report the desk sees.
    fn resolve(&self, universe: &Universe) -> Result<HedgePolicy> {
        let record = universe.get(&self.instrument).ok_or_else(|| {
            Error::invalid(format!(
                "hedge policy {} names {} as its hedge instrument, which this platform was not \
                 assembled over and holds no contract multiplier or lot size for; add it to the \
                 catalogue this platform loads — its mechanics are not defaulted, because a \
                 contract hedged at a multiplier of one when its record says otherwise is \
                 under-sized by that factor and reads as a working hedge",
                self.name,
                self.instrument.as_str()
            ))
        })?;
        let instrument = HedgeInstrument {
            object_id: record.object_id.clone(),
            symbol: record.symbol.clone(),
            contract_multiplier: record.contract_multiplier,
            lot_size: record.lot_size,
        };
        Ok(
            HedgePolicy::new(self.name.clone(), self.axis, self.bucket.clone(), self.beta)
                .with_instrument(instrument)
                .with_target(self.target_net)
                .with_de_minimis(self.de_minimis)
                .with_rationale(self.rationale.clone()),
        )
    }
}

/// What one cycle's hedge survey found.
#[derive(Clone, Debug, PartialEq)]
pub struct HedgeReview {
    /// How many policies the deployment declared.
    pub declared: usize,
    /// One outcome per declared policy, in declaration order — a proposal, a
    /// stated no-action, or a refusal carrying its numbers. Empty only when
    /// nothing was declared or the exposures themselves were refused.
    pub outcomes: Vec<HedgeOutcome>,
    /// Why no policy could be surveyed at all, where the exposure read itself
    /// failed. Fails closed: no proposal is produced from a book this module
    /// could not describe.
    pub exposures_refused: Option<String>,
}

impl HedgeReview {
    /// The proposals, for the approval path and for the log.
    pub fn proposals(&self) -> Vec<&HedgeProposal> {
        self.outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                HedgeOutcome::Proposed(proposal) => Some(proposal.as_ref()),
                _ => None,
            })
            .collect()
    }

    /// The refusals, each already carrying the numbers it was refused on.
    pub fn refusals(&self) -> Vec<&HedgeRefusal> {
        self.outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                HedgeOutcome::Refused(refusal) => Some(refusal),
                _ => None,
            })
            .collect()
    }

    fn no_action(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome, HedgeOutcome::NoAction { .. }))
            .count()
    }

    /// Whether this cycle's survey is worth a record of its own.
    ///
    /// A proposal or a refusal, or an exposure read that failed. A cycle in
    /// which every declared policy found the book inside its threshold writes
    /// nothing, on [`crate::cross_margin::CrossMarginFinding`]'s rule: a
    /// record that says "nothing" every cycle is a record nobody reads, and
    /// the summary line on the stage already says it.
    pub fn is_finding(&self) -> bool {
        self.exposures_refused.is_some()
            || self
                .outcomes
                .iter()
                .any(|outcome| !matches!(outcome, HedgeOutcome::NoAction { .. }))
    }

    /// The line the DECIDE stage carries, never empty.
    pub fn summary(&self) -> String {
        if let Some(refusal) = &self.exposures_refused {
            return format!(
                "{} hedge policy(ies) declared and none surveyed: {refusal}",
                self.declared
            );
        }
        if self.declared == 0 {
            return "no hedge policy is declared, so no exposure is hedged".to_string();
        }
        format!(
            "{} hedge policy(ies) surveyed: {} proposed, {} refused, {} within threshold",
            self.declared,
            self.proposals().len(),
            self.refusals().len(),
            self.no_action()
        )
    }
}

/// The `reason` label on `qip_hedge_refusals_total` for a refusal the engine
/// or the resolver produced.
///
/// An exhaustive `match` over source-file literals, and both halves of that
/// are the cardinality bound rather than a comment asserting one: nothing
/// formats a string here, so no deployment's configuration can reach the
/// label, and a sixth [`HedgeRefusal`] arm will not compile until somebody
/// chooses its literal. A `Debug` rendering would have satisfied the first
/// half and not the second, and the variants carry policy names and instrument
/// symbols, which is exactly the unbounded label this refuses to publish.
pub(crate) fn refusal_reason(refusal: &HedgeRefusal) -> &'static str {
    match refusal {
        HedgeRefusal::NoInstrumentDeclared { .. } => "no_instrument_declared",
        HedgeRefusal::MisdeclaredPolicy { .. } => "misdeclared_policy",
        HedgeRefusal::UnknownInstrument { .. } => "unknown_instrument",
        HedgeRefusal::UnusablePrice { .. } => "unusable_price",
        HedgeRefusal::WouldBreachLimits { .. } => "would_breach_limits",
    }
}

/// The `reason` for the survey-wide refusal, where [`exposures_of`] failed and
/// so **no** policy was surveyed at all.
///
/// Not one of [`refusal_reason`]'s arms because it is not one policy's
/// refusal: it is the whole book being undescribable, which leaves every
/// declared exposure unhedged at once. An operator who saw it folded in with
/// the per-policy reasons would read one refusal where there are `declared` of
/// them.
pub(crate) const REASON_EXPOSURES_UNREADABLE: &str = "exposures_unreadable";

/// Survey every declared policy against the book as it stands.
///
/// Deterministic, like the engine it calls: the same state, catalogue,
/// snapshot, limits and instant produce the same outcomes to the digit.
/// Nothing here reads a clock or a model.
pub(crate) fn review(
    declarations: &[HedgePolicyDeclaration],
    universe: &Universe,
    snapshot: &MarketSnapshot,
    limits: &LimitSet,
    state: &RiskState,
    at: Timestamp,
) -> HedgeReview {
    if declarations.is_empty() {
        return HedgeReview {
            declared: 0,
            outcomes: Vec::new(),
            exposures_refused: None,
        };
    }
    let exposures = match exposures_of(state, universe) {
        Ok(exposures) => exposures,
        // Fail closed and refuse every policy, not only the axis that failed:
        // a producer that got its own walk wrong cannot vouch for the part it
        // had not reached. The same discipline `shared_cause::exposure_of`
        // applies, and for the same reason — a partial exposure read sizes a
        // hedge against a book nobody described.
        Err(error) => {
            return HedgeReview {
                declared: declarations.len(),
                outcomes: Vec::new(),
                exposures_refused: Some(error.message().to_string()),
            };
        }
    };

    let mut outcomes = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let policy = match declaration.resolve(universe) {
            Ok(policy) => policy,
            Err(error) => {
                outcomes.push(HedgeOutcome::Refused(HedgeRefusal::UnknownInstrument {
                    policy: declaration.name.clone(),
                    instrument: declaration.instrument.as_str().to_string(),
                    detail: error.message().to_string(),
                }));
                continue;
            }
        };
        // One price, for this policy's own instrument. A map rather than a
        // scalar because that is the engine's shape, and an absent key is the
        // engine's `UnusablePrice` refusal — which is the right answer for an
        // instrument the platform has seen no trade and no quote in.
        let mut prices = BTreeMap::new();
        if let Some(instrument) = &policy.instrument
            && let Some(price) = snapshot
                .get(&instrument.object_id)
                .and_then(|state| state.reference_price())
        {
            prices.insert(instrument.object_id.as_str().to_string(), price);
        }
        outcomes.push(propose_hedge(
            &policy, &exposures, &prices, limits, state, at,
        ));
    }

    HedgeReview {
        declared: declarations.len(),
        outcomes,
        exposures_refused: None,
    }
}

/// The book's exposures along every axis a hedge policy may name.
///
/// Built from signed position notionals and the catalogue, and from nothing
/// else. The instrument axis is the aggregate's own map; the five
/// classification axes come from the record; the factor axis scales the
/// notional by the record's loading.
///
/// **The factor axis is the one crossing from a statistic to money here**, and
/// it is refused rather than absorbed on both halves, exactly as
/// `Portfolio::exposures` refuses them: `Decimal::from_f64` for a loading that
/// is not a representable finite number, `checked_mul` for a product that does
/// not fit. Reading either as zero would report a factor the book is running
/// as flat — an exposure nobody hedges, which is precisely the thing a hedge
/// policy on that axis exists to hedge.
fn exposures_of(state: &RiskState, universe: &Universe) -> Result<HedgeExposures> {
    let mut breakdown = ExposureBreakdown::default();
    let mut by_instrument = qip_portfolio::exposure::Exposure::new();
    for (instrument, notional) in &state.position_notionals {
        by_instrument.add(instrument.clone(), *notional);
        let Some(record) = universe.get(&ObjectId::from_string(instrument.clone())) else {
            breakdown.by_asset_class.add(UNCLASSIFIED, *notional);
            continue;
        };
        breakdown
            .by_asset_class
            .add(record.asset_class.as_str(), *notional);
        breakdown.by_sector.add(record.sector.as_str(), *notional);
        breakdown.by_country.add(&record.geography, *notional);
        breakdown
            .by_currency
            .add(record.currency.as_str(), *notional);
        breakdown.by_issuer.add(
            record
                .issuer
                .clone()
                .unwrap_or_else(|| record.symbol.clone()),
            *notional,
        );
        for (factor, loading) in &record.risk.factor_exposures.loadings {
            let scaled = Decimal::from_f64(*loading).ok_or_else(|| {
                Error::invalid(format!(
                    "{instrument} states a loading of {loading} on factor {factor}, which is not \
                     a number a position notional can be multiplied by; correct the reference \
                     record — a factor read as flat is one no hedge policy can defend"
                ))
            })?;
            let contribution = scaled.checked_mul(*notional).ok_or_else(|| {
                Error::numeric(format!(
                    "{instrument} states a loading of {loading} on factor {factor}, and its \
                     contribution to a notional of {notional} is not representable; correct the \
                     reference record"
                ))
            })?;
            breakdown.by_factor.add(factor, contribution);
        }
    }
    Ok(HedgeExposures::new(breakdown).with_instruments(by_instrument))
}

/// The hedge survey, as the log carries it.
///
/// The whole outcome list, not a count: §31.4's value is that a person can
/// re-derive why a hedge was proposed or refused from the log alone, and every
/// outcome already carries its own numbers and its reasoning in words. Bounded
/// by the declared policy count, which is a deployment constant.
///
/// Filed under [`Topic::RiskEvaluated`] — the same topic
/// [`crate::cross_margin::CrossMarginFinding`] uses, and for the same reason:
/// a survey is a risk evaluation and not a decision. Nothing refuses an order
/// on it and nothing submits one from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeSurveyed {
    pub declared: usize,
    pub proposed: usize,
    pub refused: usize,
    pub outcomes: Vec<HedgeOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposures_refused: Option<String>,
    pub cycle: u64,
    pub at: Timestamp,
}

impl HedgeSurveyed {
    /// The record for a review, or `None` where there is nothing to say.
    pub fn of(review: &HedgeReview, cycle: u64, at: Timestamp) -> Option<Self> {
        review.is_finding().then(|| Self {
            declared: review.declared,
            proposed: review.proposals().len(),
            refused: review.refusals().len(),
            outcomes: review.outcomes.clone(),
            exposures_refused: review.exposures_refused.clone(),
            cycle,
            at,
        })
    }
}

impl EventBody for HedgeSurveyed {
    const TOPIC: Topic = Topic::RiskEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("hedge-survey:{}", self.cycle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::costs::LiquidityProfile;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::risk_profile::{FactorExposures, RiskCharacteristics};
    use qip_market::quote::{Trade, TradeCondition};

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn record(id: &str, sector: Sector) -> FinancialObject {
        FinancialObject::builder(
            ObjectId::from_string(id),
            id,
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0),
        )
        .venue("XNAS")
        .geography("US")
        .sector(sector)
        .provenance(Provenance::synthetic("catalogue", at()))
        .build(at())
        .expect("a listed equity record")
    }

    #[test]
    fn an_instrument_the_catalogue_does_not_hold_is_charged_to_the_unclassified_bucket() {
        // The rule `Portfolio::exposures` states: dropping the position would
        // understate the book, so it is bucketed rather than discarded. A
        // hedge sized against a book that quietly omitted a holding is sized
        // against a book nobody holds.
        let state = RiskState {
            position_notionals: BTreeMap::from([("obj-GHOST".to_string(), dec!("1000"))]),
            ..RiskState::default()
        };
        let exposures = exposures_of(&state, &Universe::new()).expect("no loadings to refuse");
        assert_eq!(
            exposures.along(HedgeAxis::Instrument).net_of("obj-GHOST"),
            dec!("1000"),
            "the premise: the instrument axis carries the holding"
        );
        assert_eq!(
            exposures.along(HedgeAxis::AssetClass).net_of(UNCLASSIFIED),
            dec!("1000")
        );
    }

    #[test]
    fn a_short_and_a_long_in_one_sector_net_on_the_sector_axis_and_not_on_the_instrument_axis() {
        // The sign convention is the whole basis of the hedge: `propose_hedge`
        // sells a positive excess and buys a negative one, so an axis that
        // summed magnitudes would hedge a flat book and put on a naked
        // position. Both instruments are in one sector on purpose.
        let mut universe = Universe::new();
        universe
            .insert(record("obj-AAA", Sector::InformationTechnology))
            .expect("insertable");
        universe
            .insert(record("obj-BBB", Sector::InformationTechnology))
            .expect("insertable");
        let state = RiskState {
            position_notionals: BTreeMap::from([
                ("obj-AAA".to_string(), dec!("3000")),
                ("obj-BBB".to_string(), dec!("-1000")),
            ]),
            ..RiskState::default()
        };
        let exposures = exposures_of(&state, &universe).expect("no loadings to refuse");
        assert_eq!(
            exposures.along(HedgeAxis::Instrument).net_of("obj-BBB"),
            dec!("-1000"),
            "the premise: the short is carried short"
        );
        assert_eq!(
            exposures
                .along(HedgeAxis::Sector)
                .net_of("information_technology"),
            dec!("2000"),
            "the sector axis must net, not sum magnitudes"
        );
    }

    #[test]
    fn a_review_with_no_declared_policy_says_so_rather_than_falling_silent() {
        let review = review(
            &[],
            &Universe::new(),
            &MarketSnapshot::new(at()),
            &LimitSet::new("wide"),
            &RiskState::default(),
            at(),
        );
        assert_eq!(review.declared, 0, "the premise: nothing was declared");
        assert!(!review.is_finding());
        assert_eq!(
            review.summary(),
            "no hedge policy is declared, so no exposure is hedged"
        );
    }

    /// The instrument the book holds, and whose record states the loading.
    const EXPOSED: &str = "obj-AAA";
    /// The instrument both policies hedge with. Its own record states no
    /// loading, so the refusals below are about the record under test.
    const HEDGE: &str = "obj-HDG";
    /// The factor one of the two policies names.
    const FACTOR: &str = "momentum";
    /// The sector the *other* policy names — an axis the factor walk never
    /// touches, which is the whole point of declaring it.
    const SECTOR: &str = "information_technology";

    /// The same record as [`record`], with one stated factor loading.
    ///
    /// `RiskCharacteristics::is_coherent` — the gate `Universe::insert` and
    /// `ObjectBuilder::build` both run — checks the volatility, the beta and
    /// the three fractions, and does **not** look at `factor_exposures` at
    /// all. So a loading of `NaN` is registrable, which is exactly why
    /// [`exposures_of`] has to refuse it downstream rather than trusting the
    /// catalogue gate to have done it.
    fn record_loading(id: &str, loading: f64) -> FinancialObject {
        FinancialObject::builder(
            ObjectId::from_string(id),
            id,
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0),
        )
        .venue("XNAS")
        .geography("US")
        .sector(Sector::InformationTechnology)
        .risk(RiskCharacteristics {
            factor_exposures: FactorExposures::new().with(FACTOR, loading),
            ..RiskCharacteristics::default()
        })
        .provenance(Provenance::synthetic("catalogue", at()))
        .build(at())
        .expect("a listed equity record; the coherence gate does not read loadings")
    }

    /// A catalogue over the same two instruments, differing only in the
    /// loading the held one states.
    fn catalogue(loading: f64) -> Universe {
        let mut universe = Universe::new();
        universe
            .insert(record_loading(EXPOSED, loading))
            .expect("a record with any loading is insertable");
        // Deliberately outside the hedged sector, so the hedge instrument does
        // not add to the exposure it is sized against.
        universe
            .insert(record(HEDGE, Sector::Financials))
            .expect("insertable");
        universe
    }

    /// A printed trade in the hedge instrument, so a proposal can be sized.
    /// Without it every policy would refuse on `UnusablePrice` and the admit
    /// half of each test below would prove nothing.
    fn priced() -> MarketSnapshot {
        let mut snapshot = MarketSnapshot::new(at());
        snapshot.apply_trade(Trade {
            object_id: ObjectId::from_string(HEDGE),
            venue: "XNAS".into(),
            at: at(),
            price: dec!("100"),
            size: Decimal::from_int(100),
            aggressor: None,
            condition: TradeCondition::Regular,
            trade_id: None,
            quality: Default::default(),
        });
        snapshot
    }

    /// Two policies: one on the factor axis the bad record poisons, one on a
    /// sector axis it never touches.
    fn two_policies() -> Vec<HedgePolicyDeclaration> {
        vec![
            HedgePolicyDeclaration::new(
                "factor-hedge",
                HedgeAxis::Factor,
                FACTOR,
                ObjectId::from_string(HEDGE),
                dec!("1"),
            ),
            HedgePolicyDeclaration::new(
                "sector-hedge",
                HedgeAxis::Sector,
                SECTOR,
                ObjectId::from_string(HEDGE),
                dec!("1"),
            ),
        ]
    }

    fn book(notional: Decimal) -> RiskState {
        RiskState {
            position_notionals: BTreeMap::from([(EXPOSED.to_string(), notional)]),
            ..RiskState::default()
        }
    }

    #[test]
    fn a_stated_loading_that_is_not_a_number_refuses_every_policy_and_not_only_the_factor_axis() {
        // The arm this drives had no test at all until this one: `review`'s
        // `Err` branch on `exposures_of` was reachable from a catalogue record
        // — `is_coherent` does not read loadings — and nothing proved it fired
        // or that it failed closed. A refusal nothing drives reads as
        // protection and is not; that is the `MaxExpectedShortfall` finding,
        // and this is the same shape.
        let snapshot = priced();
        let limits = LimitSet::new("wide");
        let state = book(dec!("3000"));

        // The admit half, and the premise. The same book, the same two
        // policies, the same prices — only the stated loading differs. A gate
        // that refused every catalogue would satisfy the refusal assertions
        // below and protect nothing.
        let admitted = review(
            &two_policies(),
            &catalogue(0.5),
            &snapshot,
            &limits,
            &state,
            at(),
        );
        assert!(
            admitted.exposures_refused.is_none(),
            "a well-formed catalogue was refused: {:?}",
            admitted.exposures_refused
        );
        assert_eq!(
            admitted.proposals().len(),
            2,
            "the premise: over a well-formed catalogue both policies propose, so the refusal \
             below is the loading and not an empty book: {:?}",
            admitted.outcomes
        );

        let refused = review(
            &two_policies(),
            &catalogue(f64::NAN),
            &snapshot,
            &limits,
            &state,
            at(),
        );
        assert_eq!(
            refused.declared, 2,
            "the premise: the same two policies were declared"
        );
        // Fail closed, and the word "every" is the assertion: the sector
        // policy names an axis the factor walk never reaches and proposed a
        // moment ago over the same book. A survey that produced its proposal
        // anyway would be sizing against a book this module could not finish
        // describing.
        assert!(
            refused.outcomes.is_empty(),
            "the survey produced an outcome over a book it could not read: {:?}",
            refused.outcomes
        );
        assert_eq!(refused.proposals().len(), 0);
        assert_eq!(refused.refusals().len(), 0);
        let message = refused
            .exposures_refused
            .clone()
            .expect("the exposure read was refused");
        // The right reason, not merely a refusal. The two arms of
        // `exposures_of` need opposite corrections and their messages are
        // deliberately disjoint: this phrase appears in the non-finite arm
        // only, and the overflow arm's "is not representable" appears in the
        // other only.
        assert!(
            message.contains("not a number a position notional can be multiplied by"),
            "the survey was refused for some other reason: {message}"
        );
        assert!(
            message.contains(EXPOSED) && message.contains(FACTOR),
            "the refusal names neither the record nor the factor a person must correct: {message}"
        );
        assert!(
            refused.is_finding(),
            "a book nobody could read is a finding"
        );
        assert!(
            refused
                .summary()
                .starts_with("2 hedge policy(ies) declared and none surveyed"),
            "the summary does not say that every declared exposure went unhedged: {}",
            refused.summary()
        );
    }

    #[test]
    fn a_loading_whose_product_with_the_notional_does_not_fit_refuses_the_survey_as_unrepresentable()
     {
        // The second half of `exposures_of`'s crossing from a statistic to
        // money, and a distinct arm from the one above: both operands are
        // individually representable and their product is not. Reading it as
        // zero would report a factor the book is running as flat, which is
        // precisely the exposure a factor policy exists to hedge.
        //
        // The fixture is a corrupt record rather than a plausible one, and
        // that is honest: a loading of 10^20 is not a number any estimator
        // produces. It is the number a mis-scaled or mis-parsed field carries,
        // which is the case this arm is for.
        const HUGE: f64 = 1e20;
        let notional = Decimal::from_int(1_000_000_000_000_000_000);
        let snapshot = priced();
        let limits = LimitSet::new("wide");
        let state = book(notional);

        // Premise one: the loading itself converts, so the refusal below
        // cannot be the non-finite arm firing under another name.
        assert!(
            Decimal::from_f64(HUGE).is_some(),
            "the fixture loading does not even convert, so this test would drive the other arm"
        );
        // Premise two: the same book with an ordinary loading is surveyed
        // without refusal, so what is refused is the product and not the size
        // of the position.
        let admitted = review(
            &two_policies(),
            &catalogue(1.0),
            &snapshot,
            &limits,
            &state,
            at(),
        );
        assert!(
            admitted.exposures_refused.is_none(),
            "the premise: this book is readable at an ordinary loading: {:?}",
            admitted.exposures_refused
        );

        let refused = review(
            &two_policies(),
            &catalogue(HUGE),
            &snapshot,
            &limits,
            &state,
            at(),
        );
        assert!(
            refused.outcomes.is_empty(),
            "the survey produced an outcome over a book it could not read: {:?}",
            refused.outcomes
        );
        let message = refused
            .exposures_refused
            .clone()
            .expect("the exposure read was refused");
        assert!(
            message.contains("is not representable"),
            "the overflow arm did not fire; this is some other refusal: {message}"
        );
        assert!(
            message.contains(EXPOSED) && message.contains(FACTOR),
            "the refusal names neither the record nor the factor a person must correct: {message}"
        );
    }

    #[test]
    fn a_policy_naming_an_instrument_the_catalogue_does_not_hold_is_refused_not_defaulted() {
        // The refusal that keeps a mis-sized hedge off the desk: a multiplier
        // defaulted to one on a contract whose record says fifty produces a
        // hedge under-sized fiftyfold, which reads on every report exactly
        // like a hedge that worked.
        let declarations = vec![HedgePolicyDeclaration::new(
            "tech-hedge",
            HedgeAxis::Sector,
            "technology",
            ObjectId::from_string("obj-ABSENT"),
            dec!("1"),
        )];
        let review = review(
            &declarations,
            &Universe::new(),
            &MarketSnapshot::new(at()),
            &LimitSet::new("wide"),
            &RiskState::default(),
            at(),
        );
        assert_eq!(review.declared, 1, "the premise: one policy was declared");
        assert_eq!(review.proposals().len(), 0);
        let refusals = review.refusals();
        assert_eq!(refusals.len(), 1);
        assert!(
            matches!(refusals[0], HedgeRefusal::UnknownInstrument { .. }),
            "an absent catalogue record must refuse, not default the mechanics"
        );
        assert!(review.is_finding());
    }
}
