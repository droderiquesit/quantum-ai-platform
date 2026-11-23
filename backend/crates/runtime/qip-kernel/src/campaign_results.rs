//! The event-log record of what a research campaign's strategy backtest
//! produced (R12-REASON-3003, §22.4's "emit results, statistics, manifest").
//!
//! A campaign's manifest — what was fetched, verified and sketched — is
//! journaled as a `ResearchCampaignClosed` record the instant the campaign
//! closes, before a single candidate is scored on its window. That ordering
//! is deliberate and is kept: a round that fails mid-search still leaves on
//! the log what its window was. But it meant the *results* were nowhere the
//! log could see. The campaign id reached only the deep brain's in-memory
//! round summary and its cycle line; the factory journals no registration;
//! and `Topic::StrategyCreated` had no production writer. So a regulator's
//! question — "which strategies did this campaign's backtest produce, and on
//! exactly which bars were they scored?" — could be answered about the
//! learning desk (its model card carries the campaign id) and not about the
//! strategy search at all.
//!
//! This record is the other half of the arrow. It is written once per round
//! after the backtest, under the campaign id the round's window came
//! through, and it carries:
//!
//! * the [`DatasetManifest`] of the bars the backtest *consumed* — taken
//!   from the slice the search scored, not from the campaign's account of
//!   what it fetched, so a backtest that ran on anything other than the
//!   campaign's window is a record whose hash disagrees with the manifest
//!   entry's extent rather than one that silently agrees;
//! * every registered candidate's strategy id, the dataset manifest its own
//!   holdout evidence carries, and what the gate made of it;
//! * the round's counts and the trial count every holdout was deflated by.
//!
//! It is filed under `Topic::StrategyCreated` — the strategies a search
//! created, with their per-strategy evidence, which is that topic's
//! compact-derived retention row — and is keyed on the campaign id, so one
//! campaign has one results record however often the journal is retried.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_events::{EventBody, EventFilter, Topic};
use qip_lifecycle::evidence::DatasetManifest;
use serde::{Deserialize, Serialize};

/// What the holdout gate made of one registered candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateVerdict {
    /// The gate admitted it to the first rung of the ladder.
    Promoted,
    /// The gate read its evidence and refused it.
    GateRefused,
    /// Registered and deliberately not judged, because the subject's data
    /// class is backed by fewer independent sources than §22.3 requires.
    HeldBack,
}

/// One candidate the round registered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateResult {
    pub strategy_id: String,
    /// The bars this candidate's holdout evidence was simulated over, as the
    /// evidence itself records them.
    pub window: DatasetManifest,
    pub verdict: CandidateVerdict,
}

/// One strategy round's results, under the campaign its window came through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchCampaignResults {
    /// The `ResearchCampaignClosed` record this round's window came through.
    pub campaign_id: String,
    pub subject: String,
    pub at: Timestamp,
    /// The bars the backtest consumed.
    pub window: DatasetManifest,
    pub proposed: usize,
    pub registered: usize,
    pub discarded: usize,
    pub promoted: usize,
    pub gate_refused: usize,
    pub held_back: usize,
    /// The search's cumulative trial count after the round.
    pub trials: usize,
    /// Every registered candidate, in the order the round scored them.
    pub candidates: Vec<CandidateResult>,
}

impl EventBody for ResearchCampaignResults {
    const TOPIC: Topic = Topic::StrategyCreated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("campaign-results:{}", self.campaign_id))
    }
}

impl Platform {
    /// Journal one round's results. Returns whether a record was written:
    /// `false` means the log already holds results under this campaign id.
    pub fn journal_campaign_results(&mut self, results: ResearchCampaignResults) -> Result<bool> {
        let at = results.at;
        self.journal_once(results, "kernel/campaign-results", at)
    }

    /// Every results record the log holds, in log order.
    pub fn campaign_results(&self) -> Result<Vec<ResearchCampaignResults>> {
        self.replay_journal(&EventFilter::new().topic(Topic::StrategyCreated))?
            .iter()
            .map(|envelope| Ok(envelope.decode::<ResearchCampaignResults>()?.body))
            .collect()
    }
}
