//! The World Model Federation as the UNDERSTAND stage runs it (blueprint
//! §9.3; WORLD-3037, -3038, -3050, -3051, -3052).
//!
//! `qip_world_model::federation` was built, tested and constructed by
//! nothing: the cycle read one `WorldModel`, and one model cannot say "the
//! models disagree" because there is no second model. This module is the
//! composition root's half. It declares a population specialised on all five
//! axes, feeds every member a view each pass from what the platform has
//! actually observed, scores those views against what the market then did,
//! arbitrates, and hands the kernel one [`FederationReview`] per cycle to
//! append to the hash-chained log.
//!
//! # Advisory, and structurally so
//!
//! [`FederationStage::run`] takes a [`FederationInputs`] by reference and
//! returns a record; it holds no handle to a thesis, a sizing, a reservation
//! or a broker, and the kernel calls it from UNDERSTAND, before any
//! hypothesis exists. An arbitrated probability that moved a size would be a
//! control nobody chose.
//!
//! One power reaches DECIDE, and ADR 0125 decision 4 admits it: a *narrowing*.
//! [`narrowings_of`] reads each instrument's own arbitration and
//! [`Narrowing::of_spread`] maps a material spread to a multiplier in
//! `(0, 1)` from [`NARROWING_SCHEDULE`]; [`narrow`] multiplies it into
//! DECIDE's caps beside ADR 0063's and ADR 0066's. It has no arm above one,
//! it never creates a proposal, it moves no probability or conviction, and an
//! instrument with no arbitration (thin history, an uncovered scope, a silent
//! member) earns nothing, because absence is not disagreement.
//!
//! # What each member is, honestly
//!
//! The members are deliberately simple, declared statistics over series the
//! platform already holds — not fitted models. Their value is that they are
//! *different*: momentum and reversion read the same tape and answer
//! opposite questions, so their disagreement is real and measured, and every
//! one is scored by Brier against the next observed close. A member with no
//! input this pass (no chain observed, say) produces no state, is not
//! renewed, and expires at its lease; the population check then names the
//! scope as uncovered rather than reporting a coverage nobody computed.
//!
//! # Bounded
//!
//! At most [`FEDERATION_MAX_SUBJECTS`] instruments are assessed per pass, in
//! identifier order; a forecast reality does not answer within
//! [`FEDERATION_PENDING_CYCLES`] passes is abandoned on the record rather than
//! scored against an outcome nobody saw; and the federation's own journal is
//! drained into each review, so the event log is the record and the
//! in-process copy does not grow with uptime.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::federation::{
    Abstraction, Arbitration, Assessment, CalibrationClaim, Decision, Declaration, Event,
    Federation, InformationStake, ModelKind, Scope, WorldModelSpec,
};
use serde::{Deserialize, Serialize};

/// Instruments assessed per pass, a deterministic prefix of identifier order.
pub const FEDERATION_MAX_SUBJECTS: usize = 32;

/// Returns an instrument needs before any member forms a view on it. Twenty,
/// because the reversion member's window is twenty returns and a shorter
/// series would have it reasoning about a window it does not hold.
pub const FEDERATION_MIN_RETURNS: usize = 20;

/// Passes a forecast may wait for reality before it is abandoned unscored.
pub const FEDERATION_PENDING_CYCLES: u64 = 8;

/// How long a member stays admissible after it last produced state. Renewed
/// on every pass it produces any; a member fed nothing for this long is
/// excluded from arbitration at exactly its expiry.
pub const MEMBER_LEASE: Duration = Duration::from_days(7);

/// The members, by id. Fixed, so a replay of the log names the same models.
pub const MOMENTUM: &str = "markets.momentum.global.micro";
/// The order-flow member (ADR 0231, R12-WORLD-3046): signed aggressor
/// volume per instrument per cycle, read from the paper feed's own
/// aggressor side and labelled simulated by its source, so a licensed tape
/// later replaces the source and not the member.
pub const ORDER_FLOW: &str = "markets.order-flow.global.micro";

/// The macro member (ADR 0232, R12-WORLD-3043): whether each series' next
/// release surprises its consensus in the same direction as its last one.
pub const MACRO: &str = "macro.surprise-persistence.global";

/// The source the macro member declares: the catalogued macro connectors.
/// It speaks the day one is switched on at the deploy and is silent until.
pub const MACRO_SOURCE: &str = "sense:macro-connectors";

/// How long a macro forecast waits for its series' next release, in time
/// and not in passes. Releases are monthly at best and a deployed cycle runs
/// every few minutes or faster, so any count of passes would abandon every
/// macro forecast before reality could answer it.
pub const MACRO_PATIENCE: Duration = Duration::from_days(60);

/// Series the macro member remembers a last release for. A bound on top of
/// the catalogue, because the readings arrive from the feature store.
pub const MAX_MACRO_SERIES: usize = 1_024;

/// The source the order-flow member declares: the simulated tape's
/// aggressor side, named as simulated wherever the member is listed.
pub const ORDER_FLOW_SOURCE: &str = "sense:trade-aggressor:simulated";

/// How strongly a fully one-sided cycle of flow leans the member's view of
/// the next close, on the logistic scale. Declared, not fitted: at 1.0 a
/// cycle of pure buying reads as 0.73 up, short of the tape members' reach,
/// because one cycle of one-sided prints is weak evidence of the next close.
pub const ORDER_FLOW_SLOPE: f64 = 1.0;
pub const REVERSION: &str = "markets.reversion.global.meso";
pub const STRESS_SCENARIO: &str = "markets.stress-world.global.macro";
pub const STANDING_ASIDE: &str = "markets.standing-aside.global.meso";
/// The global causal model as a member (R12-WORLD-3042): the world model's
/// graph, propagated into each instrument, scored like the tape members.
pub const CAUSAL_GLOBAL: &str = "markets.causal-graph.global.macro";
/// The rare-regime member (WORLD-3051): what an instrument does on the day
/// after one of its own tail moves, read from the tape's history of them.
pub const RARE_REGIME: &str = "markets.rare-regime.global.macro";
/// The alternate-future member (WORLD-3051): the stress world's inverse — the
/// future in which the shock the stress member applies does not arrive and
/// its opposite does.
pub const ALTERNATE_FUTURE: &str = "markets.alternate-future.global.macro";
/// The share of an instrument's largest absolute moves the rare-regime
/// member treats as its rare days. One in twenty is the tail the stress
/// member's shock is drawn from, so the two read the same regime.
pub const RARE_REGIME_TAIL: f64 = 0.05;
pub const SELF_SYSTEM: &str = "self.operational-health.global";
pub const ON_CHAIN: &str = "crypto-onchain.flows.global";
/// The company/fundamental member (R12-WORLD-3044, §9.3): an instrument's
/// issuer's latest reported surprise against consensus, read as a lean on
/// its next close.
pub const COMPANY: &str = "company.fundamental.global";
/// The capital member (R12-WORLD-3048, ADR 0194): will the next pass find the
/// paper book's cash lower than this one did — a funding draw?
pub const CAPITAL: &str = "portfolio.capital.global";
/// How sharply a fundamental surprise leans the company member's view: a
/// five per cent beat reads at `logistic(1.0)`, about 0.73. A chosen
/// constant, stated so a reader can see the mapping is a slope and not a
/// measurement.
pub const FUNDAMENTAL_SURPRISE_SLOPE: f64 = 20.0;
/// The regional member's id is `markets.microstructure.<region>.micro`.
pub const REGIONAL_PREFIX: &str = "markets.microstructure.";
/// The domain members' id prefix (MODEL-4009): one per asset class the
/// composition root has named, `domain.<class>.global.meso`, declared the
/// first pass the class is seen.
pub const DOMAIN_PREFIX: &str = "domain.";
/// The domain members' hypothesis: an instrument moves with the breadth of
/// its own asset class, not with its own tape.
pub const DOMAIN_HYPOTHESIS: &str = "cross-sectional-breadth";
/// The share of a disagreement's value of information an active-sensing
/// request may spend (AMBIENT-10093): a tenth, so settling a question is
/// never allowed to cost what the question is worth.
pub const REQUEST_BUDGET_SHARE_BP: u32 = 1_000;
/// How long an active-sensing request's answer stays useful: the next
/// close, which is what every instrument proposition is about.
pub const REQUEST_DEADLINE: Duration = Duration::from_days(1);

/// Live branches the stage keeps at once. A branch mirrors the member it
/// follows, so without a ceiling a persistently split panel would add one
/// duplicate voice per pass and the arbitration would drift toward whichever
/// member kept dissenting.
pub const FEDERATION_MAX_LIVE_BRANCHES: usize = 8;

/// The causal graph's reading of one instrument: the expected move its live
/// incoming claims transmit from causes that were seen to move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CausalView {
    /// Sum over live incoming claims of the cause's latest return times the
    /// claim's signed transmission: the next return the graph expects, a
    /// statistic in return units.
    pub lean: f64,
    /// Live claims into the instrument.
    pub claims: usize,
    /// Mean confidence of those claims, in `[0, 1]`.
    pub confidence: f64,
}

/// One instrument as the platform has observed it.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentView {
    pub subject: String,
    /// Simple returns of the observed closes, oldest first.
    pub returns: Vec<f64>,
    /// The last close observed, a statistic here (no money is computed).
    pub last_close: f64,
    /// When the last bar closed; what tells a new observation from none.
    pub observed_through: Timestamp,
    /// What the causal graph's live claims into this instrument imply for its
    /// next return (R12-WORLD-3040, -3042); `None` where the graph holds no
    /// live claim into it.
    pub causal: Option<CausalView>,
    /// Latest quoted spread over its own median, where one was measured.
    pub spread_ratio: Option<f64>,
    /// This cycle's signed aggressor flow per venue (ADR 0231): trades
    /// with no aggressor side contributed nothing. Empty where no trade
    /// stated one, and the order-flow member is then silent on it.
    pub order_flow: Vec<qip_world_model::order_flow::FlowObservation>,
    /// The instrument's issuer's latest reported fundamental surprise against
    /// consensus, as a fraction (R12-WORLD-3044); `None` where no issuer of it
    /// has reported one, which is what leaves the company member silent.
    pub fundamental_surprise: Option<f64>,
}

/// The platform's own state, for the self/system member (WORLD-3052).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemView {
    /// Whether the kill switch is tripped globally.
    pub halted: bool,
    /// The autonomy level in force — the platform's authority.
    pub autonomy: String,
    /// Whether that authority is a live one. Recorded so a reader sees the
    /// boundary as the platform saw it, never acted on here.
    pub live_authority: bool,
    /// Agent manifests past their review interval: a capability the
    /// organisation has and may not use.
    pub expired_manifests: usize,
    /// Instruments priced but below the history a member needs: a blind spot.
    pub thin_subjects: usize,
    /// The slowest stage of the previous cycle on the platform's clock
    /// (WORLD-3052), as [`FederationStage::observe_cycle`] last recorded it;
    /// `None` before the first cycle has finished. Absent on a record
    /// written before the field existed.
    #[serde(default)]
    pub slowest_stage: Option<Duration>,
    /// Central capabilities the degradation table holds as anything but
    /// fresh (WORLD-3052); `None` when the table could not be read, which
    /// the member counts as unknown health rather than good health.
    #[serde(default)]
    pub degraded_capabilities: Option<usize>,
}

/// A stage slower than this is a platform the self/system member reads as
/// at risk: a cycle is daily, and a stage that takes a minute of it is a
/// process waiting on something it should not be (WORLD-3052).
pub const SELF_STAGE_LATENCY_BUDGET: Duration = Duration::from_secs(60);

/// What the on-chain member reads (ASSET-3017's federation half).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainView {
    /// Trades derived at the configured confirmation depth.
    pub confirmed_trades: u64,
}

/// One macro series' latest surprise against its consensus, as the macro
/// member reads it (ADR 0232). Only a release that stated a consensus has a
/// surprise, so a series without one is absent rather than read as zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroView {
    pub series: String,
    /// Value minus consensus, as the world model recorded it.
    pub surprise: f64,
    /// The release's reference period: what tells a new release from one
    /// already read.
    pub valid_at: Timestamp,
    /// The release came from a synthetic tape, and its proposition says so.
    pub simulated: bool,
}

/// One currency series' latest fixing, as the macro member reads it
/// (ADR 0232): a fixing states no consensus, so the member reads its move
/// from the previous fixing instead of a surprise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurrencyView {
    pub series: String,
    pub level: f64,
    pub valid_at: Timestamp,
    pub simulated: bool,
}

/// The proposition one fixing's move forecast answers: the next fixing
/// moves the same way as this one did.
pub fn currency_proposition(series: &str, cycle: u64, simulated: bool) -> String {
    let label = if simulated { ":simulated" } else { "" };
    format!("macro:{series}{label}:move-persists@{cycle}")
}

/// The proposition one release's persistence forecast answers. A simulated
/// release's proposition carries the label, so the journal never records a
/// synthetic surprise's forecast as one about a real economy.
pub fn macro_proposition(series: &str, cycle: u64, simulated: bool) -> String {
    let label = if simulated { ":simulated" } else { "" };
    format!("macro:{series}{label}:surprise-persists@{cycle}")
}

/// What the capital member reads (R12-WORLD-3048, ADR 0194): the paper
/// book's funding and leverage, as statistics. Built only where equity is
/// positive, because a share of nothing is not a number; the member is
/// silent rather than reading such a book as one that draws nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalView {
    /// Cash after every fill and charge, in the book's currency, as `f64`:
    /// the resolver compares the next pass's against it, and no amount
    /// comes back out.
    pub cash: f64,
    /// Cash over equity.
    pub cash_share: f64,
    /// Gross exposure at cost over equity: the book's leverage.
    pub leverage: f64,
    /// Net exposure at cost over equity.
    pub net_share: f64,
    /// Approved theses waiting for construction: the book's open claims on
    /// capital for the next pass.
    pub pending_theses: usize,
}

