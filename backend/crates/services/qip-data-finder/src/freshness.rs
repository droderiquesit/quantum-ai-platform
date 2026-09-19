//! §7.6.4's freshness, measured rather than assumed.
//!
//! "A source that runs ahead of the surface web by hours is worth more than
//! one that runs ahead by minutes, and a source that is merely a copy of what
//! is already public carries no edge at all. Freshness is measured, not
//! assumed — the platform records how far in advance each source's facts
//! preceded the same fact appearing elsewhere, and sources are ranked on it."
//!
//! Until this module existed the only freshness this crate computed was
//! [`crate::finder::DataFinder`]'s score at registration: the age of a
//! source's newest record against the cadence the source itself promises —
//! whether the source is late *for itself*. That is a real reading and it
//! stays, but it is not the one the section defines, and the two must not be
//! confused: a source can be perfectly on time by its own schedule and still
//! carry nothing that a second source did not carry an hour earlier.
//!
//! # What is measured
//!
//! A *fact* here is a figure with a subject and a reference instant, stated
//! by a source that names itself: a macro release (series, region, reference
//! date, value, unit) or a fundamental figure (entity, metric, period end,
//! value, unit). Two records from two different sources with the same
//! [`FactKey`] are the same fact appearing twice, and the difference between
//! the instants the platform *learned* each — [`Provenance::ingestion_time`],
//! the knowable instant, never the valid one — is the lead the earlier
//! source had over the later. The valid instant cannot be the measure: both
//! sources report the same reference date, so the gap between them is
//! entirely in when each one said so.
//!
//! A tick, a quote, a trade, a bar or a book carries a venue and no source,
//! and a news item's headline is a vendor's own wording of an event rather
//! than the event — so none of those is comparable across sources here, and
//! [`LeadOutcome::NotComparable`] says so rather than a `false` that could
//! also mean "seen once". A revision or a restatement is keyed apart from the
//! original it revises: a corrected figure is a different fact, and folding
//! it in would credit the source that corrected late with having led.
//!
//! # What "the same fact" means, exactly
//!
//! The same figure. Two vendors whose closes for one bar differ in the fourth
//! decimal are not reporting one fact — they are disagreeing, and a
//! disagreement is worth seeing rather than collapsing. So the value is part
//! of the key, canonicalised through the type's own `Display`, and a source
//! that publishes a rounded copy of another's figure is never credited as its
//! follower. That is a deliberately narrow reading: it under-counts shared
//! facts and never mis-credits a lead.
//!
//! # Why the same source twice is not a measurement
//!
//! A redelivery — the same source, the same fact, a second poll — is the
//! dedup window's business ([`qip_market_ingestion::connector::dedup`]) and
//! says nothing about lead. It is reported as [`LeadOutcome::Redelivered`]
//! and moves no counter, because a source that re-serves its last page every
//! poll would otherwise accumulate a lead over itself.
//!
//! # Bounds
//!
//! Every working set here is capped and the caps are stated. Facts in flight
//! are held until [`FACTS_IN_FLIGHT`] and then evicted oldest-first, so a
//! fact whose second sighting arrives after the eviction is simply seen as
//! new again — the same honest trade the dedup window makes. Sources ranked
//! are capped at [`SOURCES_RANKED`]; a sighting from a source past the cap
//! still measures the fact but is counted as unranked rather than growing
//! the map. Neither cap is a refusal, because an observation is not an
//! input to validate — it is a thing that happened — and the counts are what
//! an operator sizes the caps from.
//!
//! # What reads it
//!
//! `qip-kernel`'s `Platform::observe` feeds every sensed record through
//! [`LeadLedger::observe`] on the SENSE stage, and `stage_sense` reports the
//! ranking in its detail once any fact has been seen from two sources; the
//! ranking itself is [`LeadLedger::ranking`]. Nothing here changes a source's
//! routing class or its registration score: a measured lead is evidence for
//! a person deciding which sources to keep, and a score that moved on it by
//! implication would be a control nobody chose.

