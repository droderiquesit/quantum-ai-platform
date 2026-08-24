//! Replay of recorded records from a file.
//!
//! The same adapter interface as a live feed, reading JSONL. Two things depend
//! on it: reproducing a past session exactly, and driving a backtest from
//! captured data rather than from the simulator.
//!
//! # Why a replay is not internally licensed until someone says so
//!
//! A capture file is data this platform did not author. It may be a session
//! this deployment recorded itself, or a vendor's own extract that somebody
//! dropped on a disk, and nothing in the file says which. The file carries no
//! licence, so the adapter has none to report — and the value it used to
//! report in that case was [`LicensingClass::Internal`], which is
//! [`LicensingClass::default`] and for which
//! [`LicensingClass::allows_raw_display`] is **true**. A path handed over by
//! [`std::env`] therefore arrived downstream as permission to show a vendor's
//! raw prices, and [`SourceDescriptor::is_production_grade`] said yes to it as
//! well.
//!
//! So an undeclared replay now reports [`LicensingClass::Restricted`]:
//! non-displayable, derived values only. That is the same answer
//! [`crate::narrative`] and [`crate::alternative`] give a feed configured with
//! no class, and it is chosen over the stricter `Synthetic` because `Synthetic`
//! is a claim of its own — recorded data is not generated data, and a replay
//! of a real session is exactly the thing a backtest is supposed to be allowed
//! to reason from.
//!
//! [`ReplayAdapter::with_licensing`] is how a caller that *does* know says so.
//! It is a statement about the file, made by whoever chose the path.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_events::Topic;
use qip_financial::quality::LicensingClass;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::adapter::{DataAdapter, SensedRecord, SourceDescriptor};

/// What a replay whose licensing nobody declared reports.
///
/// Non-displayable rather than the `Internal` a `LicensingClass::default()`
/// supplies, because a file's silence about its terms is not the file granting
/// any. The same value [`crate::narrative`] and [`crate::alternative`] fall
/// back to for an unconfigured feed.
const UNDECLARED_LICENSING: LicensingClass = LicensingClass::Restricted;

/// Reads [`SensedRecord`]s from a JSONL file in timestamp order.
#[derive(Debug)]
pub struct ReplayAdapter {
    name: String,
    path: PathBuf,
    records: Vec<SensedRecord>,
    cursor: usize,
    /// The class the caller declared, if any. `None` is "nobody said", which
    /// the descriptor reports as `Restricted` rather than as the permissive
    /// `Internal` a `LicensingClass::default()` would have supplied.
    licensing: Option<LicensingClass>,
    /// Malformed lines, reported rather than silently skipped.
    skipped: Vec<String>,
}

impl ReplayAdapter {
    /// Load a JSONL file. One record per line.
    pub fn open(name: impl Into<String>, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)
            .map_err(|e| Error::io(format!("cannot open replay file {}: {e}", path.display())))?;

        let mut records = Vec::new();
        let mut skipped = Vec::new();
        for (number, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            match serde_json::from_str::<SensedRecord>(&line) {
                Ok(record) => records.push(record),
                Err(e) => skipped.push(format!("line {}: {e}", number + 1)),
            }
        }
        // Replay must be in event order regardless of how the file was written.
        records.sort_by_key(|r| r.occurred_at().as_nanos());

        Ok(Self {
            name: name.into(),
            path,
            records,
            cursor: 0,
            // Deliberately not `LicensingClass::Internal`: see the module.
            licensing: None,
            skipped,
        })
    }

    /// Build from records already in memory.
    pub fn from_records(name: impl Into<String>, mut records: Vec<SensedRecord>) -> Self {
        records.sort_by_key(|r| r.occurred_at().as_nanos());
        Self {
            name: name.into(),
            path: PathBuf::new(),
            records,
            cursor: 0,
            // Deliberately not `LicensingClass::Internal`: see the module.
            licensing: None,
            skipped: Vec::new(),
        }
    }

    /// Declare the licensing class of the recorded data.
    ///
    /// The file cannot state its own terms, so this is the only way one gets
    /// stated. Until it is called, [`DataAdapter::descriptor`] reports
    /// [`LicensingClass::Restricted`].
    pub fn with_licensing(mut self, licensing: LicensingClass) -> Self {
        self.licensing = Some(licensing);
        self
    }

    /// The class this replay reports, which is `Restricted` until declared.
    pub fn licensing(&self) -> LicensingClass {
        self.licensing.unwrap_or(UNDECLARED_LICENSING)
    }

    /// Write records to a JSONL file, for later replay.
    pub fn write(path: impl AsRef<Path>, records: &[SensedRecord]) -> Result<usize> {
        use std::io::Write;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(path)?;
        for record in records {
            writeln!(file, "{}", serde_json::to_string(record)?)?;
        }
        Ok(records.len())
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.records.len().saturating_sub(self.cursor)
    }

    /// Lines that could not be parsed.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Timestamp of the first and last record.
    pub fn span(&self) -> Option<(Timestamp, Timestamp)> {
        Some((
            self.records.first()?.occurred_at(),
            self.records.last()?.occurred_at(),
        ))
    }

    /// Rewind to the beginning.
    pub fn reset(&mut self) {
        self.cursor = 0;
    }
}

impl DataAdapter for ReplayAdapter {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: self.name.clone(),
            provider: format!("replay of {}", self.path.display()),
            licensing: self.licensing(),
            topics: Topic::ALL
                .iter()
                .copied()
                .filter(|t| t.group() == qip_events::topic::TopicGroup::Sense)
                .collect(),
            expected_latency: Duration::ZERO,
            production_requirement: None,
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let mut out = Vec::new();
        while self.cursor < self.records.len() {
            let record = &self.records[self.cursor];
            if record.occurred_at() > until {
                break;
            }
            out.push(record.clone());
            self.cursor += 1;
        }
        Ok(out)
    }
}
