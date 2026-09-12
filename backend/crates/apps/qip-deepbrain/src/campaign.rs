//! §22.4's caller: the research node's learning assembly, run as a bounded,
//! TTL-scoped fetch campaign.
//!
//! The blueprint's arrow is
//!
//! ```text
//! campaign starts -> resolve references -> fetch into TTL cache -> verify
//! hashes -> backtest and purged CV over the cache -> emit results,
//! statistics, manifest -> cache expires and is deleted.
//! ```
//!
//! and until this module nothing outside a test walked it. `qip-data-finder`
//! built the campaign, the cache, the manifest and the concentration check
//! (ADR 0056) and `docs/DELIVERY-STATUS.md` recorded, correctly, that a
//! continuously-running connector is not a campaign and that forcing one
//! poll per open/close would misrepresent a permanent feed as a bounded
//! research run. What *is* a bounded research run is this node's learning
//! round: on its cadence it assembles one subject's window of history and
//! fits a model on it. That assembly is the campaign, and each step of the
//! arrow lands on a named line of [`assemble`]:
//!
//! * **resolve references** — the subject's stream is resolved to the door
//!   it came through: a catalogue-admitted connector the platform holds an
//!   admission for — a live connector, or a replay recorded from one and
//!   admitted under its licence — or a stream the platform generated itself
//!   (the synthetic exchange, a committed tape). A stream that is neither is
//!   refused before any bytes are read, because a fit on data whose licence
//!   nobody evaluated is the "use before evaluation" the data domain forbids.
//!   The refusal is [`Assembly::RefusedAtDoor`], a fact about one subject
//!   and one round that the round line carries and
//!   `qip_research_campaigns_refused_total{gate="door"}` counts — not an
//!   error, because until 2026-09-12 it was one, `maybe_learn` propagated
//!   it, and the deep brain's node loop stopped on the first due learning
//!   round over an undeclared replay. A connector-fed deep brain now fails
//!   closed per subject, not per process.
//! * **fetch into TTL cache** — the window is serialised, referenced through
//!   that door, and fetched into a [`FetchCampaign`] under a stated
//!   [`CacheBound`]. The bars the desk then fits on are *read back out of the
//!   cache*, so the cache is on the path rather than beside it.
//! * **verify hashes** — the reference is recorded on the platform's ledger,
//!   which compares it hash to hash against the last reference to the same
//!   extent. A revision — the ledger's, this round or an earlier one — flags
//!   the campaign's manifest entry for the window, and the kernel has already
//!   journaled and counted it. The blueprint's "flagged, not silently
//!   invalidated" is exactly the shape: the fit proceeds, and the record says
//!   what it was fitted on.
//! * **statistics** — one sketched statistic, bars per subject across the
//!   fetched extents, from a count-min sketch whose `(ε, δ)` is declared on
//!   the manifest beside the number. The consumer — the fit that needs a
//!   count it can trust — refuses when the declared error at this volume
//!   exceeds the tolerance it can afford (§22.4's "bounds are declared and
//!   monitored"). With one subject per campaign the sketch sees one key and
//!   the estimate is exact; the bound and the refusal are what the manifest
//!   carries, and the memory the sketch saves is what a many-subject campaign
//!   would draw on.
//! * **manifest** — the closed campaign's manifest is journaled to the
//!   hash-chained log as a `ResearchCampaignClosed` record, under its own
//!   permanently retained topic, so a regulatory demand for what a fit used
//!   is answered from the log. Only from the log: a second copy in the
//!   node's key-value store was two claims about one fact, and the log is
//!   the record.
//! * **cache expires and is deleted** — `FetchCampaign::close` drops it.
//!
//! Two of §22.4's mitigations sit at the edges of the arrow. A subject whose
//! own stream no longer holds enough history is assembled from §22.1's
//! fallback series instead, and the manifest says so ("vendor withdraws
//! historical access"). And the distinct sources backing the subject —
//! this stream plus every source the platform's ledger holds references
//! from naming it, each with the door it came through — are put to
//! `assess_concentration`, which counts only the vendor doors, and whose
//! verdict rides on the manifest and, in `EvolutionEngine::turn`, holds a
//! universe with fewer than two vendors back from promotion past validation.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_data_finder::admission::AdmittedSource;
use qip_data_finder::campaign::{
    CacheBound, ConcentrationVerdict, FetchCampaign, SketchedStatistic, assess_concentration,
};
use qip_data_finder::reference::{DataPeriod, DataReference, SourceOrigin};
use qip_data_finder::schema::SourceSchema;
use qip_financial::quality::LicensingClass;
use qip_kernel::Platform;
use qip_kernel::references::ResearchCampaignClosed;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SourceDescriptor;
use qip_numerics::sketch::{CountMinSketch, ErrorBound};
use qip_observability::metrics::{labels, names};

