//! Deploying the strategies a payload names into a running cell.
//!
//! The pass loop has run `Cell::work` since `6340610`, over whatever the
//! tests deployed — and no composition root deployed anything, so a
//! deployed node fired no strategy, ever. The reason was the same as the
//! desk's before `584c96b`: the payload's compiled-plan slot
//! (`PolicyPayload::compiled_plan`, §41.5 item 2) names the plan by digest
//! and count and carries no strategy, "the plan itself ships elsewhere", and
//! nothing on the node held the elsewhere. This module is that half. It
//! reads the plan from a file the node is configured with, refuses it unless
//! its bytes digest to what the *verified* payload names, and deploys each
//! strategy it lists once a grant for it has arrived — under the pricing
//! policy the node is configured with, because the slot does not yet carry
//! one and a cell refuses an unpriced deploy already.
//!
//! # What refuses, and where
//!
//! The payload decides *which* plan; the file supplies *what it says*. A
//! file whose digest is not the one the signed payload names deploys
//! nothing, whatever it contains: the digest is the only thing tying the
//! bytes on this machine's disk to a decision the centre signed, and a plan
//! deployed on the strength of being at the configured path would be a plan
//! anybody with write access to the path could choose. A stale slot deploys
//! nothing, as a stale whitelist installs no desk. An unset pricing deploys
//! nothing and says so on every tick; a pricing the node cannot parse stops
//! the process at start, because a value somebody wrote and nobody reads is
//! the control that does nothing.
//!
//! # What this holds
//!
//! Grants for strategies the cell does not yet run, bounded, keyed by
//! strategy, the newest replacing the older. `Cell::renew_capital` refuses
//! such a grant and is right to — a cell does not deploy a strategy because
//! capital arrived — so this is the one place it may wait, and it waits for
//! a plan the centre signed that names the strategy. The arbitrage installer
//! refuses a grant for any strategy but its own on the argument that a
//! waiting grant may be spent by whatever is later deployed under that
//! name. Here what is deployed under a name is what a verified, fresh,
//! digest-matched plan says, which is the same authority that issued the
//! grant; the bound is what keeps the wait from becoming a store.
//!
//! # What is not here
//!
//! A pricing policy per strategy. The slot carries none today; the
//! node-wide default stands in until it does, and the lines the slot needs
//! are in the change that added this module rather than invented here in a
//! crate that does not own the slot.

use qip_contracts::policy::PlanDigest;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use qip_edge::cell::{Cell, PricingPolicy};
use qip_edge::envelope::VerifiedEnvelope;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::StrategySpec;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The environment variable naming how every deployed strategy's intents are
/// priced, until the plan slot carries a policy of its own.
pub const PRICING_VARIABLE: &str = "QIP_DEFAULT_PRICING";
/// The environment variable naming the file the compiled plan is read from.
pub const PLAN_VARIABLE: &str = "QIP_STRATEGY_PLAN_PATH";

/// The one word that prices every intent at the touch.
pub const MARKETABLE: &str = "marketable";
/// The prefix of the resting form, `rest-at-mid:<seconds>`.
pub const REST_AT_MID_PREFIX: &str = "rest-at-mid:";

/// The most bytes a plan file may hold. Read whole, and refused above this
/// rather than truncated: a plan read in part would digest to something the
/// payload does not name and be refused anyway, but by the wrong message.
pub const MAX_PLAN_BYTES: u64 = 1 << 20;
/// The most strategies a plan may name. The blueprint's hot tier is capped
/// at 1,200 (§26.2); this node deploys each into its own runtime, and a
/// plan past this is refused whole rather than deployed in part.
pub const MAX_PLAN_STRATEGIES: usize = 256;
/// The most grants held for strategies not yet deployed. One per strategy,
/// so this is also the most strategies a plan can fund at once; a grant
/// arriving at the bound is refused and reported, never silently dropped.
pub const MAX_HELD_GRANTS: usize = 256;

