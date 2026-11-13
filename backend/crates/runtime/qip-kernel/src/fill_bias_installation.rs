//! The paper fill model's installed fill-bias corrections (R12-EXPAND-6050).
//!
//! A research campaign (`qip_expansion::executor::run_fill_bias_campaign`) that
//! beats the uncorrected twin in its shadow window installs a constant
//! entry-price correction for a venue. Before this module that installation was
//! a *record on the campaign result* and nothing acted on it: the deep brain
//! read the platform behind a shared reference and counted the installation,
//! so "installed in the paper fill model" named the instant the paper window
//! opened rather than a correction the model applied — and the paper window's
//! fills were the platform's own pre-existing fills, not fills produced under
//! the correction. A control that reads as an effect while changing nothing is
//! this repository's recurring failure mode (`MaxExpectedShortfall`, a limit
//! that could never fire); this closes it for the expansion loop's paper fills.
//!
//! [`InstalledFillBias`] is one venue's installed correction. The platform
//! holds a set of them in [`InstalledFillBiases`], applies each to the signed
//! fill-error it records for that venue's scored paper fills
//! (`Platform::score_filled`), and journals every install and withdrawal so
//! the installed set is the left fold of its own log records and [`resume`]
//! rebuilds it from the log alone. A replay therefore reproduces the installed
//! correction, which is what the row asks for: the act is on the log, not only
//! in memory.
//!
//! **The reader, and its one limit.** The correction shifts the *statistic*
//! the twin records about a fill — `FillScore::trade_error_bps`, the signed
//! basis points between the twin's predicted entry and the venue's — toward
//! zero by the fitted bias. It subtracts from the twin's prediction, never
//! from a price anything trades at: no money figure moves, no order is sized,
//! no venue is withdrawn or released on it. It only ever corrects a diagnostic
//! the next campaign reads, so a wrong correction makes the next campaign's
//! residuals wrong and is caught there, not on a realised line.

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer on every install/withdraw record, told apart from the
/// promotion, grant, hold, versions and microstructure-fold records that share
/// [`Topic::ComplianceEvaluated`]. [`resume`] filters on it, so a record of
/// another producer on the same topic is never decoded as one of these.
pub const FILL_BIAS_INSTALLATION_ORIGIN: &str = "kernel/fill-bias-installation";

/// One venue's installed fill-bias correction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstalledFillBias {
    pub venue: String,
    /// Basis points subtracted from the twin's predicted entry at this venue:
    /// the mean signed fill residual the campaign fitted. The sign is the
    /// finding; applying it shifts the recorded residual toward zero exactly
    /// when the twin was biased the way the fit measured.
    pub bias_bps: f64,
    /// The campaign whose shadow window cleared the bar and installed it.
    pub campaign: String,
    pub installed_at: Timestamp,
}

/// One install-or-withdraw fact as the log carries it: re-derivable from the
/// record alone, so [`resume`] needs nothing beside the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillBiasInstallationRecord {
    pub venue: String,
    pub bias_bps: f64,
    pub campaign: String,
    /// `true` installs the correction, `false` withdraws it. A withdrawal
    /// carries the bias and campaign of the installation it removes, so the
    /// record reads whole without the install beside it.
    pub installed: bool,
    pub at: Timestamp,
}

impl EventBody for FillBiasInstallationRecord {
    /// A fact that changes the residual the next campaign reads, so in the
    /// group the log never evicts, beside the grant, hold and microstructure
    /// records: a record evicted from the log is a state [`resume`] cannot
    /// rebuild.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "fill-bias-installation:{}:{}:{}",
            self.venue,
            self.installed,
            self.at.to_rfc3339()
        ))
    }
}

/// The installed set: per venue, the correction in force. A left fold of the
/// install/withdraw records, so two platforms that read the same log hold the
/// same set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstalledFillBiases {
    by_venue: BTreeMap<String, InstalledFillBias>,
}

impl InstalledFillBiases {
    /// The correction installed for `venue`, if any.
    pub fn get(&self, venue: &str) -> Option<&InstalledFillBias> {
        self.by_venue.get(venue)
    }

    /// The basis points the correction subtracts from the twin's predicted
    /// entry at `venue`, if one is installed.
    pub fn bias_bps(&self, venue: &str) -> Option<f64> {
        self.by_venue.get(venue).map(|installed| installed.bias_bps)
    }

    /// The venues with a correction installed, in venue order.
    pub fn venues(&self) -> impl Iterator<Item = &String> {
        self.by_venue.keys()
    }

    pub fn len(&self) -> usize {
        self.by_venue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_venue.is_empty()
    }

    /// Fold one record: an install inserts or replaces, a withdrawal removes.
    /// A withdrawal of a venue holding nothing is a no-op, which is the right
    /// reading of a campaign whose paper window refused a correction no
    /// earlier sweep had installed.
    pub(crate) fn apply(&mut self, record: &FillBiasInstallationRecord) {
        if record.installed {
            self.by_venue.insert(
                record.venue.clone(),
                InstalledFillBias {
                    venue: record.venue.clone(),
                    bias_bps: record.bias_bps,
                    campaign: record.campaign.clone(),
                    installed_at: record.at,
                },
            );
        } else {
            self.by_venue.remove(&record.venue);
        }
    }
}

/// The installed set rebuilt from the log: every install/withdraw record, in
/// log order, folded again. A record the fold cannot decode stops the resume
/// rather than being skipped — a set rebuilt around a record it could not read
/// is not the set the platform acted on.
pub fn resume(log: &EventLog) -> Result<InstalledFillBiases> {
    let mut state = InstalledFillBiases::default();
    for event in log.events() {
        if event.topic != FillBiasInstallationRecord::TOPIC
            || event.lineage.producer != FILL_BIAS_INSTALLATION_ORIGIN
        {
            continue;
        }
        let record = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<FillBiasInstallationRecord>())
            .map_err(|error| {
                Error::invalid(format!(
                    "a fill-bias installation record did not decode: {}",
                    error.message()
                ))
            })?
            .body;
        state.apply(&record);
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(venue: &str, bias: f64, installed: bool, at: i64) -> FillBiasInstallationRecord {
        FillBiasInstallationRecord {
            venue: venue.to_string(),
            bias_bps: bias,
            campaign: format!("fill bias at {venue}"),
            installed,
            at: Timestamp::from_secs(at),
        }
    }

    #[test]
    fn an_install_then_a_withdrawal_leaves_the_venue_with_no_correction() {
        let mut state = InstalledFillBiases::default();
        state.apply(&record("XNYS", 12.0, true, 1));
        assert_eq!(state.bias_bps("XNYS"), Some(12.0), "the install took");
        assert_eq!(state.len(), 1);
        state.apply(&record("XNYS", 12.0, false, 2));
        assert_eq!(
            state.bias_bps("XNYS"),
            None,
            "the withdrawal left the correction in force"
        );
        assert!(state.is_empty());
    }

    #[test]
    fn a_second_install_replaces_the_first_and_a_withdrawal_of_nothing_is_a_no_op() {
        let mut state = InstalledFillBiases::default();
        state.apply(&record("XNYS", 12.0, true, 1));
        state.apply(&record("XNYS", -3.0, true, 2));
        assert_eq!(
            state.bias_bps("XNYS"),
            Some(-3.0),
            "the later install stands"
        );
        state.apply(&record("XLON", 5.0, false, 3));
        assert_eq!(
            state.len(),
            1,
            "a withdrawal of an uninstalled venue grew the set"
        );
    }
}