/// How long a campaign's cache may hold an extract. A learning round
/// finishes inside one cycle, so an hour is generous; it is a ceiling, not a
/// target, and `FetchCampaign::close` deletes the cache long before it.
pub const CAMPAIGN_TTL: Duration = Duration::from_hours(1);

/// Extracts a campaign's cache may hold. One window per subject and one
/// subject per round today; four leaves room for a round that assembles a
/// subject's peers without letting the cache become a second history.
pub const CAMPAIGN_CACHE_ENTRIES: usize = 4;

/// The sketch's declared `(ε, δ)`: an overcount of at most a tenth of a
/// percent of everything counted, with probability 99%.
pub const SKETCH_EPSILON: f64 = 0.001;
pub const SKETCH_DELTA: f64 = 0.01;

/// The share of the desk's minimum bar count a sketched count may be off by
/// before the fit refuses to rely on it: a desk that needs 256 bars cannot
/// be told it has them by a sketch that may be twenty over.
pub const BAR_COUNT_TOLERANCE: f64 = 0.05;

/// The bounds a campaign runs under. Built rather than defaulted because
/// both halves are refusable values.
#[derive(Clone, Debug)]
pub struct CampaignConfig {
    pub cache: CacheBound,
    pub sketch: ErrorBound,
    /// See [`BAR_COUNT_TOLERANCE`].
    pub tolerance_fraction: f64,
}

impl CampaignConfig {
    pub fn standard() -> Result<Self> {
        Ok(Self {
            cache: CacheBound::new(CAMPAIGN_TTL, CAMPAIGN_CACHE_ENTRIES)?,
            sketch: ErrorBound::new(SKETCH_EPSILON, SKETCH_DELTA)?,
            tolerance_fraction: BAR_COUNT_TOLERANCE,
        })
    }
}

/// What one campaign did, for the round's cycle line and its tests.
#[derive(Clone, Debug)]
pub struct CampaignSummary {
    pub id: String,
    pub subject: String,
    pub source_id: String,
    pub origin: SourceOrigin,
    pub bars: usize,
    pub period: DataPeriod,
    /// What the platform's ledger said of the window's reference: `first`,
    /// `unchanged` or `revised`.
    pub ledger: &'static str,
    /// Manifest entries a revision flagged, read off the manifest.
    pub flagged: usize,
    /// Whether the window came from the fallback series, read off the
    /// manifest.
    pub fallback_used: bool,
    pub concentration: ConcentrationVerdict,
    pub statistic: SketchedStatistic,
}

impl CampaignSummary {
    pub fn describe(&self) -> String {
        format!(
            "campaign {}: {} bar(s) of {} from {} ({}), {} to {}, ledger {}, {} flagged{}, \
             {}; {}{}",
            self.id,
            self.bars,
            self.subject,
            self.source_id,
            self.origin.as_str(),
            self.period.start().to_rfc3339(),
            self.period.end().to_rfc3339(),
            self.ledger,
            self.flagged,
            if self.fallback_used {
                ", assembled from the fallback series"
            } else {
                ""
            },
            self.concentration.describe(),
            self.statistic.describe(),
            "; manifest journaled"
        )
    }
}

/// The window a round fits on, read back from the campaign's cache, and
/// the campaign's own account of itself.
#[derive(Clone, Debug)]
pub struct AssembledWindow {
    pub bars: Vec<Bar>,
    pub summary: CampaignSummary,
}

/// The locator a subject's window is referenced under. Stated once so a
/// test that wants to pre-record a reference to the same extent can spell
/// it the way the campaign does.
pub fn locator_for(source_id: &str, subject: &ObjectId, interval: Interval) -> String {
    format!(
        "bars://{source_id}/{}?interval={}",
        subject.as_str(),
        interval.as_str()
    )
}

/// The period a window of bars describes: the first bar's open to the last
/// bar's close. Refuses an empty window, which describes nothing.
pub fn period_of(bars: &[Bar]) -> Result<DataPeriod> {
    let (Some(first), Some(last)) = (bars.first(), bars.last()) else {
        return Err(Error::invalid(
            "an empty window of bars describes no period of the world",
        ));
    };
    DataPeriod::new(first.open_time, last.close_time())
}

