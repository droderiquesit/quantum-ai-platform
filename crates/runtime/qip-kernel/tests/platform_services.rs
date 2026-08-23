//! What the deployed process actually contains.
//!
//! A crate is not a service until a process ships it, and a dependency whose
//! field is never read is not a service either. Every test here observes an
//! effect that only exists because [`Platform`] calls into one of the composed
//! crates: a source reaching the mesh catalogue, a chain refusing to be read
//! shallower than its stated depth, a hypothesis becoming a claim something can
//! contradict, and a cycle arriving in a hash-chained durable log.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_chain::adapter::{ChainAdapter, SyntheticChain, SyntheticChainConfig};
use qip_contracts::governance::Usage;
use qip_core::Context;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, ObjectId, dec};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::{Platform, RecordedPrediction};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_prediction::resolution::Observation;
use qip_prediction::{Observations, Verdict};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("test", start()))
                    .build(start())
                    .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A price series with a jump partway through, so the detectors have something
/// real to find.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).unwrap(),
                high: Decimal::from_f64(open.max(price) * 1.002).unwrap(),
                low: Decimal::from_f64(open.min(price) * 0.998).unwrap(),
                close: Decimal::from_f64(price).unwrap(),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

const URL: &str = "https://quotes.example.com/v1/prices.json";

/// A candidate whose licence permits trading on it, which is the usage the
/// platform's finder is configured for.
fn tradeable_candidate(id: &str) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        SourceEndpoint::parse(
            URL,
            AccessMechanism::Rest {
                auth: AuthRequirement::None,
                incremental_parameter: Some("since".to_string()),
                page_size: 500,
            },
        )?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::UsEast],
            ["US0001".to_string()],
            UpdateFrequency::Minutely,
        )?
        .with_history_from(start().saturating_sub(Duration::from_days(3_650))),
        LicensingPosture::declared(SourceLicense::new(
            "vendor-terms-2026",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
        SourceCost::free(Currency::USD),
        SourceRegion::UsEast,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )
}

/// A candidate whose licence permits research only, so a finder configured for
/// trading must refuse it.
fn research_only_candidate(id: &str) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        SourceEndpoint::parse(
            "https://research.example.com/v1/series.json",
            AccessMechanism::Rest {
                auth: AuthRequirement::None,
                incremental_parameter: None,
                page_size: 100,
            },
        )?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::UsEast],
            ["US0002".to_string()],
            UpdateFrequency::Daily,
        )?,
        LicensingPosture::declared(SourceLicense::new("research-terms", [Usage::Research])?),
        SourceCost::free(Currency::USD),
        SourceRegion::UsEast,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )
}

