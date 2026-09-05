//! The committed demonstration tape, through the real cycle.
//!
//! What this proves: a node on the tape feed runs one cycle per tape period
//! on tape time, the detectors find the structure planted on the tape, the
//! panel convenes, a falsifiable claim is written down, and — because tape
//! time advances — the claim's horizon passes on the tape and LEARN scores
//! it into a calibration record. Every assertion is premise-first: the tape's
//! own shape is asserted before anything the platform made of it.
//!
//! The tape carries four sections and the second test below reads each of
//! them through the analyst that should: the macro analyst reads the four
//! series the macro arm writes, keyed by NWSC's economy, and takes a
//! direction; the alternative-data analyst finds the `web-traffic` series
//! and refuses it, because nothing in this repository licenses that dataset
//! and the platform's default licenses none. Both are asserted on the
//! findings the organisation recorded, which is the only place a finding
//! lives.
//!
//! What this does not prove, stated so nobody reads it as proven: no order
//! and no fill. The second test states the arithmetic exactly as the review
//! reported it, and the bar is the reasoning control (ADR 0005). If a future
//! tape clears it, the assertion on the bar fails and says what the test
//! must then grow to assert.

use qip_agents::finding::{AgentFinding, Direction, FindingStatus};
use qip_core::{Clock, Duration, Timestamp};
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_financial::universe::Universe;
use qip_investment_agents::ids;
use qip_investment_agents::vocabulary::{AltMetric, MacroSeries};
use qip_kernel::{Platform, PlatformConfig, Stage};
use qip_market_ingestion::tape::Tape;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::path::PathBuf;
use std::sync::Arc;

const NWSC: &str = "OBJ00000000000000000NWSC";
const MRDN: &str = "OBJ00000000000000000MRDN";
const PERIODS: usize = 600;

fn tape_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../data/datasets/loop-demonstration-tape.json")
}

fn universe(now: Timestamp) -> Universe {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../data/datasets/universe.json");
    let text = std::fs::read_to_string(&path).expect("the committed catalogue reads");
    qip_financial::catalogue::load(&text, now)
        .expect("the committed catalogue loads")
        .universe
}

