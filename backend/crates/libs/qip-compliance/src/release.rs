//! Release governance: the promotion precondition, the signed-runbook
//! executor and the change-window gate (CICD-001, CICD-025, CICD-068).
//!
//! Three refusals, one shape. Each takes the facts a human might otherwise
//! assert ("all five gates passed", "that runbook is approved", "we are inside
//! the window") and demands them as values, so a call site that skipped the
//! check cannot build the argument. None of them calls out, deploys or runs a
//! shell: [`RemediationExecutor::run`] hands the approved action to a closure
//! the composition root supplies, and the paper-trading boundary is untouched.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::Timestamp;
use qip_core::error::{Error, Result};

use crate::signing::SigningKey;

/// The five gates every deploy passes before promotion (master blueprint
/// §29). Declared as an enum so that a sixth cannot be added, or one dropped,
/// without every `match` and [`Gate::ALL`] noticing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gate {
    Simulation,
    Integration,
    Security,
    Performance,
    PaperTrading,
}

impl Gate {
    pub const ALL: [Gate; 5] = [
        Gate::Simulation,
        Gate::Integration,
        Gate::Security,
        Gate::Performance,
        Gate::PaperTrading,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Gate::Simulation => "simulation",
            Gate::Integration => "integration",
            Gate::Security => "security",
            Gate::Performance => "performance",
            Gate::PaperTrading => "paper-trading",
        }
    }
}

/// One gate's verdict on one artifact digest. The digest travels with the
/// verdict so a pass earned by yesterday's build cannot promote today's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateVerdict {
    pub gate: Gate,
    pub digest: String,
    pub passed: bool,
}

/// The artifact digest a policy admitted for promotion. Only
/// [`PromotionPolicy::admit`] builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promotable {
    digest: String,
}

impl Promotable {
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Promotion is permitted only when all five gates carry a passing verdict for
/// the same digest. A deploy promoted on a subset reaches the next environment
/// with the untested dimension unexamined.
#[derive(Clone, Copy, Debug, Default)]
pub struct PromotionPolicy;

impl PromotionPolicy {
    /// The gates the policy demands, by name.
    pub fn gates(&self) -> Vec<&'static str> {
        Gate::ALL.iter().map(|g| g.name()).collect()
    }

    /// Admit `digest` or refuse, naming every gate that is missing or failing
    /// so the operator fixes all of them in one pass.
    pub fn admit(&self, digest: &str, verdicts: &[GateVerdict]) -> Result<Promotable> {
        if digest.trim().is_empty() {
            return Err(Error::invalid(
                "promotion needs the artifact digest; an empty digest names nothing",
            ));
        }
        let mut by_gate: BTreeMap<Gate, bool> = BTreeMap::new();
        for v in verdicts.iter().filter(|v| v.digest == digest) {
            // A gate reported twice must pass both times: a later green run
            // does not erase an earlier red one for the same digest.
            let entry = by_gate.entry(v.gate).or_insert(true);
            *entry &= v.passed;
        }
        let mut missing: Vec<&str> = Vec::new();
        let mut failing: Vec<&str> = Vec::new();
        for gate in Gate::ALL {
            match by_gate.get(&gate) {
                None => missing.push(gate.name()),
                Some(false) => failing.push(gate.name()),
                Some(true) => {}
            }
        }
        if missing.is_empty() && failing.is_empty() {
            return Ok(Promotable {
                digest: digest.to_string(),
            });
        }
        Err(Error::denied(format!(
            "{digest} is not promotable: missing verdict for [{}], failing [{}]; run the missing \
             gates against this exact digest and fix the failing ones",
            missing.join(", "),
            failing.join(", ")
        )))
    }
}

/// Scanner severities, ordered so `>=` reads as "at or above".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// One scanner finding against one artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    /// Whether a fixed version exists. The pipeline's scans run with
    /// `--ignore-unfixed`, so an unfixed finding is reported, not blocking.
    pub fix_available: bool,
}

/// The severity policy behind the security gate (CICD-017): a fixable finding
/// at or above `blocking` stops promotion. Without a stated threshold the
/// scanner's `--severity CRITICAL,HIGH` flag is the only record of it, and a
/// gate that cannot be shown to refuse at the line and admit below it is
/// indistinguishable from one that refuses everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeverityPolicy {
    blocking: Severity,
}