fn probe() -> InMemoryProbe {
    let head = HeadResponse {
        status: 200,
        content_type: Some("application/json".to_string()),
        content_length: Some(512),
        last_modified: Some(start()),
        latency: Duration::from_millis(40),
    };
    let sample = |body: &str| PayloadSample {
        body: body.to_string(),
        media_type: "application/json".to_string(),
        payload_at: Some(start()),
        latency: Duration::from_millis(55),
    };
    InMemoryProbe::new()
        .with_robots(
            "quotes.example.com",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        )
        .with_robots(
            "research.example.com",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        )
        .with_head(URL, head.clone())
        .with_sample(
            URL,
            sample(r#"{"symbol":"US0001","bid":10.25,"ask":10.27}"#),
        )
        .with_head("https://research.example.com/v1/series.json", head)
        .with_sample(
            "https://research.example.com/v1/series.json",
            sample(r#"{"symbol":"US0002","level":41.0}"#),
        )
}

// --- the data finder, and the catalogue it feeds ----------------------------

#[test]
fn a_source_the_finder_registers_appears_in_the_mesh_catalogue() -> Result<()> {
    // The gap the audit named: the mesh knows what datasets exist and nothing
    // decided what *should* exist. One call closes it, and the catalogue entry
    // is the finder's own rather than a second one this crate invented.
    let mut platform = platform(PlatformConfig::default().with_owner("market-data"))?;
    assert!(platform.catalog().is_empty(), "nothing is catalogued yet");

    let mut probe = probe();
    let assessment =
        platform.assess_sources(vec![tradeable_candidate("quotes")?], &mut probe, start())?;

    assert_eq!(assessment.registered(), 1, "{:?}", assessment.decisions);
    assert!(
        assessment.catalogue_problems.is_empty(),
        "{:?}",
        assessment.catalogue_problems
    );
    assert_eq!(assessment.catalogued, vec!["source.quotes".to_string()]);

    // The dataset is in the mesh, owned by the configured owner, and carries
    // the entitlement the licence granted — one answer to "what may this be
    // used for", not two that can disagree.
    let entry = platform.catalog().require("source.quotes")?;
    assert_eq!(entry.owner, "market-data");
    assert!(
        platform
            .catalog()
            .usable_for("source.quotes", Usage::Trade, start())
            .is_ok(),
        "a source registered for trading must be usable for it"
    );
    assert_eq!(platform.registered_sources().len(), 1);
    Ok(())
}

#[test]
fn a_source_licensed_only_for_research_never_reaches_the_catalogue() -> Result<()> {
    // The finder is configured for `Usage::Trade` and assesses against that
    // usage and no other. A licence that does not grant it is a rejection, not
    // a quiet downgrade — and a rejected source has no catalogue entry to
    // offer.
    let mut platform = platform(PlatformConfig::default())?;
    let mut probe = probe();
    let assessment = platform.assess_sources(
        vec![research_only_candidate("series")?],
        &mut probe,
        start(),
    )?;

    assert_eq!(assessment.registered(), 0);
    assert!(assessment.catalogued.is_empty());
    assert!(platform.catalog().is_empty());
    assert!(platform.catalog().get("source.series").is_none());
    Ok(())
}

#[test]
fn the_sense_stage_reports_the_registry_only_once_there_is_one() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let before = platform.run_cycle(start());
    let sense = before.stage(Stage::Sense).expect("sense ran");
    assert!(
        !sense.detail.contains("registered source"),
        "an empty registry is not worth a clause: {}",
        sense.detail
    );

    let mut probe = probe();
    platform.assess_sources(vec![tradeable_candidate("quotes")?], &mut probe, start())?;
    let after = platform.run_cycle(start());
    assert!(
        after
            .stage(Stage::Sense)
            .expect("sense ran")
            .detail
            .contains("1 registered source(s)"),
        "{}",
        after.stage(Stage::Sense).expect("sense ran").detail
    );
    Ok(())
}

// --- the chain --------------------------------------------------------------

#[test]
fn chain_state_cannot_be_read_shallower_than_the_configured_depth() -> Result<()> {
    // The past is revisable, and the type refuses to let that be forgotten:
    // there is no accessor on the platform that returns chain state without a
    // confirmation depth, and a chain that is not yet that deep says so rather
    // than answering zero.
    let mut platform = platform(PlatformConfig::default().with_chain_confirmations(8))?;
    assert!(platform.chain().is_none(), "no chain has been observed yet");
    assert!(platform.confirmed_chain().is_err());

    let mut chain = SyntheticChain::new(SyntheticChainConfig::demo(7)?, start())?;
    let shallow = chain.poll(start().saturating_add(Duration::from_secs(24)))?;
    let absorption = platform.observe_chain(shallow);
    assert!(absorption.extended > 0, "{absorption:?}");
    assert!(
        absorption.confirmed_trades.is_none(),
        "three blocks cannot support an eight-confirmation view"
    );
    assert!(absorption.unconfirmable.is_some());
    assert!(
        absorption.describe().contains("block(s) applied"),
        "an absorption has to be readable: {}",
        absorption.describe()
    );

    // Deeper, and the view opens.
    let deeper = chain.poll(start().saturating_add(Duration::from_mins(10)))?;
    let absorption = platform.observe_chain(deeper);
    assert!(absorption.confirmed_trades.is_some(), "{absorption:?}");
    assert!(
        absorption.describe().contains("confirmed trade(s)"),
        "{}",
        absorption.describe()
    );
    let view = platform.confirmed_chain()?;
    assert_eq!(view.required(), platform.confirmations());
    assert!(view.as_of() <= view.head(), "a view cannot outrun the head");
    Ok(())
}

#[test]
fn the_understand_stage_reports_the_chain_at_the_depth_it_was_read_at() -> Result<()> {
    let mut platform = platform(PlatformConfig::default().with_chain_confirmations(2))?;
    let quiet = platform.run_cycle(start());
    assert!(
        !quiet
            .stage(Stage::Understand)
            .expect("understand ran")
            .detail
            .contains("chain"),
        "an unobserved chain is not mentioned"
    );

    let mut chain = SyntheticChain::new(SyntheticChainConfig::demo(3)?, start())?;
    let updates = chain.poll(start().saturating_add(Duration::from_mins(10)))?;
    platform.observe_chain(updates);

    let report = platform.run_cycle(start().saturating_add(Duration::from_mins(10)));
    let detail = &report
        .stage(Stage::Understand)
        .expect("understand ran")
        .detail;
    assert!(detail.contains("chain at height"), "{detail}");
    assert!(
        detail.contains("2 confirmations"),
        "the depth the state was read at travels with it: {detail}"
    );
    Ok(())
}

#[test]
fn a_block_from_another_chain_is_refused_without_losing_the_batch() -> Result<()> {
    let mut platform = platform(PlatformConfig::default().with_chain_confirmations(1))?;
    let mut first = SyntheticChain::new(SyntheticChainConfig::demo(11)?, start())?;
    let mut other_config = SyntheticChainConfig::demo(12)?;
    other_config.chain = qip_chain::ChainId::new("synthetic-2");
    let mut second = SyntheticChain::new(other_config, start())?;

    let until = start().saturating_add(Duration::from_mins(5));
    let mut updates = first.poll(until)?;
    updates.extend(second.poll(until)?);

    let absorption = platform.observe_chain(updates);
    assert!(
        !absorption.problems.is_empty(),
        "a block from a different chain must be refused: {absorption:?}"
    );
    assert!(
        absorption.extended > 0,
        "and the batch's good blocks must still be applied: {absorption:?}"
    );
    Ok(())
}

// --- predictions ------------------------------------------------------------

/// The metric the platform wrote its first claim against.
///
/// Read off the proposition rather than hard-coded, because which detector
/// fires on a given series is the detectors' business: the property under test
/// is that the criterion names *an* observable and *the* instrument, so an
/// observation about anything else cannot settle it.
fn first_metric(platform: &Platform) -> String {
    platform.predictions()[0]
        .proposition
        .criteria
        .metrics()
        .first()
        .cloned()
        .expect("a threshold criterion names its metric")
}

#[test]
fn a_hypothesis_becomes_a_claim_something_can_later_contradict() -> Result<()> {
    // A confidence with no resolution criteria is an opinion. The prediction is
    // what makes a hypothesis scoreable against something published rather than
    // against whether it felt right.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    assert!(
        !platform.predictions().is_empty(),
        "a directional hypothesis should have been written down: {}",
        report.summarise()
    );
    let metric = first_metric(&platform);
    let prediction = &platform.predictions()[0];
    assert!(prediction.is_open());
    assert!(
        prediction.proposition.resolves_at > start(),
        "a claim resolving in the past is not a prediction"
    );
    // The metric names the observable and the instrument, so an observation
    // about another name — or about another quantity on the same name —
    // cannot settle this one.
    assert!(
        metric.ends_with(":obj-AAA") && metric.contains(':'),
        "the criterion must name what it measures and on what: {metric}"
    );
    assert!(
        prediction.proposition.source.publishes_metric(&metric),
        "a proposition whose source does not publish its metric is unsettleable"
    );
    // The stage says so, so an operator can see that the cycle produced
    // something falsifiable rather than merely confident.
    assert!(
        report
            .stage(Stage::Reason)
            .expect("reason ran")
            .detail
            .contains("falsifiable"),
        "{}",
        report.stage(Stage::Reason).expect("reason ran").detail
    );
    Ok(())
}

