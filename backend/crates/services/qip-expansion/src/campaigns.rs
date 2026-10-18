//! Step 1 of a tick-replay campaign: notice that a microstructure residual has
//! grown past its threshold, or that a venue nobody has seen before has
//! appeared, and open a [`TickCampaign`] scoped to what was affected
//! (EXPAND-017).
//!
//! A residual that grows without anyone investigating it becomes the loss
//! nobody could explain. This module decides that a campaign is owed and,
//! through [`observations`], states the campaigns a sweep opened as the
//! [`Observation`]s the curriculum queues research from (RM-P9-08). It
//! replays nothing and schedules nothing: the desk decides what is queued
//! and what starts. When a campaign's residuals are re-measured,
//! [`TickCampaignDetector::conclude`] states what the replay found as the
//! §27 `MicrostructureDelta`, under the campaign's own id.
//!
//! Fed by the deep brain's research loop (`qip-deepbrain/src/research.rs`)
//! from the kernel's scored fills: each fill's trade error is a fill
//! prediction residual and each venue it names is a sighting. Lead/lag and
//! world-event reaction residuals have an observer here
//! ([`TickCampaignDetector::observe_residual`]) and no producer yet, because
//! nothing in the tree measures either. Whether a composition root hands the
//! campaigns it opens to [`observations`] is the root's to show, and a
//! command shows it where a sentence here would go stale:
//! `grep -rn 'campaigns::observations' backend/crates/apps`.

use crate::gap::{GapTrigger, Observation, Wanting};
use qip_contracts::microstructure_delta::{
    MicrostructureDelta, MicrostructureDeltaFields, Residual, ResidualKind,
};
use qip_contracts::validated::Contract;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A campaign to replay and study tick-level microstructure after a residual
/// anomaly or a new venue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickCampaign {
    /// Identifier, issued in sequence by the detector that opened it, so the
    /// same observations replayed through a fresh detector name the same
    /// campaigns. A wall-clock identifier would make a replay disagree with
    /// the record it is replaying.
    pub campaign_id: String,
    /// Why this campaign was opened.
    pub trigger: TickCampaignTrigger,
    /// Instruments affected by the residual or the new venue.
    pub affected_instruments: BTreeSet<String>,
    /// The venue affected, if venue-specific.
    pub venue: Option<String>,
    /// The observed residual that opened the campaign; `None` for a new venue.
    pub residual_value: Option<Decimal>,
    /// The threshold the residual's magnitude exceeded; `None` for a new
    /// venue, which has no threshold rather than a threshold of zero.
    pub threshold: Option<Decimal>,
}

/// The reason a [`TickCampaign`] was opened. Ordered by declaration, which is
/// the order [`observations`] states a sweep's campaigns in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TickCampaignTrigger {
    /// Unexplained slippage exceeds the threshold.
    UnexplainedSlippage,
    /// Fill prediction error exceeds the threshold.
    FillPredictionError,
    /// Queue prediction error exceeds the threshold.
    QueuePredictionError,
    /// Lead/lag relationship anomaly detected.
    AbnormalLeadLag,
    /// World-event reaction residuals grow.
    WorldEventReaction,
    /// Divergence between paper fills and replayed fills.
    PaperReplayDivergence,
    /// A venue not previously seen.
    NewVenue,
}

impl TickCampaignTrigger {
    /// Why the expansion loop opens on a campaign of this trigger, which
    /// decides whether its research is ranked as reducing a risk the platform
    /// runs (`desk::is_failure`). The four residuals of the platform's own
    /// execution are its models failing to explain its own results; a
    /// lead/lag or world-event reaction residual is the market departing from
    /// what the world model expected; a new venue is one no brain or tool can
    /// yet answer for, as `detectors::coverage` files an asset class with no
    /// history.
    const fn gap_trigger(self) -> GapTrigger {
        match self {
            Self::UnexplainedSlippage
            | Self::FillPredictionError
            | Self::QueuePredictionError
            | Self::PaperReplayDivergence => GapTrigger::UnexplainedPnl,
            Self::AbnormalLeadLag | Self::WorldEventReaction => GapTrigger::Surprise,
            Self::NewVenue => GapTrigger::UnsupportedRequest,
        }
    }
}

