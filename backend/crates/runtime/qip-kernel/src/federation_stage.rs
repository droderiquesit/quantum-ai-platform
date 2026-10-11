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
//! Nothing here reaches the order path. [`FederationStage::run`] takes a
//! [`FederationInputs`] by reference and returns a record; it holds no handle
//! to a thesis, a sizing, a reservation or a broker, and the kernel calls it
//! from UNDERSTAND, before any hypothesis exists. An arbitrated probability
//! that moved a size would be a control nobody chose and no ADR admits.
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

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::federation::{
    Abstraction, Arbitration, Assessment, CalibrationClaim, Decision, Declaration, Event,
    Federation, ModelKind, Scope, WorldModelSpec,
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
pub const REVERSION: &str = "markets.reversion.global.meso";
pub const STRESS_SCENARIO: &str = "markets.stress-world.global.macro";
pub const STANDING_ASIDE: &str = "markets.standing-aside.global.meso";
pub const SELF_SYSTEM: &str = "self.operational-health.global";
pub const ON_CHAIN: &str = "crypto-onchain.flows.global";
/// The regional member's id is `markets.microstructure.<region>.micro`.
pub const REGIONAL_PREFIX: &str = "markets.microstructure.";

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
    /// Latest quoted spread over its own median, where one was measured.
    pub spread_ratio: Option<f64>,
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
}

/// What the on-chain member reads (ASSET-3017's federation half).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainView {
    /// Trades derived at the configured confirmation depth.
    pub confirmed_trades: u64,
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
        format!(
            "; federation of {} model(s) arbitrated {} proposition(s) ({}), resolved {}, \
             abandoned {}, {} silent{uncovered}",
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
}