use qip_core::{Duration, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How many facts are held awaiting a second sighting before the oldest is
/// evicted.
pub const FACTS_IN_FLIGHT: usize = 4_096;

/// How many distinct sources the ledger ranks. A sighting from a source past
/// this bound still measures the fact it belongs to and is counted as
/// unranked.
pub const SOURCES_RANKED: usize = 256;

/// A source-independent identity for one fact.
///
/// Built only by [`FactKey::of`], which decides which records carry a fact at
/// all; there is no constructor taking a string, so a key cannot be forged
/// from a subject alone.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FactKey(String);

impl FactKey {
    /// The fact a record states, with the source that stated it and the
    /// instant the platform learned it — or `None` for a record kind that
    /// carries no source-independent fact.
    ///
    /// The parts are length-prefixed for the reason the dedup fingerprint's
    /// are: a `|`-joined key would let a series id containing the separator
    /// collide with a different fact.
    pub fn of(record: &SensedRecord) -> Option<(Self, &str, Timestamp)> {
        let (parts, source, known_at): (Vec<String>, &str, Timestamp) = match record {
            SensedRecord::Macro(observation) => (
                vec![
                    "macro".to_string(),
                    observation.series_id.clone(),
                    observation.region.clone(),
                    observation.reference_date.as_nanos().to_string(),
                    // `Display` of an `f64` is its shortest round-trip
                    // representation, so `1.0` and `1.00` key the same and
                    // `1.0` and `1.0000001` key apart, which is the reading
                    // the module doc argues for.
                    observation.value.to_string(),
                    observation.unit.clone(),
                    observation.is_revision.to_string(),
                ],
                observation.provenance.source.as_str(),
                observation.provenance.ingestion_time,
            ),
            SensedRecord::Fundamental(update) => (
                vec![
                    "fundamental".to_string(),
                    update.entity_id.clone(),
                    update.metric.clone(),
                    update.period_end.as_nanos().to_string(),
                    update.value.to_string(),
                    update.unit.clone(),
                    update.is_restatement.to_string(),
                ],
                update.provenance.source.as_str(),
                update.provenance.ingestion_time,
            ),
            SensedRecord::Tick(_)
            | SensedRecord::Quote(_)
            | SensedRecord::Trade(_)
            | SensedRecord::Book(_)
            | SensedRecord::Bar(_)
            | SensedRecord::CorporateAction(_)
            | SensedRecord::News(_)
            | SensedRecord::AlternativeData(_)
            | SensedRecord::ReferenceData(_) => return None,
        };
        if source.trim().is_empty() {
            // A fact nobody claims to have stated cannot lead or trail.
            return None;
        }
        let mut material = String::new();
        for part in parts {
            material.push_str(&part.len().to_string());
            material.push(':');
            material.push_str(&part);
            material.push(';');
        }
        Some((Self(material), source, known_at))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one sighting of a fact told the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeadOutcome {
    /// The record carries no source-independent fact — a tick, a quote, a
    /// news item — so it can neither lead nor trail.
    NotComparable,
    /// The first time this fact has been seen, from this source.
    First,
    /// The same source stating the same fact again: a redelivery, which is
    /// the dedup window's business and not a lead.
    Redelivered,
    /// A second source has stated a fact another source stated first.
    Measured {
        /// The source whose sighting the platform learned of earlier.
        leader: String,
        /// The source that stated the same fact later.
        follower: String,
        /// How far ahead the leader was, by knowable instant. Zero when
        /// both were learned in the same instant, which is the "merely a
        /// copy" case the section names.
        lead: Duration,
    },
}

/// The first sighting of a fact, and every source that has since stated it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FirstSighting {
    source: String,
    known_at: Timestamp,
    reporters: BTreeSet<String>,
}

/// One source's measured lead over the others, accumulated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLead {
    /// Facts this source stated before another source did.
    led: u64,
    /// Facts this source stated after another source already had.
    trailed: u64,
    /// The sum of the leads this source had, in nanoseconds, over `led`.
    total_lead_nanos: i64,
    /// The single longest lead this source had.
    longest_lead_nanos: i64,
}

impl SourceLead {
    pub const fn led(&self) -> u64 {
        self.led
    }

    pub const fn trailed(&self) -> u64 {
        self.trailed
    }

    /// The mean lead over the facts this source led on, or zero when it has
    /// never led — the "copy of what is already public" reading.
    pub fn mean_lead(&self) -> Duration {
        if self.led == 0 {
            return Duration::ZERO;
        }
        // Statistics, not money: an average of durations as `f64` seconds
        // is a reading and settles nothing.
        let mean = self.total_lead_nanos as f64 / self.led as f64;
        Duration::from_nanos(mean as i64)
    }

    pub const fn longest_lead(&self) -> Duration {
        Duration::from_nanos(self.longest_lead_nanos)
    }

    /// Whether every shared fact this source stated had already been stated
    /// elsewhere. Such a source carries no edge on the evidence so far.
    pub const fn never_led(&self) -> bool {
        self.led == 0 && self.trailed > 0
    }
}

/// One row of the ranking: a source and its measured lead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLeadSummary {
    pub source: String,
    pub led: u64,
    pub trailed: u64,
    pub mean_lead: Duration,
    pub longest_lead: Duration,
}