#[test]
fn a_prediction_is_scored_only_by_what_its_source_actually_published() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    assert!(!platform.predictions().is_empty());
    let metric = first_metric(&platform);

    let horizon = start().saturating_add(Duration::from_days(3_650));

    // Nothing published: the claim stays open. Resolving it as failure is how a
    // system marks itself right by scoring the questions nobody answered.
    let silent = platform.score_predictions(&Observations::at(horizon), horizon);
    assert!(silent.is_empty(), "{silent:?}");
    assert!(
        platform
            .predictions()
            .iter()
            .all(RecordedPrediction::is_open)
    );

    // An observation about a different series settles nothing either.
    let wrong_name = Observations::at(horizon).with(
        metric.replace("obj-AAA", "obj-BBB"),
        Observation::Numeric(dec!("1")),
    );
    assert!(platform.score_predictions(&wrong_name, horizon).is_empty());

    // What the source published settles it, one way or the other.
    let published = Observations::at(horizon).with(&metric, Observation::Numeric(dec!("0.5")));
    let scored = platform.score_predictions(&published, horizon);
    assert_eq!(scored.len(), platform.predictions().len());
    assert!(scored.iter().all(|(_, verdict)| verdict.is_determined()));
    assert!(matches!(scored[0].1, Verdict::Holds | Verdict::Fails));
    assert!(platform.predictions().iter().all(|p| !p.is_open()));

    // The verdict is kept, so the record says which way it went rather than
    // only that it was answered.
    assert_eq!(
        platform.predictions()[0].held(),
        scored[0].1.holds(),
        "a scored claim has to remember its own answer"
    );
    assert_eq!(platform.predictions()[0].scored_at, Some(horizon));

    // And a scored claim is not scored twice.
    assert!(platform.score_predictions(&published, horizon).is_empty());
    Ok(())
}

