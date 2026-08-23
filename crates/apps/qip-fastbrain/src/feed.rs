//! Where records come from.
//!
//! Both sources this build has are [`DataAdapter`]s, which is why this module
//! is thin: the run loop pulls with a timestamp it owns, so the synthetic
//! exchange, a recorded replay and a licensed feed that does not exist yet are
//! the same seam rather than three shapes of loop.
//!
//! Nothing here is production-grade, and the node says so at start-up rather
//! than letting a synthetic tape look like a tape. `is_production_grade` comes
//! off the adapter's own descriptor, so a source that started claiming to be
//! licensed would change this answer without anything here being edited.
//!
//! Every record is validated before the platform sees it. A record that fails
//! is counted and reported rather than dropped: bad data must never silently
//! become an investment input, and a rejection nobody counts is a silent one.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_market::bar::Interval;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
use qip_market_ingestion::replay::ReplayAdapter;
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};

/// One poll's worth of records, split by whether they may be believed.
#[derive(Debug, Default)]
pub struct Batch {
    /// Records that passed validation, ready for the platform.
    pub accepted: Vec<SensedRecord>,
    /// Why each rejected record was rejected.
    pub rejections: Vec<String>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.rejections.is_empty()
    }
}

/// The node's record source.
///
/// An enum rather than a `Box<dyn DataAdapter>` because the loop needs one
/// question the trait does not answer: whether a replay has run out. A replay
/// that has reached its last record is a finished session, and a node that kept
/// cycling on an empty feed would look busy and be idle.
#[derive(Debug)]
pub enum Feed {
    /// The in-tree synthetic exchange. Boxed because the environment carries a
    /// whole market and the enum is moved around the loop.
    Synthetic(Box<SyntheticEnvironment>),
    /// Recorded records from a JSONL file.
    Replay(Box<ReplayAdapter>),
}

impl Feed {
    /// Open the synthetic exchange, seeded so a session is reproducible.
    ///
    /// The bar cadence is chosen from the step rather than left at the
    /// environment's minute default. The platform's SENSE stage reads price
    /// history, and price history is fed by bars: a node cycling ten times a
    /// second against minute bars spends the first six hundred cycles
    /// reporting that it is running blind, and would be right.
    pub fn synthetic(seed: u64, step: Duration, start: Timestamp) -> Self {
        let config = EnvironmentConfig {
            seed,
            step,
            bar_interval: bar_interval_for(step),
            ..EnvironmentConfig::default()
        };
        Self::Synthetic(Box::new(SyntheticEnvironment::demo(start, config)))
    }

    /// Open a recorded feed, refusing a file with nothing in it.
    ///
    /// An empty replay is almost always a wrong path rather than an empty
    /// session, and a node that started and immediately reported "the feed is
    /// exhausted" would have hidden the mistake behind a clean exit.
    pub fn replay(path: &str) -> Result<Self> {
        let adapter = ReplayAdapter::open("replay", path)?;
        if adapter.is_empty() {
            return Err(Error::invalid(format!(
                "the replay file {path} holds no record this node can read; check the path before \
                 checking the node"
            )));
        }
        Ok(Self::Replay(Box::new(adapter)))
    }

    /// Choose a source from the configuration.
    pub fn open(
        replay_path: Option<&str>,
        seed: u64,
        step: Duration,
        start: Timestamp,
    ) -> Result<Self> {
        match replay_path {
            Some(path) => Self::replay(path),
            None => Ok(Self::synthetic(seed, step, start)),
        }
    }

    fn adapter_mut(&mut self) -> &mut dyn DataAdapter {
        match self {
            Self::Synthetic(environment) => environment.as_mut(),
            Self::Replay(adapter) => adapter.as_mut(),
        }
    }

    pub fn descriptor(&self) -> SourceDescriptor {
        match self {
            Self::Synthetic(environment) => environment.descriptor(),
            Self::Replay(adapter) => adapter.descriptor(),
        }
    }

    /// Whether records from this source may drive a real capital decision.
    pub fn is_production_grade(&self) -> bool {
        self.descriptor().is_production_grade()
    }

    /// What a production deployment would still have to supply, if anything.
    pub fn production_requirement(&self) -> Option<String> {
        self.descriptor().production_requirement
    }

    /// Whether this source has nothing left to give.
    ///
    /// Always false for the synthetic exchange: it generates forever, which is
    /// what makes it useful for a node meant to stay up.
    pub fn is_exhausted(&self) -> bool {
        match self {
            Self::Synthetic(_) => false,
            Self::Replay(adapter) => adapter.remaining() == 0,
        }
    }