fn log_returns(closes: &[f64]) -> Vec<f64> {
    closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// The tape's own shape, asserted before anything the platform made of it.
fn assert_tape_premises(tape: &Tape) {
    assert_eq!(
        tape.periods(),
        PERIODS,
        "the tape does not hold {PERIODS} periods"
    );
    assert_eq!(tape.instruments().len(), 4);

    // The jump the return-anomaly detector is aimed at: period 100's return
    // on NWSC stands far outside its neighbours.
    let nwsc = log_returns(&tape.closes(NWSC));
    let jump = nwsc[99];
    let neighbours: Vec<f64> = nwsc[..99].iter().map(|r| r.abs()).collect();
    let typical = mean(&neighbours);
    assert!(
        jump > 3.0 * typical && jump > 0.015,
        "the premise failed: NWSC's period-100 return {jump:.4} is not the planted jump \
         against a typical |return| of {typical:.4}"
    );

    // The drift the structural-break detector is aimed at: MRDN's mean return
    // over periods 180–239 is well above its mean before them, while no single
    // period in the segment is itself an outlier.
    let mrdn = log_returns(&tape.closes(MRDN));
    let quiet = mean(&mrdn[..179]);
    let drift = mean(&mrdn[179..239]);
    let largest_in_segment = mrdn[179..239].iter().cloned().fold(0.0_f64, f64::max);
    assert!(
        drift > 0.004 && drift > quiet + 0.004,
        "the premise failed: MRDN's drift segment averages {drift:.5} against {quiet:.5} before it"
    );
    assert!(
        largest_in_segment < 0.02,
        "the premise failed: the drift segment holds an outlier of {largest_in_segment:.4}, so a \
         return-anomaly would find it rather than the structural break"
    );

    // Every release is one the world model's vocabulary recognises, keyed by
    // NWSC's economy, and every reading is a vocabulary metric from its own
    // dataset. This crate depends on the vocabulary and the tape's crate
    // does not, so this is where the fixture is held to it.
    assert!(!tape.releases().is_empty(), "the tape carries no release");
    for entry in tape.releases() {
        let observation = &entry.observation;
        assert!(
            MacroSeries::recognise(&observation.series_id, &observation.region).is_some(),
            "release {} is not a series the macro analyst reads",
            observation.series_id
        );
        assert_eq!(observation.region, "US", "NWSC's economy is US");
    }
    assert!(!tape.readings().is_empty(), "the tape carries no reading");
    for entry in tape.readings() {
        let point = &entry.point;
        assert_eq!(
            AltMetric::recognise(&point.dataset, &point.metric).expect("its own dataset"),
            Some(AltMetric::WebTrafficIndex)
        );
        assert_eq!(point.subject_id, NWSC);
    }
    // The macro analyst needs thirty observations knowable before it reads;
    // the tape's history is what supplies them, and the December print is
    // knowable before the panel convenes on the jump at period 101.
    let policy = tape.series(&MacroSeries::PolicyRate.series_id("US"));
    assert!(
        policy.len() >= 31,
        "only {} policy-rate prints",
        policy.len()
    );
    let jump_known_at = tape.entries()[100 * 4].known_at;
    assert!(
        policy.iter().all(|(known_at, _)| *known_at < jump_known_at),
        "a print is knowable only after the jump"
    );
    // And the jump has its catalyst: the declaration precedes it.
    let declaration = tape.declarations().first().expect("one declaration");
    assert_eq!(declaration.action.object_id.as_str(), NWSC);
    assert!(declaration.known_at < jump_known_at);
}

fn platform(feed: &Feed) -> Platform {
    let tape_clock = feed.owned_clock().expect("a tape owns its clock");
    let config = PlatformConfig::default();
    let clock: Arc<dyn Clock> = tape_clock;
    let context = qip_core::Context::new(clock, config.seed);
    // The catalogue is read at the wall clock, as `main.rs` reads it: it is
    // dated 2026 and refuses to be ingested before it happened, and the tape
    // is from 2025. The roster and the cycles run on tape time.
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(qip_core::SystemClock.now()),
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles")
}

#[test]
fn the_demonstration_tape_drives_the_loop_through_tape_time_to_a_scored_claim() {
    // ----- the premise: what is on the tape --------------------------------
    let tape = Tape::open(tape_path()).expect("the committed tape loads");
    assert_tape_premises(&tape);

    // ----- the run: one cycle per tape period, on tape time ----------------
    let mut feed = Feed::tape(&tape_path().display().to_string()).expect("the tape feed opens");
    let tape_clock = feed.owned_clock().expect("a tape owns its clock");
    let start = tape_clock.now();
    let mut platform = platform(&feed);

    // The wall clock is handed to the feed exactly as `node::run` hands it,
    // and it is the real one: a harness that passed the tape clock here
    // could not tell a feed that ran on tape time from one that ran on the
    // wall clock, and a mutation doing the latter survived until this line.
    let wall = qip_core::SystemClock;
    let mut cycles = 0usize;
    let mut opportunities_found = 0usize;
    let mut panels_convened = 0usize;
    let mut last_instant = start;
    while !feed.is_exhausted() {
        let now = feed
            .cycle_instant(&wall)
            .expect("an unexhausted tape has a next period");
        assert!(
            now > last_instant || cycles == 0,
            "tape time did not advance between cycles"
        );
        last_instant = now;
        let outcome =
            node::step(&mut platform, &mut feed, now, MAXIMUM_BUDGET).expect("a step runs");
        cycles += 1;
        let report = &outcome.report;
        assert!(report.traversed_every_stage(), "a cycle skipped a stage");
        opportunities_found += report.stage(Stage::Discover).map_or(0, |s| s.produced);
        if let Some(reason) = report.stage(Stage::Reason)
            && reason.detail.contains("run(s)")
        {
            panels_convened += 1;
        }
    }
    assert_eq!(cycles, PERIODS, "the loop did not run one cycle per period");
    assert_eq!(
        platform.cycle_count(),
        PERIODS as u64,
        "the platform did not see one cycle per period"
    );
    assert!(
        tape_clock.now().since(start) >= Duration::from_days(24),
        "tape time did not span the tape"
    );

    for prediction in platform.predictions().iter().take(5) {
        eprintln!(
            "prediction {} cycle {} recorded {} resolves {} verdict {:?}",
            prediction.hypothesis,
            prediction.cycle,
            prediction.recorded_at.to_rfc3339(),
            prediction.proposition.resolves_at.to_rfc3339(),
            prediction.verdict
        );
    }
    eprintln!(
        "found {opportunities_found} opportunities, convened {panels_convened} panels, {} predictions, {} scored",
        platform.predictions().len(),
        platform
            .predictions()
            .iter()
            .filter(|p| p.verdict.is_some())
            .count()
    );

    // ----- what the platform made of it ------------------------------------
    assert!(
        opportunities_found >= 1,
        "the detectors found nothing on a tape with a planted jump and a planted drift"
    );
    assert!(
        panels_convened >= 1,
        "no panel was convened on any opportunity"
    );
    let scored = platform
        .predictions()
        .iter()
        .filter(|prediction| prediction.verdict.is_some())
        .count();
    assert!(
        scored >= 1,
        "no claim was scored: tape time passed {} horizon(s) and LEARN graded none",
        platform.predictions().len()
    );
    let calibrations = platform
        .journal_entries()
        .expect("the journal decodes")
        .into_iter()
        .filter(|entry| entry.calibration.is_some())
        .count();
    assert!(
        calibrations >= 1,
        "a claim was scored and no cycle journal entry carries a calibration record"
    );

    // Nothing became live, on a tape or otherwise.
    assert!(!platform.is_live_capable());
    assert!(!platform.orders().has_live_fills());
}

/// Parse the effective confidence the REASON stage prints, if the line
/// carries one.
fn reported_confidence(detail: &str) -> Option<f64> {
    let rest = detail.split("at confidence ").nth(1)?;
    rest.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .ok()
}

fn findings_of<'a>(
    records: &'a [qip_agents::runtime::AgentRunRecord],
    agent: &str,
) -> Vec<&'a AgentFinding> {
    records
        .iter()
        .filter(|record| record.agent_id == agent)
        .filter_map(|record| record.finding.as_ref())
        .collect()
}

