//! §22.1's retention classes, as the vocabulary every retained record is
//! filed under.
//!
//! Blueprint §56.4 rule 33: "Every retained byte belongs to a declared
//! retention class. Data with no class does not get written." Until ADR 0089
//! the class existed as a type in `qip-data-finder` — a service — and the
//! event log, which is where the platform's retained bytes actually live,
//! retained by [`crate::topic::TopicGroup`] instead: a permanence tier
//! derived from which stage of the cycle a topic belonged to, with two
//! topics named by exception. A lib may not depend on a service, so the log
//! could not read the class, and the class could not be read by the one
//! thing that retains.
//!
//! So the table lives here, one crate below where it was, and every
//! [`crate::topic::Topic`] declares its row in
//! [`crate::topic::Topic::retention_class`] — exhaustively, with no wildcard
//! arm, so a topic added without a class is a compile error rather than a
//! record filed under whatever its group implied. The log's two retention
//! seams read the class and nothing else; the two predicates the streaming
//! router and the mesh already ask ([`crate::topic::Topic::is_lossy_tolerable`]
//! and [`crate::topic::Topic::requires_permanent_retention`]) are derived
//! from it, so there is one claim about what a record is and not two that
//! can drift.
//!
//! `qip-data-finder` re-exports the type and keeps the one row that needed a
//! structure of its own, the fallback bar series.
//!
//! # What a class decides in the log
//!
//! [`Retention`] is the row's own policy in the row's own words, and the log
//! reads two things off it:
//!
//! * [`Retention::is_replaceable`] — the record is the log's *working set*
//!   and not its record: bounded by the snapshot window, rolled by age, and
//!   the first thing spent under pressure. `Never` ("a bounded ring measured
//!   in seconds") and `InMemoryFixed` ("fixed size regardless of
//!   throughput") both say that the structure holding the fact is bounded by
//!   something other than the log.
//! * [`Retention::is_permanent`] — the record is why the log exists, and it
//!   is never evicted; when nothing else remains the append is refused.
//!   `Permanent` says so in one word and `Indefinite` in another.
//!
//! Everything between — a referenced document, a series, a rolling window,
//! a three-year fallback — is an *observation*: kept until pressure has
//! spent every replaceable record, and never rolled by age, because the
//! structure the row names (the manifest, the series) is what retains it and
//! the log is only its index.

use qip_core::Duration;
use serde::{Deserialize, Serialize};

/// The nine rows of §22.1's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Raw ticks, book deltas, quote updates, source text.
    Transient,
    /// Features, moments, covariance, sketches, reservoirs.
    DerivedState,
    /// Own orders, fills, intents, verdicts, quotes, receipt timestamps,
    /// transfers.
    Irreplaceable,
    /// Per-strategy returns, family correlations, dispersion by venue pair,
    /// solver deltas, counterfactual scores.
    CompactDerived,
    /// Compressed state with outcome, indexed for retrieval.
    Episodic,
    /// Entities, relations, causal edges, beliefs, extracted facts.
    Semantic,
    /// Book state at each own order, fill, quote, veto, unwind.
    EventAnchored,
    /// Bars for instruments in an active class or universe.
    FallbackSeries,
    /// External market history, filings, registries.
    Referenced,
}

/// A row's answer to "retained?", in the table's own terms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    /// A bounded ring measured in seconds, then gone.
    Never,
    /// In memory, fixed size regardless of throughput.
    InMemoryFixed,
    /// Permanently; only this platform has these.
    Permanent,
    /// Series, not observations.
    Series,
    /// Indefinitely; compressed meaning.
    Indefinite,
    /// A rolling window.
    Rolling(Duration),
    /// For a stated span behind the newest observation.
    For(Duration),
    /// A manifest with source, range and content hash; fetched on demand.
    ManifestOnly,
}

impl Retention {
    /// Whether a record under this policy is the log's working set rather
    /// than its record: rolled by age behind the snapshot window and the
    /// first thing spent under pressure.
    ///
    /// The two rows here are the two whose retaining structure is bounded by
    /// something other than the log — a ring measured in seconds, a fixed
    /// in-memory size — so the log's copy is an index into a bound that
    /// already exists.
    pub const fn is_replaceable(&self) -> bool {
        matches!(self, Self::Never | Self::InMemoryFixed)
    }