impl Default for SeverityPolicy {
    /// HIGH, matching the `trivy --severity CRITICAL,HIGH` steps in `ci.yml`
    /// and `deploy.yml`.
    fn default() -> Self {
        Self {
            blocking: Severity::High,
        }
    }
}

impl SeverityPolicy {
    pub fn new(blocking: Severity) -> Self {
        Self { blocking }
    }

    /// Refuse if any fixable finding is at or above the blocking severity,
    /// naming each so the operator upgrades them in one pass.
    pub fn admit(&self, findings: &[Finding]) -> Result<()> {
        let blocking: Vec<&str> = findings
            .iter()
            .filter(|f| f.fix_available && f.severity >= self.blocking)
            .map(|f| f.id.as_str())
            .collect();
        if blocking.is_empty() {
            return Ok(());
        }
        Err(Error::denied(format!(
            "promotion blocked by fixable findings at or above {:?}: [{}]; upgrade the affected \
             packages and rescan the same artifact",
            self.blocking,
            blocking.join(", ")
        )))
    }
}

/// A runbook: a named action and the body a person reviewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runbook {
    pub id: String,
    pub action: String,
    pub body: String,
}

impl Runbook {
    fn payload(&self) -> String {
        format!("{}|{}|{}", self.id, self.action, self.body)
    }

    /// Sign a runbook under the release key.
    pub fn sign(&self, key: &SigningKey) -> String {
        key.sign(&self.payload())
    }
}

/// Runs remediation only for a runbook whose signature verifies **and** whose
/// action is on the approved list. The list is separate from the signature on
/// purpose: a validly signed runbook for an action nobody approved for
/// automatic use is still a refusal.
pub struct RemediationExecutor<'a> {
    key: &'a SigningKey,
    approved_actions: BTreeSet<String>,
}

impl std::fmt::Debug for RemediationExecutor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is deliberately not printed.
        f.debug_struct("RemediationExecutor")
            .field("key_id", &self.key.key_id())
            .field("approved_actions", &self.approved_actions)
            .finish()
    }
}

impl<'a> RemediationExecutor<'a> {
    pub fn new(key: &'a SigningKey, approved_actions: impl IntoIterator<Item = String>) -> Self {
        Self {
            key,
            approved_actions: approved_actions.into_iter().collect(),
        }
    }

    /// Verify, check approval, then hand the action to `run`.
    pub fn run<T>(
        &self,
        runbook: &Runbook,
        signature: &str,
        run: impl FnOnce(&Runbook) -> Result<T>,
    ) -> Result<T> {
        self.key.require(
            &format!("runbook {}", runbook.id),
            &runbook.payload(),
            signature,
        )?;
        if !self.approved_actions.contains(&runbook.action) {
            return Err(Error::denied(format!(
                "runbook {} requests action '{}', which is not on the approved remediation \
                 list; have it approved or remediate by hand",
                runbook.id, runbook.action
            )));
        }
        run(runbook)
    }
}

/// What a change touches. The four named kinds are the ones the blueprint
/// confines to change windows; anything else is [`ChangeKind::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Folder,
    Project,
    Network,
    Database,
    Other,
}

impl ChangeKind {
    fn windowed(self) -> bool {
        !matches!(self, ChangeKind::Other)
    }
}

/// Declared windows, as half-open `[start, end)` intervals.
#[derive(Clone, Debug, Default)]
pub struct ChangeWindows {
    windows: Vec<(Timestamp, Timestamp)>,
}

impl ChangeWindows {
    /// Declare a window. An empty or inverted one is refused rather than
    /// silently never opening.
    pub fn declare(&mut self, start: Timestamp, end: Timestamp) -> Result<()> {
        if end <= start {
            return Err(Error::invalid(
                "a change window must end after it starts; declare start < end",
            ));
        }
        self.windows.push((start, end));
        Ok(())
    }

    /// The apply gate: a windowed change at `at` outside every declared window
    /// is refused. No windows declared means none open, which is the
    /// restrictive default.
    pub fn admit(&self, kind: ChangeKind, at: Timestamp) -> Result<()> {
        if !kind.windowed() || self.windows.iter().any(|(s, e)| *s <= at && at < *e) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "a {kind:?} change at {} is outside every declared change window; declare a window \
             covering it or wait for the next one",
            at.to_rfc3339()
        )))
    }
}