#[test]
fn the_macro_arm_feeds_the_macro_analyst_on_the_tape_and_the_review_still_holds_the_bar() {
    // ----- the premise: the tape, and the wiring ---------------------------
    let tape = Tape::open(tape_path()).expect("the committed tape loads");
    assert_tape_premises(&tape);

    let mut feed = Feed::tape(&tape_path().display().to_string()).expect("the tape feed opens");
    let mut platform = platform(&feed);
    // Structurally, before a single bar: the desk's gates read the
    // platform's own slots. This is the wiring every earlier run lacked.
    assert!(
        platform.desk_is_fed(),
        "the desk the agents hold is a copy, not the platform's world and market"
    );

    // ----- the run ---------------------------------------------------------
    let wall = qip_core::SystemClock;
    let mut convened = 0usize;
    let mut best_confidence = 0.0_f64;
    let mut best_line = String::new();
    // The jump's own panel: the first cycle whose queue head is the NWSC
    // opportunity, with what the review printed for it and the instant, so
    // the analysts' findings from that panel can be read back below.
    let mut nwsc_panel: Option<(usize, Timestamp, f64, String)> = None;
    let mut rejections_at_the_bar = 0usize;
    let mut cycle = 0usize;
    while !feed.is_exhausted() {
        let now = feed
            .cycle_instant(&wall)
            .expect("an unexhausted tape has a next period");
        let outcome =
            node::step(&mut platform, &mut feed, now, MAXIMUM_BUDGET).expect("a step runs");
        cycle += 1;
        let reason = outcome
            .report
            .stage(Stage::Reason)
            .expect("REASON reports every cycle");
        if reason.detail.contains("run(s)") {
            convened += 1;
        }
        let line = || {
            let head = platform.queue().first().map_or_else(
                || "an empty queue".to_string(),
                |opportunity| {
                    format!(
                        "{} (detectors {:?}, horizon {:.1}d)",
                        opportunity.headline,
                        opportunity.detectors,
                        opportunity.horizon.as_days_f64()
                    )
                },
            );
            format!(
                "cycle {cycle} on {head}: {}{}",
                reason.detail,
                reason
                    .problems
                    .iter()
                    .map(|p| format!("\n    ! {p}"))
                    .collect::<String>()
            )
        };
        if let Some(confidence) = reported_confidence(&reason.detail) {
            if confidence > best_confidence {
                best_confidence = confidence;
                best_line = line();
            }
            if nwsc_panel.is_none()
                && platform
                    .queue()
                    .first()
                    .is_some_and(|o| o.affected_objects.iter().any(|id| id.as_str() == NWSC))
            {
                nwsc_panel = Some((cycle, now, confidence, line()));
            }
        }
        if reason
            .problems
            .iter()
            .any(|problem| problem.contains("below the 0.500 required"))
        {
            rejections_at_the_bar += 1;
        }
    }
    assert!(convened >= 1, "no panel was convened on the tape");
    let (nwsc_cycle, nwsc_at, nwsc_confidence, nwsc_line) =
        nwsc_panel.expect("premise: a panel convened on the NWSC jump and reviewed a hypothesis");
    eprintln!("the jump's panel — {nwsc_line}");
    eprintln!("best hypothesis on the tape — {best_line}");

    // ----- what the agents saw ---------------------------------------------
    let nwsc = qip_core::ObjectId::from_string(NWSC);
    let bars_on_the_desk = platform
        .market_view()
        .snapshot
        .get(&nwsc)
        .map_or(0, |state| state.bars.len());
    assert!(
        bars_on_the_desk >= 100,
        "the desk holds {bars_on_the_desk} bars for NWSC after {PERIODS} periods; the agents \
         were reading a cold copy"
    );

    let records = platform.organisation().audit().records();

    // The equity analyst, reading the bars it was handed. Premise first — it
    // ran and produced findings at all — then the direction.
    let equity_findings = findings_of(records, ids::EQUITY);
    assert!(
        !equity_findings.is_empty(),
        "the equity analyst produced no finding on {} recorded runs",
        records.len()
    );
    let directional = |findings: &[&AgentFinding]| {
        findings
            .iter()
            .filter(|finding| {
                finding.status == FindingStatus::Complete && finding.direction != Direction::Neutral
            })
            .count()
    };
    assert!(
        directional(&equity_findings) >= 1,
        "the equity analyst took no direction in {} findings; it saw no bars",
        equity_findings.len()
    );

    // The macro analyst, reading the series the macro arm wrote from the
    // tape's releases, keyed by NWSC's economy. Before the vocabulary
    // existed it read `policy_rate@global`, which nothing wrote, and every
    // one of its findings on this tape was no-data. The December print is
    // hawkish on every series, so the direction is negative for a risk
    // asset — the way the jump's overvalued claim leans.
    let macro_findings = findings_of(records, ids::MACRO);
    assert!(
        !macro_findings.is_empty(),
        "the macro analyst produced no finding on {} recorded runs",
        records.len()
    );
    let macro_directional: Vec<&&AgentFinding> = macro_findings
        .iter()
        .filter(|f| f.status == FindingStatus::Complete && f.direction == Direction::Negative)
        .collect();
    assert!(
        !macro_directional.is_empty(),
        "the macro analyst took no negative direction in {} findings; the first is {:?}",
        macro_findings.len(),
        macro_findings.first().map(|f| &f.claim)
    );
    let macro_finding = macro_directional[0];
    eprintln!(
        "macro analyst: {} (conviction {:.2}, evidence {:?})",
        macro_finding.claim, macro_finding.conviction, macro_finding.evidence
    );
    assert!(
        macro_finding.evidence.iter().all(|e| e.ends_with("@US")),
        "the macro evidence is not keyed by NWSC's economy: {:?}",
        macro_finding.evidence
    );

    // The alternative-data analyst finds the series the arm wrote and
    // refuses it: the default licenses nothing, and this repository holds
    // no licensing posture for the `web-traffic` dataset. The refusal names
    // the dataset, so the panel says what would change its answer.
    let alt_findings = findings_of(records, ids::ALT_DATA);
    assert!(
        !alt_findings.is_empty(),
        "the alternative-data analyst produced no finding"
    );
    let refusal = alt_findings[0];
    assert_eq!(refusal.status, FindingStatus::NoView, "{}", refusal.claim);
    assert!(
        refusal.claim.contains("not licensed") && refusal.claim.contains("web-traffic"),
        "the refusal does not name the unlicensed dataset: {}",
        refusal.claim
    );
    eprintln!("alternative-data analyst: {}", refusal.claim);

    // ----- the arithmetic, as the review reported it -----------------------
    // The jump's own panel, cycle 101. Every analyst's stance from that
    // panel is printed from the audit trail so the origins the hypothesis
    // had are on the record, and the macro analyst is among them — the
    // seam this test exists for. At that cycle the equity analyst reads
    // neutral (its trend and reversion cancel on the bar after the jump),
    // so the hypothesis's supporting origins are the anomaly and the macro
    // analyst, with the simulation analyst dissenting; the review's number
    // is 0.34 and the 0.50 bar holds. The macro origin does not lift it
    // above what two origins reached on the bars-only tape (0.36), because
    // the jump's anomaly is now a catalyst-explained move whose own
    // confidence sets the attenuation toward the prior; the arithmetic is
    // the reasoning control (ADR 0005) and is not touched here.
    let at_the_panel: Vec<String> = records
        .iter()
        .filter(|record| record.started_at == nwsc_at)
        .filter_map(|record| record.finding.as_ref())
        .map(|finding| {
            format!(
                "{}: {:?} {:?} conviction {:.2}",
                finding.agent_id, finding.status, finding.direction, finding.conviction
            )
        })
        .collect();
    eprintln!(
        "stances at cycle {nwsc_cycle}:\n  {}",
        at_the_panel.join("\n  ")
    );
    assert!(
        at_the_panel
            .iter()
            .any(|line| line.starts_with(ids::MACRO) && line.contains("Negative")),
        "the macro analyst was not a negative origin at the jump's panel: {at_the_panel:?}"
    );
    assert!(
        (0.30..0.50).contains(&nwsc_confidence),
        "the jump's hypothesis was reviewed at {nwsc_confidence:.3}; this test states 0.34 \
         against the 0.50 bar and must be re-read if the arithmetic moved"
    );
    assert!(
        rejections_at_the_bar >= 1,
        "no cycle reported the review's shortfall against the bar"
    );

    // Later on the tape the review approves a hypothesis — on the MRDN
    // drift, once enough of it has printed — at an effective confidence
    // above the review's 0.50. Approved is not sized: the DECIDE stage
    // holds a thesis to `PlatformConfig::reasoning_confidence_bar`, the
    // panel's own documented resolving power, and nothing on this tape
    // reaches it. So there is no proposal, no order and no fill, and this
    // test says so with the two numbers side by side rather than reading
    // "approved" as "acted on".
    let decide_bar = PlatformConfig::default().reasoning_confidence_bar;
    assert!(
        best_confidence >= 0.50,
        "no hypothesis on the tape was approved by the review: best {best_confidence:.3}"
    );
    assert!(
        best_confidence < decide_bar,
        "the best effective confidence {best_confidence:.3} reaches the {decide_bar:.2} decide \
         bar: a thesis may now be sized on this tape, and this test must grow to assert the \
         proposal, the order, the fill and the LEARN attribution that follow"
    );
    // DECIDE records a proposal every cycle, with no legs on a cycle that
    // sized nothing; the working set is the premise and the legs the claim.
    assert!(
        !platform.proposals().is_empty(),
        "premise: DECIDE recorded no proposal at all"
    );
    let legs: usize = platform.proposals().iter().map(|p| p.len()).sum();
    assert_eq!(
        legs, 0,
        "a thesis was sized below the {decide_bar:.2} decide bar: {legs} leg(s) proposed"
    );
    assert_eq!(
        platform.orders().fills().len(),
        0,
        "a fill was booked on a tape on which nothing was proposed"
    );

    // Nothing became live, on a tape or otherwise.
    assert!(!platform.is_live_capable());
    assert!(!platform.orders().has_live_fills());
}