/// One instrument's residual as the concluding replay re-measured it: what
/// the model said and what the venue did, in the kind's own unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remeasured {
    pub instrument: String,
    pub modelled: Decimal,
    pub observed: Decimal,
    pub samples: u64,
}

impl TickCampaignTrigger {
    /// The §27 residual family a campaign of this trigger re-measures. A
    /// queue prediction residual is a fill-probability one, because queue
    /// position is what the fill model prices; a world-event reaction, a
    /// paper-versus-replay divergence and a new venue are the venue's own
    /// behaviour.
    pub const fn residual_kind(self) -> ResidualKind {
        match self {
            Self::UnexplainedSlippage => ResidualKind::Slippage,
            Self::FillPredictionError | Self::QueuePredictionError => ResidualKind::FillProbability,
            Self::AbnormalLeadLag => ResidualKind::LeadLag,
            Self::WorldEventReaction | Self::PaperReplayDivergence | Self::NewVenue => {
                ResidualKind::VenueBehaviour
            }
        }
    }
}

/// The first evidence id of every observation [`observations`] states, so the
/// queued item's derivation names this detector as the rule that raised it
/// (`plan::Derivation::of` takes a leading `detector:` id as the rule), as
/// each detector in `detectors.rs` names its own.
pub const ORIGIN: &str = "detector:tick_campaign";

/// The campaigns one sweep opened, as the observations the curriculum queues
/// research from (RM-P9-08): one per trigger and venue, each wanting
/// execution, so each is raised as an execution gap whose acquisition is
/// `plan::AcquisitionAction::ReplayTicks` — the tick replay the campaign is.
///
/// The failure this closes is a campaign opened and only counted. The counter
/// rises, the curriculum never hears of the campaign, and the residual that
/// opened it stays the loss nobody could explain, which is the outcome the
/// detector exists to prevent.
///
/// One observation per trigger and venue, not one per campaign. The detector
/// opens a campaign on every residual past its threshold, so a venue whose
/// twin misses a dozen fills opens a dozen; stated one by one they would be a
/// dozen signals for one question, each weighed against the attention budget
/// before the desk refused eleven of them as duplicates. The question is fixed
/// by trigger and venue, so a venue that keeps missing is one item that stays
/// raised. The campaigns — each residual
/// against its own threshold — and the instruments they were scoped to ride
/// in the evidence. The severity is the largest of the group's, as
/// `detectors::anomaly_clusters` keeps each subject's worst.
///
/// Refuses, naming the campaign, one the detector cannot have opened: a
/// residual campaign carrying no residual or no threshold, a residual not
/// strictly past a threshold of zero or more, and a new-venue campaign that
/// names no venue or carries a residual. Ranking any of those at a severity
/// nobody measured would hide the caller's bug behind a plausible item.
pub fn observations(campaigns: &[TickCampaign]) -> Result<Vec<Observation>> {
    #[derive(Debug, Default)]
    struct Group<'a> {
        severity: f64,
        evidence: Vec<String>,
        instruments: BTreeSet<&'a str>,
    }
    let mut groups: BTreeMap<(TickCampaignTrigger, Option<&str>), Group<'_>> = BTreeMap::new();
    for campaign in campaigns {
        let severity = severity(campaign)?;
        let group = groups
            .entry((campaign.trigger, campaign.venue.as_deref()))
            .or_default();
        group.severity = group.severity.max(severity);
        group.evidence.push(evidence(campaign));
        group
            .instruments
            .extend(campaign.affected_instruments.iter().map(String::as_str));
    }
    Ok(groups
        .into_iter()
        .map(|((trigger, venue), group)| Observation {
            trigger: trigger.gap_trigger(),
            // A tick campaign is a tick replay whatever opened it, and
            // replaying ticks is the acquisition an execution gap asks for.
            wanting: vec![Wanting::Execution],
            observation: question(trigger, venue),
            affected_domains: vec!["execution".to_string()],
            evidence: std::iter::once(ORIGIN.to_string())
                .chain(group.evidence)
                .chain(
                    group
                        .instruments
                        .into_iter()
                        .map(|instrument| format!("instrument:{instrument}")),
                )
                .collect(),
            severity: group.severity,
            // No campaign measures money. Zero is stated, not defaulted, as
            // every detector in `detectors.rs` states it.
            economic_value: Decimal::ZERO,
            // Nor exposure: a campaign is a statement about sources, not a
            // position, and a figure here would be one nobody measured.
            risk_value: Decimal::ZERO,
        })
        .collect())
}

