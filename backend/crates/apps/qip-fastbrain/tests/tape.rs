//! The committed demonstration tape, through the real cycle.
//!
//! What this proves: a node on the tape feed runs one cycle per tape period
//! on tape time, the detectors find the structure planted on the tape, the
//! panel convenes, a falsifiable claim is written down, and — because tape
//! time advances — the claim's horizon passes on the tape and LEARN scores
//! it into a calibration record. Every assertion is premise-first: the tape's
//! own shape is asserted before anything the platform made of it.
//!
//! What this does not prove, stated so nobody reads it as proven: no order
//! and no fill. Every hypothesis is rejected on review, and the arithmetic
//! is structural rather than a matter of data — see `qip_fastbrain::feed`'s
//! tape documentation and the register on this test.

use qip_core::{Clock, Duration, Timestamp};
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig, Stage};
use qip_market_ingestion::tape::Tape;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::path::PathBuf;
use std::sync::Arc;

const NWSC: &str = "OBJ00000000000000000NWSC";
const MRDN: &str = "OBJ00000000000000000MRDN";

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

#[test]
fn the_demonstration_tape_drives_the_loop_through_tape_time_to_a_scored_claim() {
    // ----- the premise: what is on the tape --------------------------------
    let tape = Tape::open(tape_path()).expect("the committed tape loads");
    assert_eq!(tape.periods(), 320, "the tape does not hold 320 periods");
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

    // ----- the run: one cycle per tape period, on tape time ----------------
    let mut feed = Feed::tape(&tape_path().display().to_string()).expect("the tape feed opens");
    let tape_clock = feed.owned_clock().expect("a tape owns its clock");
    let start = tape_clock.now();
    let config = PlatformConfig::default();
    let clock: Arc<dyn Clock> = tape_clock.clone();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    // The catalogue is read at the wall clock, as `main.rs` reads it: it is
    // dated 2026 and refuses to be ingested before it happened, and the tape
    // is from 2025. The roster and the cycles run on tape time.
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(qip_core::SystemClock.now()),
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles");

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
        for opportunity in platform.queue() {
            eprintln!(
                "cycle {cycles} queue: {} horizon {:.1}d detectors {:?}",
                opportunity.headline,
                opportunity.horizon.as_days_f64(),
                opportunity.detectors
            );
        }
    }
    assert_eq!(cycles, 320, "the loop did not run one cycle per period");
    assert_eq!(
        platform.cycle_count(),
        320,
        "the platform did not see one cycle per period"
    );
    assert!(
        tape_clock.now().since(start) >= Duration::from_days(13),
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