    /// Pull everything available up to `until`, validating as it goes.
    pub fn poll(&mut self, until: Timestamp) -> Result<Batch> {
        let source = self.descriptor().name;
        let mut batch = Batch::default();
        for record in self.adapter_mut().poll(until)? {
            let issues = record.validate();
            if issues.is_empty() {
                batch.accepted.push(record);
            } else {
                batch.rejections.push(format!(
                    "{source} produced an unusable {}: {}",
                    record.topic().name(),
                    issues.join("; ")
                ));
            }
        }
        Ok(batch)
    }
}

/// The finest bar a step of this size can close.
///
/// A bar closes when the step crosses a bucket boundary, so a bucket shorter
/// than the step would still only produce one bar per step while claiming a
/// resolution the data does not have.
fn bar_interval_for(step: Duration) -> Interval {
    if step < Duration::from_mins(1) {
        Interval::Second
    } else {
        Interval::Minute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_synthetic_exchange_produces_records_and_never_runs_out() {
        let mut feed = Feed::synthetic(7, Duration::from_secs(60), start());
        let batch = feed
            .poll(start().saturating_add(Duration::from_mins(30)))
            .expect("the synthetic exchange polls");
        assert!(
            !batch.accepted.is_empty(),
            "half an hour of synthetic market produced no record"
        );
        assert!(!feed.is_exhausted());
    }

    #[test]
    fn the_same_seed_over_the_same_span_produces_the_same_records() {
        // Reproducibility is the reason the seed is configuration rather than
        // drawn at start-up: a session that cannot be re-run cannot be debugged.
        let until = start().saturating_add(Duration::from_mins(20));
        let mut first = Feed::synthetic(99, Duration::from_secs(60), start());
        let mut second = Feed::synthetic(99, Duration::from_secs(60), start());
        let a = first.poll(until).expect("polls");
        let b = second.poll(until).expect("polls");
        assert_eq!(a.accepted.len(), b.accepted.len());
        assert_eq!(
            serde_json::to_string(&a.accepted).expect("records serialise"),
            serde_json::to_string(&b.accepted).expect("records serialise")
        );
    }

    #[test]
    fn a_node_that_cycles_faster_than_a_minute_is_fed_bars_faster_than_a_minute() {
        // The blindness this prevents: SENSE reads price history, price history
        // is fed by bars, and a bar that closes once a minute leaves a node
        // cycling ten times a second with nothing to sense for six hundred
        // cycles.
        assert_eq!(
            bar_interval_for(Duration::from_millis(100)),
            Interval::Second
        );
        assert_eq!(bar_interval_for(Duration::from_secs(59)), Interval::Second);
        assert_eq!(bar_interval_for(Duration::from_mins(1)), Interval::Minute);

        let mut feed = Feed::synthetic(31, Duration::from_millis(100), start());
        let batch = feed
            .poll(start().saturating_add(Duration::from_secs(5)))
            .expect("polls");
        let bars = batch
            .accepted
            .iter()
            .filter(|record| matches!(record, qip_market_ingestion::adapter::SensedRecord::Bar(_)))
            .count();
        assert!(
            bars > 0,
            "five seconds of a hundred-millisecond feed closed no bar, so the node would sense \
             nothing"
        );
    }

    #[test]
    fn no_source_this_build_has_is_production_grade_and_each_says_what_is_missing() {
        let feed = Feed::synthetic(1, Duration::from_secs(60), start());
        assert!(
            !feed.is_production_grade(),
            "a synthetic tape must never pass for a licensed one"
        );
        assert!(
            feed.production_requirement().is_some(),
            "the synthetic source does not say what production would have to supply"
        );
    }

    #[test]
    fn a_replay_reports_itself_exhausted_once_its_last_record_has_been_read() {
        let mut source = Feed::synthetic(3, Duration::from_secs(60), start());
        let recorded = source
            .poll(start().saturating_add(Duration::from_mins(10)))
            .expect("polls")
            .accepted;
        assert!(!recorded.is_empty(), "nothing was recorded to replay");

        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-replay-{}", std::process::id()));
        let path = directory.join("records.jsonl");
        ReplayAdapter::write(&path, &recorded).expect("the replay file is written");

        let mut feed = Feed::replay(&path.display().to_string()).expect("the replay file opens");
        assert!(!feed.is_exhausted(), "a fresh replay has records left");
        let batch = feed.poll(Timestamp::MAX).expect("polls");
        assert_eq!(batch.accepted.len(), recorded.len());
        assert!(
            feed.is_exhausted(),
            "a replay whose records have all been read still claims to have more"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_replay_file_that_holds_nothing_readable_is_refused_rather_than_opened_empty() {
        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-empty-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the directory is created");
        let path = directory.join("empty.jsonl");
        std::fs::write(&path, "").expect("the file is written");

        let refusal = Feed::replay(&path.display().to_string())
            .expect_err("an empty replay is a wrong path, not an empty session");
        assert!(
            refusal.message().contains("check the path"),
            "the refusal does not point at the likely cause: {}",
            refusal.message()
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