impl SourceLeadSummary {
    pub fn describe(&self) -> String {
        if self.led == 0 {
            format!(
                "{} never first, trailed on {} fact(s)",
                self.source, self.trailed
            )
        } else {
            format!(
                "{} led on {} fact(s) by {:.0}s on average (longest {:.0}s), trailed on {}",
                self.source,
                self.led,
                self.mean_lead.as_secs_f64(),
                self.longest_lead.as_secs_f64(),
                self.trailed
            )
        }
    }
}

/// The bounded record of which source stated each fact first, and by how
/// much.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadLedger {
    in_flight: BTreeMap<FactKey, FirstSighting>,
    order: VecDeque<FactKey>,
    sources: BTreeMap<String, SourceLead>,
    /// Facts seen from at least two sources.
    measured: u64,
    /// Sightings from sources past [`SOURCES_RANKED`], counted rather than
    /// ranked.
    unranked: u64,
    /// Facts evicted awaiting a second sighting, so a deployment can see
    /// whether the in-flight bound is the reason its ranking is thin.
    evicted: u64,
}

impl Default for LeadLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl LeadLedger {
    pub fn new() -> Self {
        Self {
            in_flight: BTreeMap::new(),
            order: VecDeque::new(),
            sources: BTreeMap::new(),
            measured: 0,
            unranked: 0,
            evicted: 0,
        }
    }

    /// Record one sighting.
    pub fn observe(&mut self, record: &SensedRecord) -> LeadOutcome {
        let Some((key, source, known_at)) = FactKey::of(record) else {
            return LeadOutcome::NotComparable;
        };
        let Some(first) = self.in_flight.get_mut(&key) else {
            self.admit(key, source, known_at);
            return LeadOutcome::First;
        };
        if !first.reporters.insert(source.to_string()) {
            return LeadOutcome::Redelivered;
        }
        // The earlier knowable instant leads, whichever arrived at this
        // ledger first: a batch can carry the follower's record ahead of the
        // leader's, and crediting arrival order would credit the batch.
        let (leader, follower, lead) = if known_at < first.known_at {
            (
                source.to_string(),
                first.source.clone(),
                first.known_at.since(known_at),
            )
        } else {
            (
                first.source.clone(),
                source.to_string(),
                known_at.since(first.known_at),
            )
        };
        self.measured = self.measured.saturating_add(1);
        self.credit(&leader, lead);
        self.debit(&follower);
        LeadOutcome::Measured {
            leader,
            follower,
            lead,
        }
    }