/// Which door the subject's stream came through.
enum Door {
    Admitted(AdmittedSource),
    Generated,
}

/// What assembling a subject's window produced.
#[derive(Debug)]
pub enum Assembly {
    /// The window, read back from the campaign's cache, and the campaign's
    /// own account of itself. Boxed: a window is hundreds of bars and the
    /// other two arms are a sentence, and an enum sized for its largest arm
    /// would carry that on every "not yet".
    Window(Box<AssembledWindow>),
    /// Neither the subject's stream nor the fallback series holds enough
    /// history yet — the ordinary "not yet" of a node that has just started.
    NotYet,
    /// The stream was refused at the door before a byte was read: neither a
    /// source the platform holds an admission for nor one it generated. A
    /// fact about this subject and this round, carried on the round line
    /// and counted, and not an error that leaves the node loop.
    RefusedAtDoor(String),
}

/// The gate label a door refusal is counted under.
pub const GATE_DOOR: &str = "door";

/// Count a campaign refused at the door, once per round. Recorded here, at
/// the seam where the refusal is known, for the standing-admission refusal
/// `EvolutionEngine::maybe_learn` makes before it reaches [`assemble`] as
/// well as for the door refusal inside it.
pub fn count_door_refusal(platform: &Platform) {
    platform.telemetry().metrics.count(
        names::RESEARCH_CAMPAIGNS_REFUSED,
        labels([("gate", GATE_DOOR)]),
    );
}