#[test]
fn a_claim_whose_horizon_has_not_passed_is_not_scored_early() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let metric = first_metric(&platform);

    let published = Observations::at(start()).with(&metric, Observation::Numeric(dec!("0.5")));
    assert!(
        platform.score_predictions(&published, start()).is_empty(),
        "a claim resolves at its horizon and not before"
    );
    Ok(())
}

// --- the journal ------------------------------------------------------------

#[test]
fn every_cycle_reaches_the_durable_log_and_comes_back_unchanged() -> Result<()> {
    // The mirror is the point: the platform's own log holds the frame and the
    // durable transport holds it wearing the ingestion envelope, both
    // hash-chained, so a truncated or edited history is detectable in either.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 90));

    let mut summaries = Vec::new();
    for step in 0..3 {
        let at = start().saturating_add(Duration::from_mins(5 * step));
        summaries.push(platform.run_cycle(at).summarise());
    }

    assert_eq!(platform.journal().len(), 3, "one entry per cycle");
    assert_eq!(
        platform.event_log().len(),
        3,
        "and one in the platform's own log"
    );
    assert!(
        platform.journal().verify_chain().is_ok(),
        "the journal's hash chain must hold"
    );

    let replayed = platform.journal_entries()?;
    assert_eq!(replayed.len(), 3);
    for (index, entry) in replayed.iter().enumerate() {
        assert_eq!(entry.cycle, index as u64 + 1);
        assert_eq!(entry.stages_ran, 8);
        assert_eq!(
            entry.summary, summaries[index],
            "what an operator would have read must survive the round trip"
        );
        assert!(
            entry.compute_cost.is_positive(),
            "a cycle that ran is not free"
        );
    }
    Ok(())
}

#[test]
fn the_journal_is_replayable_as_of_an_instant() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let first = start();
    let second = start().saturating_add(Duration::from_hours(1));
    platform.run_cycle(first);
    platform.run_cycle(second);

    let early = platform
        .replay_journal(&EventFilter::new().as_of(first.saturating_add(Duration::from_nanos(1))))?;
    assert_eq!(
        early.len(),
        1,
        "a replay reads what was knowable at an instant"
    );
    let all = platform.replay_journal(
        &EventFilter::new().as_of(second.saturating_add(Duration::from_nanos(1))),
    )?;
    assert_eq!(all.len(), 2);

    // The envelope's payload hash is computed on the way in and recomputed on
    // the way back; an edited payload would not survive this.
    for envelope in &all {
        envelope.verify_payload_hash()?;
    }
    Ok(())
}

#[test]
fn one_correlation_id_reconstructs_a_cycle_from_the_journal() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let report = platform.run_cycle(start());
    let correlation = report.correlation_id.clone();
    assert_eq!(platform.last_correlation(), Some(correlation.clone()));

    let found = platform.replay_journal(&EventFilter::new().correlation(correlation))?;
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0]
            .decode::<qip_kernel::CycleJournalEntry>()?
            .body
            .cycle,
        1
    );
    Ok(())
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_seed_and_the_same_inputs_produce_the_same_journal() -> Result<()> {
    let run = || -> Result<Vec<String>> {
        let mut platform = platform(PlatformConfig::default())?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
        Ok(platform
            .journal_entries()?
            .into_iter()
            .map(|entry| {
                format!(
                    "{}|{}|{}",
                    entry.correlation_id, entry.compute_cost, entry.summary
                )
            })
            .collect())
    };
    assert_eq!(run()?, run()?);
    Ok(())
}