    fn admit(&mut self, key: FactKey, source: &str, known_at: Timestamp) {
        if self.order.len() >= FACTS_IN_FLIGHT
            && let Some(oldest) = self.order.pop_front()
        {
            self.in_flight.remove(&oldest);
            self.evicted = self.evicted.saturating_add(1);
        }
        let mut reporters = BTreeSet::new();
        reporters.insert(source.to_string());
        self.in_flight.insert(
            key.clone(),
            FirstSighting {
                source: source.to_string(),
                known_at,
                reporters,
            },
        );
        self.order.push_back(key);
    }

    fn entry(&mut self, source: &str) -> Option<&mut SourceLead> {
        if !self.sources.contains_key(source) && self.sources.len() >= SOURCES_RANKED {
            self.unranked = self.unranked.saturating_add(1);
            return None;
        }
        Some(self.sources.entry(source.to_string()).or_default())
    }

    fn credit(&mut self, source: &str, lead: Duration) {
        if let Some(entry) = self.entry(source) {
            entry.led = entry.led.saturating_add(1);
            entry.total_lead_nanos = entry.total_lead_nanos.saturating_add(lead.as_nanos());
            entry.longest_lead_nanos = entry.longest_lead_nanos.max(lead.as_nanos());
        }
    }

    fn debit(&mut self, source: &str) {
        if let Some(entry) = self.entry(source) {
            entry.trailed = entry.trailed.saturating_add(1);
        }
    }

    /// Facts seen from at least two sources so far.
    pub const fn measured(&self) -> u64 {
        self.measured
    }

    pub const fn unranked(&self) -> u64 {
        self.unranked
    }

    pub const fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Facts awaiting a second sighting.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    pub fn lead_of(&self, source: &str) -> Option<&SourceLead> {
        self.sources.get(source)
    }

    /// Every ranked source, best lead first.
    ///
    /// Ordered by mean lead descending, then by how many facts the source
    /// led on, then by name — so two sources with the same reading order the
    /// same way on every replay. A source that has never led sorts last,
    /// which is where the section puts a copy of what is already public.
    pub fn ranking(&self) -> Vec<SourceLeadSummary> {
        let mut rows: Vec<SourceLeadSummary> = self
            .sources
            .iter()
            .map(|(source, lead)| SourceLeadSummary {
                source: source.clone(),
                led: lead.led,
                trailed: lead.trailed,
                mean_lead: lead.mean_lead(),
                longest_lead: lead.longest_lead(),
            })
            .collect();
        rows.sort_by(|left, right| {
            right
                .mean_lead
                .as_nanos()
                .cmp(&left.mean_lead.as_nanos())
                .then_with(|| right.led.cmp(&left.led))
                .then_with(|| left.source.cmp(&right.source))
        });
        rows
    }

