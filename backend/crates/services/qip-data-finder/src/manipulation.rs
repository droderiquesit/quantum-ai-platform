//! Manipulation risk: whether what a source serves can be trusted to be what
//! it says it is (DATA-016).
//!
//! Six of the seven things a candidate is inspected for were already read off
//! the probe — access method, terms, schema, freshness, cost, uniqueness.
//! This is the seventh, and until it existed a source whose records were
//! dated after the instant they were fetched registered like any other: the
//! freshness score *rewards* a payload stamped in the future, because its age
//! clamps to zero. A bitemporal store that admits such a record has a fact
//! "true" before it was knowable, which is point-in-time leakage arriving
//! through the front door.
//!
//! Pure over one probing's evidence. It reads nothing but what the probe
//! observed and decides nothing but whether the source may be registered.

use crate::source::Source;
use qip_core::Duration;

/// How far ahead of the probe's own clock a source's timestamp may sit before
/// it is read as a claim about the future rather than as clock skew.
pub const CLOCK_SKEW_TOLERANCE: Duration = Duration::from_mins(5);

/// What one probing shows about whether a source can be manipulated, or is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManipulationRisk {
    /// Findings that stop registration.
    flagged: Vec<String>,
    /// Findings recorded and scored elsewhere, which do not stop it.
    elevated: Vec<String>,
}

impl ManipulationRisk {
    /// Inspect a probed source.
    // ponytail: two signals a single probing can observe. A source that
    // rewrites an extent after use is caught later by the reference ledger
    // (`ledger::LedgerOutcome::Revised`), not here; add cross-probe signals
    // when a registered source's earlier evidence is compared at this seam.
    pub fn inspect(source: &Source) -> Self {
        let evidence = source.evidence();
        let observed = evidence.observed_at();
        let horizon = observed.saturating_add(CLOCK_SKEW_TOLERANCE);
        let mut flagged = Vec::new();
        for (what, stamp) in [
            ("its newest record", evidence.sample().payload_at()),
            ("its last-modified time", evidence.head().last_modified),
        ] {
            if let Some(stamp) = stamp
                && stamp > horizon
            {
                flagged.push(format!(
                    "{what} is dated {stamp}, after it was fetched at {observed}; a source \
                     cannot have observed an instant that has not happened"
                ));
            }
        }
        let mut elevated = Vec::new();
        if !source.endpoint().scheme().is_encrypted() {
            elevated
                .push("served over plaintext, so anything on the path can rewrite it".to_string());
        }
        Self { flagged, elevated }
    }

    /// False when a finding stops registration.
    pub fn permits_registration(&self) -> bool {
        self.flagged.is_empty()
    }

    pub fn flagged(&self) -> &[String] {
        &self.flagged
    }

    pub fn elevated(&self) -> &[String] {
        &self.elevated
    }

    pub fn describe(&self) -> String {
        match (self.flagged.is_empty(), self.elevated.is_empty()) {
            (true, true) => "manipulation risk low: nothing observed".to_string(),
            (true, false) => format!("manipulation risk elevated: {}", self.elevated.join("; ")),
            (false, _) => format!("manipulation risk flagged: {}", self.flagged.join("; ")),
        }
    }
}