impl Pending {
    fn cycle(&self) -> u64 {
        match self {
            Self::Instrument { cycle, .. } | Self::Halted { cycle } | Self::Chain { cycle, .. } => {
                *cycle
            }
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
}

/// Hand one view to `model`; true when the federation took it. A refusal is
/// recorded against the model rather than dropped.
fn feed(
    federation: &mut Federation,
    model: &str,
    assessment: Assessment,
    fed: &mut BTreeSet<String>,
    refused: &mut Vec<(String, String)>,
) -> bool {
    match federation.assess(model, assessment) {
        Ok(()) => {
            fed.insert(model.to_string());
            true
        }
        Err(error) => {
            refused.push((model.to_string(), error.message().to_string()));
            false
        }
    }
}

/// The scope families the population must cover (WORLD-3037, -3050, -3051,
/// -3052 and ASSET-3017's federation half). A family with no live,
/// unexpired member is named in [`FederationReview::uncovered`].
pub const REQUIRED_FAMILIES: [&str; 5] = [
    "regional",
    "scenario",
    "counterfactual",
    "self/system",
    "crypto/on-chain",
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

fn tail(xs: &[f64]) -> &[f64] {
    &xs[xs.len().saturating_sub(20)..]
}

/// The proposition one instrument's next close answers.
pub fn instrument_proposition(subject: &str, cycle: u64) -> String {
    format!("{subject}:next-close-up@{cycle}")
}

/// The proposition the next pass's kill-switch state answers.
pub fn halted_proposition(cycle: u64) -> String {
    format!("self:halted-next-pass@{cycle}")
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
            if !(sd.is_finite() && sd > 0.0 && view.last_close.is_finite()) {
                // A flat or broken series is not a distribution any member
                // can reason about. Skipped, and named in the review: an
                // instrument silently missing from the federation reads
                // exactly like one every member was content with.
                unreadable.push(view.subject.clone());
                continue;
            }
            let fit = (r.len() as f64 / 100.0).min(1.0);
            let short = &r[r.len().saturating_sub(5)..];
            let z_momentum = mean(short) / (sd / (short.len() as f64).sqrt());
            let window = tail(r);
            let z_reversion = window.iter().sum::<f64>() / (sd * (window.len() as f64).sqrt());
            let mut sorted = r.clone();
            sorted.sort_by(f64::total_cmp);
            let shock = sorted[(sorted.len() / 20).min(sorted.len() - 1)];
            let stressed: Vec<f64> = window.iter().map(|x| x + shock).collect();
            let base = share_positive(r);
            let microstructure_uncertainty = match view.spread_ratio {
                Some(x) if x.is_finite() => (0.3 + 0.2 * (x - 1.0).max(0.0)).min(1.0),
                // Microstructure unmeasured: the member says it barely knows.
                _ => 0.7,
            };
            let at = now;
            let views: [(&str, f64, f64, f64, f64); 5] = [
                (
                    MOMENTUM,
                    logistic(0.5 * z_momentum),
                    fit,
                    0.5,
                    1.0 / (1.0 + z_momentum.abs()),
                ),
                (
                    REVERSION,
                    logistic(-0.5 * z_reversion),
                    fit,
                    0.5,
                    1.0 / (1.0 + z_reversion.abs()),
                ),
                (
                    regional.as_str(),
                    share_positive(window),
                    fit,
                    0.4,
                    microstructure_uncertainty,
                ),
                (STRESS_SCENARIO, share_positive(&stressed), fit, 0.4, 0.6),
                (
                    STANDING_ASIDE,
                    base,
                    fit,
                    0.3,
                    (2.0 * (base * (1.0 - base) / r.len() as f64).sqrt()).min(1.0),
                ),
            ];
            let mut any = false;
            for (model, probability, evidence_fit, causal_coherence, uncertainty) in views {
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
        let system = &inputs.system;
        let halted_p = if system.halted {
            0.9
        } else {
            0.05 + 0.1 * (system.expired_manifests.min(5) as f64 / 5.0)
        };
        let proposition = halted_proposition(inputs.cycle);
        if feed(
            &mut self.federation,
            SELF_SYSTEM,
            Assessment {
                proposition: proposition.clone(),
                probability: halted_p,
                evidence_fit: 1.0,
                causal_coherence: 0.6,
                uncertainty: if system.halted { 0.1 } else { 0.3 },
                at: now,
            },
            &mut fed,
            &mut refused,
        ) {
            self.pending.insert(
                proposition.clone(),
                Pending::Halted {
                    cycle: inputs.cycle,
                },
            );
            propositions.push(proposition);
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
        for proposition in &propositions {
            match self.federation.arbitrate(proposition, now) {
                Ok(a) => arbitrations.push(a),
                Err(error) => refused.push((proposition.clone(), error.message().to_string())),
            }
        }

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
            };
            match answer {
                Some(Some(outcome)) => {
                    let closed = self.federation.resolve(&proposition, outcome, now);
                    resolved.push(Resolution {
                        proposition,
                        outcome,
                        closed,
                    });
                }
                Some(None) => {
                    self.federation.abandon(&proposition, now);
                    abandoned.push(proposition);
                }
                None if inputs.cycle.saturating_sub(waiting.cycle())
                    >= FEDERATION_PENDING_CYCLES =>
                {
                    self.federation.abandon(&proposition, now);
                    abandoned.push(proposition);
                }
                None => {
                    self.pending.insert(proposition, waiting);
                }
            }
        }
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
        reviews.push(review);
    }
    Ok(reviews)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            }],
            system: SystemView {
                halted: false,
                autonomy: "paper_trading".into(),
                live_authority: false,
                expired_manifests: 0,
                thin_subjects: 0,
            },
            chain: chain.map(|confirmed_trades| ChainView { confirmed_trades }),
        }
    }

    fn day(n: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000 + n * 86_400)
    }

    #[test]
    fn a_population_differing_on_all_five_axes_is_built_and_every_fed_member_produces_state() {
        let mut stage = FederationStage::new();
        let review = stage.run(&inputs(1, 100.0, day(0), Some(3)), day(0));
        // Premise: the population exists and nothing was refused.
        assert_eq!(review.population.len(), 7, "{review:?}");
        assert!(review.refused.is_empty(), "{:?}", review.refused);
        let spanned = stage.federation().spanned();
        assert!(spanned.domains.len() >= 3);
        assert!(spanned.regions.len() >= 2);
        assert!(spanned.horizons.len() >= 3);
        assert_eq!(spanned.abstractions.len(), 3);
        assert_eq!(spanned.kinds.len(), 3);
        assert!(review.uncovered.is_empty(), "{:?}", review.uncovered);
        assert!(review.silent.is_empty(), "{:?}", review.silent);
        // One instrument proposition with five views, plus self and chain.
        assert_eq!(review.arbitrations.len(), 3);
        let instrument = &review.arbitrations[0];
        assert_eq!(instrument.disagreement.views.len(), 5);
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
        // No chain for longer than the lease.
        let later = stage.run(&inputs(2, 101.0, day(8), None), day(8));
        assert_eq!(later.uncovered, vec!["crypto/on-chain".to_string()]);
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
}