/// One campaign's severity in `[0, 1]`, or a refusal naming why the detector
/// cannot have opened it.
fn severity(campaign: &TickCampaign) -> Result<f64> {
    let id = &campaign.campaign_id;
    if campaign.trigger == TickCampaignTrigger::NewVenue {
        if campaign.venue.is_none()
            || campaign.residual_value.is_some()
            || campaign.threshold.is_some()
        {
            return Err(Error::invalid(format!(
                "{id} is a new-venue campaign that names no venue or carries a residual; a first sighting names its venue and has no residual, so open it through TickCampaignDetector::observe_venue"
            )));
        }
        // A venue seen for the first time has no history at all: its unseen
        // share is the whole of it, which is how `detectors::coverage` scores
        // an asset class with none of its instruments covered.
        return Ok(1.0);
    }
    let (Some(residual), Some(threshold)) = (campaign.residual_value, campaign.threshold) else {
        return Err(Error::invalid(format!(
            "{id} was opened on {:?} and carries no residual or no threshold; a residual campaign is ranked by its residual against its threshold, so open it through TickCampaignDetector",
            campaign.trigger
        )));
    };
    if threshold < Decimal::ZERO || residual.abs() <= threshold {
        return Err(Error::invalid(format!(
            "{id}: a residual of {residual} against a threshold of {threshold} is not one the detector opens a campaign on, which takes a magnitude strictly past a threshold of zero or more; open it through TickCampaignDetector"
        )));
    }
    if threshold.is_zero() {
        // Every miss is past a zero threshold by more than any multiple of it.
        return Ok(1.0);
    }
    // Decimal to f64 here and nowhere else in this module: severity is a
    // ranking statistic, not money, and the residual and threshold stay exact
    // in the evidence. Saturating at ten times the threshold is the scale
    // `detectors::unexplained_residuals` ranks a twin's mean miss on, so a
    // campaign and a residual gap at one venue rank against each other.
    Ok((residual.abs().to_f64() / (10.0 * threshold.to_f64())).min(1.0))
}

/// The research question a campaign asks, fixed by its trigger and venue
/// alone. The residual rides in the evidence, not here: a question that moved
/// with every sweep's residual would be a new item each sweep while the one
/// already started went quiet, the failure `detectors::unexplained_residuals`
/// records for its own text.
fn question(trigger: TickCampaignTrigger, venue: Option<&str>) -> String {
    match (trigger, venue) {
        (TickCampaignTrigger::NewVenue, Some(venue)) => {
            format!("tick replay at `{venue}`, a venue not seen before")
        }
        (_, Some(venue)) => {
            format!("tick replay at `{venue}` after {trigger:?} past its threshold")
        }
        (_, None) => format!("tick replay after {trigger:?} past its threshold, at no named venue"),
    }
}