/// Assemble `subject`'s research window through a campaign. See the module
/// doc for what each step is.
///
/// `bars` is the subject's own history as the engine holds it. A stream
/// that is neither catalogue-admitted nor generated is
/// [`Assembly::RefusedAtDoor`], counted; a sketch whose declared error
/// exceeds the tolerance is refused as an error, because it is a
/// configuration the deployment chose and not a fact about one subject.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    platform: &mut Platform,
    descriptor: &SourceDescriptor,
    subject: &ObjectId,
    bars: &[Bar],
    minimum_bars: usize,
    cycle: u64,
    now: Timestamp,
    config: &CampaignConfig,
) -> Result<Assembly> {
    // Resolve the door before reading a byte.
    let door = match platform.admitted_source(&descriptor.name) {
        Some(admitted) => Door::Admitted(admitted.clone()),
        None if descriptor.licensing == LicensingClass::Synthetic => Door::Generated,
        None => {
            count_door_refusal(platform);
            return Ok(Assembly::RefusedAtDoor(format!(
                "the research stream `{}` declares licensing class `{:?}` and this platform \
                 holds no admission for it; a fit on data whose licence nobody evaluated is \
                 refused. Admit the source through the licensing gate — a replay names the \
                 connector it was recorded from with `# recorded-from:` — or research over \
                 a stream this platform generated",
                descriptor.name, descriptor.licensing
            )));
        }
    };

    // The window: the subject's own history, or §22.1's insurance when the
    // stream no longer holds enough of it.
    let (window, fallback_used) = if bars.len() >= minimum_bars {
        (bars.to_vec(), false)
    } else {
        let fallback = platform.fallback_bars(subject.as_str());
        if fallback.len() >= minimum_bars {
            (fallback.to_vec(), true)
        } else {
            return Ok(Assembly::NotYet);
        }
    };
    let Some(first) = window.first() else {
        return Ok(Assembly::NotYet);
    };

    // The reference: the window's bytes through the door it came from.
    let bytes = serde_json::to_vec(&window)?;
    let schema = SourceSchema::from_json(&serde_json::to_value(first)?);
    let period = period_of(&window)?;
    let locator = locator_for(&descriptor.name, subject, first.interval);
    let symbols = [subject.as_str().to_string()];
    let reference = match &door {
        Door::Admitted(admitted) => DataReference::of_admitted(
            admitted,
            &locator,
            symbols,
            period,
            &bytes,
            now,
            Decimal::ZERO,
            1.0,
        )?,
        Door::Generated => {
            DataReference::of_generated(descriptor, &locator, symbols, period, schema, &bytes, now)?
        }
    };
    let source_id = reference.source_id().to_string();
    let origin = reference.origin();

    // The ledger: hash verification across rounds, with the kernel's
    // consequence on a revision.
    let recorded = platform.record_reference(reference.clone(), now)?;

    // The campaign: fetch into the TTL cache, flag what the ledger knows,
    // sketch, and read the window back out of the cache. The ledger's
    // revision flags this campaign's entry only if this campaign read the
    // withdrawn bytes (`RevisionRecord::contradicts`); a campaign that
    // fetched the corrected extent is not the backtest that used the
    // original, and the kernel has already named the closed campaign that
    // was, on the log, when it recorded the reference above.
    let id = format!("learn-{}-{cycle}", subject.as_str());
    let mut campaign = FetchCampaign::open(&id, config.cache, now)?;
    campaign.fetch(reference, &bytes, now)?;
    if let Some(revision) = platform.revision_covering(&source_id, subject.as_str(), &period) {
        campaign.flag_revised(revision);
    }
    if fallback_used {
        campaign.record_fallback(subject.as_str());
    }
    let mut sketch = CountMinSketch::new(config.sketch);
    for bar in &window {
        sketch.increment(bar.object_id.as_str());
    }
    let statistic = SketchedStatistic::new(
        "bars_per_subject",
        subject.as_str(),
        sketch.estimate(subject.as_str()),
        sketch.total(),
        config.sketch,
    )?;
    let tolerance = config.tolerance_fraction * minimum_bars as f64;
    if !config.sketch.tolerable_for(sketch.total(), tolerance) {
        return Err(Error::denied(format!(
            "the campaign's sketch declares an error of up to {:.1} bar(s) at {} counted, and \
             the fit can tolerate at most {tolerance:.1} against its minimum of {minimum_bars}; \
             a count the fit cannot trust is refused rather than fitted on ({})",
            config.sketch.absolute_error(sketch.total()),
            sketch.total(),
            config.sketch.describe()
        )));
    }
    campaign.attach_statistic(statistic.clone());
    let (_, cached) = campaign.cache().get(&locator, now).ok_or_else(|| {
        Error::not_found(format!(
            "the campaign cache does not hold {locator} an instant after fetching it"
        ))
    })?;
    let assembled: Vec<Bar> = serde_json::from_slice(cached)?;

    // Concentration: this stream plus every source the ledger holds a
    // reference from naming the subject, each with its door; only the
    // vendor doors count.
    let mut backing = platform.sources_backing(subject.as_str());
    backing.insert(source_id.clone(), origin);
    let concentration =
        assess_concentration(backing.iter().map(|(id, origin)| (id.as_str(), *origin)));

    // Close: the cache is dropped, the manifest is kept where an audit can
    // find it — on the log, and nowhere else.
    let manifest = campaign.close();
    let closed = ResearchCampaignClosed {
        campaign_id: id.clone(),
        subject: subject.as_str().to_string(),
        opened_at: now,
        closed_at: now,
        manifest,
        concentration: concentration.clone(),
    };
    let flagged = closed.flagged();
    let fallback_used = closed.fallback_used();
    platform.journal_campaign(closed, now)?;

    Ok(Assembly::Window(Box::new(AssembledWindow {
        bars: assembled,
        summary: CampaignSummary {
            id,
            subject: subject.as_str().to_string(),
            source_id,
            origin,
            bars: window.len(),
            period,
            ledger: recorded.outcome.as_str(),
            flagged,
            fallback_used,
            concentration,
            statistic,
        },
    })))
}

impl Assembly {
    /// The window, or `None` for either other outcome. For the tests that
    /// assemble and want the window; production reads the outcome.
    pub fn window(self) -> Option<AssembledWindow> {
        match self {
            Self::Window(window) => Some(*window),
            Self::NotYet | Self::RefusedAtDoor(_) => None,
        }
    }
}