    /// Whether a record under this policy is never evicted, so that a full
    /// log refuses the next append rather than dropping one of these.
    pub const fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent | Self::Indefinite)
    }
}

impl RetentionClass {
    /// The nine rows in the table's own order.
    pub const ALL: [Self; 9] = [
        Self::Transient,
        Self::DerivedState,
        Self::Irreplaceable,
        Self::CompactDerived,
        Self::Episodic,
        Self::Semantic,
        Self::EventAnchored,
        Self::FallbackSeries,
        Self::Referenced,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::DerivedState => "derived_state",
            Self::Irreplaceable => "irreplaceable",
            Self::CompactDerived => "compact_derived",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::EventAnchored => "event_anchored",
            Self::FallbackSeries => "fallback_series",
            Self::Referenced => "referenced",
        }
    }

    /// The row's "what" column.
    pub const fn what(&self) -> &'static str {
        match self {
            Self::Transient => "raw ticks, book deltas, quote updates, source text",
            Self::DerivedState => "features, moments, covariance, sketches, reservoirs",
            Self::Irreplaceable => {
                "own orders, fills, intents, verdicts, quotes, receipt timestamps, transfers"
            }
            Self::CompactDerived => {
                "per-strategy returns, family correlations, dispersion by venue pair, solver \
                 deltas, counterfactual scores"
            }
            Self::Episodic => "compressed state with outcome, indexed for retrieval",
            Self::Semantic => "entities, relations, causal edges, beliefs, extracted facts",
            Self::EventAnchored => "book state at each own order, fill, quote, veto, unwind",
            Self::FallbackSeries => {
                "one-minute OHLCV for instruments in an active class or universe"
            }
            Self::Referenced => "external market history, filings, registries",
        }
    }

    /// The row's "retained?" column.
    pub const fn retention(&self) -> Retention {
        match self {
            Self::Transient => Retention::Never,
            Self::DerivedState => Retention::InMemoryFixed,
            Self::Irreplaceable => Retention::Permanent,
            Self::CompactDerived => Retention::Series,
            Self::Episodic | Self::Semantic => Retention::Indefinite,
            Self::EventAnchored => Retention::Rolling(Duration::from_days(90)),
            Self::FallbackSeries => Retention::For(FALLBACK_RETENTION),
            Self::Referenced => Retention::ManifestOnly,
        }
    }

    /// [`Retention::is_replaceable`] of this row's policy.
    pub const fn is_replaceable(&self) -> bool {
        self.retention().is_replaceable()
    }

    /// [`Retention::is_permanent`] of this row's policy.
    pub const fn is_permanent(&self) -> bool {
        self.retention().is_permanent()
    }
}

/// How far behind its newest bar a fallback series reaches: three years,
/// counted in days so a leap day does not shorten it.
pub const FALLBACK_RETENTION: Duration = Duration::from_days(3 * 365 + 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_two_policies_are_replaceable_and_exactly_two_are_permanent_and_they_do_not_meet() {
        // The log spends the replaceable tier first and refuses to spend the
        // permanent one at all; a policy in both would be a record the log
        // both rolls by age and refuses to evict, which is not a policy.
        let replaceable: Vec<_> = RetentionClass::ALL
            .iter()
            .filter(|class| class.is_replaceable())
            .collect();
        let permanent: Vec<_> = RetentionClass::ALL
            .iter()
            .filter(|class| class.is_permanent())
            .collect();
        assert_eq!(
            replaceable,
            [&RetentionClass::Transient, &RetentionClass::DerivedState]
        );
        assert_eq!(
            permanent,
            [
                &RetentionClass::Irreplaceable,
                &RetentionClass::Episodic,
                &RetentionClass::Semantic
            ]
        );
        for class in RetentionClass::ALL {
            assert!(
                !(class.is_replaceable() && class.is_permanent()),
                "{} is both replaceable and permanent",
                class.as_str()
            );
        }
    }
}