/// Read the default pricing, refusing anything but the two stated forms.
///
/// `None` is unset or blank, which deploys nothing and is announced. A
/// value that is neither `marketable` nor `rest-at-mid:<seconds>` is a
/// configuration error: an operator who wrote `market`, `mid` or
/// `rest-at-mid:30s` meant something, and a node that started and deployed
/// nothing would leave them reading a quiet cell rather than the mistake.
pub fn parse_pricing(value: Option<&str>) -> Result<Option<PricingPolicy>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value == MARKETABLE {
        return Ok(Some(PricingPolicy::Marketable));
    }
    if let Some(seconds) = value.strip_prefix(REST_AT_MID_PREFIX) {
        let seconds = seconds.parse::<i64>().map_err(|_| {
            Error::invalid(format!(
                "configuration: {PRICING_VARIABLE}={value} does not name a whole number of \
                 seconds after `{REST_AT_MID_PREFIX}`; write `{REST_AT_MID_PREFIX}30` for a \
                 resting order withdrawn after thirty seconds"
            ))
        })?;
        return PricingPolicy::rest_at_mid(Duration::from_secs(seconds))
            .map(Some)
            .map_err(|error| {
                Error::invalid(format!(
                    "configuration: {PRICING_VARIABLE}={value} is refused: {}",
                    error.message()
                ))
            });
    }
    Err(Error::invalid(format!(
        "configuration: {PRICING_VARIABLE}={value} names a pricing this node does not have. The \
         two values are `{MARKETABLE}`, which takes the touch, and \
         `{REST_AT_MID_PREFIX}<seconds>`, which rests at the mid and is withdrawn after that \
         many seconds. Unset deploys no strategy and says so"
    )))
}

/// The compiled plan as the file carries it: the strategy specifications,
/// compiled here against the node's own catalogue.
///
/// Unknown fields are refused, so a plan written for a later schema is
/// refused rather than deployed with whatever this build understood of it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyPlan {
    pub strategies: Vec<StrategySpec>,
}

impl StrategyPlan {
    /// Parse a plan, refusing one past the bound or naming a strategy twice.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let plan: Self = serde_json::from_slice(bytes).map_err(|error| {
            Error::invalid(format!("the strategy plan does not parse: {error}"))
        })?;
        if plan.strategies.len() > MAX_PLAN_STRATEGIES {
            return Err(Error::invalid(format!(
                "the strategy plan names {} strategies and this node deploys at most \
                 {MAX_PLAN_STRATEGIES}; the plan is refused whole rather than deployed in part",
                plan.strategies.len()
            )));
        }
        let mut seen = BTreeMap::new();
        for spec in &plan.strategies {
            if seen.insert(spec.id.as_str().to_string(), ()).is_some() {
                return Err(Error::invalid(format!(
                    "the strategy plan names {} twice; a plan naming one strategy under two \
                     specifications cannot say which the cell should run",
                    spec.id.as_str()
                )));
            }
        }
        Ok(plan)
    }

    /// The digest the payload's slot must name for these bytes to be the
    /// plan it means.
    pub fn digest_of(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }
}

/// What one attempt to install the plan did, for the tick and the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanInstallation {
    /// Why nothing could be attempted, when nothing could.
    pub blocked: Option<String>,
    /// Strategies deployed this attempt.
    pub deployed: Vec<String>,
    /// Strategies withdrawn this attempt because the plan no longer names
    /// them, or names them differently.
    pub withdrawn: Vec<String>,
    /// Strategies the plan names and no grant has arrived for.
    pub awaiting_grant: Vec<String>,
    /// Strategies the plan or the cell refused, with the reason.
    pub refused: Vec<(String, String)>,
}

impl PlanInstallation {
    fn blocked(reason: impl Into<String>) -> Self {
        Self {
            blocked: Some(reason.into()),
            ..Self::default()
        }
    }

    /// Whether an operator needs to look.
    pub fn is_quiet(&self) -> bool {
        self.deployed.is_empty() && self.withdrawn.is_empty() && self.refused.is_empty()
    }

    pub fn describe(&self) -> String {
        if let Some(reason) = &self.blocked {
            return reason.clone();
        }
        let mut parts = Vec::new();
        if !self.deployed.is_empty() {
            parts.push(format!("deployed {}", self.deployed.join(", ")));
        }
        if !self.withdrawn.is_empty() {
            parts.push(format!("withdrew {}", self.withdrawn.join(", ")));
        }
        if !self.awaiting_grant.is_empty() {
            parts.push(format!(
                "awaiting a grant for {}",
                self.awaiting_grant.join(", ")
            ));
        }
        for (strategy, reason) in &self.refused {
            parts.push(format!("refused {strategy}: {reason}"));
        }
        if parts.is_empty() {
            "the plan is deployed as named".to_string()
        } else {
            parts.join("; ")
        }
    }
}

/// A plan the installer has read and checked against a payload's digest.
#[derive(Debug)]
struct ReadPlan {
    digest: String,
    specs: BTreeMap<String, StrategySpec>,
}