// The workspace denies `panic_in_result_fn` for production code; in a test
// the assertion is the deliverable and `?` is what keeps the setup readable.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::Context;
    use qip_data_finder::admission;
    use qip_events::Topic;
    use qip_financial::quality::DataQuality;
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_market_ingestion::adapter::SensedRecord;
    use qip_market_ingestion::connectors::{CoinbaseTickerConnector, FrankfurterRatesConnector};
    use qip_observability::Telemetry;
    use qip_observability::metrics::{labels, names};
    use qip_risk::limits::LimitSet;
    use qip_streaming::envelope::StreamEnvelope;

    const MINIMUM: usize = 256;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn subject() -> ObjectId {
        ObjectId::from_string("OBJ0000000000000000000AAA")
    }

    fn platform() -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    fn descriptor(licensing: LicensingClass) -> SourceDescriptor {
        SourceDescriptor {
            name: "synthetic-exchange".to_string(),
            provider: "this process".to_string(),
            licensing,
            topics: vec![Topic::MarketBar],
            expected_latency: Duration::ZERO,
            production_requirement: None,
        }
    }

    fn bars(count: usize, interval: Interval, close_offset: i64) -> Vec<Bar> {
        bars_of(&subject(), count, interval, close_offset)
    }

    fn bars_of(
        subject: &ObjectId,
        count: usize,
        interval: Interval,
        close_offset: i64,
    ) -> Vec<Bar> {
        (0..count)
            .map(|index| {
                let close = 100 + index as i64 + close_offset;
                Bar {
                    object_id: subject.clone(),
                    venue: "XSIM".to_string(),
                    interval,
                    open_time: start().saturating_add(Duration::from_millis(
                        interval.duration().as_millis() * index as i64,
                    )),
                    open: Decimal::from_int(close - 1),
                    high: Decimal::from_int(close + 1),
                    low: Decimal::from_int(close - 2),
                    close: Decimal::from_int(close),
                    volume: Decimal::from_int(1_000 + index as i64),
                    vwap: None,
                    trade_count: 10,
                    quality: DataQuality::default(),
                }
            })
            .collect()
    }

    /// Two shipped connectors, through the real catalogue and the real gate
    /// — the two whose terms are read — each referencing an extent that
    /// names `subject` onto the platform's ledger: the premise "two
    /// independent vendors back this subject", stated the way the ledger
    /// states it.
    fn back_with_two_admitted_sources(
        platform: &mut Platform,
        subject: &ObjectId,
        now: Timestamp,
    ) -> Result<()> {
        for manifest in [
            FrankfurterRatesConnector::shipped_manifest()?,
            CoinbaseTickerConnector::shipped_manifest()?,
        ] {
            let decision = admission::admit(&manifest.source_id, manifest.licensing, now)?;
            let admitted = AdmittedSource::from_decision(&decision, &manifest)?;
            platform.record_reference(
                DataReference::of_admitted(
                    &admitted,
                    format!("{}?subject={}", admitted.endpoint(), subject.as_str()),
                    [subject.as_str().to_string()],
                    DataPeriod::instant(now),
                    b"{\"served\":true}",
                    now,
                    Decimal::ZERO,
                    1.0,
                )?,
                now,
            )?;
        }
        Ok(())
    }

    fn campaigns_closed(platform: &Platform, outcome: &str) -> u64 {
        platform.telemetry().metrics.snapshot().counter(
            names::RESEARCH_CAMPAIGNS_CLOSED,
            &labels([("outcome", outcome)]),
        )
    }

    /// The closed-campaign records on the log, decoded, in log order.
    fn closed_campaigns(platform: &Platform) -> Result<Vec<ResearchCampaignClosed>> {
        platform
            .event_log()
            .by_topic(Topic::ResearchCampaignClosed)
            .into_iter()
            .map(|frame| {
                Ok(StreamEnvelope::from_frame(frame)?
                    .decode::<ResearchCampaignClosed>()?
                    .body)
            })
            .collect()
    }

    /// The whole arrow, once: the window is referenced through the generated
    /// door, recorded first on the ledger, fetched into and read back from
    /// the cache, sketched with its bound attached, and the manifest is
    /// journaled under its own topic — and only journaled: the log is the
    /// record, and the cycle journal still reads. And the concentration
    /// verdict is about the backing: a second source on the ledger turns it.
    ///
    /// Mutated by deleting the `campaign.fetch(reference, &bytes, now)?`
    /// line — confirmed the cache read-back then fails with `not_found` and
    /// this test with it, then restored.
    #[test]
    fn a_window_is_assembled_through_a_campaign_whose_manifest_is_journaled() -> Result<()> {
        let mut platform = platform()?;
        let stream = bars(300, Interval::Minute, 0);
        let config = CampaignConfig::standard()?;
        // Premise for the journal assertion below: a cycle has run, so the
        // cycle journal holds an entry a foreign body would break the read
        // of — which is what a campaign closing under `LearningCompleted`
        // did until 2026-09-12.
        let _ = platform.run_cycle(start());
        assert_eq!(platform.journal_entries()?.len(), 1);

        let window = assemble(
            &mut platform,
            &descriptor(LicensingClass::Synthetic),
            &subject(),
            &stream,
            MINIMUM,
            1,
            start(),
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("a window from 300 bars against a minimum of 256"))?;

        assert_eq!(
            window.bars, stream,
            "the desk fits on the window read back from the cache"
        );
        let summary = &window.summary;
        assert_eq!(summary.id, "learn-OBJ0000000000000000000AAA-1");
        assert_eq!(summary.origin, SourceOrigin::Generated);
        assert_eq!(summary.source_id, "synthetic-exchange");
        assert_eq!(summary.ledger, "first");
        assert_eq!(summary.flagged, 0);
        assert!(!summary.fallback_used);
        assert_eq!(summary.bars, 300);
        assert_eq!(summary.period, period_of(&stream)?);
        assert_eq!(summary.statistic.estimate(), 300);
        assert_eq!(summary.statistic.total(), 300);
        assert!(
            !summary.concentration.is_sufficient(),
            "one stream was read as sufficient backing"
        );
        assert_eq!(
            summary.concentration.viable_sources(),
            0,
            "a generated stream is not a vendor and must not count as one"
        );
        assert_eq!(summary.concentration.generated(), 1);

        // The ledger holds the reference, the log holds the closed record
        // under its own topic, the cycle journal still decodes, and the
        // series moved.
        assert_eq!(platform.reference_ledger().len(), 1);
        let closed = closed_campaigns(&platform)?;
        assert_eq!(closed.len(), 1, "one campaign closed, one record");
        assert_eq!(closed[0].campaign_id, summary.id);
        assert_eq!(closed[0].manifest.entries().len(), 1);
        assert_eq!(closed[0].manifest.statistics().len(), 1);
        assert_eq!(closed[0].flagged(), 0);
        assert!(!closed[0].fallback_used());
        assert_eq!(
            platform.journal_entries()?.len(),
            1,
            "a closed campaign on the log must not break the cycle journal's read"
        );
        assert_eq!(campaigns_closed(&platform, "clean"), 1);
        assert_eq!(campaigns_closed(&platform, "flagged"), 0);

        // Two admitted vendors naming the subject on the ledger, and the
        // next round's verdict turns; a second generated stream would not
        // have turned it.
        back_with_two_admitted_sources(&mut platform, &subject(), start())?;
        let next = assemble(
            &mut platform,
            &descriptor(LicensingClass::Synthetic),
            &subject(),
            &stream,
            MINIMUM,
            2,
            start(),
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("a second window"))?;
        assert_eq!(
            next.summary.ledger, "unchanged",
            "the same bytes re-referenced"
        );
        assert!(next.summary.concentration.is_sufficient());
        assert_eq!(next.summary.concentration.viable_sources(), 2);
        assert_eq!(next.summary.concentration.generated(), 1);
        Ok(())
    }

    /// §22.3's actual requirement, in two halves. A campaign that fitted on
    /// an extent the source later revises is the backtest that used the
    /// original: the kernel finds it on the log, closed, and names it in a
    /// `ResearchCampaignFlagged` record. The campaign that fetched the
    /// corrected bytes — the one open when the revision was detected — is
    /// *not* flagged, because it did not use the original; until 2026-09-12
    /// it was the only one that was. The fit still proceeds either way:
    /// flagged, not silently invalidated.
    ///
    /// Three mutations, each confirmed to fail this test and restored
    /// byte-for-byte. `RevisionRecord::contradicts` with its time-and-hash
    /// clause dropped (source, subject and period alone): the second round's
    /// own manifest entry is then flagged and this fails on "the campaign
    /// that read the corrected bytes". `ResearchCampaignClosed::
    /// used_revised_extent` replaced with `true`: the unrelated subject's
    /// closed campaign is then named too and this fails on the record count.
    /// The same replaced with `false`: the first round is then flagged by
    /// nothing and this fails on the missing record.
    #[test]
    fn the_campaign_that_used_the_original_is_flagged_and_the_one_that_used_the_revision_is_not()
    -> Result<()> {
        let mut platform = platform()?;
        let source = descriptor(LicensingClass::Synthetic);
        let config = CampaignConfig::standard()?;
        let original = bars(300, Interval::Minute, 0);

        // Round one fits on the original bytes and closes on the log.
        let earlier = start().saturating_sub(Duration::from_hours(1));
        let first = assemble(
            &mut platform,
            &source,
            &subject(),
            &original,
            MINIMUM,
            1,
            earlier,
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("the first window"))?;
        assert_eq!(first.summary.ledger, "first", "premise");
        assert_eq!(first.summary.flagged, 0, "premise: nothing to flag yet");
        assert!(
            platform
                .event_log()
                .by_topic(Topic::ResearchCampaignFlagged)
                .is_empty(),
            "premise: nothing is flagged before any revision"
        );
        // An unrelated campaign closes on the log too — another subject on
        // the same stream — so the join below has something to *not* name.
        let other = ObjectId::from_string("OBJ0000000000000000000BBB");
        let unrelated = assemble(
            &mut platform,
            &source,
            &other,
            &bars_of(&other, 300, Interval::Minute, 0),
            MINIMUM,
            1,
            earlier,
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("the unrelated window"))?;
        assert_ne!(
            unrelated.summary.id, first.summary.id,
            "premise: two campaigns"
        );

        // The source rewrites the same extent — same locator, same period,
        // other closes — and round two fits on the corrected bytes.
        let revised = bars(300, Interval::Minute, 7);
        assert_eq!(
            period_of(&revised)?,
            period_of(&original)?,
            "premise: one extent"
        );
        let second = assemble(
            &mut platform,
            &source,
            &subject(),
            &revised,
            MINIMUM,
            2,
            start(),
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("the second window"))?;
        assert_eq!(second.summary.ledger, "revised");
        assert_eq!(second.bars.len(), 300, "the fit still gets its window");
        assert_eq!(
            second.summary.flagged, 0,
            "the campaign that read the corrected bytes is not the backtest that used the \
             original, and must not be flagged"
        );
        assert_eq!(campaigns_closed(&platform, "flagged"), 0);
        assert_eq!(campaigns_closed(&platform, "clean"), 3);

        // The kernel's consequence: the revision on the log under its own
        // topic, the series moved, and the *first* campaign named.
        assert_eq!(
            platform.telemetry().metrics.snapshot().counter(
                names::DATA_REVISIONS_DETECTED,
                &labels([("origin", "generated")])
            ),
            1,
            "the kernel's own consequence fired for the generated door"
        );
        assert_eq!(
            platform
                .event_log()
                .by_topic(Topic::SourceRevisionDetected)
                .len(),
            1
        );
        let flagged = platform
            .event_log()
            .by_topic(Topic::ResearchCampaignFlagged);
        assert_eq!(
            flagged.len(),
            1,
            "exactly one closed campaign used the original; the unrelated subject's must not \
             be named"
        );
        let record = StreamEnvelope::from_frame(flagged[0])?
            .decode::<qip_kernel::references::ResearchCampaignFlagged>()?
            .body;
        assert_eq!(
            record.campaign_id, first.summary.id,
            "the campaign named is the one that fitted on the original"
        );
        assert_eq!(record.subject, subject().as_str());
        let detected = platform
            .reference_ledger()
            .revisions()
            .next()
            .ok_or_else(|| Error::not_found("the ledger's revision"))?;
        assert_eq!(
            record.revision.now(),
            detected.now(),
            "the flag carries the revision the ledger detected"
        );
        assert_eq!(
            platform.telemetry().metrics.snapshot().counter(
                names::RESEARCH_CAMPAIGNS_FLAGGED,
                &labels([("origin", "generated")])
            ),
            1
        );
        // And the record is idempotent: the same revision re-recorded names
        // the campaign once.
        let again = assemble(
            &mut platform,
            &source,
            &subject(),
            &revised,
            MINIMUM,
            3,
            start().saturating_add(Duration::from_hours(1)),
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("the third window"))?;
        assert_eq!(again.summary.ledger, "unchanged");
        assert_eq!(
            platform
                .event_log()
                .by_topic(Topic::ResearchCampaignFlagged)
                .len(),
            1
        );
        Ok(())
    }

    /// The consumer's refusal: a sketch whose declared error at this volume
    /// exceeds what the fit can tolerate stops the assembly by name, and
    /// nothing is journaled for a campaign that did not close.
    ///
    /// Mutated by replacing `!config.sketch.tolerable_for(...)` with `false`
    /// — confirmed the assembly then succeeds and this test fails, then
    /// restored.
    #[test]
    fn a_sketch_whose_declared_error_exceeds_the_fits_tolerance_refuses_the_assembly() -> Result<()>
    {
        let mut platform = platform()?;
        let stream = bars(300, Interval::Minute, 0);
        let loose = CampaignConfig {
            sketch: ErrorBound::new(0.5, 0.01)?,
            ..CampaignConfig::standard()?
        };
        // Premise: at 300 bars a half-error sketch declares 150, against a
        // tolerance of five percent of 256.
        assert!(
            !loose
                .sketch
                .tolerable_for(300, BAR_COUNT_TOLERANCE * MINIMUM as f64)
        );

        let error = assemble(
            &mut platform,
            &descriptor(LicensingClass::Synthetic),
            &subject(),
            &stream,
            MINIMUM,
            1,
            start(),
            &loose,
        )
        .expect_err("a fit was assembled on a count the sketch cannot vouch for");
        assert_eq!(error.code(), "denied", "got {error:?}");
        assert!(
            error.message().contains("refused rather than fitted on"),
            "the refusal does not name the bound: {error}"
        );
        assert!(
            platform
                .event_log()
                .by_topic(Topic::ResearchCampaignClosed)
                .is_empty(),
            "a campaign that did not close must not journal a manifest"
        );
        Ok(())
    }

    /// A stream that is neither catalogue-admitted nor generated is refused
    /// at the door, before a byte is serialised or referenced — as an
    /// outcome that names the door and is counted, not as an error.
    ///
    /// Mutated by changing the `None if descriptor.licensing == Synthetic`
    /// arm to a bare `None => Door::Generated` — confirmed the assembly then
    /// proceeds to `of_generated`, which refuses as an error, and this test
    /// fails on the outcome, then restored.
    #[test]
    fn a_stream_that_is_neither_admitted_nor_generated_is_refused_at_the_door() -> Result<()> {
        let mut platform = platform()?;
        let stream = bars(300, Interval::Minute, 0);
        let outcome = assemble(
            &mut platform,
            &descriptor(LicensingClass::Licensed),
            &subject(),
            &stream,
            MINIMUM,
            1,
            start(),
            &CampaignConfig::standard()?,
        )?;
        let Assembly::RefusedAtDoor(reason) = outcome else {
            return Err(Error::invalid(format!(
                "a licensed stream nobody admitted was researched over: {outcome:?}"
            )));
        };
        assert!(
            reason.contains("holds no admission for it"),
            "the refusal is not the door's: {reason}"
        );
        assert!(
            platform.reference_ledger().is_empty(),
            "nothing was referenced"
        );
        assert_eq!(
            platform.telemetry().metrics.snapshot().counter(
                names::RESEARCH_CAMPAIGNS_REFUSED,
                &labels([("gate", GATE_DOOR)])
            ),
            1,
            "a door refusal is counted"
        );
        Ok(())
    }

    /// A subject whose own stream no longer holds enough history is
    /// assembled from §22.1's fallback series, and the manifest says so; a
    /// subject with too little history anywhere is "not yet", not an error.
    ///
    /// Mutated by replacing the fallback branch with `return Ok(None)` —
    /// confirmed the first half then fails, then restored.
    #[test]
    fn a_subject_whose_stream_lost_its_history_is_assembled_from_the_fallback_series() -> Result<()>
    {
        let mut platform = platform()?;
        let config = CampaignConfig::standard()?;
        let too_little = bars(10, Interval::Minute, 0);

        // Nothing anywhere: not yet.
        assert!(
            assemble(
                &mut platform,
                &descriptor(LicensingClass::Synthetic),
                &subject(),
                &too_little,
                MINIMUM,
                1,
                start(),
                &config,
            )?
            .window()
            .is_none()
        );

        // The platform has been observing this subject's daily bars all
        // along; the stream has since lost its history.
        let absorbed = platform.observe(
            bars(300, Interval::Day, 0)
                .into_iter()
                .map(|bar| SensedRecord::Bar(Box::new(bar)))
                .collect(),
        );
        assert_eq!(absorbed, 300, "premise: the daily bars were absorbed");
        assert_eq!(platform.fallback_bars(subject().as_str()).len(), 300);

        let window = assemble(
            &mut platform,
            &descriptor(LicensingClass::Synthetic),
            &subject(),
            &too_little,
            MINIMUM,
            2,
            start(),
            &config,
        )?
        .window()
        .ok_or_else(|| Error::not_found("a window from the fallback series"))?;
        assert!(window.summary.fallback_used);
        assert_eq!(window.bars.len(), 300);
        assert_eq!(window.bars[0].interval, Interval::Day);
        let closed = closed_campaigns(&platform)?;
        let journaled = closed
            .last()
            .ok_or_else(|| Error::not_found("the closed record"))?;
        assert!(journaled.fallback_used());
        assert!(
            journaled.manifest.fallbacks().contains(subject().as_str()),
            "the manifest must record that the insurance was drawn on"
        );
        Ok(())
    }
}