/// Everything one pass reads.
#[derive(Clone, Debug, PartialEq)]
pub struct FederationInputs {
    pub cycle: u64,
    /// The region this process serves; the regional member's scope.
    pub region: String,
    pub instruments: Vec<InstrumentView>,
    pub system: SystemView,
    pub chain: Option<ChainView>,
    /// The paper book's funding state, for the capital member; `None` where
    /// equity is not positive.
    pub capital: Option<CapitalView>,
    /// The latest macro surprise per series readable this pass (ADR 0232).
    pub macros: Vec<MacroView>,
    /// The latest currency fixing per series readable this pass (ADR 0232).
    pub currencies: Vec<CurrencyView>,
}

/// One member as the population check read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberStanding {
    pub id: String,
    pub scope: Scope,
    pub expires_at: Timestamp,
    pub calibration: qip_world_model::federation::Calibration,
}

/// A forecast that reality answered this pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub proposition: String,
    pub outcome: bool,
    /// Branches closed because they forecast the wrong side.
    pub closed: Vec<String>,
    /// The probability each member had given the proposition, by member id:
    /// what the platform charges to that member in its self-model. Absent
    /// from a review journaled before it existed, which reads as empty.
    #[serde(default)]
    pub forecasts: BTreeMap<String, f64>,
    /// The federation's own arbitrated probability for the proposition at
    /// its latest arbitration before it resolved: what the platform charges
    /// to the federation as an ensemble (R12-MODEL-6016). `None` where no
    /// arbitration merged a number, and on a review journaled before it
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ensemble: Option<f64>,
}

/// The pass, once a cycle, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FederationReview {
    pub cycle: u64,
    pub at: Timestamp,
    /// Every live, unexpired member, in id order.
    pub population: Vec<MemberStanding>,
    /// Required scope families no live member covers. Empty is the claim
    /// WORLD-3037/-3050/-3051/-3052 make; non-empty is the finding.
    pub uncovered: Vec<String>,
    /// Members that produced no state this pass.
    pub silent: Vec<String>,
    /// Members whose assessment the federation refused, and why.
    pub refused: Vec<(String, String)>,
    /// Instruments with enough history whose series is flat or non-finite,
    /// so no member could form a view on them.
    pub unreadable: Vec<String>,
    pub resolved: Vec<Resolution>,
    pub abandoned: Vec<String>,
    /// Every arbitration, disagreement attached; there is no merged number
    /// in this record apart from the views it came from.
    pub arbitrations: Vec<Arbitration>,
    pub system: SystemView,
    /// The federation's own journal for this pass, drained into the log.
    /// `journal_base` is the absolute position of its first event.
    pub journal_base: usize,
    pub events: Vec<Event>,
    /// The §24.9 active-sensing requests this pass's disagreements were
    /// worth issuing (AMBIENT-10093, CONTRACT-6111): one per arbitration
    /// whose spread is material on a proposition with exposure at stake,
    /// as [`Federation::information_request`] builds it. Absent on a record
    /// written before the field existed.
    #[serde(default)]
    pub information_requests: Vec<qip_contracts::control::InformationRequest>,
    /// The disagreement spread behind each request, in basis points, in the
    /// same order as `information_requests` (ADR 0158 decision 10). The
    /// contract states what is asked and what it is worth, not how much the
    /// views differ, and the research loop ranks a request by how much
    /// uncertainty answering it would remove — so the spread travels beside
    /// the request rather than being guessed back from a value and an
    /// exposure. Absent on a record written before the field existed.
    #[serde(default)]
    pub request_spreads_bp: Vec<u32>,
    /// Every proposition on which two views point opposite ways, each past
    /// [`CONTRADICTION_MARGIN`] from even odds (R12-WORLD-1181). This is
    /// distinct from a wide spread. Two members at 0.55 and 0.95 disagree on
    /// how likely the outcome is; two at 0.85 and 0.15 assert opposite
    /// outcomes, and only the second is a contradiction. Derived from the
    /// recorded views alone, so [`replay`] recomputes and checks it. Absent
    /// on a record written before the field existed.
    #[serde(default)]
    pub contradictions: Vec<ViewContradiction>,
}

/// How far from even odds each of two opposed views must be before the
/// pair is recorded as a contradiction, not a lean. A chosen constant: a
/// view at 0.55 against one at 0.45 is two members near indifference, not
/// two claims, and recording it would bury the contradictions that matter.
pub const CONTRADICTION_MARGIN: f64 = 0.1;

/// Two members asserting opposite outcomes for one proposition. Both views
/// are kept as recorded; nothing here picks a side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewContradiction {
    pub proposition: String,
    /// The member most sure the proposition holds, and its probability.
    pub for_model: String,
    pub for_probability: f64,
    /// The member most sure it does not, and its probability.
    pub against_model: String,
    pub against_probability: f64,
}

/// The contradictions in `arbitrations`: per proposition, the most confident
/// view on each side of even odds, when both are past
/// [`CONTRADICTION_MARGIN`]. Ties go to the lower model id, so a replay
/// names the same pair.
pub fn contradictions_in(arbitrations: &[Arbitration]) -> Vec<ViewContradiction> {
    let mut found = Vec::new();
    for a in arbitrations {
        let mut views: Vec<&qip_world_model::federation::ScoredView> =
            a.disagreement.views.iter().collect();
        views.sort_by(|x, y| x.model.cmp(&y.model));
        let most_for = views
            .iter()
            .filter(|v| v.probability >= 0.5 + CONTRADICTION_MARGIN)
            .fold(
                None,
                |best: Option<&&qip_world_model::federation::ScoredView>, v| match best {
                    Some(b) if b.probability >= v.probability => Some(b),
                    _ => Some(v),
                },
            );
        let most_against = views
            .iter()
            .filter(|v| v.probability <= 0.5 - CONTRADICTION_MARGIN)
            .fold(
                None,
                |best: Option<&&qip_world_model::federation::ScoredView>, v| match best {
                    Some(b) if b.probability <= v.probability => Some(b),
                    _ => Some(v),
                },
            );
        if let (Some(f), Some(g)) = (most_for, most_against) {
            found.push(ViewContradiction {
                proposition: a.proposition.clone(),
                for_model: f.model.clone(),
                for_probability: f.probability,
                against_model: g.model.clone(),
                against_probability: g.probability,
            });
        }
    }
    found
}

impl EventBody for FederationReview {
    const TOPIC: Topic = Topic::WorldModelUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.cycle))
    }
}

/// What every review's idempotency key starts with: how a replay tells this
/// record from anything else written under the same topic.
pub const IDEMPOTENCY_PREFIX: &str = "federation:";

impl FederationReview {
    /// How many arbitrations reached each decision, as a short clause.
    pub fn detail(&self) -> String {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &self.arbitrations {
            let name = match a.decision {
                Decision::Merge => "merged",
                Decision::Branch => "branched",
                Decision::Abstain => "abstained",
                Decision::RequestMoreInformation(_) => "asked for more",
            };
            *counts.entry(name).or_default() += 1;
        }
        let decisions: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        let uncovered = if self.uncovered.is_empty() {
            String::new()
        } else {
            format!(", uncovered: {}", self.uncovered.join(", "))
        };
        let contradicted = if self.contradictions.is_empty() {
            String::new()
        } else {
            format!(", {} contradiction(s)", self.contradictions.len())
        };
        format!(
            "; federation of {} model(s) arbitrated {} proposition(s) ({}), resolved {}, \
             abandoned {}, {} silent{uncovered}{contradicted}",
            self.population.len(),
            self.arbitrations.len(),
            if decisions.is_empty() {
                "none".to_string()
            } else {
                decisions.join(", ")
            },
            self.resolved.len(),
            self.abandoned.len(),
            self.silent.len(),
        )
    }
}

/// What a pending forecast resolves against.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Instrument {
        subject: String,
        close: f64,
        through: Timestamp,
        cycle: u64,
    },
    Halted {
        cycle: u64,
    },
    Chain {
        trades: u64,
        cycle: u64,
    },
    Funding {
        cash: f64,
        cycle: u64,
    },
    Macro {
        series: String,
        positive: bool,
        valid_at: Timestamp,
        cycle: u64,
        since: Timestamp,
    },
    Currency {
        series: String,
        level: f64,
        up: bool,
        valid_at: Timestamp,
        cycle: u64,
        since: Timestamp,
    },
}

impl Pending {
    fn cycle(&self) -> u64 {
        match self {
            Self::Instrument { cycle, .. }
            | Self::Halted { cycle }
            | Self::Chain { cycle, .. }
            | Self::Funding { cycle, .. }
            | Self::Macro { cycle, .. }
            | Self::Currency { cycle, .. } => *cycle,
        }
    }

    /// Whether this forecast has waited past its patience: a macro or
    /// currency forecast by elapsed time ([`MACRO_PATIENCE`]), every other by
    /// passes ([`FEDERATION_PENDING_CYCLES`]).
    fn exhausted(&self, cycle: u64, now: Timestamp) -> bool {
        match self {
            Self::Macro { since, .. } | Self::Currency { since, .. } => {
                now.since(*since) >= MACRO_PATIENCE
            }
            _ => cycle.saturating_sub(self.cycle()) >= FEDERATION_PENDING_CYCLES,
        }
    }
}

/// The federation and what it is waiting to learn.
#[derive(Debug, Default)]
pub struct FederationStage {
    federation: Federation,
    registered: bool,
    pending: BTreeMap<String, Pending>,
    /// The confirmed trade count the on-chain member last read.
    last_chain_trades: Option<u64>,
    /// The reference period of the last release the macro member read, per
    /// series (ADR 0232): a release is fed once, not every pass it stays
    /// readable. Bounded by [`MAX_MACRO_SERIES`].
    last_macro: BTreeMap<String, Timestamp>,
    /// The last fixing the macro member read per currency series: its level
    /// and period. Bounded by [`MAX_MACRO_SERIES`] with `last_macro`.
    last_fixing: BTreeMap<String, (f64, Timestamp)>,
    /// Account-currency exposure per subject, as the composition root last
    /// stated it ([`Self::set_stakes`]); what a disagreement's value of
    /// information is scaled by. Empty means nothing is at stake anywhere
    /// and no request is issued.
    stakes: BTreeMap<String, Decimal>,
    /// Asset class per subject ([`Self::set_asset_classes`]); what the
    /// domain members are declared and fed from.
    asset_classes: BTreeMap<String, String>,
    /// Classes whose domain member has been declared.
    domain_members: BTreeSet<String>,
    /// The probability each member gave each pending proposition, as the
    /// federation took it: what a resolution charges to the member's entry
    /// in the self-model (R12-MODEL-6016). Kept only for propositions still
    /// pending, so it is bounded by `pending`.
    views: BTreeMap<String, BTreeMap<String, f64>>,
    /// The federation's latest arbitrated probability per pending
    /// proposition: what a resolution charges to the federation as an
    /// ensemble (R12-MODEL-6016). Bounded by `pending`, like `views`.
    ensembles: BTreeMap<String, f64>,
    /// The previous cycle's slowest stage ([`Self::observe_cycle`]).
    slowest_stage: Option<Duration>,
    /// Each branch the stage opened, mapped to the member whose reading it
    /// follows: a branch is fed whatever that member is fed, so it holds a
    /// forecast on every proposition until an outcome closes it (WORLD-3041).
    /// Entries leave when the federation closes or merges the branch.
    branches: BTreeMap<String, String>,
    /// Ids already spent on branches and merges; the federation never
    /// reuses an id.
    serial: u64,
}

/// Hand one view to `model`; true when the federation took it. A refusal is
/// recorded against the model rather than dropped.
fn feed(
    federation: &mut Federation,
    model: &str,
    assessment: Assessment,
    fed: &mut BTreeSet<String>,
    refused: &mut Vec<(String, String)>,
    views: &mut BTreeMap<String, BTreeMap<String, f64>>,
    branches: &BTreeMap<String, String>,
) -> bool {
    let (proposition, probability) = (assessment.proposition.clone(), assessment.probability);
    let followers: Vec<(&String, Assessment)> = branches
        .iter()
        .filter(|(_, source)| source.as_str() == model)
        .map(|(branch, _)| (branch, assessment.clone()))
        .collect();
    match federation.assess(model, assessment) {
        Ok(()) => {
            fed.insert(model.to_string());
            views
                .entry(proposition.clone())
                .or_default()
                .insert(model.to_string(), probability);
            for (branch, mirrored) in followers {
                // A branch the federation already closed or merged is no
                // longer fed; its record stays readable and says why.
                if !matches!(
                    federation.status(branch),
                    Ok(qip_world_model::federation::Status::Live)
                ) {
                    continue;
                }
                match federation.assess(branch, mirrored) {
                    Ok(()) => {
                        fed.insert(branch.clone());
                        views
                            .entry(proposition.clone())
                            .or_default()
                            .insert(branch.clone(), probability);
                    }
                    Err(error) => refused.push((branch.clone(), error.message().to_string())),
                }
            }
            true
        }
        Err(error) => {
            refused.push((model.to_string(), error.message().to_string()));
            false
        }
    }
}

/// How many of the latest returns a member's own residuals are read over.
const RETRO_STEPS: usize = 20;

/// The seven tape members' readings of one return series, in the order
/// momentum, reversion, regional microstructure, stress world, rare regime,
/// alternate future, standing aside, and the intermediates the uncertainty terms need.
struct TapeReading {
    p: [f64; 7],
    z_momentum: f64,
    z_reversion: f64,
    base: f64,
    after_tail: f64,
    tail_days: usize,
}