/// Holds grants until a plan that spends them arrives, and deploys what the
/// plan names.
#[derive(Debug)]
pub struct StrategyInstaller {
    plan_path: Option<PathBuf>,
    pricing: Option<PricingPolicy>,
    grants: BTreeMap<String, VerifiedEnvelope>,
    /// The plan last read and matched to a payload, so the file is read once
    /// per digest rather than once per tick.
    read: Option<ReadPlan>,
    /// The specification each strategy this installer deployed was compiled
    /// from, so a later plan naming the strategy differently is seen as a
    /// change and not as "already deployed". Bounded by the plan bound: an
    /// entry exists only for a strategy the cell runs.
    deployed_from: BTreeMap<String, StrategySpec>,
}

impl StrategyInstaller {
    /// An installer with what the node was configured with. Either may be
    /// absent; both absences are reported on every attempt, and neither
    /// makes the installer refuse a grant, because a grant that arrives
    /// before the configuration is completed is not the operator's mistake.
    pub fn new(plan_path: Option<PathBuf>, pricing: Option<PricingPolicy>) -> Self {
        Self {
            plan_path,
            pricing,
            grants: BTreeMap::new(),
            read: None,
            deployed_from: BTreeMap::new(),
        }
    }

    pub fn plan_path(&self) -> Option<&Path> {
        self.plan_path.as_deref()
    }

    pub fn pricing(&self) -> Option<PricingPolicy> {
        self.pricing
    }

    /// The strategies a grant is held for and not yet spent.
    pub fn held_grants(&self) -> Vec<&str> {
        self.grants.keys().map(String::as_str).collect()
    }

    /// Hold a verified grant for a strategy the cell does not run yet.
    ///
    /// Refused at the bound rather than evicting: the grant evicted would be
    /// one the centre issued and the cell never heard about, and the centre
    /// would read the next delta as the cell having ignored it.
    pub fn offer(&mut self, envelope: VerifiedEnvelope) -> Result<()> {
        let strategy = envelope.strategy().as_str().to_string();
        if !self.grants.contains_key(&strategy) && self.grants.len() >= MAX_HELD_GRANTS {
            return Err(Error::guard(format!(
                "the installer holds grants for {MAX_HELD_GRANTS} strategies the cell does not \
                 run, the most it will hold; the grant for {strategy} is refused rather than \
                 evicting one, and a plan naming what is held would spend them"
            )));
        }
        self.grants.insert(strategy, envelope);
        Ok(())
    }

    /// Deploy what the fresh plan names and withdraw what it dropped, and
    /// say what happened either way.
    pub fn install(&mut self, cell: &mut Cell, now: Timestamp) -> PlanInstallation {
        let Some(named) = cell.compiled_plan(now).cloned() else {
            return PlanInstallation::blocked("no fresh compiled plan applied");
        };
        let Some(pricing) = self.pricing else {
            return PlanInstallation::blocked(format!(
                "{PRICING_VARIABLE} is unset, so the plan the payload names deploys nothing; \
                 set it to `{MARKETABLE}` or `{REST_AT_MID_PREFIX}<seconds>`"
            ));
        };
        let Some(path) = self.plan_path.clone() else {
            return PlanInstallation::blocked(format!(
                "{PLAN_VARIABLE} is unset, so the plan the payload names by digest cannot be \
                 read; nothing is deployed"
            ));
        };
        if let Err(error) = self.read_plan(&path, &named) {
            return PlanInstallation::blocked(error.message().to_string());
        }
        let Some(plan) = self.read.as_ref() else {
            return PlanInstallation::blocked(
                "the plan was read and then was not; nothing is deployed",
            );
        };
        let specs = plan.specs.clone();
        let mut outcome = PlanInstallation::default();

        // What the plan dropped or changed goes first, so a strategy the
        // plan renames is withdrawn under its old specification before it
        // is deployed under the new one, and the grant it ran under is the
        // grant it is redeployed under.
        let deployed: Vec<String> = cell
            .deployed_strategies()
            .into_iter()
            .map(str::to_string)
            .collect();
        for strategy in deployed {
            let Some(spec) = specs.get(&strategy) else {
                self.withdraw(cell, &strategy, now, &mut outcome, false);
                continue;
            };
            if !self.deployed_matches(&strategy, spec) {
                self.withdraw(cell, &strategy, now, &mut outcome, true);
            }
        }

        for (strategy, spec) in &specs {
            if cell.deployed_strategies().contains(&strategy.as_str()) {
                continue;
            }
            let Some(envelope) = self.grants.get(strategy).cloned() else {
                outcome.awaiting_grant.push(strategy.clone());
                continue;
            };
            // One compiler per strategy, so each deployment gets the arena
            // its plan was compiled against and nothing else's — the
            // aliasing `Cell::deploy` refuses is never built here.
            let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
            let compiled = match compiler.compile(spec) {
                Ok(compiled) => compiled,
                Err(error) => {
                    outcome
                        .refused
                        .push((strategy.clone(), error.message().to_string()));
                    continue;
                }
            };
            let program = compiler.into_program();
            match cell.deploy_with_pricing(compiled, program, envelope, pricing) {
                Ok(()) => {
                    // Spent: the cell holds it now, and `renew_capital`
                    // replaces it from here on as it does any strategy's.
                    self.grants.remove(strategy);
                    self.deployed_from.insert(strategy.clone(), spec.clone());
                    outcome.deployed.push(strategy.clone());
                }
                Err(error) => outcome
                    .refused
                    .push((strategy.clone(), error.message().to_string())),
            }
        }
        outcome
    }

