//! Shared machinery for the specialist agents.
//!
//! Eighteen agents written independently drift. These helpers keep the parts
//! that must not drift in one place: how a finding is assembled, how missing
//! data is reported, and how a conviction is derived from a z-score.
//!
//! The conviction mapping is the one worth reading. Every agent needs to turn
//! "this is 2.3 standard deviations from normal" into a number in `[0, 1]`, and
//! if each invented its own the same evidence would carry different weight
//! depending on which desk happened to look at it.
//!
//! The fact constructors are the other part that must not drift. A number an
//! agent reports is either *observed* — read off a record the desk holds — or
//! *computed* from named inputs, and the audit trail is only worth reading if
//! the two are told apart. For a long time this module offered only
//! [`computed`], so every price and rate an agent read straight from the
//! feature store or the order book was recorded as computed by the agent from
//! itself, and no agent-reported number ever named a source or a record. The
//! observed constructors here take the record rather than a source string, so
//! an observed fact cannot be built without the record it claims to have been
//! read from, and its value is a projection of that record rather than a
//! number the author typed in beside it.

use qip_agents::finding::{AgentFinding, Direction, NumericFact, NumericProvenance};
use qip_agents::runtime::AgentContext;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_events::EventBody;
use qip_market::book::OrderBook;
use qip_numerics::stats;
use qip_world_model::features::{FeatureStore, FeatureValue};

/// Turn a standardised deviation into a conviction in `[0, 1]`.
///
/// `tanh(|z| / 2)` reaches 0.46 at one sigma, 0.76 at two and 0.90 at three,
/// and never saturates. The alternative — a normal CDF — hits 0.999 at three
/// sigma, which would let a single three-sigma reading produce near-certainty
/// out of what is, in fat-tailed financial data, a monthly occurrence.
pub fn conviction_from_z(z: f64) -> f64 {
    if !z.is_finite() {
        return 0.0;
    }
    (z.abs() / 2.0).tanh().clamp(0.0, 1.0)
}

/// Direction implied by a signed reading, with a dead zone around zero.
///
/// Readings inside the dead zone are `Neutral` rather than a weak view: an
/// agent reporting a direction it does not hold pollutes the aggregate.
pub fn direction_from(value: f64, dead_zone: f64) -> Direction {
    if !value.is_finite() || value.abs() <= dead_zone {
        return Direction::Neutral;
    }
    if value > 0.0 {
        Direction::Positive
    } else {
        Direction::Negative
    }
}

/// Standardise the last observation against its own history.
///
/// Returns `None` rather than zero when there is not enough history or the
/// series does not vary: a z-score of zero would read as "perfectly normal",
/// which is a different claim from "cannot tell".
pub fn z_score_of_last(series: &[f64], minimum: usize) -> Option<f64> {
    if series.len() < minimum {
        return None;
    }
    let (head, last) = series.split_at(series.len() - 1);
    let sigma = stats::stddev(head);
    if !sigma.is_finite() || sigma < 1e-12 {
        return None;
    }
    let z = (last[0] - stats::mean(head)) / sigma;
    z.is_finite().then_some(z)
}

/// Robust variant, standardising on the median absolute deviation.
///
/// Preferred where a single outlier would otherwise inflate the denominator
/// and hide the very move the agent is looking for.
pub fn robust_z_score_of_last(series: &[f64], minimum: usize) -> Option<f64> {
    if series.len() < minimum {
        return None;
    }
    let (head, last) = series.split_at(series.len() - 1);
    let mad = stats::median_absolute_deviation(head);
    if !mad.is_finite() || mad < 1e-12 {
        return None;
    }
    // 1.4826 scales the MAD to a standard deviation for normal data.
    let z = (last[0] - stats::median(head)) / (1.4826 * mad);
    z.is_finite().then_some(z)
}

/// Assemble a finding, filling in the fields every agent must supply.
#[derive(Debug)]
pub struct FindingBuilder {
    finding: AgentFinding,
}

impl FindingBuilder {
    pub fn new(ctx: &AgentContext, as_of: Timestamp, claim: impl Into<String>) -> Self {
        Self {
            finding: AgentFinding::new(
                ctx.run_id().clone(),
                ctx.manifest().id.clone(),
                ctx.now(),
                as_of,
                claim,
            ),
        }
    }

    pub fn direction(mut self, direction: Direction, conviction: f64) -> Self {
        self.finding = self.finding.with_direction(direction, conviction);
        self
    }

    pub fn fact(mut self, fact: NumericFact) -> Self {
        self.finding = self.finding.with_fact(fact);
        self
    }

    pub fn evidence(mut self, evidence: Vec<String>) -> Self {
        self.finding = self.finding.with_evidence(evidence);
        self
    }