/// The members' probabilities that the next return is positive, from `r`
/// alone. One function, used both for the live view and for replaying the
/// members over the series' own past (see [`residual_fit`]), so a member's
/// fit is measured with the arithmetic that produces its forecast.
fn tape_reading(r: &[f64]) -> Option<TapeReading> {
    if r.len() < 2 {
        return None;
    }
    let sd = deviation(r);
    if !(sd.is_finite() && sd > 0.0) {
        return None;
    }
    let short = &r[r.len().saturating_sub(5)..];
    let z_momentum = mean(short) / (sd / (short.len() as f64).sqrt());
    let window = tail(r);
    let z_reversion = window.iter().sum::<f64>() / (sd * (window.len() as f64).sqrt());
    let mut sorted = r.to_vec();
    sorted.sort_by(f64::total_cmp);
    let shock = sorted[(sorted.len() / 20).min(sorted.len() - 1)];
    let stressed: Vec<f64> = window.iter().map(|x| x + shock).collect();
    let base = share_positive(r);
    let (after_tail, tail_days) = after_tail_moves(r);
    Some(TapeReading {
        p: [
            logistic(0.5 * z_momentum),
            logistic(-0.5 * z_reversion),
            share_positive(window),
            share_positive(&stressed),
            after_tail,
            1.0 - share_positive(&stressed),
            base,
        ],
        z_momentum,
        z_reversion,
        base,
        after_tail,
        tail_days,
    })
}

/// A member's evidence fit on one instrument (R12-WORLD-3040): how much
/// history the member has, discounted by how well it forecast the series'
/// own latest returns when replayed over the history before each of them —
/// `1 - Brier` over up to [`RETRO_STEPS`] steps. Two members on one tape get
/// different fits when one of them has been wrong about it. A series too
/// short to replay at all is half-credited, because no residual is no
/// evidence of fit in either direction.
fn residual_fit(r: &[f64], member: usize) -> f64 {
    let sufficiency = (r.len() as f64 / 100.0).min(1.0);
    let first = r
        .len()
        .saturating_sub(RETRO_STEPS)
        .max(FEDERATION_MIN_RETURNS / 2);
    let (mut sum, mut steps) = (0.0, 0usize);
    for t in first..r.len() {
        if let Some(reading) = tape_reading(&r[..t]) {
            let outcome = if r[t] > 0.0 { 1.0 } else { 0.0 };
            sum += (reading.p[member] - outcome).powi(2);
            steps += 1;
        }
    }
    if steps == 0 {
        return sufficiency * 0.5;
    }
    sufficiency * (1.0 - sum / steps as f64).clamp(0.0, 1.0)
}

/// The scope families the population must cover (WORLD-3037, -3050, -3051,
/// -3052 and ASSET-3017's federation half). A family with no live,
/// unexpired member is named in [`FederationReview::uncovered`].
pub const REQUIRED_FAMILIES: [&str; 7] = [
    "regional",
    "scenario",
    "counterfactual",
    "self/system",
    "crypto/on-chain",
    "company/fundamental",
    "portfolio/capital",
];

/// Whether a member's scope covers `family`, for a process serving `region`.
pub fn covers(family: &str, scope: &Scope, region: &str) -> bool {
    match family {
        // `global` is never a region in the regional sense: a model of
        // everywhere has no local venues, regulation or language.
        "regional" => scope.region == region && scope.region != "global",
        "scenario" => scope.kind == ModelKind::Scenario,
        "counterfactual" => scope.kind == ModelKind::Counterfactual,
        "self/system" => scope.domain == "self",
        "crypto/on-chain" => scope.domain == "crypto/on-chain",
        "company/fundamental" => scope.domain == "company/fundamental",
        "portfolio/capital" => scope.domain == "portfolio/capital",
        // Not in `REQUIRED_FAMILIES`: a domain member exists only once the
        // composition root has named an asset class, so requiring one would
        // report a process with no classes named as uncovered every cycle.
        "domain" => scope.hypothesis == DOMAIN_HYPOTHESIS,
        _ => false,
    }
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn deviation(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
}

fn share_positive(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.5;
    }
    xs.iter().filter(|r| **r > 0.0).count() as f64 / xs.len() as f64
}

/// The rare-regime member's reading: the share of positive returns on the
/// day after a move in the largest [`RARE_REGIME_TAIL`] of absolute moves,
/// and how many such days there were. Zero days — the only tail move is the
/// latest, with no day after it yet — is reported as zero, not guessed.
fn after_tail_moves(xs: &[f64]) -> (f64, usize) {
    let mut magnitudes: Vec<f64> = xs.iter().map(|x| x.abs()).collect();
    magnitudes.sort_by(f64::total_cmp);
    let rank = ((magnitudes.len() as f64) * (1.0 - RARE_REGIME_TAIL)).floor() as usize;
    let Some(threshold) = magnitudes.get(rank.min(magnitudes.len().saturating_sub(1))) else {
        return (0.5, 0);
    };
    let after: Vec<f64> = xs
        .windows(2)
        .filter(|pair| pair[0].abs() >= *threshold)
        .map(|pair| pair[1])
        .collect();
    if after.is_empty() {
        return (0.5, 0);
    }
    (share_positive(&after), after.len())
}

fn tail(xs: &[f64]) -> &[f64] {
    &xs[xs.len().saturating_sub(20)..]
}

/// A narrowing the federation may apply to an instrument's weight bound, in
/// `(0, 1]` (ADR 0125 decision 4).
///
/// The only way to obtain one is [`Narrowing::of_spread`], and it has no arm
/// above one, so a federation that disagrees can only subtract from a size and
/// an agreeing population changes nothing. The field is private for the same
/// reason `Stance::multiplier` has no constructor taking a number: a bound that
/// could be raised from here would be a control nobody chose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Narrowing(f64);

/// The multiplier a disagreement of at least this spread earns, widest first.
/// Declared rather than fitted: the first band starts at the federation's own
/// definition of a material disagreement
/// ([`qip_world_model::federation::MATERIAL_DISAGREEMENT_SPREAD`]), and a
/// spread below it is not disagreement worth a smaller size. Nobody fitted
/// these numbers; LEARN has not scored them (ADR 0125, "What it costs").
pub const NARROWING_SCHEDULE: [(f64, f64); 2] = [(0.4, 0.5), (0.2, 0.75)];

impl Narrowing {
    /// The narrowing a disagreement of `spread` earns, or `None` where it
    /// earns none. A spread that is not a finite number earns none either: a
    /// view nobody could measure is absence, which ADR 0125 says is not
    /// disagreement.
    pub fn of_spread(spread: f64) -> Option<Self> {
        if !spread.is_finite() {
            return None;
        }
        NARROWING_SCHEDULE
            .iter()
            .find(|(at_least, _)| spread >= *at_least)
            .map(|(_, multiplier)| Self(*multiplier))
    }

    /// The multiplier, in `(0, 1)`.
    pub const fn multiplier(self) -> f64 {
        self.0
    }
}

/// The narrowings this review's arbitrations earn, by instrument.
///
/// Read from the arbitration of each instrument's own proposition at the
/// review's cycle, so an instrument with no arbitration (too little history,
/// an uncovered scope, a silent member) earns nothing.
pub fn narrowings_of(
    review: &FederationReview,
    subjects: impl IntoIterator<Item = String>,
) -> BTreeMap<String, (Narrowing, f64)> {
    let mut narrowed = BTreeMap::new();
    for subject in subjects {
        let proposition = instrument_proposition(&subject, review.cycle);
        let Some(arbitration) = review
            .arbitrations
            .iter()
            .find(|arbitration| arbitration.proposition == proposition)
        else {
            continue;
        };
        let spread = arbitration.disagreement.spread;
        if let Some(narrowing) = Narrowing::of_spread(spread) {
            narrowed.insert(subject, (narrowing, spread));
        }
    }
    narrowed
}

/// Write the federation's narrowings into DECIDE's `caps` map, multiplying an
/// existing entry (ADR 0125 decision 6) and naming each one in `notes`.
///
/// One narrowing per instrument per construction, however many theses name it,
/// for the reason `regime_allocation::narrow` gives: applying it per thesis
/// would square it.
pub fn narrow(
    caps: &mut BTreeMap<String, f64>,
    notes: &mut Vec<String>,
    objects: impl IntoIterator<Item = String>,
    narrowings: &BTreeMap<String, (Narrowing, f64)>,
) {
    let distinct: std::collections::BTreeSet<String> = objects.into_iter().collect();
    for object in distinct {
        let Some((narrowing, spread)) = narrowings.get(&object) else {
            continue;
        };
        *caps.entry(object.clone()).or_insert(1.0) *= narrowing.multiplier();
        notes.push(format!(
            "{object}: the world models disagree by {spread:.2}, narrowed to {:.2} of the bound",
            narrowing.multiplier()
        ));
    }
}

/// The proposition one instrument's next close answers.
pub fn instrument_proposition(subject: &str, cycle: u64) -> String {
    format!("{subject}:next-close-up@{cycle}")
}

/// The proposition the next pass's kill-switch state answers.
pub fn halted_proposition(cycle: u64) -> String {
    format!("self:halted-next-pass@{cycle}")
}

/// The proposition the next pass's book cash answers (ADR 0194).
pub fn funding_proposition(cycle: u64) -> String {
    format!("capital:funding-draw-next-pass@{cycle}")
}

/// The proposition the next confirmed chain view answers.
pub fn chain_proposition(cycle: u64) -> String {
    format!("crypto-onchain:confirmed-trades-rise@{cycle}")
}