    /// Whether the strategy deployed under `strategy` was compiled from
    /// `spec`.
    ///
    /// The specification is compared rather than the compiled form because
    /// the specification is what the centre wrote. A strategy this
    /// installer did not deploy — one a test deployed by hand — has no
    /// record to compare and is left as it is: withdrawing it would be
    /// acting on a plan that never named it.
    fn deployed_matches(&self, strategy: &str, spec: &StrategySpec) -> bool {
        self.deployed_from
            .get(strategy)
            .is_none_or(|deployed| deployed == spec)
    }

    fn withdraw(
        &mut self,
        cell: &mut Cell,
        strategy: &str,
        now: Timestamp,
        outcome: &mut PlanInstallation,
        redeploying: bool,
    ) {
        match cell.withdraw(strategy, now) {
            Ok(envelope) => {
                self.deployed_from.remove(strategy);
                outcome.withdrawn.push(strategy.to_string());
                if redeploying {
                    // Held again so the deployment below finds it: the same
                    // grant, the same strategy, a new specification.
                    self.grants.insert(strategy.to_string(), envelope);
                }
                // A dropped strategy's envelope is dropped with it: the
                // centre signed it for a strategy this cell no longer runs,
                // and holding it would be holding capital nobody can spend.
            }
            Err(error) => outcome
                .refused
                .push((strategy.to_string(), error.message().to_string())),
        }
    }

    /// Read the plan at `path` if its digest is not the one already held,
    /// and refuse it unless it is the one the payload names.
    fn read_plan(&mut self, path: &Path, named: &PlanDigest) -> Result<()> {
        if self
            .read
            .as_ref()
            .is_some_and(|plan| plan.digest == named.digest)
        {
            return Ok(());
        }
        let length = std::fs::metadata(path)
            .map_err(|error| {
                Error::io(format!(
                    "the strategy plan at {} cannot be read: {error}; the payload names plan \
                     {} and nothing is deployed until the file is there",
                    path.display(),
                    named.digest
                ))
            })?
            .len();
        if length > MAX_PLAN_BYTES {
            return Err(Error::invalid(format!(
                "the strategy plan at {} is {length} bytes and this node reads at most \
                 {MAX_PLAN_BYTES}; it is refused whole rather than read in part",
                path.display()
            )));
        }
        let bytes = std::fs::read(path).map_err(|error| {
            Error::io(format!(
                "the strategy plan at {} cannot be read: {error}",
                path.display()
            ))
        })?;
        let digest = StrategyPlan::digest_of(&bytes);
        if digest != named.digest {
            return Err(Error::denied(format!(
                "the strategy plan at {} digests to {digest} and the verified payload names \
                 {}; the file is not the plan the centre signed for, and nothing is deployed \
                 from it",
                path.display(),
                named.digest
            )));
        }
        let plan = StrategyPlan::from_bytes(&bytes)?;
        let count = u64::try_from(plan.strategies.len()).unwrap_or(u64::MAX);
        if count != named.strategies {
            return Err(Error::denied(format!(
                "the strategy plan at {} names {count} strategies and the payload says {}; two \
                 claims about one plan, and nothing is deployed until they agree",
                path.display(),
                named.strategies
            )));
        }
        self.read = Some(ReadPlan {
            digest,
            specs: plan
                .strategies
                .into_iter()
                .map(|spec| (spec.id.as_str().to_string(), spec))
                .collect(),
        });
        Ok(())
    }
}