    /// The ranking in one line, for a stage detail. Empty when nothing has
    /// been measured, so a caller can append it without a conditional.
    pub fn describe(&self) -> String {
        if self.measured == 0 {
            return String::new();
        }
        let rows: Vec<String> = self
            .ranking()
            .iter()
            .map(SourceLeadSummary::describe)
            .collect();
        let unranked = if self.unranked == 0 {
            String::new()
        } else {
            format!(
                "; {} sighting(s) from sources past the ranking bound",
                self.unranked
            )
        };
        format!(
            "{} fact(s) seen from two sources, ranked on measured lead: {}{unranked}",
            self.measured,
            rows.join(", ")
        )
    }
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` still has to assert.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::Decimal;
    use qip_financial::intelligence::{FiscalPeriod, FundamentalUpdate, MacroObservation};
    use qip_financial::quality::{DataQuality, Provenance};

    fn start() -> Timestamp {
        Timestamp::from_civil(2026, 9, 1)
    }

    fn release(source: &str, value: f64, learned_at: Timestamp) -> SensedRecord {
        let reference = start().saturating_sub(Duration::from_days(1));
        SensedRecord::Macro(Box::new(MacroObservation {
            series_id: "US.CPI.YOY".to_string(),
            region: "US".to_string(),
            value,
            unit: "percent".to_string(),
            reference_date: reference,
            consensus: None,
            previous: None,
            is_revision: false,
            provenance: Provenance::new(source, reference, learned_at),
            quality: DataQuality::clean(),
        }))
    }

    fn figure(source: &str, learned_at: Timestamp) -> SensedRecord {
        let period_end = start().saturating_sub(Duration::from_days(30));
        SensedRecord::Fundamental(Box::new(FundamentalUpdate {
            entity_id: "ent:acme".to_string(),
            metric: "revenue".to_string(),
            value: Decimal::from_int(1_250),
            unit: "USD".to_string(),
            period_end,
            period: FiscalPeriod::Quarter,
            consensus: None,
            prior_value: None,
            is_restatement: false,
            provenance: Provenance::new(source, period_end, learned_at),
            quality: DataQuality::clean(),
        }))
    }

    #[test]
    fn the_source_that_stated_a_fact_first_is_credited_with_the_lead_it_had() {
        // The failure: freshness read off a source's own cadence, so a copy
        // of a public figure served promptly by its own schedule scored as
        // fresh as the source that had it an hour earlier.
        let mut ledger = LeadLedger::new();
        let early = start();
        let late = start().saturating_add(Duration::from_hours(1));

        assert_eq!(
            ledger.observe(&release("wire-a", 3.4, early)),
            LeadOutcome::First
        );
        // Premise: one fact in flight and nothing measured yet.
        assert_eq!(ledger.in_flight(), 1);
        assert_eq!(ledger.measured(), 0);

        let outcome = ledger.observe(&release("wire-b", 3.4, late));
        assert_eq!(
            outcome,
            LeadOutcome::Measured {
                leader: "wire-a".to_string(),
                follower: "wire-b".to_string(),
                lead: Duration::from_hours(1),
            }
        );
        assert_eq!(ledger.measured(), 1);
        let ranking = ledger.ranking();
        assert_eq!(ranking.len(), 2);
        assert_eq!(ranking[0].source, "wire-a");
        assert_eq!(ranking[0].mean_lead, Duration::from_hours(1));
        assert_eq!(ranking[0].led, 1);
        assert_eq!(ranking[1].source, "wire-b");
        assert_eq!(ranking[1].trailed, 1);
        assert!(ledger.lead_of("wire-b").is_some_and(SourceLead::never_led));
        assert!(
            ledger
                .describe()
                .contains("wire-a led on 1 fact(s) by 3600s")
        );
    }

    #[test]
    fn the_earlier_knowable_instant_leads_even_when_its_record_arrives_second() {
        // The failure: a batch that carries the follower's record ahead of
        // the leader's credits the batch order rather than the instant each
        // source actually had the figure.
        let mut ledger = LeadLedger::new();
        let early = start();
        let late = start().saturating_add(Duration::from_mins(20));

        assert_eq!(
            ledger.observe(&release("late-wire", 3.4, late)),
            LeadOutcome::First
        );
        let outcome = ledger.observe(&release("early-wire", 3.4, early));
        assert_eq!(
            outcome,
            LeadOutcome::Measured {
                leader: "early-wire".to_string(),
                follower: "late-wire".to_string(),
                lead: Duration::from_mins(20),
            }
        );
        assert_eq!(ledger.ranking()[0].source, "early-wire");
    }

    #[test]
    fn a_redelivery_from_the_same_source_measures_nothing() {
        // The failure: a source re-serving its last page every poll
        // accumulating a lead over itself.
        let mut ledger = LeadLedger::new();
        assert_eq!(
            ledger.observe(&release("wire-a", 3.4, start())),
            LeadOutcome::First
        );
        let again = start().saturating_add(Duration::from_hours(2));
        assert_eq!(
            ledger.observe(&release("wire-a", 3.4, again)),
            LeadOutcome::Redelivered
        );
        assert_eq!(ledger.measured(), 0);
        assert!(ledger.ranking().is_empty());
        assert!(ledger.describe().is_empty());
    }

    #[test]
    fn a_different_figure_is_a_different_fact_and_a_tick_is_no_fact_at_all() {
        // The failure: two vendors disagreeing on a figure collapsed into one
        // fact, crediting one of them with a lead over a number it never
        // published.
        let mut ledger = LeadLedger::new();
        assert_eq!(
            ledger.observe(&release("wire-a", 3.4, start())),
            LeadOutcome::First
        );
        assert_eq!(
            ledger.observe(&release(
                "wire-b",
                3.5,
                start().saturating_add(Duration::from_hours(1))
            )),
            LeadOutcome::First
        );
        assert_eq!(ledger.in_flight(), 2);
        assert_eq!(ledger.measured(), 0);

        let tick = SensedRecord::Tick(qip_market::Tick {
            object_id: qip_core::ids::ObjectId::from_string("obj:aaa"),
            venue: "sim".to_string(),
            at: start(),
            price: Decimal::from_int(10),
            volume: Decimal::ZERO,
            quality: DataQuality::clean(),
        });
        assert_eq!(ledger.observe(&tick), LeadOutcome::NotComparable);
        assert_eq!(ledger.in_flight(), 2);
    }

    #[test]
    fn a_fundamental_figure_is_compared_across_sources_like_a_release() {
        let mut ledger = LeadLedger::new();
        assert_eq!(
            ledger.observe(&figure("filings", start())),
            LeadOutcome::First
        );
        let outcome = ledger.observe(&figure(
            "aggregator",
            start().saturating_add(Duration::from_days(1)),
        ));
        assert!(
            matches!(&outcome, LeadOutcome::Measured { leader, lead, .. }
                if leader == "filings" && *lead == Duration::from_days(1)),
            "{outcome:?}"
        );
    }

    #[test]
    fn facts_in_flight_are_bounded_and_the_eviction_is_counted() {
        // The failure: a ledger that remembers every fact ever seen until
        // the process dies of memory during the incident where a source
        // replays its history.
        let mut ledger = LeadLedger::new();
        for index in 0..(FACTS_IN_FLIGHT + 3) {
            let record = release("wire-a", index as f64, start());
            assert_eq!(ledger.observe(&record), LeadOutcome::First);
        }
        assert_eq!(ledger.in_flight(), FACTS_IN_FLIGHT);
        assert_eq!(ledger.evicted(), 3);
        // The evicted fact is seen as new again rather than matched.
        assert_eq!(
            ledger.observe(&release("wire-b", 0.0, start())),
            LeadOutcome::First
        );
    }

    #[test]
    fn sources_past_the_ranking_bound_are_counted_rather_than_ranked() {
        let mut ledger = LeadLedger::new();
        let early = start();
        let late = start().saturating_add(Duration::from_secs(1));
        // `origin` plus exactly `SOURCES_RANKED` copies is one source past
        // the bound, so exactly one sighting lands unranked.
        for index in 0..SOURCES_RANKED {
            let value = index as f64;
            ledger.observe(&release("origin", value, early));
            ledger.observe(&release(&format!("copy-{index:03}"), value, late));
        }
        assert_eq!(ledger.ranking().len(), SOURCES_RANKED);
        assert_eq!(ledger.unranked(), 1);
        assert_eq!(ledger.measured(), SOURCES_RANKED as u64);
        assert!(
            ledger
                .describe()
                .contains("1 sighting(s) from sources past the ranking bound")
        );
    }
}