impl FederationStage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn federation(&self) -> &Federation {
        &self.federation
    }

    /// Forecasts still waiting for reality, by proposition.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// State the exposure riding on each subject, in the account currency,
    /// before a pass (AMBIENT-10093). Read by the pass that follows; a
    /// subject not named has nothing at stake.
    pub fn set_stakes(&mut self, stakes: BTreeMap<String, Decimal>) {
        self.stakes = stakes;
    }

    /// The exposure last stated for `subject` ([`Self::set_stakes`]), so a
    /// composition can be proven to state it without running a pass over a tape.
    pub fn stake_of(&self, subject: &str) -> Option<Decimal> {
        self.stakes.get(subject).copied()
    }

    /// Name each subject's asset class before a pass (MODEL-4009). A class
    /// seen for the first time gets its domain member declared on that
    /// pass; a subject not named is read by no domain member.
    pub fn set_asset_classes(&mut self, classes: BTreeMap<String, String>) {
        self.asset_classes = classes;
    }

    /// The id of the domain member for `class`.
    /// Record a finished cycle's slowest stage, for the next pass's
    /// self/system member to read.
    pub fn observe_cycle(&mut self, slowest_stage: Duration) {
        self.slowest_stage = Some(slowest_stage);
    }

    /// The slowest stage [`Self::observe_cycle`] last recorded.
    pub fn slowest_stage(&self) -> Option<Duration> {
        self.slowest_stage
    }

    pub fn domain_member(class: &str) -> String {
        format!("{DOMAIN_PREFIX}{class}.global.meso")
    }

    fn declare_domain_members(&mut self, now: Timestamp, refused: &mut Vec<(String, String)>) {
        let classes: BTreeSet<String> = self.asset_classes.values().cloned().collect();
        for class in classes {
            if self.domain_members.contains(&class) {
                continue;
            }
            let id = Self::domain_member(&class);
            let declaration = Declaration {
                id: id.clone(),
                scope: Some(Scope {
                    domain: class.clone(),
                    region: "global".to_string(),
                    horizon: Duration::from_days(5),
                    abstraction: Abstraction::Meso,
                    hypothesis: DOMAIN_HYPOTHESIS.to_string(),
                    kind: ModelKind::Observational,
                }),
                evidence_lineage: vec!["sense:bar-closes".to_string()],
                calibration: Some(CalibrationClaim { max_brier: 0.25 }),
                update_cadence: Some(Duration::from_days(1)),
                expires_at: Some(now.saturating_add(MEMBER_LEASE)),
            };
            match WorldModelSpec::declare(declaration)
                .and_then(|spec| self.federation.register(spec, now))
            {
                Ok(()) => {
                    self.domain_members.insert(class);
                }
                Err(error) => refused.push((id, error.message().to_string())),
            }
        }
    }

    /// Each named class's breadth over the readable instruments: the share
    /// whose short-window mean return is positive, and how many were read.
    fn class_breadth(&self, inputs: &FederationInputs) -> BTreeMap<String, (f64, usize)> {
        let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for view in inputs
            .instruments
            .iter()
            .filter(|v| v.returns.len() >= FEDERATION_MIN_RETURNS)
            .take(FEDERATION_MAX_SUBJECTS)
        {
            let Some(class) = self.asset_classes.get(&view.subject) else {
                continue;
            };
            let short = &view.returns[view.returns.len().saturating_sub(5)..];
            let entry = tally.entry(class.clone()).or_insert((0, 0));
            entry.1 += 1;
            if mean(short) > 0.0 {
                entry.0 += 1;
            }
        }
        tally
            .into_iter()
            .map(|(class, (up, n))| (class, (up as f64 / n as f64, n)))
            .collect()
    }

    fn declarations(region: &str, at: Timestamp) -> Vec<Declaration> {
        let declare = |id: String,
                       domain: &str,
                       region: &str,
                       days: i64,
                       abstraction: Abstraction,
                       hypothesis: &str,
                       kind: ModelKind,
                       lineage: &[&str],
                       max_brier: f64| Declaration {
            id,
            scope: Some(Scope {
                domain: domain.to_string(),
                region: region.to_string(),
                horizon: Duration::from_days(days),
                abstraction,
                hypothesis: hypothesis.to_string(),
                kind,
            }),
            evidence_lineage: lineage.iter().map(|s| (*s).to_string()).collect(),
            calibration: Some(CalibrationClaim { max_brier }),
            update_cadence: Some(Duration::from_days(1)),
            expires_at: Some(at.saturating_add(MEMBER_LEASE)),
        };
        // A Brier ceiling of 0.25 is a coin flip's score: the claim each
        // tape member makes is that it is no worse than not knowing.
        vec![
            declare(
                MOMENTUM.into(),
                "markets",
                "global",
                1,
                Abstraction::Micro,
                "momentum",
                ModelKind::Observational,
                &["sense:bar-closes"],
                0.25,
            ),
            declare(
                ORDER_FLOW.into(),
                "markets",
                "global",
                1,
                Abstraction::Micro,
                "order-flow-imbalance",
                ModelKind::Observational,
                &[ORDER_FLOW_SOURCE],
                0.25,
            ),
            declare(
                REVERSION.into(),
                "markets",
                "global",
                5,
                Abstraction::Meso,
                "mean-reversion",
                ModelKind::Observational,
                &["sense:bar-closes"],
                0.25,
            ),
            declare(
                format!("{REGIONAL_PREFIX}{region}.micro"),
                "markets",
                region,
                1,
                Abstraction::Micro,
                "regional-microstructure",
                ModelKind::Observational,
                &["sense:bar-closes", "sense:quoted-spreads"],
                0.25,
            ),
            declare(
                STRESS_SCENARIO.into(),
                "markets",
                "global",
                20,
                Abstraction::Macro,
                "stress-world",
                ModelKind::Scenario,
                &["sense:bar-closes"],
                0.3,
            ),
            declare(
                RARE_REGIME.into(),
                "markets",
                "global",
                10,
                Abstraction::Macro,
                "rare-regime",
                ModelKind::Scenario,
                &["sense:bar-closes"],
                0.3,
            ),
            declare(
                ALTERNATE_FUTURE.into(),
                "markets",
                "global",
                20,
                Abstraction::Macro,
                "alternate-future",
                ModelKind::Counterfactual,
                &["sense:bar-closes"],
                0.3,
            ),
            declare(
                STANDING_ASIDE.into(),
                "markets",
                "global",
                5,
                Abstraction::Meso,
                "standing-aside",
                ModelKind::Counterfactual,
                &["sense:bar-closes"],
                0.25,
            ),
            declare(
                CAUSAL_GLOBAL.into(),
                "markets",
                "global",
                10,
                Abstraction::Macro,
                "causal-graph",
                ModelKind::Observational,
                &["world:causal-graph", "sense:bar-closes"],
                0.3,
            ),
            declare(
                SELF_SYSTEM.into(),
                "self",
                "global",
                1,
                Abstraction::Macro,
                "operational-health",
                ModelKind::Observational,
                &[
                    "kernel:kill-switch",
                    "kernel:autonomy",
                    "kernel:agent-manifests",
                ],
                0.25,
            ),
            declare(
                COMPANY.into(),
                "company/fundamental",
                "global",
                90,
                Abstraction::Meso,
                "fundamental-surprise",
                ModelKind::Observational,
                &["sense:fundamentals", "world:issuer-graph"],
                0.3,
            ),
            declare(
                CAPITAL.into(),
                "portfolio/capital",
                "global",
                1,
                Abstraction::Meso,
                "funding-draw",
                ModelKind::Observational,
                &["kernel:capital", "kernel:pending-theses"],
                0.25,
            ),
            declare(
                MACRO.into(),
                "macro",
                "global",
                30,
                Abstraction::Macro,
                "surprise-persistence",
                ModelKind::Observational,
                &[MACRO_SOURCE],
                0.25,
            ),
            declare(
                ON_CHAIN.into(),
                "crypto/on-chain",
                "global",
                1,
                Abstraction::Micro,
                "on-chain-flow",
                ModelKind::Observational,
                &["chain:confirmed-view"],
                0.25,
            ),
        ]
    }

    /// One pass: resolve what reality answered, register on first use, feed
    /// every member a view, arbitrate, and return the record to journal.
    ///
    /// Infallible on purpose. A refusal from the federation is a finding the
    /// review carries in `refused`, not a reason to lose the other members'
    /// state for the pass.
    pub fn run(&mut self, inputs: &FederationInputs, now: Timestamp) -> FederationReview {
        let mut refused = Vec::new();
        if !self.registered {
            for declaration in Self::declarations(&inputs.region, now) {
                let id = declaration.id.clone();
                if let Err(error) = WorldModelSpec::declare(declaration)
                    .and_then(|spec| self.federation.register(spec, now))
                {
                    refused.push((id, error.message().to_string()));
                }
            }
            self.registered = true;
        }
        self.declare_domain_members(now, &mut refused);
        let breadth = self.class_breadth(inputs);

        let (resolved, abandoned) = self.resolve(inputs, now);

        let mut fed: BTreeSet<String> = BTreeSet::new();
        let mut propositions = Vec::new();

        let regional = format!("{REGIONAL_PREFIX}{}.micro", inputs.region);
        let mut unreadable = Vec::new();
        for view in inputs
            .instruments
            .iter()
            .filter(|v| v.returns.len() >= FEDERATION_MIN_RETURNS)
            .take(FEDERATION_MAX_SUBJECTS)
        {
            let proposition = instrument_proposition(&view.subject, inputs.cycle);
            let r = &view.returns;
            let sd = deviation(r);
            let Some(TapeReading {
                p,
                z_momentum,
                z_reversion,
                base,
                after_tail,
                tail_days,
            }) = tape_reading(r).filter(|_| view.last_close.is_finite())
            else {
                // A flat or broken series is not a distribution any member
                // can reason about. Skipped, and named in the review: an
                // instrument silently missing from the federation reads
                // exactly like one every member was content with.
                unreadable.push(view.subject.clone());
                continue;
            };
            let sufficiency = (r.len() as f64 / 100.0).min(1.0);
            // Each member's own fit on this tape (R12-WORLD-3040), not one
            // number for all of them.
            let fits: [f64; 7] = std::array::from_fn(|member| residual_fit(r, member));
            // Causal coherence (R12-WORLD-3040): how far a member's reading
            // is from what the causal graph's live claims into this
            // instrument imply. With no claim there is nothing to cohere
            // with, and the member keeps the prior it was declared under.
            let causal_p = view
                .causal
                .filter(|c| c.lean.is_finite())
                .map(|c| logistic(c.lean / sd));
            let coherence = |probability: f64, prior: f64| match causal_p {
                Some(graph) => (1.0 - (probability - graph).abs()).clamp(0.0, 1.0),
                None => prior,
            };
            let microstructure_uncertainty = match view.spread_ratio {
                Some(x) if x.is_finite() => (0.3 + 0.2 * (x - 1.0).max(0.0)).min(1.0),
                // Microstructure unmeasured: the member says it barely knows.
                _ => 0.7,
            };
            let at = now;
            let views: [(&str, f64, f64, f64, f64); 7] = [
                (
                    MOMENTUM,
                    p[0],
                    fits[0],
                    coherence(p[0], 0.5),
                    1.0 / (1.0 + z_momentum.abs()),
                ),
                (
                    REVERSION,
                    p[1],
                    fits[1],
                    coherence(p[1], 0.5),
                    1.0 / (1.0 + z_reversion.abs()),
                ),
                (
                    regional.as_str(),
                    p[2],
                    fits[2],
                    coherence(p[2], 0.4),
                    microstructure_uncertainty,
                ),
                (STRESS_SCENARIO, p[3], fits[3], coherence(p[3], 0.4), 0.6),
                // No tail day with a day after it is no rare-regime
                // evidence at all: the member says it knows nothing.
                (
                    RARE_REGIME,
                    p[4],
                    fits[4],
                    coherence(p[4], 0.4),
                    if tail_days == 0 {
                        1.0
                    } else {
                        (2.0 * (after_tail * (1.0 - after_tail) / tail_days as f64).sqrt())
                            .clamp(0.1, 1.0)
                    },
                ),
                (ALTERNATE_FUTURE, p[5], fits[5], coherence(p[5], 0.3), 0.6),
                (
                    STANDING_ASIDE,
                    p[6],
                    fits[6],
                    coherence(p[6], 0.3),
                    (2.0 * (base * (1.0 - base) / r.len() as f64).sqrt()).min(1.0),
                ),
            ];
            // The global causal model's own view (R12-WORLD-3042): the graph's
            // live claims into this instrument, each cause's latest move
            // carried over its signed transmission. Where the graph holds no
            // claim into it the member says so — an even view it has almost
            // no evidence for — rather than going silent, so the population
            // check reports a member that stopped producing state and not
            // one with nothing to say about this tape.
            let causal_view = match view.causal.filter(|c| c.lean.is_finite()) {
                Some(c) => (
                    CAUSAL_GLOBAL.to_string(),
                    logistic(c.lean / sd),
                    sufficiency * (c.claims as f64 / 3.0).min(1.0),
                    0.6,
                    (1.0 - c.confidence).clamp(0.1, 1.0),
                ),
                None => (CAUSAL_GLOBAL.to_string(), 0.5, 0.05, 0.5, 0.9),
            };
            // The domain member's view (MODEL-4009): the breadth of the
            // instrument's own asset class, which says nothing about this
            // tape and everything about its neighbours' — a view the tape
            // members cannot hold, so their disagreement is real.
            let domain_view = self
                .asset_classes
                .get(&view.subject)
                .and_then(|class| breadth.get(class).map(|b| (class.clone(), *b)))
                .map(|(class, (share, n))| {
                    (
                        Self::domain_member(&class),
                        share,
                        sufficiency,
                        0.4,
                        (2.0 * (share * (1.0 - share) / n as f64).sqrt()).clamp(0.1, 1.0),
                    )
                });
            // The company member's view (R12-WORLD-3044): the issuer's latest
            // surprise, which says nothing about the tape.
            let company_view = view.fundamental_surprise.map(|surprise| {
                (
                    COMPANY.to_string(),
                    logistic(FUNDAMENTAL_SURPRISE_SLOPE * surprise),
                    0.5,
                    0.5,
                    0.6,
                )
            });
            // The order-flow member's view (ADR 0231): this cycle's signed
            // aggressor volume summed across venues, as an imbalance in
            // [-1, 1]. Its evidence grows with the prints that stated a side,
            // up to twenty; with none it is silent rather than even, because
            // a tape that named no aggressor told it nothing.
            let flow_view = {
                let (buy, sell, prints) =
                    view.order_flow
                        .iter()
                        .fold((0.0_f64, 0.0_f64, 0_u64), |(b, s, n), o| {
                            (
                                b + o.buy_volume.to_f64(),
                                s + o.sell_volume.to_f64(),
                                n + o.buy_count + o.sell_count,
                            )
                        });
                let total = buy + sell;
                (total > 0.0 && total.is_finite()).then(|| {
                    let imbalance = (buy - sell) / total;
                    (
                        ORDER_FLOW.to_string(),
                        logistic(ORDER_FLOW_SLOPE * imbalance),
                        (prints as f64 / 20.0).min(1.0),
                        0.4,
                        0.6,
                    )
                })
            };
            let mut any = false;
            for (model, probability, evidence_fit, causal_coherence, uncertainty) in views
                .into_iter()
                .map(|(m, p, f, c, u)| (m.to_string(), p, f, c, u))
                .chain(domain_view)
                .chain(company_view)
                .chain(flow_view)
                .chain(std::iter::once(causal_view))
            {
                let model = model.as_str();
                any |= feed(
                    &mut self.federation,
                    model,
                    Assessment {
                        proposition: proposition.clone(),
                        probability,
                        evidence_fit,
                        causal_coherence,
                        uncertainty,
                        at,
                    },
                    &mut fed,
                    &mut refused,
                    &mut self.views,
                    &self.branches,
                );
            }
            if any {
                self.pending.insert(
                    proposition.clone(),
                    Pending::Instrument {
                        subject: view.subject.clone(),
                        close: view.last_close,
                        through: view.observed_through,
                        cycle: inputs.cycle,
                    },
                );
                propositions.push(proposition);
            }
        }

        // The self/system member: will the next pass find the platform
        // halted? A tripped switch is sticky, so it says yes; otherwise the
        // risk rises with the capabilities the organisation holds and may not
        // use. Its view is the platform's own failure state as a forecast.
        // WORLD-3052: latency and data health are the platform's own state
        // too. A stage past its budget or a capability the degradation table
        // holds stale each raise the risk; health nobody could read is
        // counted as one degraded capability and widens the uncertainty,
        // because unknown health is not good health.
        let system = &inputs.system;
        let slow = system
            .slowest_stage
            .is_some_and(|d| d > SELF_STAGE_LATENCY_BUDGET);
        let capabilities = qip_contracts::degradation::Capability::all().len();
        let degraded = system.degraded_capabilities.unwrap_or(1).min(capabilities);
        let halted_p = if system.halted {
            0.9
        } else {
            0.05 + 0.1 * (system.expired_manifests.min(5) as f64 / 5.0)
                + if slow { 0.1 } else { 0.0 }
                + 0.1 * (degraded as f64 / capabilities as f64)
        };
        let health_known = system.slowest_stage.is_some() && system.degraded_capabilities.is_some();
        let proposition = halted_proposition(inputs.cycle);
        if feed(
            &mut self.federation,
            SELF_SYSTEM,
            Assessment {
                proposition: proposition.clone(),
                probability: halted_p,
                evidence_fit: 1.0,
                causal_coherence: 0.6,
                uncertainty: match (system.halted, health_known) {
                    (true, _) => 0.1,
                    (false, true) => 0.3,
                    (false, false) => 0.45,
                },
                at: now,
            },
            &mut fed,
            &mut refused,
            &mut self.views,
            &self.branches,
        ) {
            self.pending.insert(
                proposition.clone(),
                Pending::Halted {
                    cycle: inputs.cycle,
                },
            );
            propositions.push(proposition);
        }

        // The capital member (ADR 0194): will the next pass find less cash?
        // A thesis waiting for capital is the book's own claim on funding,
        // and more cash on hand is more room to draw on. The formula is a
        // stated prior, not a fit: calibration charges every miss.
        if let Some(capital) = inputs.capital {
            let waiting = if capital.pending_theses > 0 { 0.5 } else { 0.0 };
            let probability = 0.15 + waiting + 0.15 * capital.cash_share.clamp(0.0, 1.0);
            let proposition = funding_proposition(inputs.cycle);
            if feed(
                &mut self.federation,
                CAPITAL,
                Assessment {
                    proposition: proposition.clone(),
                    probability,
                    evidence_fit: 0.6,
                    causal_coherence: 0.5,
                    uncertainty: 0.4,
                    at: now,
                },
                &mut fed,
                &mut refused,
                &mut self.views,
                &self.branches,
            ) {
                self.pending.insert(
                    proposition.clone(),
                    Pending::Funding {
                        cash: capital.cash,
                        cycle: inputs.cycle,
                    },
                );
                propositions.push(proposition);
            }
        }

        // The macro member (ADR 0232): a release that surprised its
        // consensus is read as likelier than not to be followed by a surprise
        // the same way — persistence, declared not fitted, at 0.6. Fed once
        // per release; a pass with no new release leaves it silent.
        for view in inputs
            .macros
            .iter()
            .filter(|v| v.surprise.is_finite() && v.surprise != 0.0)
        {
            let newer = self
                .last_macro
                .get(&view.series)
                .is_none_or(|read| view.valid_at > *read);
            if !newer {
                continue;
            }
            if !self.last_macro.contains_key(&view.series)
                && self.last_macro.len() + self.last_fixing.len() >= MAX_MACRO_SERIES
            {
                refused.push((
                    MACRO.to_string(),
                    format!(
                        "{} is a new series past the {MAX_MACRO_SERIES}-series bound; its \
                         release is not read",
                        view.series
                    ),
                ));
                continue;
            }
            self.last_macro.insert(view.series.clone(), view.valid_at);
            let proposition = macro_proposition(&view.series, inputs.cycle, view.simulated);
            if feed(
                &mut self.federation,
                MACRO,
                Assessment {
                    proposition: proposition.clone(),
                    probability: 0.6,
                    evidence_fit: if view.simulated { 0.15 } else { 0.3 },
                    causal_coherence: 0.4,
                    uncertainty: 0.7,
                    at: now,
                },
                &mut fed,
                &mut refused,
                &mut self.views,
                &self.branches,
            ) {
                self.pending.insert(
                    proposition.clone(),
                    Pending::Macro {
                        series: view.series.clone(),
                        positive: view.surprise > 0.0,
                        valid_at: view.valid_at,
                        cycle: inputs.cycle,
                        since: now,
                    },
                );
                propositions.push(proposition);
            }
        }

        // Currencies (ADR 0232): a fixing has no consensus and so no
        // surprise; the member reads its move from the previous fixing and
        // forecasts, at the same declared persistence, that the next fixing
        // moves the same way. The first fixing of a series only seeds the
        // level; a fixing equal to the last states no direction.
        for view in inputs.currencies.iter().filter(|v| v.level.is_finite()) {
            let previous = self.last_fixing.get(&view.series).copied();
            if previous.is_some_and(|(_, at)| view.valid_at <= at) {
                continue;
            }
            if previous.is_none()
                && self.last_macro.len() + self.last_fixing.len() >= MAX_MACRO_SERIES
            {
                refused.push((
                    MACRO.to_string(),
                    format!(
                        "{} is a new series past the {MAX_MACRO_SERIES}-series bound; its \
                         fixing is not read",
                        view.series
                    ),
                ));
                continue;
            }
            self.last_fixing
                .insert(view.series.clone(), (view.level, view.valid_at));
            let Some((last, _)) = previous.filter(|(last, _)| (*last - view.level).abs() > 0.0)
            else {
                continue;
            };
            let proposition = currency_proposition(&view.series, inputs.cycle, view.simulated);
            if feed(
                &mut self.federation,
                MACRO,
                Assessment {
                    proposition: proposition.clone(),
                    probability: 0.6,
                    evidence_fit: if view.simulated { 0.15 } else { 0.3 },
                    causal_coherence: 0.4,
                    uncertainty: 0.7,
                    at: now,
                },
                &mut fed,
                &mut refused,
                &mut self.views,
                &self.branches,
            ) {
                self.pending.insert(
                    proposition.clone(),
                    Pending::Currency {
                        series: view.series.clone(),
                        level: view.level,
                        up: view.level > last,
                        valid_at: view.valid_at,
                        cycle: inputs.cycle,
                        since: now,
                    },
                );
                propositions.push(proposition);
            }
        }

        if let Some(chain) = inputs.chain {
            // Persistence: activity that rose last pass is read as likelier
            // to rise again. A first sighting has no previous count and says
            // so through a lower evidence fit rather than a guessed trend.
            let previous = self.last_chain_trades.replace(chain.confirmed_trades);
            let rose = previous.is_some_and(|t| chain.confirmed_trades > t);
            let proposition = chain_proposition(inputs.cycle);
            if feed(
                &mut self.federation,
                ON_CHAIN,
                Assessment {
                    proposition: proposition.clone(),
                    probability: if rose { 0.6 } else { 0.4 },
                    evidence_fit: if previous.is_some() { 0.6 } else { 0.3 },
                    causal_coherence: 0.4,
                    uncertainty: 0.6,
                    at: now,
                },
                &mut fed,
                &mut refused,
                &mut self.views,
                &self.branches,
            ) {
                self.pending.insert(
                    proposition.clone(),
                    Pending::Chain {
                        trades: chain.confirmed_trades,
                        cycle: inputs.cycle,
                    },
                );
                propositions.push(proposition);
            }
        }

        // Renewed only for state actually produced: a member fed nothing
        // lapses at its lease and the population check says so.
        for model in &fed {
            if let Err(error) = self
                .federation
                .renew(model, now.saturating_add(MEMBER_LEASE), now)
            {
                refused.push((model.clone(), error.message().to_string()));
            }
        }

        let mut arbitrations = Vec::new();
        let mut information_requests = Vec::new();
        let mut request_spreads_bp = Vec::new();
        for proposition in &propositions {
            match self.federation.arbitrate(proposition, now) {
                Ok(a) => {
                    // A material disagreement on a proposition with exposure
                    // at stake is worth a question to active sensing
                    // (AMBIENT-10093); the federation gates it on the value
                    // of information and the contract refuses an ill-formed
                    // request, so what reaches the review is well-formed.
                    let exposure = match self.pending.get(proposition) {
                        Some(Pending::Instrument { subject, .. }) => {
                            self.stakes.get(subject).copied()
                        }
                        _ => None,
                    };
                    if let Some(exposure) = exposure {
                        let stake = InformationStake {
                            exposure,
                            budget_share_bp: REQUEST_BUDGET_SHARE_BP,
                            deadline: now.saturating_add(REQUEST_DEADLINE),
                            extra_sources: Vec::new(),
                        };
                        match self.federation.information_request(&a, &stake) {
                            Ok(Some(request)) => {
                                // The spread is a probability difference; one
                                // outside [0, 1] is a model bug to refuse, not
                                // a figure to round into range.
                                let spread = a.disagreement.spread;
                                if !(0.0..=1.0).contains(&spread) {
                                    refused.push((
                                        proposition.clone(),
                                        format!(
                                            "the disagreement spread {spread} is not in [0, 1]"
                                        ),
                                    ));
                                } else {
                                    information_requests.push(request);
                                    // Statistic to integer basis points.
                                    request_spreads_bp.push((spread * 10_000.0).round() as u32);
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                refused.push((proposition.clone(), error.message().to_string()));
                            }
                        }
                    }
                    if let Some(merged) = a.arbitrated_probability {
                        self.ensembles.insert(proposition.clone(), merged);
                    }
                    arbitrations.push(a);
                }
                Err(error) => refused.push((proposition.clone(), error.message().to_string())),
            }
        }

        // Decisions are acted on after every arbitration of the pass is
        // recorded, so a branch opened or a merge made for one proposition
        // cannot change which views another proposition's arbitration saw.
        self.act_on_decisions(&arbitrations, now, &mut fed, &mut refused);

        let live = self.federation.live_specs(now);
        let population: Vec<MemberStanding> = live
            .iter()
            .map(|spec| MemberStanding {
                id: spec.id().to_string(),
                scope: spec.scope().clone(),
                expires_at: spec.expires_at(),
                calibration: self
                    .federation
                    .calibration(spec.id())
                    .unwrap_or(qip_world_model::federation::Calibration::Unscored),
            })
            .collect();
        let uncovered: Vec<String> = REQUIRED_FAMILIES
            .iter()
            .filter(|family| {
                !live
                    .iter()
                    .any(|spec| covers(family, spec.scope(), &inputs.region))
            })
            .map(|family| (*family).to_string())
            .collect();
        let silent: Vec<String> = population
            .iter()
            .filter(|m| !fed.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        let journal_base = self.federation.journal_base();
        let events = self.federation.drain_journal();
        let contradictions = contradictions_in(&arbitrations);
        FederationReview {
            cycle: inputs.cycle,
            at: now,
            population,
            uncovered,
            silent,
            refused,
            unreadable,
            resolved,
            abandoned,
            arbitrations,
            system: inputs.system.clone(),
            journal_base,
            events,
            information_requests,
            request_spreads_bp,
            contradictions,
        }
    }

    /// Act on the `Branch` and `Merge` decisions the pass reached
    /// (WORLD-1181, WORLD-3041); until now they were only labelled in the
    /// review, so "preserve multiple live branches until an outcome resolves
    /// them" held for no branch the cycle ever opened.
    ///
    /// A `Branch` decision means the top two scored views are too close to
    /// pick between and too far apart to average: the runner-up's reading is
    /// opened as a branch of the leader, fed whatever the runner-up is fed,
    /// and scored when the proposition resolves, at which point the
    /// federation closes it if it forecast the wrong side. A `Merge` decision
    /// folds the two best branches into one. **Only branches are merged**: a
    /// merge marks both parents merged and the federation stops feeding them,
    /// so merging two roots would silence two members of the population the
    /// stage is required to keep covered.
    fn act_on_decisions(
        &mut self,
        arbitrations: &[Arbitration],
        now: Timestamp,
        fed: &mut BTreeSet<String>,
        refused: &mut Vec<(String, String)>,
    ) {
        use qip_world_model::federation::ScoredView;
        let by_standing = |views: &[ScoredView]| {
            let mut ranked: Vec<ScoredView> = views.to_vec();
            ranked.sort_by(|a, b| {
                b.composite
                    .unwrap_or(0.0)
                    .total_cmp(&a.composite.unwrap_or(0.0))
                    .then_with(|| a.model.cmp(&b.model))
            });
            ranked
        };
        for a in arbitrations {
            match a.decision {
                Decision::Branch => {
                    let live = self
                        .branches
                        .keys()
                        .filter(|id| {
                            matches!(
                                self.federation.status(id),
                                Ok(qip_world_model::federation::Status::Live)
                            )
                        })
                        .count();
                    let ranked = by_standing(&a.disagreement.views);
                    let (Some(leader), Some(rival)) = (ranked.first(), ranked.get(1)) else {
                        continue;
                    };
                    // One branch per followed reading: a second branch of
                    // the same member would be the same voice twice.
                    if live >= FEDERATION_MAX_LIVE_BRANCHES
                        || self.branches.values().any(|source| *source == rival.model)
                        || self.branches.contains_key(&rival.model)
                    {
                        continue;
                    }
                    self.serial += 1;
                    let root = leader.model.split('~').next().unwrap_or(&leader.model);
                    let id = format!("{root}~branch-{}", self.serial);
                    let hypothesis = format!(
                        "follows {} at {:.3} against {} at {:.3}",
                        rival.model, rival.probability, leader.model, leader.probability
                    );
                    let trigger = format!("{}: {}", a.proposition, a.reason);
                    if let Err(error) =
                        self.federation
                            .branch(&leader.model, &id, &hypothesis, &trigger, now)
                    {
                        refused.push((id, error.message().to_string()));
                        continue;
                    }
                    let seeded = self.federation.assess(
                        &id,
                        Assessment {
                            proposition: a.proposition.clone(),
                            probability: rival.probability,
                            evidence_fit: rival.evidence_fit,
                            causal_coherence: rival.causal_coherence,
                            uncertainty: rival.uncertainty,
                            at: now,
                        },
                    );
                    match seeded {
                        Ok(()) => {
                            fed.insert(id.clone());
                            self.views
                                .entry(a.proposition.clone())
                                .or_default()
                                .insert(id.clone(), rival.probability);
                            self.branches.insert(id, rival.model.clone());
                        }
                        Err(error) => refused.push((id, error.message().to_string())),
                    }
                }
                Decision::Merge => {
                    let ranked: Vec<ScoredView> = by_standing(&a.disagreement.views)
                        .into_iter()
                        .filter(|v| self.branches.contains_key(&v.model))
                        .collect();
                    let (Some(first), Some(second)) = (ranked.first(), ranked.get(1)) else {
                        continue;
                    };
                    self.serial += 1;
                    let id = format!("merge-{}", self.serial);
                    match self
                        .federation
                        .merge(&first.model, &second.model, &id, &a.reason, now)
                    {
                        Ok(()) => {
                            let followed = self.branches.remove(&first.model);
                            self.branches.remove(&second.model);
                            if let Some(source) = followed {
                                self.branches.insert(id, source);
                            }
                        }
                        Err(error) => refused.push((id, error.message().to_string())),
                    }
                }
                Decision::Abstain | Decision::RequestMoreInformation(_) => {}
            }
        }
    }

    /// Score every pending forecast reality has answered; abandon the ones it
    /// cannot or did not answer in time.
    fn resolve(
        &mut self,
        inputs: &FederationInputs,
        now: Timestamp,
    ) -> (Vec<Resolution>, Vec<String>) {
        let by_subject: BTreeMap<&str, &InstrumentView> = inputs
            .instruments
            .iter()
            .map(|v| (v.subject.as_str(), v))
            .collect();
        let mut resolved = Vec::new();
        let mut abandoned = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        for (proposition, waiting) in pending {
            let answer = match &waiting {
                Pending::Instrument {
                    subject,
                    close,
                    through,
                    ..
                } => match by_subject.get(subject.as_str()) {
                    Some(view) if view.observed_through > *through => {
                        Some(Some(view.last_close > *close))
                    }
                    Some(_) => None,
                    // The subject left the universe: nothing will answer.
                    None => Some(None),
                },
                Pending::Halted { cycle } if *cycle < inputs.cycle => {
                    Some(Some(inputs.system.halted))
                }
                Pending::Halted { .. } => None,
                Pending::Chain { trades, cycle } if *cycle < inputs.cycle => {
                    inputs.chain.map(|c| Some(c.confirmed_trades > *trades))
                }
                Pending::Chain { .. } => None,
                Pending::Funding { cash, cycle } if *cycle < inputs.cycle => {
                    inputs.capital.map(|c| Some(c.cash < *cash))
                }
                Pending::Funding { .. } => None,
                // Answered by the series' next release: did it surprise the
                // same way? A release of exactly consensus answers `false`.
                Pending::Macro {
                    series,
                    positive,
                    valid_at,
                    ..
                } => inputs
                    .macros
                    .iter()
                    .find(|v| &v.series == series && v.valid_at > *valid_at)
                    .map(|v| Some((v.surprise > 0.0) == *positive && v.surprise != 0.0)),
                // Answered by the series' next fixing: did it move the same way?
                Pending::Currency {
                    series,
                    level,
                    up,
                    valid_at,
                    ..
                } => inputs
                    .currencies
                    .iter()
                    .find(|v| &v.series == series && v.valid_at > *valid_at)
                    .map(|v| Some((v.level > *level) == *up && (v.level - *level).abs() > 0.0)),
            };
            match answer {
                Some(Some(outcome)) => {
                    let closed = self.federation.resolve(&proposition, outcome, now);
                    for id in &closed {
                        self.branches.remove(id);
                    }
                    let forecasts = self.views.remove(&proposition).unwrap_or_default();
                    let ensemble = self.ensembles.remove(&proposition);
                    resolved.push(Resolution {
                        proposition,
                        outcome,
                        closed,
                        forecasts,
                        ensemble,
                    });
                }
                Some(None) => {
                    self.federation.abandon(&proposition, now);
                    abandoned.push(proposition);
                }
                None if waiting.exhausted(inputs.cycle, now) => {
                    self.federation.abandon(&proposition, now);
                    abandoned.push(proposition);
                }
                None => {
                    self.pending.insert(proposition, waiting);
                }
            }
        }
        let pending = &self.pending;
        self.views
            .retain(|proposition, _| pending.contains_key(proposition));
        self.ensembles
            .retain(|proposition, _| pending.contains_key(proposition));
        (resolved, abandoned)
    }
}

/// Every review the log holds, oldest first, read back from the journal
/// alone — and each one checked: an arbitration whose recorded decision does
/// not follow from its own recorded views is refused, because a record that
/// cannot be re-derived from itself is not a replay of anything.
pub fn replay(platform: &crate::platform::Platform) -> Result<Vec<FederationReview>> {
    let envelopes = platform.replay_journal(
        &qip_events::EventFilter::new().topic(<FederationReview as EventBody>::TOPIC),
    )?;
    let mut reviews = Vec::new();
    for envelope in envelopes {
        // The envelope's key is `<topic name>:<body key>`; read the body's
        // key exactly rather than searching for the prefix anywhere in it.
        let key = envelope.idempotency_key();
        let ours = key
            .strip_prefix(<FederationReview as EventBody>::TOPIC.name())
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|k| k.starts_with(IDEMPOTENCY_PREFIX));
        if !ours {
            continue;
        }
        let review = envelope.decode::<FederationReview>()?.body;
        for a in &review.arbitrations {
            let (decision, reason, merged) =
                qip_world_model::federation::decide_on(&a.proposition, &a.disagreement);
            if decision != a.decision || reason != a.reason || merged != a.arbitrated_probability {
                return Err(qip_core::error::Error::invalid(format!(
                    "the journaled arbitration of `{}` in cycle {} does not follow from its own \
                     recorded views; the record is not trusted as a replay",
                    a.proposition, review.cycle
                )));
            }
        }
        if review.contradictions != contradictions_in(&review.arbitrations) {
            return Err(qip_core::error::Error::invalid(format!(
                "the journaled contradictions of cycle {} are not the ones its own recorded \
                 views contain; the record is not trusted as a replay",
                review.cycle
            )));
        }
        reviews.push(review);
    }
    Ok(reviews)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_world_model::federation::{MATERIAL_DISAGREEMENT_SPREAD, ScoredView};

    fn series(n: usize, drift: f64) -> Vec<f64> {
        (0..n)
            .map(|i| drift + if i % 2 == 0 { 0.01 } else { -0.008 })
            .collect()
    }

    fn inputs(cycle: u64, close: f64, through: Timestamp, chain: Option<u64>) -> FederationInputs {
        FederationInputs {
            cycle,
            region: "home".into(),
            instruments: vec![InstrumentView {
                subject: "AAA".into(),
                returns: series(40, 0.001),
                last_close: close,
                observed_through: through,
                spread_ratio: Some(1.2),
                causal: None,
                fundamental_surprise: None,
                order_flow: Vec::new(),
            }],
            system: SystemView {
                halted: false,
                autonomy: "paper_trading".into(),
                live_authority: false,
                expired_manifests: 0,
                thin_subjects: 0,
                slowest_stage: Some(Duration::from_secs(1)),
                degraded_capabilities: Some(0),
            },
            chain: chain.map(|confirmed_trades| ChainView { confirmed_trades }),
            capital: None,
            macros: Vec::new(),
            currencies: Vec::new(),
        }
    }

    /// A paper book holding `cash` of a 1,000 equity, with `waiting` theses
    /// waiting for capital.
    fn book(cash: f64, waiting: usize) -> CapitalView {
        CapitalView {
            cash,
            cash_share: cash / 1_000.0,
            leverage: 1.0 - cash / 1_000.0,
            net_share: 1.0 - cash / 1_000.0,
            pending_theses: waiting,
        }
    }

    fn day(n: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000 + n * 86_400)
    }

    #[test]
    fn a_population_differing_on_all_five_axes_is_built_and_every_fed_member_produces_state() {
        let mut stage = FederationStage::new();
        // The company member produces state only for an instrument whose
        // issuer has reported a surprise, so this one has (R12-WORLD-3044).
        let mut fed_inputs = inputs(1, 100.0, day(0), Some(3));
        fed_inputs.instruments[0].fundamental_surprise = Some(0.05);
        // The order-flow member produces state only where a print stated its
        // aggressor (ADR 0231), so this instrument has one cycle's flow.
        fed_inputs.instruments[0].order_flow = vec![qip_world_model::order_flow::FlowObservation {
            object_id: qip_core::ObjectId::from_string("AAA"),
            venue: qip_contracts::venue::VenueId::new("XNYS"),
            observed_at: day(0),
            buy_volume: Decimal::from_int(300),
            sell_volume: Decimal::from_int(100),
            buy_count: 3,
            sell_count: 1,
        }];
        // The capital member produces state only for a book with positive
        // equity, so this one has one (R12-WORLD-3048).
        fed_inputs.capital = Some(book(1_000.0, 0));
        // The macro member produces state only for a release that surprised
        // its consensus (ADR 0232), so this pass reads one.
        fed_inputs.currencies = Vec::new();
        fed_inputs.macros = vec![MacroView {
            series: "US.CPI.YOY".into(),
            surprise: 0.3,
            valid_at: day(0),
            simulated: false,
        }];
        let review = stage.run(&fed_inputs, day(0));
        // Premise: the population exists and nothing was refused.
        assert_eq!(review.population.len(), 14, "{review:?}");
        assert!(review.refused.is_empty(), "{:?}", review.refused);
        let spanned = stage.federation().spanned();
        assert!(spanned.domains.len() >= 3);
        assert!(spanned.regions.len() >= 2);
        assert!(spanned.horizons.len() >= 3);
        assert_eq!(spanned.abstractions.len(), 3);
        assert_eq!(spanned.kinds.len(), 3);
        assert!(review.uncovered.is_empty(), "{:?}", review.uncovered);
        assert!(review.silent.is_empty(), "{:?}", review.silent);
        // One instrument proposition with ten views (seven tape members, the
        // company member, the order-flow member and the causal model), plus
        // self, capital, macro and chain.
        assert_eq!(review.arbitrations.len(), 5);
        let instrument = &review.arbitrations[0];
        assert_eq!(instrument.disagreement.views.len(), 10);
        // WORLD-3051 and -3042: the rare-regime, alternate-future and causal
        // members are in the population and each produced a view on the instrument.
        for member in [RARE_REGIME, ALTERNATE_FUTURE, CAUSAL_GLOBAL] {
            assert!(
                review.population.iter().any(|m| m.id == member),
                "{member} not in the population"
            );
            assert!(
                instrument
                    .disagreement
                    .views
                    .iter()
                    .any(|v| v.model == member),
                "{member} produced no view: {:?}",
                instrument.disagreement.views
            );
        }
    }

    /// WORLD-3052: the self/system member's state carries latency and data
    /// health. A slow stage alone, and a stale capability alone, each raise
    /// its forecast that the platform is found halted. Mutation: ignore the
    /// two inputs — all three forecasts are equal.
    #[test]
    fn a_slow_stage_or_a_stale_capability_moves_the_self_system_members_state() {
        let forecast = |slowest: Duration, degraded: usize| {
            let mut stage = FederationStage::new();
            let mut inputs = inputs(1, 100.0, day(0), None);
            inputs.system.slowest_stage = Some(slowest);
            inputs.system.degraded_capabilities = Some(degraded);
            let review = stage.run(&inputs, day(0));
            review
                .arbitrations
                .iter()
                .find(|a| a.proposition == halted_proposition(1))
                .and_then(|a| a.disagreement.views.iter().find(|v| v.model == SELF_SYSTEM))
                .map(|v| v.probability)
                .expect("the self/system member forecast the halt")
        };
        let healthy = forecast(Duration::from_secs(1), 0);
        // Premise: the slow stage is past the budget and the healthy one is
        // inside it, so the comparison is about the budget being read.
        let past = Duration::from_secs(61);
        assert!(
            past > SELF_STAGE_LATENCY_BUDGET && Duration::from_secs(1) < SELF_STAGE_LATENCY_BUDGET
        );
        let slow = forecast(past, 0);
        let stale = forecast(Duration::from_secs(1), 2);
        assert!(slow > healthy, "slow {slow} vs healthy {healthy}");
        assert!(stale > healthy, "stale {stale} vs healthy {healthy}");
    }

    /// WORLD-3051: the rare-regime member reads the day after the tape's own
    /// tail moves, and the alternate-future member is the stress world's
    /// inverse. The tape's two largest moves are each followed by a rise,
    /// so the rare-regime view is a rise for certain while the tape as a
    /// whole is a coin flip. Mutation: feed the rare-regime member the
    /// whole tape's share of rises — its view falls to about one half.
    #[test]
    fn the_rare_regime_member_reads_the_day_after_tail_moves_and_the_alternate_future_inverts_the_stress_world()
     {
        let mut stage = FederationStage::new();
        let mut inputs = inputs(1, 100.0, day(0), None);
        let mut returns = series(40, 0.0);
        returns[10] = 0.08;
        returns[11] = 0.02;
        returns[25] = -0.09;
        returns[26] = 0.03;
        // A late run of rises, smaller than either tail move, so the stress
        // world is not the coin flip its inverse would equal.
        for r in &mut returns[36..] {
            *r = 0.03;
        }
        // Premise: the whole tape is not one-sided, so a rise for certain
        // can only come from reading the days after the tail moves.
        let whole = share_positive(&returns);
        assert!(whole > 0.3 && whole < 0.7, "{whole}");
        inputs.instruments[0].returns = returns;
        let review = stage.run(&inputs, day(0));
        let views = &review.arbitrations[0].disagreement.views;
        let view = |model: &str| {
            views
                .iter()
                .find(|v| v.model == model)
                .unwrap_or_else(|| panic!("{model} produced no view: {views:?}"))
        };
        assert!((view(RARE_REGIME).probability - 1.0).abs() < 1e-12);
        assert!((view(RARE_REGIME).uncertainty - 0.1).abs() < 1e-12);
        let stress = view(STRESS_SCENARIO).probability;
        assert!(
            (view(ALTERNATE_FUTURE).probability - (1.0 - stress)).abs() < 1e-12,
            "alternate {} vs stress {stress}",
            view(ALTERNATE_FUTURE).probability
        );
        // Premise for the inversion to mean anything: the stress view is
        // not one half, where a view and its inverse coincide.
        assert!((stress - 0.5).abs() > 1e-9, "{stress}");
    }

    /// MODEL-4009: a domain member per named asset class is declared and
    /// fed the class's breadth, so an instrument rising while its class
    /// falls is arbitrated between its momentum view and its class's view,
    /// and the disagreement records both. Mutation: feed the domain member a
    /// constant breadth of one half — the view's probability is wrong.
    #[test]
    fn a_domain_member_fed_its_asset_classs_breadth_disagrees_with_the_tape_members_and_is_recorded()
     {
        let mut stage = FederationStage::new();
        let mut inputs = inputs(1, 100.0, day(0), None);
        // AAA rises; BBB and CCC, the rest of the class, fall.
        for (subject, drift) in [("BBB", -0.01), ("CCC", -0.01)] {
            inputs.instruments.push(InstrumentView {
                subject: subject.into(),
                returns: series(40, drift),
                last_close: 50.0,
                observed_through: day(0),
                spread_ratio: None,
                causal: None,
                fundamental_surprise: None,
                order_flow: Vec::new(),
            });
        }
        inputs.instruments[0].returns = series(40, 0.01);
        stage.set_asset_classes(
            ["AAA", "BBB", "CCC"]
                .into_iter()
                .map(|s| (s.to_string(), "equity".to_string()))
                .collect(),
        );
        let review = stage.run(&inputs, day(0));
        assert!(review.refused.is_empty(), "{:?}", review.refused);
        let member = FederationStage::domain_member("equity");
        assert!(
            review.population.iter().any(|m| m.id == member),
            "premise: the domain member is in the population: {:?}",
            review.population.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        assert!(
            review
                .population
                .iter()
                .any(|m| covers("domain", &m.scope, "home")),
            "premise: the domain family is covered"
        );
        let aaa = review
            .arbitrations
            .iter()
            .find(|a| a.proposition == instrument_proposition("AAA", 1))
            .expect("AAA was arbitrated");
        let domain = aaa
            .disagreement
            .views
            .iter()
            .find(|v| v.model == member)
            .expect("the domain member holds a view on AAA");
        let momentum = aaa
            .disagreement
            .views
            .iter()
            .find(|v| v.model == MOMENTUM)
            .expect("the momentum member holds a view on AAA");
        assert!(
            (domain.probability - 1.0 / 3.0).abs() < 1e-9,
            "one of three in the class rose: {}",
            domain.probability
        );
        assert!(momentum.probability > 0.6, "{}", momentum.probability);
        assert!(
            aaa.disagreement.spread >= MATERIAL_DISAGREEMENT_SPREAD,
            "{}",
            aaa.disagreement.spread
        );
        assert!(
            review
                .events
                .iter()
                .any(|e| matches!(e, Event::ResearchRaised(t) if t.proposition == aaa.proposition)),
            "the disagreement raised research"
        );
    }

    /// AMBIENT-10093, CONTRACT-6111: with exposure at stake on a subject
    /// whose proposition the members disagree on materially, the pass
    /// issues the §24.9 contract request on the review; with nothing at
    /// stake it issues none. Mutation: skip the stake lookup in `run` —
    /// no request is issued.
    #[test]
    fn a_material_disagreement_on_a_subject_with_exposure_at_stake_issues_an_information_request() {
        let mut stage = FederationStage::new();
        let mut inputs = inputs(1, 100.0, day(0), None);
        for (subject, drift) in [("BBB", -0.01), ("CCC", -0.01)] {
            inputs.instruments.push(InstrumentView {
                subject: subject.into(),
                returns: series(40, drift),
                last_close: 50.0,
                observed_through: day(0),
                spread_ratio: None,
                causal: None,
                fundamental_surprise: None,
                order_flow: Vec::new(),
            });
        }
        inputs.instruments[0].returns = series(40, 0.01);
        stage.set_asset_classes(
            ["AAA", "BBB", "CCC"]
                .into_iter()
                .map(|s| (s.to_string(), "equity".to_string()))
                .collect(),
        );
        let unstaked = stage.run(&inputs, day(0));
        let aaa = unstaked
            .arbitrations
            .iter()
            .find(|a| a.proposition == instrument_proposition("AAA", 1))
            .expect("AAA was arbitrated");
        assert!(
            aaa.disagreement.spread >= MATERIAL_DISAGREEMENT_SPREAD,
            "premise: material"
        );
        assert!(unstaked.information_requests.is_empty(), "nothing at stake");

        let mut staked = FederationStage::new();
        staked.set_asset_classes(
            ["AAA", "BBB", "CCC"]
                .into_iter()
                .map(|s| (s.to_string(), "equity".to_string()))
                .collect(),
        );
        staked.set_stakes(BTreeMap::from([(
            "AAA".to_string(),
            Decimal::from_int(1_000_000),
        )]));
        let review = staked.run(&inputs, day(0));
        assert_eq!(review.information_requests.len(), 1, "{:?}", review.refused);
        let request = &review.information_requests[0];
        assert!(
            request
                .request_id
                .starts_with(&format!("ir:{}@", instrument_proposition("AAA", 1)))
        );
        assert!(request.expected_information_value.is_positive());
        assert_eq!(request.deadline, day(0).saturating_add(REQUEST_DEADLINE));
        assert!(
            request
                .allowed_sources
                .contains(&"sense:bar-closes".to_string())
        );
        let json = serde_json::to_value(&review).unwrap();
        let back: FederationReview = serde_json::from_value(json).unwrap();
        assert_eq!(back.information_requests, review.information_requests);
    }

    /// A tape every drifting member reads as rising, so the panel splits and
    /// the rival to the leader holds a one-sided view a falling close refutes.
    fn rising(cycle: u64, close: f64, through: Timestamp) -> FederationInputs {
        let mut i = inputs(cycle, close, through, None);
        i.instruments[0].returns = series(40, 0.01);
        i
    }

    fn branch_ids(stage: &FederationStage, now: Timestamp) -> Vec<String> {
        stage
            .federation()
            .live_specs(now)
            .iter()
            .map(|s| s.id().to_string())
            .filter(|id| id.contains("~branch-"))
            .collect()
    }

    fn view_of<'a>(review: &'a FederationReview, member: &str) -> &'a ScoredView {
        review.arbitrations[0]
            .disagreement
            .views
            .iter()
            .find(|v| v.model == member)
            .unwrap_or_else(|| panic!("{member} produced no view"))
    }

    fn causal_input(lean: f64, claims: usize) -> FederationInputs {
        let mut inputs = rising(1, 100.0, day(0));
        inputs.instruments[0].causal = Some(CausalView {
            lean,
            claims,
            confidence: 0.8,
        });
        inputs
    }

    /// R12-WORLD-3042: the global causal model is a registered member whose
    /// view is the graph's own reading of the instrument, scored like the
    /// others. Mutation: drop its declaration — the population premise fails;
    /// feed it a constant — the two views below are equal.
    #[test]
    fn the_global_causal_model_is_a_member_that_reads_the_graphs_claims_into_the_instrument() {
        let mut stage = FederationStage::new();
        let up = stage.run(&causal_input(0.02, 3), day(0));
        // Premise: it is in the population, fed, with a scope of its own.
        let standing = up
            .population
            .iter()
            .find(|m| m.id == CAUSAL_GLOBAL)
            .expect("the causal model is a member");
        assert_eq!(standing.scope.hypothesis, "causal-graph");
        assert!(
            !up.silent.contains(&CAUSAL_GLOBAL.to_string()),
            "{:?}",
            up.silent
        );
        let rising_view = view_of(&up, CAUSAL_GLOBAL).clone();

        let mut stage = FederationStage::new();
        let down = stage.run(&causal_input(-0.02, 3), day(0));
        let falling_view = view_of(&down, CAUSAL_GLOBAL);
        assert!(rising_view.probability > 0.6, "{rising_view:?}");
        assert!(falling_view.probability < 0.4, "{falling_view:?}");

        // With no live claim into the instrument it says it knows almost
        // nothing, rather than reading "no claim" as "no move".
        let mut stage = FederationStage::new();
        let bare = stage.run(&rising(1, 100.0, day(0)), day(0));
        let silent_view = view_of(&bare, CAUSAL_GLOBAL);
        assert!((silent_view.probability - 0.5).abs() < 1e-9);
        assert!(silent_view.uncertainty > rising_view.uncertainty);
    }

    /// R12-WORLD-3040: causal coherence is the member's agreement with the
    /// graph's live claims into the instrument, and evidence fit is the
    /// member's own residuals — neither a constant per member. Mutation:
    /// restore the `0.5` constants for momentum and reversion — the ordering
    /// below does not flip with the sign of the claim.
    #[test]
    fn two_members_on_one_tape_differ_in_coherence_when_one_contradicts_a_live_claim_and_in_fit() {
        let run = |lean: f64| {
            let mut stage = FederationStage::new();
            stage.run(&causal_input(lean, 3), day(0))
        };
        let agrees_with_rise = run(0.05);
        let agrees_with_fall = run(-0.05);
        let (m_up, r_up) = (
            view_of(&agrees_with_rise, MOMENTUM).causal_coherence,
            view_of(&agrees_with_rise, REVERSION).causal_coherence,
        );
        let (m_down, r_down) = (
            view_of(&agrees_with_fall, MOMENTUM).causal_coherence,
            view_of(&agrees_with_fall, REVERSION).causal_coherence,
        );
        // On the rising tape momentum reads up and reversion down, so a claim
        // of a rise agrees with momentum and contradicts reversion, and a
        // claim of a fall does the opposite.
        assert!(m_up > r_up, "{m_up} {r_up}");
        assert!(r_down > m_down, "{r_down} {m_down}");
        assert!((m_up - m_down).abs() > 1e-6);

        // Fit is per member: they have not all been equally right about this
        // tape's own past.
        let fits: std::collections::BTreeSet<u64> = agrees_with_rise.arbitrations[0]
            .disagreement
            .views
            .iter()
            .filter(|v| v.model != CAUSAL_GLOBAL)
            .map(|v| v.evidence_fit.to_bits())
            .collect();
        assert!(fits.len() > 1, "every member carries one fit: {fits:?}");
    }

    /// WORLD-1181 / WORLD-3041: a `Branch` decision was only labelled, so the
    /// cycle never held a competing hypothesis. Mutation: skip the
    /// `Federation::branch` call in `act_on_decisions` — the population stays
    /// nine and no `Branched` event is journaled.
    #[test]
    fn a_branch_decision_opens_one_live_branch_of_the_leader_and_journals_its_lineage() {
        let mut stage = FederationStage::new();
        let first = stage.run(&rising(1, 100.0, day(0)), day(0));
        // Premise: fourteen members (the order-flow member, ADR 0231, and the
        // macro member, ADR 0232, among them), no branch, and nothing scored
        // yet to decide on.
        assert_eq!(first.population.len(), 14);
        assert!(branch_ids(&stage, day(0)).is_empty());
        let second = stage.run(&rising(2, 101.0, day(1)), day(1));
        let instrument = &second.arbitrations[0];
        // Premise: the pass reached the decision under test.
        assert_eq!(instrument.decision, Decision::Branch, "{instrument:?}");
        let branches = branch_ids(&stage, day(1));
        assert_eq!(branches.len(), 1, "{branches:?}");
        assert_eq!(second.population.len(), 15);
        let lineage = stage.federation().lineage(&branches[0]).expect("lineage");
        assert_eq!(lineage.parents.len(), 1);
        assert!(branches[0].starts_with(&lineage.parents[0]));
        assert!(
            second.events.iter().any(|e| matches!(
                e,
                qip_world_model::federation::Event::Branched { id, .. } if *id == branches[0]
            )),
            "{:?}",
            second.events
        );
        // The same split persisting does not stack a second branch on the
        // member that is already followed.
        let third = stage.run(&rising(3, 101.5, day(2)), day(2));
        assert_eq!(branch_ids(&stage, day(2)).len(), 1);
        assert!(!third.silent.contains(&branches[0]), "{:?}", third.silent);
    }

    /// WORLD-3041: the branch holds a forecast until reality answers, and the
    /// federation closes it with its Brier score when it forecast the wrong
    /// side. Mutation: never branch — there is nothing to close and the
    /// `expect` below fails.
    #[test]
    fn a_branch_that_forecast_the_wrong_side_is_closed_with_its_score_when_the_close_answers() {
        let mut stage = FederationStage::new();
        stage.run(&rising(1, 100.0, day(0)), day(0));
        stage.run(&rising(2, 101.0, day(1)), day(1));
        let branch = branch_ids(&stage, day(1))
            .pop()
            .expect("a branch was opened");
        // The rival it follows read the tape as certain to rise; the close
        // falls.
        let third = stage.run(&rising(3, 100.0, day(2)), day(2));
        let answered = third
            .resolved
            .iter()
            .find(|r| r.proposition == instrument_proposition("AAA", 2))
            .expect("the cycle-2 proposition resolves");
        assert!(!answered.outcome);
        assert!(answered.closed.contains(&branch), "{answered:?}");
        assert!(matches!(
            stage.federation().status(&branch),
            Ok(qip_world_model::federation::Status::Closed { score, .. }) if (*score - 1.0).abs() < 1e-9
        ));
        assert!(!branch_ids(&stage, day(2)).contains(&branch));
        // A closed branch is no longer fed or reported silent.
        assert!(third.refused.is_empty(), "{:?}", third.refused);
    }

    /// WORLD-1181: a `Merge` folds two branches into one and never two roots.
    /// Mutation: drop the `branches.contains_key` filter — the roots are
    /// merged and the roots-kept assertion fails.
    #[test]
    fn a_merge_decision_folds_two_branches_into_one_and_leaves_the_roots_alone() {
        use qip_world_model::federation::{Disagreement, ScoredView, ViewLineage};
        let mut stage = FederationStage::new();
        stage.run(&rising(1, 100.0, day(0)), day(0));
        let roots: Vec<String> = stage
            .federation()
            .live_specs(day(0))
            .iter()
            .map(|s| s.id().to_string())
            .collect();
        for (n, source) in [(1, MOMENTUM), (2, REVERSION)] {
            let id = format!("{STANDING_ASIDE}~branch-{n}");
            stage
                .federation
                .branch(STANDING_ASIDE, &id, "test", "test", day(0))
                .expect("branch");
            stage.branches.insert(id, source.to_string());
        }
        let view = |model: &str, parents: Vec<String>| ScoredView {
            model: model.to_string(),
            probability: 0.6,
            evidence_fit: 0.5,
            predictive_performance: Some(0.5),
            causal_coherence: 0.5,
            uncertainty: 0.3,
            composite: Some(0.5),
            lineage: ViewLineage {
                depth: parents.len(),
                parents,
                trigger: None,
            },
        };
        let mut views: Vec<ScoredView> = roots.iter().map(|r| view(r, Vec::new())).collect();
        views.extend(
            stage
                .branches
                .keys()
                .map(|b| view(b, vec![STANDING_ASIDE.to_string()])),
        );
        let arbitration = Arbitration {
            proposition: "AAA:next-close-up@1".into(),
            at: day(0),
            decision: Decision::Merge,
            reason: "views agree".into(),
            disagreement: Disagreement { views, spread: 0.0 },
            arbitrated_probability: Some(0.6),
            expired: Vec::new(),
            research: None,
        };
        assert_eq!(stage.federation().live_specs(day(0)).len(), 16);
        let (mut fed, mut refused) = (BTreeSet::new(), Vec::new());
        stage.act_on_decisions(
            std::slice::from_ref(&arbitration),
            day(0),
            &mut fed,
            &mut refused,
        );
        assert!(refused.is_empty(), "{refused:?}");
        // 16 live, two branches merged away, one result: 15, every root kept.
        let live: Vec<String> = stage
            .federation()
            .live_specs(day(0))
            .iter()
            .map(|s| s.id().to_string())
            .collect();
        assert_eq!(live.len(), 15, "{live:?}");
        assert!(roots.iter().all(|r| live.contains(r)));
        assert_eq!(stage.branches.len(), 1);
        // Replaying the same decision has no live pair left to fold.
        stage.act_on_decisions(&[arbitration], day(0), &mut fed, &mut refused);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(stage.federation().live_specs(day(0)).len(), 15);
    }

    /// ADR 0232: a macro forecast waits for its series' next release in
    /// time, not in passes. The skeptic's finding: patience was 90 passes on
    /// the premise of daily cycles, and a deployed cycle runs every 300 s on
    /// the deep brain and every 100 ms on the fast brain, so every monthly
    /// forecast was abandoned within hours and nothing was ever scored.
    /// Mutation: count a macro forecast's patience in passes
    /// (`FEDERATION_PENDING_CYCLES`) — it is abandoned before the release.
    #[test]
    fn a_macro_forecast_waits_for_its_next_release_in_time_not_in_passes() {
        let mut stage = FederationStage::new();
        let release = |surprise: f64, period: i64| MacroView {
            series: "US.CPI.YOY".into(),
            surprise,
            valid_at: day(period),
            simulated: false,
        };
        let mut first = inputs(1, 100.0, day(0), None);
        first.macros = vec![release(0.3, -20)];
        stage.run(&first, day(0));
        let proposition = macro_proposition("US.CPI.YOY", 1, false);
        // Two hundred passes inside one day, no new release.
        for cycle in 2..202 {
            let mut quiet = inputs(cycle, 100.0, day(0), None);
            quiet.macros = vec![release(0.3, -20)];
            let review = stage.run(
                &quiet,
                day(0).saturating_add(Duration::from_mins(cycle as i64)),
            );
            assert!(
                !review.abandoned.contains(&proposition),
                "abandoned after {cycle} passes inside one day"
            );
        }
        // The next release, thirty days on, answers it.
        let mut next = inputs(202, 100.0, day(0), None);
        next.macros = vec![release(0.4, 10)];
        let review = stage.run(&next, day(30));
        let resolution = review
            .resolved
            .iter()
            .find(|r| r.proposition == proposition)
            .expect("the next release answered the forecast");
        assert!(resolution.outcome);
    }

    /// ADR 0232: a currency fixing states no consensus, so the macro member
    /// reads its move. The first fixing seeds the level, the second's move is
    /// forecast to persist, and the third answers it. Mutation: record the
    /// move inverted (`view.level < last`) — the forecast resolves false.
    #[test]
    fn a_currency_fixing_that_moves_is_forecast_to_keep_moving_and_the_next_fixing_answers_it() {
        let mut stage = FederationStage::new();
        let fixing = |level: f64, period: i64| CurrencyView {
            series: "FX.EUR.USD".into(),
            level,
            valid_at: day(period),
            simulated: false,
        };
        let pass = |stage: &mut FederationStage, cycle: u64, level: f64, period: i64| {
            let mut i = inputs(cycle, 100.0, day(0), None);
            i.currencies = vec![fixing(level, period)];
            stage.run(&i, day(period))
        };
        let seeded = pass(&mut stage, 1, 1.10, 0);
        assert!(
            !seeded
                .arbitrations
                .iter()
                .any(|a| a.proposition.starts_with("macro:FX.EUR.USD")),
            "a first fixing has no move to forecast"
        );
        let moved = pass(&mut stage, 2, 1.12, 1);
        let proposition = currency_proposition("FX.EUR.USD", 2, false);
        assert!(
            moved
                .arbitrations
                .iter()
                .any(|a| a.proposition == proposition),
            "a fixing that moved was not forecast: {:?}",
            moved
                .arbitrations
                .iter()
                .map(|a| a.proposition.clone())
                .collect::<Vec<_>>()
        );
        let answered = pass(&mut stage, 3, 1.13, 2);
        let resolution = answered
            .resolved
            .iter()
            .find(|r| r.proposition == proposition)
            .expect("the next fixing answered the move forecast");
        assert!(resolution.outcome, "a move that persisted resolved false");
    }

    #[test]
    fn a_forecast_is_scored_against_the_next_observed_close_and_not_before_it() {
        let mut stage = FederationStage::new();
        stage.run(&inputs(1, 100.0, day(0), None), day(0));
        assert!(stage.pending() >= 1);
        // No new bar: nothing about the instrument is resolved.
        let same = stage.run(&inputs(2, 100.0, day(0), None), day(0));
        assert!(
            !same
                .resolved
                .iter()
                .any(|r| r.proposition == instrument_proposition("AAA", 1)),
            "{:?}",
            same.resolved
        );
        // A new, higher close answers it `true`.
        let next = stage.run(&inputs(3, 101.0, day(1), None), day(1));
        let resolution = next
            .resolved
            .iter()
            .find(|r| r.proposition == instrument_proposition("AAA", 1))
            .expect("resolved on the new bar");
        assert!(resolution.outcome);
        assert!(matches!(
            stage.federation().calibration(MOMENTUM),
            Ok(qip_world_model::federation::Calibration::Brier { .. })
        ));
    }

    #[test]
    fn a_member_fed_nothing_lapses_at_its_lease_and_the_population_check_names_the_scope() {
        let mut stage = FederationStage::new();
        let first = stage.run(&inputs(1, 100.0, day(0), Some(1)), day(0));
        assert!(first.uncovered.is_empty(), "{:?}", first.uncovered);
        // No chain for longer than the lease, and no issuer surprise either,
        // so the company member lapses too (R12-WORLD-3044); and no book
        // view, so the capital member does (R12-WORLD-3048).
        let later = stage.run(&inputs(2, 101.0, day(8), None), day(8));
        assert_eq!(
            later.uncovered,
            vec![
                "crypto/on-chain".to_string(),
                "company/fundamental".to_string(),
                "portfolio/capital".to_string()
            ]
        );
        assert!(!later.population.iter().any(|m| m.id == COMPANY));
        assert!(!later.population.iter().any(|m| m.id == ON_CHAIN));
    }

    #[test]
    fn a_forecast_reality_never_answers_is_abandoned_rather_than_scored() {
        let mut stage = FederationStage::new();
        stage.run(&inputs(1, 100.0, day(0), None), day(0));
        let mut gone = inputs(2, 100.0, day(1), None);
        gone.instruments.clear();
        let review = stage.run(&gone, day(1));
        assert!(
            review.abandoned.contains(&instrument_proposition("AAA", 1)),
            "{review:?}"
        );
        assert!(matches!(
            stage.federation().calibration(MOMENTUM),
            Ok(qip_world_model::federation::Calibration::Unscored)
        ));
    }

    #[test]
    fn the_federation_journal_is_drained_into_each_review_so_it_stays_bounded() {
        let mut stage = FederationStage::new();
        let first = stage.run(&inputs(1, 100.0, day(0), None), day(0));
        assert!(!first.events.is_empty());
        assert!(stage.federation().journal().is_empty());
        let second = stage.run(&inputs(2, 101.0, day(1), None), day(1));
        assert_eq!(second.journal_base, first.events.len());
    }

    /// R12-WORLD-1181: a contradiction is two views on opposite sides of even
    /// odds, each past the margin; a wide spread on one side, or two views
    /// near indifference, is not one. Mutation: drop the `most_against`
    /// condition in `contradictions_in` -- the one-sided spread is then
    /// named a contradiction.
    #[test]
    fn only_views_on_opposite_sides_past_the_margin_are_a_contradiction() {
        use qip_world_model::federation::{Arbitration, Disagreement, ViewLineage};
        let view = |model: &str, probability: f64| ScoredView {
            model: model.to_string(),
            probability,
            evidence_fit: 0.5,
            predictive_performance: Some(0.5),
            causal_coherence: 0.5,
            uncertainty: 0.3,
            composite: Some(0.5),
            lineage: ViewLineage {
                depth: 0,
                parents: Vec::new(),
                trigger: None,
            },
        };
        let arbitration = |proposition: &str, views: Vec<ScoredView>| Arbitration {
            proposition: proposition.into(),
            at: day(0),
            decision: Decision::Branch,
            reason: "test".into(),
            disagreement: Disagreement { views, spread: 0.0 },
            arbitrated_probability: None,
            expired: Vec::new(),
            research: None,
        };
        let found = contradictions_in(&[
            arbitration("one-sided", vec![view("a", 0.65), view("b", 0.95)]),
            arbitration("indifferent", vec![view("a", 0.55), view("b", 0.45)]),
            arbitration(
                "opposed",
                vec![view("c", 0.15), view("a", 0.85), view("b", 0.7)],
            ),
        ]);
        assert_eq!(
            found,
            vec![ViewContradiction {
                proposition: "opposed".into(),
                for_model: "a".into(),
                for_probability: 0.85,
                against_model: "c".into(),
                against_probability: 0.15,
            }]
        );
    }

    /// R12-WORLD-3044: the federation's company/fundamental member exists and
    /// produces state. An instrument whose issuer reported a fundamental
    /// surprise gives it a view, leaning the way the surprise did, and the
    /// population check covers its family. With no surprise it gives no view.
    ///
    /// Mutation: drop the company view from the instrument's views -- the
    /// member then never produces state and the first assertion fails.
    #[test]
    fn a_fundamental_surprise_on_an_issuer_gives_the_company_member_a_view_and_its_absence_names_the_family_uncovered()
     {
        assert!(REQUIRED_FAMILIES.contains(&"company/fundamental"));
        let mut stage = FederationStage::new();
        let mut beat = inputs(1, 100.0, day(0), Some(1));
        beat.instruments[0].fundamental_surprise = Some(0.05);
        let review = stage.run(&beat, day(0));
        let company = review.arbitrations[0]
            .disagreement
            .views
            .iter()
            .find(|v| v.model == COMPANY)
            .unwrap_or_else(|| panic!("the company member produced no view: {review:?}"));
        assert!(
            company.probability > 0.5,
            "a beat did not lean up: {company:?}"
        );
        assert!(!review.silent.contains(&COMPANY.to_string()));
        assert!(review.uncovered.is_empty(), "{:?}", review.uncovered);

        let mut miss = inputs(2, 101.0, day(1), Some(1));
        miss.instruments[0].fundamental_surprise = Some(-0.05);
        let review = stage.run(&miss, day(1));
        let company = review.arbitrations[0]
            .disagreement
            .views
            .iter()
            .find(|v| v.model == COMPANY)
            .expect("a miss gives a view too");
        assert!(
            company.probability < 0.5,
            "a miss did not lean down: {company:?}"
        );

        let quiet = stage.run(&inputs(3, 102.0, day(2), Some(1)), day(2));
        assert!(
            quiet
                .arbitrations
                .iter()
                .flat_map(|a| a.disagreement.views.iter())
                .all(|v| v.model != COMPANY),
            "the company member gave a view with no surprise to read"
        );
        // That an unfed company member lapses and leaves its family uncovered
        // is pinned by
        // `a_member_fed_nothing_lapses_at_its_lease_and_the_population_check_names_the_scope`.
    }

    /// R12-WORLD-3048 (ADR 0194): the capital member forecasts a funding draw
    /// from the paper book, a waiting thesis raises that forecast, the next
    /// pass's cash answers it either way, and a book with no view leaves the
    /// member silent.
    ///
    /// Mutations: resolve on cash rising rather than falling; drop the
    /// waiting-thesis term; feed the member nothing.
    #[test]
    fn a_book_with_a_thesis_waiting_gives_the_capital_member_a_funding_view_the_next_pass_resolves()
    {
        let capital_view = |review: &FederationReview, cycle: u64| {
            review
                .arbitrations
                .iter()
                .find(|a| a.proposition == funding_proposition(cycle))
                .and_then(|a| a.disagreement.views.iter().find(|v| v.model == CAPITAL))
                .map(|v| v.probability)
        };
        let with_book = |cycle: u64, close: f64, at: Timestamp, view: Option<CapitalView>| {
            let mut fed = inputs(cycle, close, at, Some(1));
            fed.capital = view;
            fed
        };
        let mut idle = FederationStage::new();
        let idle_review = idle.run(&with_book(1, 100.0, day(0), Some(book(800.0, 0))), day(0));
        let idle_p = capital_view(&idle_review, 1).expect("the capital member forecast");
        let mut stage = FederationStage::new();
        let first = stage.run(&with_book(1, 100.0, day(0), Some(book(800.0, 1))), day(0));
        let waiting_p = capital_view(&first, 1).expect("the capital member forecast");
        assert!(
            waiting_p > idle_p + 0.4,
            "a thesis waiting for capital did not raise the draw ({waiting_p} vs {idle_p})"
        );
        assert!(!first.silent.contains(&CAPITAL.to_string()));

        // The next pass holds less cash: a draw.
        let second = stage.run(&with_book(2, 101.0, day(1), Some(book(600.0, 0))), day(1));
        let drew = second
            .resolved
            .iter()
            .find(|r| r.proposition == funding_proposition(1))
            .expect("the next pass's cash resolved the draw");
        assert!(drew.outcome, "cash fell and the draw resolved false");
        assert!(drew.forecasts.contains_key(CAPITAL));
        // The pass after holds more: no draw.
        let third = stage.run(&with_book(3, 102.0, day(2), Some(book(700.0, 0))), day(2));
        let held = third
            .resolved
            .iter()
            .find(|r| r.proposition == funding_proposition(2))
            .expect("the next pass's cash resolved the draw");
        assert!(!held.outcome, "cash rose and the draw resolved true");

        // A book with no positive equity gives no view: the member is silent.
        let fourth = stage.run(&with_book(4, 103.0, day(3), None), day(3));
        assert!(capital_view(&fourth, 4).is_none());
        assert!(
            fourth.silent.contains(&CAPITAL.to_string()),
            "{:?}",
            fourth.silent
        );
    }
}