    pub fn falsifiers(mut self, falsifiers: Vec<String>) -> Self {
        self.finding = self.finding.with_falsifiers(falsifiers);
        self
    }

    pub fn caveats(mut self, caveats: Vec<String>) -> Self {
        self.finding = self.finding.with_caveats(caveats);
        self
    }

    pub fn missing(mut self, missing: Vec<String>) -> Self {
        self.finding = self.finding.with_missing_inputs(missing);
        self
    }

    pub fn follow_ups(mut self, follow_ups: Vec<String>) -> Self {
        self.finding.follow_ups = follow_ups;
        self
    }

    pub fn build(self) -> Result<AgentFinding> {
        self.finding.validate()?;
        Ok(self.finding)
    }
}

/// A finding recording that the agent had nothing to work with.
///
/// Used wherever the data an agent needs is absent. Reporting this rather than
/// erroring keeps a missing feed from taking down the whole cycle, and reporting
/// it rather than returning a neutral view keeps it out of the aggregate.
pub fn no_data(ctx: &AgentContext, as_of: Timestamp, reason: impl Into<String>) -> AgentFinding {
    AgentFinding::no_view(
        ctx.run_id().clone(),
        ctx.manifest().id.clone(),
        ctx.now(),
        as_of,
        reason,
    )
}

/// A finding recording that the question was outside the agent's competence.
pub fn out_of_scope(
    ctx: &AgentContext,
    as_of: Timestamp,
    reason: impl Into<String>,
) -> AgentFinding {
    AgentFinding::deferred(
        ctx.run_id().clone(),
        ctx.manifest().id.clone(),
        ctx.now(),
        as_of,
        reason,
    )
}

/// The computed-fact constructor, with the agent's own name as the producer.
///
/// For a number the agent derived. A number the agent merely *read* — the
/// latest spread, an implied volatility, the touch of the book — belongs to
/// [`observed_feature`] or [`observed_book`]; stamping it computed records
/// the agent as the origin of a value it had no hand in producing.
pub fn computed(
    ctx: &AgentContext,
    label: &str,
    value: f64,
    unit: &str,
    inputs: &[&str],
) -> NumericFact {
    NumericFact::computed(
        label,
        value,
        unit,
        ctx.manifest().id.clone(),
        inputs.iter().map(|s| (*s).to_string()).collect(),
    )
}

/// A fact read from the feature store, stamped with where it came from.
///
/// The value is taken from the record, not passed alongside it, so the number
/// reported is the number the store holds. The source is the feature's
/// declared producer, the as-of is the instant the value describes, and the
/// record id carries both bitemporal instants so the exact read can be
/// repeated with [`FeatureStore::value_as_of`].
///
/// An imputed value was not observed by anyone, so it is refused the observed
/// stamp and recorded as computed by the producer from the record instead. A
/// helper that stamped it observed because the caller asked for an observed
/// fact would be the provenance lie this module exists to prevent.
pub fn observed_feature(
    features: &FeatureStore,
    name: &str,
    subject: &str,
    label: &str,
    unit: &str,
    value: &FeatureValue,
) -> NumericFact {
    let producer = features
        .definition(name)
        .map_or("undeclared-producer", |definition| {
            definition.producer.as_str()
        });
    let source = format!("feature-store:{producer}");
    let record_id = format!(
        "feature:{name}@{subject}:valid={}:known={}",
        value.valid_at, value.available_at
    );
    let provenance = if value.imputed {
        NumericProvenance::computed(source, vec![record_id])
    } else {
        NumericProvenance::observed(source, value.valid_at, record_id)
    };
    NumericFact {
        label: label.to_string(),
        value: value.value,
        unit: unit.to_string(),
        provenance,
    }
}

