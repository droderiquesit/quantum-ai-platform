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
//!
//! The third test is the one that closes finding M4. Every test in
//! `qip-kernel/tests/valuation_seam.rs` lowers `ReviewPolicy`'s shipped 0.50
//! floor to 0.10 to reach the sizing seam at all, so that file proves the
//! narrowing arithmetic is *correct* and proves nothing about whether it is
//! *reached*. This file's platform is `PlatformConfig::default()` and
//! `LimitSet::conservative_default()` — the shipped configuration, which no
//! app overrides — so a construction reached here is a construction a
//! deployment reaches, and the budget it is handed is the budget a deployment
//! would size against.

use qip_agents::finding::{AgentFinding, Direction, FindingStatus};
use qip_core::{Clock, Decimal, Duration, Timestamp};
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
    // above the review's 0.50. Approved is still not sized, and the reason
    // is the optimiser, not a confidence bar.
    //
    // What stood here said the DECIDE stage "holds a thesis to
    // `PlatformConfig::reasoning_confidence_bar`". It does not and never
    // did: `stage_decide` sizes whatever REASON approved and compares
    // nothing to a bar, and when that sentence was written nothing in
    // `qip-kernel` read the field at all. The assertion under it —
    // `best_confidence < 0.90` against panel confidences near 0.5 — was
    // near-vacuous, and it named a control that did not exist as the cause
    // of a number it did not cause. The bar now has one reader,
    // `Platform::reason_decision_context`, where it caps the *routing*
    // requirement; it decides which rung answers the question and has
    // nothing to say about whether an answer is sized.
    //
    // The real reason is on the record the cycles left. Every proposal on
    // this tape is one of two rationales: 149 cycles had no pending thesis
    // at all, and the 107 that did reached the construction and got an
    // infeasible solution back from the mandate-constrained optimiser —
    // "expresses 1 approved thesis(es) at -0.0% gross, sized by
    // quadratic_program: no feasible solution found". A proposal with an
    // infeasible solution has no legs. That is what this test asserts now,
    // which is a claim about the platform rather than about a constant.
    assert!(
        best_confidence >= 0.50,
        "no hypothesis on the tape was approved by the review: best {best_confidence:.3}"
    );
    // DECIDE records a proposal every cycle, with no legs on a cycle that
    // sized nothing; the working set is the premise and the legs the claim.
    assert!(
        !platform.proposals().is_empty(),
        "premise: DECIDE recorded no proposal at all"
    );
    // Premise: some cycle really did reach the construction with an approved
    // thesis. Without it the claim below would hold on a tape that approved
    // nothing, and "no legs" would prove only that the panel never agreed.
    let constructed: Vec<&str> = platform
        .proposals()
        .iter()
        .map(|proposal| proposal.rationale.as_str())
        .filter(|rationale| rationale.starts_with("expresses "))
        .collect();
    assert!(
        !constructed.is_empty(),
        "premise: no approved thesis ever reached the construction, so this tape says nothing \
         about why nothing was sized"
    );
    assert!(
        constructed
            .iter()
            .all(|rationale| rationale.contains("no feasible solution found")),
        "a construction on this tape found a feasible solution, so a thesis may now be sized \
         here and this test must grow to assert the proposal, the order, the fill and the LEARN \
         attribution that follow: {:?}",
        constructed
            .iter()
            .find(|rationale| !rationale.contains("no feasible solution found"))
    );
    let legs: usize = platform.proposals().iter().map(|p| p.len()).sum();
    assert_eq!(
        legs,
        0,
        "{} construction(s) came back infeasible and {legs} leg(s) were proposed anyway",
        constructed.len()
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

// --- the valuation seam, under the policy a deployment actually ships --------

/// The book `PlatformConfig::default()` opens with.
///
/// Stated here rather than read back off the platform. Every figure below is
/// a fraction of it, and an expectation computed from the same accessor the
/// assertion checks would move with any mutation and pin nothing — three
/// magnitude-level mutations survived `valuation_seam.rs` once for exactly
/// that reason.
const BOOK: i64 = 10_000_000;

/// What `construct_from` hands the optimiser while the self-model has never
/// absorbed a graded outcome.
///
/// §6.2 as the centre reads it on this tape: the causal graph has absorbed no
/// claim, so row 2 reads `Unavailable` and takes 0.75; the belief state was
/// written by this cycle's own REASON stage, so row 4 is fresh and takes
/// nothing; the self-model has graded nothing, so row 6 reads `Unavailable`
/// and halves. 10,000,000 x 0.75 x 0.50.
const BUDGET_BEFORE_LEARNING: i64 = 3_750_000;

/// And once LEARN has scored a claim into a calibration record.
///
/// Row 6 becomes `Stale` rather than `Unavailable` — the self-model has
/// absorbed an outcome, just not a recent enough one — so it takes 0.75
/// instead of 0.50. 10,000,000 x 0.75 x 0.75. The three-valued `Freshness`
/// exists to keep those two apart, and this is the only assertion in the tree
/// that watches the centre's budget cross between them on a running platform.
const BUDGET_AFTER_LEARNING: i64 = 5_625_000;

#[test]
fn the_shipped_review_policy_admits_a_thesis_that_reaches_the_narrowed_sizing_budget() {
    // Finding M4, closed here. Every test in
    // `qip-kernel/tests/valuation_seam.rs` lowers `ReviewPolicy`'s shipped
    // `minimum_surviving_confidence` from 0.50 to 0.10, because on a synthetic
    // single-name tape the panel tops out near 0.37 and no thesis is ever
    // approved. Raise that fixture constant back to the shipped value and all
    // five of those tests fail on their own premise — "the listed cycle
    // proposed no legs ... rationale: no thesis cleared the action bar this
    // cycle" — so what they prove is that the narrowing arithmetic is right if
    // reached, and nothing in the tree proved it was reached. A narrowing
    // chain in the sizing path that no shipped configuration enters is a
    // control nobody has proven fires.
    //
    // This platform is `PlatformConfig::default()` and
    // `LimitSet::conservative_default()`, unmodified — `grep -rn
    // minimum_surviving_confidence backend/crates` finds no app that overrides
    // the review policy — so every construction counted below is one a
    // deployment on this tape would reach.
    let tape = Tape::open(tape_path()).expect("the committed tape loads");
    assert_tape_premises(&tape);
    let mut feed = Feed::tape(&tape_path().display().to_string()).expect("the tape feed opens");
    let mut platform = platform(&feed);

    // ----- the premise: the policy under test is the shipped one -----------
    // The crux of the finding. If this drifts, everything below is about some
    // other platform than the one that deploys.
    let bar = platform.config().review.minimum_surviving_confidence;
    assert!(
        (bar - 0.50).abs() < 1e-12,
        "the premise failed: this platform's surviving-confidence floor is {bar}, not the shipped \
         0.50, so nothing below says anything about a deployed configuration"
    );
    assert_eq!(
        platform.config().initial_equity,
        Decimal::from_int(BOOK),
        "the premise failed: the shipped book is not the {BOOK} every figure below is a fraction of"
    );

    // ----- the run ---------------------------------------------------------
    // Each cycle that reached the construction, with the budget it was handed
    // and whether LEARN had already written a calibration *before* that cycle
    // began. Read before the step, not after: DECIDE runs before LEARN inside
    // one cycle, so a calibration written by cycle N is not a fact DECIDE had
    // at cycle N, and partitioning on the value after the step would put one
    // construction on the wrong side of the boundary for a reason that is
    // about stage order rather than about sizing.
    let wall = qip_core::SystemClock;
    let mut constructions: Vec<(usize, Decimal, bool)> = Vec::new();
    let mut cycle = 0usize;
    while !feed.is_exhausted() {
        let now = feed
            .cycle_instant(&wall)
            .expect("an unexhausted tape has a next period");
        let calibrated_before = platform.calibration().is_some();
        let _ = node::step(&mut platform, &mut feed, now, MAXIMUM_BUDGET).expect("a step runs");
        cycle += 1;
        if let Some(proposal) = platform.proposals().last()
            && proposal.rationale.starts_with("expresses ")
        {
            constructions.push((cycle, proposal.equity.amount, calibrated_before));
        }
    }

    // ----- the consequence -------------------------------------------------
    // The finding itself: the seam is entered, under the shipped floor, by a
    // thesis the shipped red team approved.
    assert!(
        !constructions.is_empty(),
        "no cycle in {cycle} reached the construction under the shipped review policy, so \
         `deployable_capital`, `central_degradation` and `mark_confidence_multiplier` are still \
         unreached by any configuration a deployment uses — finding M4 is open again"
    );
    // Premise for the arithmetic: nothing filled, so the tracked book is the
    // whole book and the budget's shortfall against it is the narrowing and
    // not a drawdown, a hold, or an unfunded commitment coming off free
    // capital first.
    assert_eq!(
        platform.orders().fills().len(),
        0,
        "the premise failed: a fill moved the book the budgets below are fractions of"
    );
    assert_eq!(
        platform.equity(),
        Decimal::from_int(BOOK),
        "the premise failed: the tracked book is no longer {BOOK}"
    );

    // And by exactly how much, on both sides of the self-model row. Stated as
    // fractions of a book asserted above, so halving or doubling either §6.2
    // constant moves the budget and fails here — the mutation class that
    // survived `valuation_seam.rs`, where every assertion had the platform's
    // own reading on both sides.
    let before = Decimal::from_int(BUDGET_BEFORE_LEARNING);
    let after = Decimal::from_int(BUDGET_AFTER_LEARNING);
    let uncalibrated: Vec<&(usize, Decimal, bool)> =
        constructions.iter().filter(|entry| !entry.2).collect();
    let calibrated: Vec<&(usize, Decimal, bool)> =
        constructions.iter().filter(|entry| entry.2).collect();
    assert!(
        !uncalibrated.is_empty() && !calibrated.is_empty(),
        "the premise failed: the tape's {} construction(s) all sit on one side of the self-model \
         row, so the crossing below is not observed at all",
        constructions.len()
    );
    for (at, budget, _) in &uncalibrated {
        assert_eq!(
            *budget, before,
            "cycle {at} sized against {budget} before LEARN had graded anything; §6.2 with the \
             self-model unavailable is 0.75 x 0.50 of {BOOK}"
        );
    }
    for (at, budget, _) in &calibrated {
        assert_eq!(
            *budget, after,
            "cycle {at} sized against {budget} after LEARN wrote a calibration; §6.2 with the \
             self-model stale rather than absent is 0.75 x 0.75 of {BOOK}"
        );
    }
    // The direction, stated separately: a self-model that has absorbed an
    // outcome must widen the budget, never narrow it. An arithmetic that
    // swapped the stale and unavailable constants would satisfy neither loop
    // above, but a future one that made them equal would satisfy both and
    // erase the distinction `Freshness` is three-valued to keep.
    assert!(
        before < after,
        "the self-model row grants no more budget once it has graded an outcome: {before} then \
         {after}"
    );
    // Narrowed at all, against the book rather than against another reading.
    assert!(
        after < Decimal::from_int(BOOK),
        "the widest budget the centre handed the optimiser is the whole book, so §6.2 narrowed \
         nothing: {after}"
    );

    // ----- what this still does not prove, said out loud -------------------
    // No leg. Every construction on this tape comes back infeasible, and the
    // reason is not the valuation plane: the approved theses are all
    // `Claim::Overvalued`, because the structural break the review clears is
    // an upward drift and `mechanism_for` maps a positive z-score to
    // overvalued. `thesis_from` signs conviction by the claim, so the thesis
    // is a short, and `conservative_default`'s long-only mandate has no
    // feasible solution for it. What would have to change for a leg to be
    // sized under the shipped policy is a tape carrying a *downward*
    // dislocation whose panel still clears 0.50 — the jump section reaches
    // only 0.34 today. Until such a tape exists, the seam is proven reached
    // and the optimiser's output is proven empty, which are two facts and not
    // one.
    assert!(
        constructions
            .iter()
            .all(|(_, budget, _)| budget.is_positive()),
        "a construction was handed nothing to size against, so the infeasibility below would be \
         about an empty budget rather than about the mandate"
    );
    let legs: usize = platform.proposals().iter().map(|p| p.len()).sum();
    assert_eq!(
        legs, 0,
        "a leg was sized on this tape. That is progress, not a failure — but this test now \
         understates what the tape proves, and must grow to assert the proposal, the order, the \
         fill and the LEARN attribution that follow it"
    );

    // The boundary. Nothing here went near a venue.
    assert!(!platform.is_live_capable());
    assert!(!platform.orders().has_live_fills());
}