/// One campaign as an evidence id: which campaign, and for a residual
/// campaign the residual against its threshold, exact.
fn evidence(campaign: &TickCampaign) -> String {
    match (campaign.residual_value, campaign.threshold) {
        (Some(residual), Some(threshold)) => {
            format!(
                "tick_campaign:{}:{residual}/{threshold}",
                campaign.campaign_id
            )
        }
        _ => format!("tick_campaign:{}", campaign.campaign_id),
    }
}

/// Opens tick-replay campaigns from residual observations and venue sightings.
#[derive(Debug, Clone)]
pub struct TickCampaignDetector {
    slippage_threshold: Decimal,
    known_venues: BTreeSet<String>,
    issued: u64,
}

impl TickCampaignDetector {
    /// The default slippage threshold, 0.01 (one per cent). The blueprint
    /// does not define the threshold for "grow"; this is the house default
    /// until a calibrated one exists.
    pub const DEFAULT_SLIPPAGE_THRESHOLD: Decimal = Decimal::from_raw(10_000_000);

    /// A detector with the default slippage threshold and no known venues.
    pub fn new() -> Result<Self> {
        Self::with_slippage_threshold(Self::DEFAULT_SLIPPAGE_THRESHOLD)
    }

    /// A detector with the given slippage threshold.
    ///
    /// Refuses a negative threshold: residuals are compared by magnitude, so a
    /// negative threshold would open a campaign on every observation,
    /// including a residual of zero, and drown the real ones.
    pub fn with_slippage_threshold(threshold: Decimal) -> Result<Self> {
        if threshold < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "slippage threshold {threshold} is negative; residuals are compared by magnitude, so pass a threshold of zero or more"
            )));
        }
        Ok(Self {
            slippage_threshold: threshold,
            known_venues: BTreeSet::new(),
            issued: 0,
        })
    }

    /// The threshold a slippage residual's magnitude must exceed.
    pub fn slippage_threshold(&self) -> Decimal {
        self.slippage_threshold
    }

    /// Record a venue sighting. Opens a [`TickCampaignTrigger::NewVenue`]
    /// campaign the first time a venue is seen, scoped to the instruments it
    /// was seen trading, and nothing on every later sighting.
    pub fn observe_venue(
        &mut self,
        venue: &str,
        instruments: BTreeSet<String>,
    ) -> Option<TickCampaign> {
        if !self.known_venues.insert(venue.to_string()) {
            return None;
        }
        Some(TickCampaign {
            campaign_id: self.next_id(),
            trigger: TickCampaignTrigger::NewVenue,
            affected_instruments: instruments,
            venue: Some(venue.to_string()),
            residual_value: None,
            threshold: None,
        })
    }

    /// Record an unexplained-slippage residual. Opens a campaign when its
    /// magnitude strictly exceeds the threshold — a negative residual matters
    /// as much as a positive one of the same size — and nothing otherwise.
    pub fn observe_slippage(
        &mut self,
        residual: Decimal,
        venue: Option<&str>,
        instruments: BTreeSet<String>,
    ) -> Option<TickCampaign> {
        if residual.abs() <= self.slippage_threshold {
            return None;
        }
        Some(TickCampaign {
            campaign_id: self.next_id(),
            trigger: TickCampaignTrigger::UnexplainedSlippage,
            affected_instruments: instruments,
            venue: venue.map(str::to_string),
            residual_value: Some(residual),
            threshold: Some(self.slippage_threshold),
        })
    }

    /// Record a residual for one of the four residual triggers that have no
    /// threshold of their own on the detector — fill and queue prediction
    /// error, abnormal lead/lag, and world-event reaction residuals
    /// (EXPAND-069) — against the threshold the caller measured it against.
    /// Opens a campaign when the magnitude strictly exceeds the threshold.
    ///
    /// Refuses the two triggers with their own observers, so a venue
    /// sighting or a slippage residual cannot be filed under a threshold the
    /// detector does not hold, and refuses a negative threshold for the
    /// reason [`Self::with_slippage_threshold`] gives.
    pub fn observe_residual(
        &mut self,
        trigger: TickCampaignTrigger,
        residual: Decimal,
        threshold: Decimal,
        venue: Option<&str>,
        instruments: BTreeSet<String>,
    ) -> Result<Option<TickCampaign>> {
        if matches!(
            trigger,
            TickCampaignTrigger::NewVenue | TickCampaignTrigger::UnexplainedSlippage
        ) {
            return Err(Error::invalid(format!(
                "{trigger:?} has its own observer; call observe_venue or observe_slippage"
            )));
        }
        if threshold < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "residual threshold {threshold} is negative; residuals are compared by magnitude, so pass zero or more"
            )));
        }
        if residual.abs() <= threshold {
            return Ok(None);
        }
        Ok(Some(TickCampaign {
            campaign_id: self.next_id(),
            trigger,
            affected_instruments: instruments,
            venue: venue.map(str::to_string),
            residual_value: Some(residual),
            threshold: Some(threshold),
        }))
    }

    /// Conclude a campaign on its re-measured residuals: the
    /// [`MicrostructureDelta`] the slow brain learns from (§27,
    /// R12-EXPAND-6042).
    ///
    /// The failure it prevents: a campaign opened, ran, and re-measured its
    /// venue, and the only thing left behind was the counter that it opened.
    /// What the replay found — what the model said against what the venue
    /// did, over how many samples, in which window — went nowhere a learner
    /// could read. The delta's id is the campaign's, so every residual traces
    /// to the campaign that measured it.
    ///
    /// Refuses a campaign with no venue (a delta is a venue's), and a
    /// residual for an instrument the campaign never studied, which would
    /// attribute a measurement to a campaign that did not take it. Every
    /// other refusal is the contract's: a blank cell or model version, an
    /// inverted window, no residual, zero samples, a duplicate per instrument.
    pub fn conclude(
        campaign: &TickCampaign,
        cell: &str,
        model_version: &str,
        window: (Timestamp, Timestamp),
        remeasured: Vec<Remeasured>,
    ) -> Result<MicrostructureDelta> {
        let venue = campaign.venue.as_deref().ok_or_else(|| {
            Error::invalid(format!(
                "campaign {} names no venue; a microstructure delta is a venue's, so conclude a venue-scoped campaign",
                campaign.campaign_id
            ))
        })?;
        let kind = campaign.trigger.residual_kind();
        let mut residuals = Vec::with_capacity(remeasured.len());
        for r in remeasured {
            if !campaign.affected_instruments.contains(&r.instrument) {
                return Err(Error::invalid(format!(
                    "campaign {} did not study `{}`; conclude it only on the instruments it was opened for",
                    campaign.campaign_id, r.instrument
                )));
            }
            residuals.push(Residual {
                kind,
                instrument: r.instrument,
                modelled: r.modelled,
                observed: r.observed,
                samples: r.samples,
            });
        }
        MicrostructureDelta::new(MicrostructureDeltaFields {
            schema_version: MicrostructureDeltaFields::SCHEMA_VERSION,
            delta_id: campaign.campaign_id.clone(),
            cell: cell.to_string(),
            venue: VenueId::new(venue),
            model_version: model_version.to_string(),
            window_start: window.0,
            window_end: window.1,
            residuals,
        })
    }

    fn next_id(&mut self) -> String {
        self.issued += 1;
        format!("tick-campaign-{:06}", self.issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// `n` hundredths of a per cent, built from the raw scaled integer so no
    /// fallible constructor is needed in a test.
    fn pct(hundredths_of_a_percent: i128) -> Decimal {
        Decimal::from_raw(hundredths_of_a_percent * 100_000)
    }

    fn detector() -> TickCampaignDetector {
        match TickCampaignDetector::new() {
            Ok(d) => d,
            Err(e) => panic!("the default detector must construct: {e}"),
        }
    }

    /// One recorded window: slippage that grows from 0.5% to 2% at NYSE, then
    /// a venue the detector has never seen. The row's verification asks for
    /// exactly this window.
    fn replay(detector: &mut TickCampaignDetector) -> Vec<TickCampaign> {
        let mut opened = Vec::new();
        for residual in [pct(50), pct(90), pct(200)] {
            opened.extend(detector.observe_venue("NYSE", set(&["AAPL"])));
            opened.extend(detector.observe_slippage(residual, Some("NYSE"), set(&["AAPL"])));
        }
        opened.extend(detector.observe_venue("IEX", set(&["MSFT", "GOOG"])));
        opened
    }

    #[test]
    fn a_window_where_slippage_grows_and_a_venue_appears_opens_one_scoped_campaign_for_each() {
        let mut detector = detector();
        // NYSE is known before the window starts, so the only new venue in
        // the window is IEX.
        assert!(detector.observe_venue("NYSE", set(&["AAPL"])).is_some());

        let opened = replay(&mut detector);

        assert_eq!(
            opened.len(),
            2,
            "expected one slippage and one venue campaign: {opened:?}"
        );
        let slippage = &opened[0];
        assert_eq!(slippage.trigger, TickCampaignTrigger::UnexplainedSlippage);
        assert_eq!(slippage.residual_value, Some(pct(200)));
        assert_eq!(slippage.threshold, Some(pct(100)));
        assert_eq!(slippage.venue.as_deref(), Some("NYSE"));
        assert_eq!(slippage.affected_instruments, set(&["AAPL"]));

        let venue = &opened[1];
        assert_eq!(venue.trigger, TickCampaignTrigger::NewVenue);
        assert_eq!(venue.venue.as_deref(), Some("IEX"));
        assert_eq!(venue.affected_instruments, set(&["GOOG", "MSFT"]));
        assert_eq!(venue.residual_value, None);
        assert_eq!(venue.threshold, None);
    }

    #[test]
    fn replaying_the_same_window_through_a_fresh_detector_opens_identical_campaigns() {
        let first = replay(&mut detector());
        let second = replay(&mut detector());
        // Premise: the window opened something (NYSE, the 2% slippage, IEX),
        // so equality is not two empty lists agreeing.
        assert_eq!(first.len(), 3, "{first:?}");
        assert_eq!(first, second);
        let ids: BTreeSet<&str> = first.iter().map(|c| c.campaign_id.as_str()).collect();
        assert_eq!(
            ids.len(),
            first.len(),
            "campaign ids must be distinct: {first:?}"
        );
    }

    #[test]
    fn a_residual_exactly_at_the_threshold_opens_nothing_and_one_just_above_does() {
        let mut detector = detector();
        let at = detector.slippage_threshold();
        assert_eq!(at, pct(100));
        assert_eq!(detector.observe_slippage(at, None, set(&["AAPL"])), None);
        let above = at + Decimal::from_raw(1);
        assert!(
            detector
                .observe_slippage(above, None, set(&["AAPL"]))
                .is_some()
        );
    }

    #[test]
    fn a_negative_residual_opens_a_campaign_at_the_same_magnitude_as_a_positive_one() {
        let mut detector = detector();
        let negative = Decimal::ZERO - pct(200);
        let opened = detector.observe_slippage(negative, Some("NYSE"), set(&["GOOG"]));
        assert_eq!(opened.map(|c| c.residual_value), Some(Some(negative)));
    }

    #[test]
    fn a_venue_seen_twice_opens_a_campaign_only_the_first_time() {
        let mut detector = detector();
        assert!(detector.observe_venue("NYSE", set(&["AAPL"])).is_some());
        assert_eq!(detector.observe_venue("NYSE", set(&["AAPL"])), None);
        assert!(detector.observe_venue("NASDAQ", set(&["AAPL"])).is_some());
    }

    #[test]
    fn a_negative_threshold_is_refused_rather_than_opening_a_campaign_on_everything() {
        match TickCampaignDetector::with_slippage_threshold(Decimal::ZERO - pct(1)) {
            Err(e) => assert!(e.to_string().contains("is negative"), "{e}"),
            Ok(d) => panic!("a negative threshold was admitted: {d:?}"),
        }
        assert!(TickCampaignDetector::with_slippage_threshold(Decimal::ZERO).is_ok());
    }

    // --- the campaigns as the curriculum reads them (RM-P9-08) --------------

    /// A fill-prediction campaign at `venue`, opened by `detector` against a
    /// threshold of 25 so the severities below can be checked by eye.
    fn fill_campaign(
        detector: &mut TickCampaignDetector,
        residual: i64,
        venue: &str,
        instrument: &str,
    ) -> TickCampaign {
        match detector.observe_residual(
            TickCampaignTrigger::FillPredictionError,
            Decimal::from_int(residual),
            Decimal::from_int(25),
            Some(venue),
            set(&[instrument]),
        ) {
            Ok(Some(campaign)) => campaign,
            other => panic!("premise: {residual} is past 25 and opens a campaign: {other:?}"),
        }
    }

    fn stated(campaigns: &[TickCampaign]) -> Vec<Observation> {
        match observations(campaigns) {
            Ok(stated) => stated,
            Err(e) => panic!("campaigns the detector opened must be stated: {e}"),
        }
    }

    /// Three misses at one venue are one research question, not three: each
    /// observation is an ambient signal the router charges to the attention
    /// budget, and the desk would refuse the repeats as duplicates only after
    /// the router had spent on every one.
    ///
    /// Mutation: key the groups on the campaign id as well as the trigger and
    /// venue — four observations come back and the count fails.
    #[test]
    fn the_campaigns_a_sweep_opens_at_one_venue_are_one_observation_ranked_by_its_largest_miss() {
        let mut detector = detector();
        let opened = vec![
            fill_campaign(&mut detector, 30, "XNYS", "AAPL"),
            fill_campaign(&mut detector, -100, "XNYS", "MSFT"),
            fill_campaign(&mut detector, 50, "XNYS", "AAPL"),
            fill_campaign(&mut detector, 40, "XLON", "VOD"),
        ];
        // Premise: four distinct campaigns, three of them at one venue.
        let ids: BTreeSet<&str> = opened.iter().map(|c| c.campaign_id.as_str()).collect();
        assert_eq!(ids.len(), 4, "{opened:?}");

        let stated = stated(&opened);
        assert_eq!(
            stated.len(),
            2,
            "one per venue, not one per campaign: {stated:?}"
        );
        let (xlon, xnys) = (&stated[0], &stated[1]);
        assert_eq!(
            xnys.observation,
            "tick replay at `XNYS` after FillPredictionError past its threshold"
        );
        assert_eq!(xnys.trigger, GapTrigger::UnexplainedPnl);
        assert_eq!(xnys.wanting, vec![Wanting::Execution]);
        assert_eq!(
            xnys.evidence,
            vec![
                ORIGIN.to_string(),
                "tick_campaign:tick-campaign-000001:30/25".to_string(),
                "tick_campaign:tick-campaign-000002:-100/25".to_string(),
                "tick_campaign:tick-campaign-000003:50/25".to_string(),
                "instrument:AAPL".to_string(),
                "instrument:MSFT".to_string(),
            ]
        );
        // The largest miss, |-100| against 25, on the ten-thresholds scale:
        // 100 / 250. A negative miss ranks by its magnitude.
        assert!((xnys.severity - 0.4).abs() < 1e-12, "{}", xnys.severity);
        assert_eq!(
            xlon.observation,
            "tick replay at `XLON` after FillPredictionError past its threshold"
        );
        assert!((xlon.severity - 0.16).abs() < 1e-12, "{}", xlon.severity);
    }

    /// A venue that keeps missing asks one question across sweeps, so the
    /// desk counts a repeat of the item it holds instead of queueing a new one
    /// while the started one goes quiet.
    ///
    /// Mutation: append the group's severity to the question text — the two
    /// sweeps' questions differ and the equality fails.
    #[test]
    fn a_venue_that_keeps_missing_asks_the_same_question_every_sweep() {
        let mut detector = detector();
        let first = stated(&[fill_campaign(&mut detector, 30, "XNYS", "AAPL")]);
        let second = stated(&[fill_campaign(&mut detector, 900, "XNYS", "MSFT")]);
        // Premise: one observation each, from two different misses.
        assert_eq!((first.len(), second.len()), (1, 1));
        assert_ne!(first[0].evidence, second[0].evidence);
        assert!(first[0].severity < second[0].severity);

        assert_eq!(first[0].observation, second[0].observation);
    }

    /// A campaign the detector cannot have opened is refused by name rather
    /// than ranked at a severity nobody measured: with no residual there is
    /// nothing to rank by, a miss inside its threshold is not a miss, and a
    /// new venue that names none has no question to ask.
    ///
    /// Mutations, one per refusal, each letting its case through so the match
    /// fails: rank a residual campaign with no residual at 1.0; drop the
    /// strictly-past-the-threshold check; rank a new venue naming none at 1.0.
    #[test]
    fn a_campaign_the_detector_cannot_have_opened_is_refused_rather_than_ranked_at_a_guessed_severity()
     {
        let opened = fill_campaign(&mut detector(), 40, "XNYS", "AAPL");
        let Some(venue) = detector().observe_venue("IEX", set(&["MSFT"])) else {
            panic!("premise: a first sighting opens a campaign");
        };
        // Premise: both, as the detector opened them, are stated.
        assert_eq!(stated(&[opened.clone(), venue.clone()]).len(), 2);

        let no_residual = TickCampaign {
            residual_value: None,
            ..opened.clone()
        };
        let inside = TickCampaign {
            residual_value: Some(Decimal::from_int(25)),
            ..opened.clone()
        };
        let nowhere = TickCampaign {
            venue: None,
            ..venue
        };
        for (campaign, says) in [
            (no_residual, "carries no residual or no threshold"),
            (inside, "is not one the detector opens a campaign on"),
            (nowhere, "names no venue or carries a residual"),
        ] {
            match observations(&[opened.clone(), campaign]) {
                Err(e) => assert!(e.to_string().contains(says), "expected `{says}`: {e}"),
                Ok(stated) => {
                    panic!("a campaign the detector cannot have opened was ranked: {stated:?}")
                }
            }
        }
    }

    /// New-venue discovery: a venue seen for the first time is one tick-replay
    /// question at full severity, because none of it has history — its unseen
    /// share is all of it, as `detectors::coverage` scores a class with none.
    ///
    /// Mutation: rank a new venue at 0.5 — the severity assertion fails.
    #[test]
    fn a_new_venue_is_one_tick_replay_question_at_full_severity_because_none_of_it_has_been_seen() {
        let mut detector = detector();
        let Some(campaign) = detector.observe_venue("IEX", set(&["MSFT", "GOOG"])) else {
            panic!("premise: a first sighting opens a campaign");
        };
        let stated = stated(&[campaign]);
        assert_eq!(stated.len(), 1, "{stated:?}");
        let venue = &stated[0];
        assert_eq!(
            venue.observation,
            "tick replay at `IEX`, a venue not seen before"
        );
        assert_eq!(venue.trigger, GapTrigger::UnsupportedRequest);
        assert_eq!(venue.wanting, vec![Wanting::Execution]);
        assert!((venue.severity - 1.0).abs() < 1e-12, "{}", venue.severity);
        assert_eq!(
            venue.evidence,
            vec![
                ORIGIN.to_string(),
                "tick_campaign:tick-campaign-000001".to_string(),
                "instrument:GOOG".to_string(),
                "instrument:MSFT".to_string(),
            ]
        );
    }
}