/// A fact read off an order book, stamped with the venue and the book's own
/// timestamp.
///
/// `field` selects what was read — the best bid, the best ask — and may find
/// nothing on an empty side, in which case there is no fact rather than a
/// fact about nothing. What it must not do is arithmetic: a mid or a spread
/// is computed from the touch, and belongs to [`computed`] with the touch
/// named as its inputs.
///
/// The record id is the venue's sequence number where one was assigned. A
/// venue that assigns none leaves the timestamp as the only handle, and the
/// id says so rather than inventing a sequence.
pub fn observed_book(
    label: &str,
    unit: &str,
    book: &OrderBook,
    field: impl FnOnce(&OrderBook) -> Option<Decimal>,
) -> Option<NumericFact> {
    let read = field(book)?;
    let record_id = book
        .idempotency_key()
        .unwrap_or_else(|| format!("{}:{}:unsequenced@{}", book.object_id, book.venue, book.at));
    Some(NumericFact {
        label: label.to_string(),
        // The one place a book price crosses from `Decimal` to the `f64` a
        // fact carries: facts are statistics, not money, and are never used
        // to settle anything.
        value: read.to_f64(),
        unit: unit.to_string(),
        provenance: NumericProvenance::observed(format!("book:{}", book.venue), book.at, record_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ids::ObjectId;
    use qip_market::book::BookLevel;
    use qip_world_model::features::Feature;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn a_feature_value_read_from_the_store_is_observed_from_its_declared_producer() {
        // The defect this guards: a spread read straight from the store was
        // recorded as computed by the agent from a label naming itself, so
        // the audit trail named no source and no record for any number.
        let mut features = FeatureStore::new();
        features.define(Feature::new(
            "credit_spread_bps",
            "issuer spread",
            "credit-provider",
        ));
        let valid = at();
        let known = at().saturating_add(qip_core::Duration::from_hours(6));
        features.record(
            "credit_spread_bps",
            "obj-1",
            FeatureValue::new(150.0, valid, known),
        );
        let value = features
            .value_as_of("credit_spread_bps", "obj-1", known, known)
            .expect("recorded");

        let fact = observed_feature(
            &features,
            "credit_spread_bps",
            "obj-1",
            "spread",
            "bps",
            value,
        );
        assert!(fact.validate().is_ok());
        assert!((fact.value - 150.0).abs() < f64::EPSILON, "{}", fact.value);
        match &fact.provenance {
            NumericProvenance::Observed {
                source,
                as_of,
                record_id,
            } => {
                assert_eq!(source, "feature-store:credit-provider");
                assert_eq!(*as_of, valid, "as-of is the instant the value describes");
                assert!(
                    record_id.contains(&format!("known={known}")),
                    "the record id must carry the knowable instant so the read can be repeated: {record_id}"
                );
            }
            other => panic!("a value read from the store must be observed, got {other:?}"),
        }
    }

    #[test]
    fn an_imputed_feature_value_is_refused_the_observed_stamp() {
        // Nobody observed an imputed value. Stamping it observed because the
        // caller reached for the observed constructor would launder a model's
        // fill-in as a measurement.
        let mut features = FeatureStore::new();
        features.define(Feature::new("policy_rate", "rate", "macro-provider"));
        features.record(
            "policy_rate",
            "global",
            FeatureValue::new(0.03, at(), at()).imputed(),
        );
        let value = features
            .value_as_of("policy_rate", "global", at(), at())
            .expect("recorded");
        assert!(value.imputed, "premise: the fixture value is imputed");

        let fact = observed_feature(&features, "policy_rate", "global", "rate", "ratio", value);
        assert!(fact.validate().is_ok());
        match &fact.provenance {
            NumericProvenance::Computed { by, inputs } => {
                assert_eq!(by, "feature-store:macro-provider");
                assert_eq!(inputs.len(), 1, "{inputs:?}");
            }
            other => panic!("an imputed value must not be observed, got {other:?}"),
        }
    }

    #[test]
    fn a_book_read_names_the_venue_and_produces_no_fact_for_an_empty_side() {
        let object = ObjectId::from_string("obj-1");
        let mut book = OrderBook::from_levels(
            object.clone(),
            "XNYS",
            at(),
            vec![BookLevel::new(
                Decimal::parse("99.95").unwrap(),
                Decimal::from_int(100),
            )],
            Vec::new(),
        );

        let bid = observed_book("best_bid", "price", &book, |b| {
            b.best_bid().map(|l| l.price)
        })
        .expect("the bid side has a level");
        assert!((bid.value - 99.95).abs() < 1e-9, "{}", bid.value);
        match &bid.provenance {
            NumericProvenance::Observed {
                source,
                as_of,
                record_id,
            } => {
                assert_eq!(source, "book:XNYS");
                assert_eq!(*as_of, at());
                assert!(
                    record_id.contains("unsequenced"),
                    "a book without a venue sequence must say so: {record_id}"
                );
            }
            other => panic!("a touch price is observed, got {other:?}"),
        }

        assert!(
            observed_book("best_ask", "price", &book, |b| b
                .best_ask()
                .map(|l| l.price))
            .is_none(),
            "an empty side yields no fact, not a fact about nothing"
        );

        book.sequence = 42;
        let sequenced = observed_book("best_bid", "price", &book, |b| {
            b.best_bid().map(|l| l.price)
        })
        .expect("still has a bid");
        match &sequenced.provenance {
            NumericProvenance::Observed { record_id, .. } => {
                assert_eq!(record_id, &format!("{object}:XNYS:42"));
            }
            other => panic!("{other:?}"),
        }
    }
}
