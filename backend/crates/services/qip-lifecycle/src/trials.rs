//! Cumulative trial accounting, per strategy family, across every run.
//!
//! The deflated Sharpe ratio discounts a result for how many strategies were
//! tried before it. That correction is only as honest as the count it is
//! given, and the count a single run reports is the easiest number in the
//! whole pipeline to launder: split a sweep of five thousand configurations
//! into fifty runs of a hundred and each run corrects for a hundred. Blueprint
//! rule 25 is explicit that the correction is made against the family's
//! lifetime count and never per batch, and ADR 0023 records that until
//! something in the tree counted trials across runs, "honest significance"
//! could be claimed but not computed. This module is that something.
//!
//! Three properties hold:
//!
//! * **Unknown is not zero.** A family nobody has opened has no count, and
//!   [`TrialBook::charge`] refuses rather than starting from nothing. The
//!   promotion path reads the count through the charge, so a strategy whose
//!   family was never opened cannot be evaluated at all. The refusal names the
//!   act that would make the count known.
//! * **The count only rises.** Every charge appends a [`TrialRecord`] to the
//!   family's journal carrying the lifetime total *after* it, hash-chained to
//!   the record before, so a count that has been lowered — or a family
//!   reopened at zero — fails [`TrialBook::verify`] rather than passing as a
//!   fresh start.
//! * **The quarter is budgeted.** Blueprint §20.1 caps a family at five
//!   hundred trials per calendar quarter, and a cap the book only counted
//!   against would be a limit that cannot fire. Every record also carries
//!   the family's count for the quarter it falls in — chained and verified
//!   like the lifetime — and [`TrialBook::charge`] refuses a charge that
//!   would carry the quarter past the budget, naming the family, the
//!   quarter, the count and the budget, and charging nothing. The quarter
//!   is the UTC calendar quarter of the charge's own instant, so a replay
//!   files every charge where the original did.
//! * **The count survives the process.** A book opened on a
//!   [`KeyValueStore`] writes each record before it acknowledges it and
//!   replays the journal, verifying the chain, when it is opened again. A book
//!   built with [`TrialBook::in_memory`] forgets everything at exit and says
//!   so in its name; a deployment that wants the lifetime count to mean
//!   "lifetime" opens the book on a durable adapter.
//!
//! Nothing here reads a clock. Every append takes the instant it records, so
//! a replay of the same events produces the same journal, hash for hash.

use qip_contracts::signal::StrategyId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::kv::{KeyValueStore, KeyValueStoreExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Key prefix under which a book persists its journal.
///
/// One key per record, `lifecycle/trials/<family>/<sequence>` with the
/// sequence zero-padded so the store's lexicographic prefix scan returns each
/// family's journal in order without the book having to sort it.
pub const JOURNAL_PREFIX: &str = "lifecycle/trials/";

/// The `previous` hash of a family's first record.
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Trials a family may be charged in one calendar quarter, unless a book is
/// built with another figure. Blueprint §20.1 and §54.1: five hundred per
/// family per quarter, and the reasoning there is that a per-batch cap is
/// laundered by splitting the sweep, which is why the count is per quarter
/// and per family rather than per run.
pub const DEFAULT_QUARTERLY_BUDGET: u64 = 500;

/// A calendar quarter, in UTC.
///
/// Derived from an instant and never stored on its own: the year and month
/// of [`Timestamp::civil_date`], which is UTC, and `(month − 1) / 3` for the
/// quarter, so January–March is the first. UTC on purpose — the boundary is
/// then the same instant everywhere, and two processes charging the same
/// family from different regions cannot file one charge in two quarters. The
/// arithmetic is deterministic, so a replay of the journal recovers every
/// quarterly count from the records alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Quarter {
    year: i32,
    /// Zero-based inside; [`Self::number`] is the one-based figure shown.
    index: u32,
}

impl Quarter {
    /// The quarter `at` falls in.
    pub fn of(at: Timestamp) -> Self {
        let (year, month, _) = at.civil_date();
        Self {
            year,
            index: (month - 1) / 3,
        }
    }

    pub fn year(self) -> i32 {
        self.year
    }

    /// One to four.
    pub fn number(self) -> u32 {
        self.index + 1
    }
}

impl fmt::Display for Quarter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}Q{}", self.year, self.number())
    }
}

/// A family of strategies that share a trial budget.
///
/// The unit the blueprint counts against: a template and its parameter sweep,
/// not one configuration of it. A strategy belongs to exactly one family for
/// life — [`TrialBook::enrol`] refuses to move one — because a strategy that
/// could change family could take its count with it or leave it behind.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StrategyFamily(String);

impl StrategyFamily {
    /// Name a family. Refuses a blank name and anything outside
    /// `[A-Za-z0-9._-]`, because the name becomes a segment of the journal
    /// key and a `/` in it would file one family's records under another.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a strategy family needs a name; the trial count is keyed on it",
            ));
        }
        if let Some(bad) = name
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(Error::invalid(format!(
                "family name {name:?} contains {bad:?}; only letters, digits, '.', '_' and '-' \
                 are allowed, because the name is a segment of the journal key"
            )));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StrategyFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one journal record says happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrialEvent {
    /// The family's account was opened. Its count starts at zero here and
    /// nowhere else.
    Opened,
    /// A strategy was enrolled as a member, so its evaluations charge here.
    Enrolled { strategy: StrategyId },
    /// A strategy was evaluated after `trials` configurations were tried.
    Charged { strategy: StrategyId, trials: u64 },
}

impl TrialEvent {
    /// The event as it enters the hash. A fixed spelling rather than a
    /// serialisation, so a serde change cannot silently break every chain.
    fn canonical(&self) -> String {
        match self {
            Self::Opened => "opened".to_string(),
            Self::Enrolled { strategy } => format!("enrolled:{strategy}"),
            Self::Charged { strategy, trials } => format!("charged:{strategy}:{trials}"),
        }
    }
}

/// One append to a family's journal.
///
/// Carries the lifetime total *after* the event rather than leaving it to be
/// summed, so a reader with one record knows the count at that instant, and
/// so the hash covers the total — a record whose total disagrees with its
/// predecessor's plus its own charge is a rewrite, and fails to verify.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialRecord {
    pub family: StrategyFamily,
    /// Position in the family's journal, from zero.
    pub sequence: u64,
    pub event: TrialEvent,
    /// The family's lifetime trial count once this record is applied.
    pub lifetime_after: u64,
    /// The family's trial count for the calendar quarter of `at`, once this
    /// record is applied. Zero again at the first record of a new quarter.
    /// Under the hash like the lifetime, so a quarter's count lowered by
    /// hand fails to verify rather than freeing up budget.
    pub quarter_after: u64,
    pub at: Timestamp,
    /// Hash of the record before this one, or all zeros for the first.
    pub previous: String,
    /// SHA-256 over every other field.
    pub hash: String,
}

impl TrialRecord {
    fn digest(
        family: &StrategyFamily,
        sequence: u64,
        event: &TrialEvent,
        lifetime_after: u64,
        quarter_after: u64,
        at: Timestamp,
        previous: &str,
    ) -> String {
        let canonical = format!(
            "{family}|{sequence}|{}|{lifetime_after}|{quarter_after}|{}|{previous}",
            event.canonical(),
            at.as_nanos()
        );
        sha256_hex(canonical.as_bytes())
    }

    /// Whether the record's hash is the hash of its contents.
    pub fn verify(&self) -> bool {
        self.hash
            == Self::digest(
                &self.family,
                self.sequence,
                &self.event,
                self.lifetime_after,
                self.quarter_after,
                self.at,
                &self.previous,
            )
    }

    fn key(&self) -> String {
        format!("{JOURNAL_PREFIX}{}/{:020}", self.family, self.sequence)
    }

    /// The lifetime and quarterly totals a record must carry, given the
    /// record before it. One computation for appending and for verifying, so
    /// the book cannot accept from a store a total it would not have written.
    ///
    /// The quarterly total carries over from the previous record only when
    /// both fall in the same [`Quarter`]; otherwise it starts again at zero.
    /// A record is never dated before its predecessor — [`TrialBook`] refuses
    /// that separately — so a quarter, once left, is never charged again.
    fn totals_after(
        previous: Option<&TrialRecord>,
        event: &TrialEvent,
        at: Timestamp,
    ) -> Result<(u64, u64)> {
        let (prior_lifetime, prior_quarter) = match previous {
            None => (0, 0),
            Some(p) if Quarter::of(p.at) == Quarter::of(at) => (p.lifetime_after, p.quarter_after),
            Some(p) => (p.lifetime_after, 0),
        };
        match event {
            TrialEvent::Opened => Ok((0, 0)),
            TrialEvent::Enrolled { .. } => Ok((prior_lifetime, prior_quarter)),
            TrialEvent::Charged { trials, .. } => {
                let lifetime = prior_lifetime.checked_add(*trials).ok_or_else(|| {
                    Error::numeric(format!(
                        "charging {trials} trial(s) overflows the lifetime count"
                    ))
                })?;
                let quarter = prior_quarter.checked_add(*trials).ok_or_else(|| {
                    Error::numeric(format!(
                        "charging {trials} trial(s) overflows the quarterly count"
                    ))
                })?;
                Ok((lifetime, quarter))
            }
        }
    }
}

/// The count one evaluation was charged under.
///
/// Issued by [`TrialBook::charge`] and read by the holdout gate, which
/// deflates against [`Self::lifetime`] rather than [`Self::this_run`]. The
/// fields are private so that the only way to build one in code is to charge
/// the book; a deserialised account is still possible, which is why the
/// ordinary promotion path charges the book itself and overwrites whatever
/// the submitted evidence carried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialAccount {
    family: StrategyFamily,
    strategy: StrategyId,
    this_run: u64,
    lifetime: u64,
    charged_at: Timestamp,
    sequence: u64,
    quarter: Quarter,
    quarter_trials: u64,
    quarterly_budget: u64,
}

impl TrialAccount {
    pub fn family(&self) -> &StrategyFamily {
        &self.family
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    /// Configurations tried in the run that produced this candidate.
    pub fn this_run(&self) -> u64 {
        self.this_run
    }

    /// The family's lifetime count including this run. The number the
    /// deflated Sharpe is corrected against.
    pub fn lifetime(&self) -> u64 {
        self.lifetime
    }

    /// The lifetime count before this run was charged.
    pub fn prior(&self) -> u64 {
        self.lifetime - self.this_run
    }

    pub fn charged_at(&self) -> Timestamp {
        self.charged_at
    }

    /// The journal position of the charge, for cross-reference.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The calendar quarter the charge was filed in.
    pub fn quarter(&self) -> Quarter {
        self.quarter
    }

    /// The family's count for that quarter including this run.
    pub fn quarter_trials(&self) -> u64 {
        self.quarter_trials
    }

    /// The budget the quarter was charged under.
    pub fn quarterly_budget(&self) -> u64 {
        self.quarterly_budget
    }

    pub fn describe(&self) -> String {
        format!(
            "{} trial(s) this run on top of {} already charged to family {}: {} lifetime, {} of \
             the {} budgeted for {}",
            self.this_run,
            self.prior(),
            self.family,
            self.lifetime,
            self.quarter_trials,
            self.quarterly_budget,
            self.quarter
        )
    }
}

/// Every family's journal, and which strategy charges where.
#[derive(Clone, Debug)]
pub struct TrialBook {
    store: Option<Arc<dyn KeyValueStore>>,
    journals: BTreeMap<StrategyFamily, Vec<TrialRecord>>,
    members: BTreeMap<StrategyId, StrategyFamily>,
    /// Trials a family may be charged per calendar quarter. Configuration,
    /// not record: the journal verifies the same under any budget, so a
    /// book reopened with a different figure still replays.
    quarterly_budget: u64,
}

impl TrialBook {
    /// A book that lives only as long as the process.
    ///
    /// For tests, and for a composition root that has not yet chosen a store.
    /// Its lifetime counts are lifetime counts of this process, which is the
    /// per-run accounting the blueprint forbids the moment a second process
    /// runs; the name is the warning.
    pub fn in_memory() -> Self {
        Self {
            store: None,
            journals: BTreeMap::new(),
            members: BTreeMap::new(),
            quarterly_budget: DEFAULT_QUARTERLY_BUDGET,
        }
    }

    /// Budget each family at `budget` trials per calendar quarter instead of
    /// [`DEFAULT_QUARTERLY_BUDGET`].
    ///
    /// Refuses zero. A budget of zero would refuse every evaluation in every
    /// family for ever, which is not a budget but a stop, and a stop is
    /// expressed by retiring the family, where the ledger records who did it
    /// and why. Raising the figure above the blueprint's is a decision that
    /// belongs in the composition root with its reason beside it.
    pub fn with_quarterly_budget(mut self, budget: u64) -> Result<Self> {
        if budget == 0 {
            return Err(Error::invalid(format!(
                "a quarterly trial budget of zero would refuse every evaluation; the blueprint's \
                 figure is {DEFAULT_QUARTERLY_BUDGET}, and a family that must stop is retired \
                 rather than budgeted to nothing"
            )));
        }
        self.quarterly_budget = budget;
        Ok(self)
    }

    /// Trials a family may be charged per calendar quarter.
    pub fn quarterly_budget(&self) -> u64 {
        self.quarterly_budget
    }

    /// Open the book on a store, replaying and verifying every journal in it.
    ///
    /// Refuses a store whose journal does not verify. A broken chain means a
    /// record was altered or removed, and a count rebuilt over the gap would
    /// be exactly the understated count the chain exists to catch.
    pub fn open(store: Arc<dyn KeyValueStore>) -> Result<Self> {
        let records: Vec<(String, TrialRecord)> = store.scan_as(JOURNAL_PREFIX)?;
        for (key, record) in &records {
            if *key != record.key() {
                return Err(Error::invalid(format!(
                    "trial journal record at {key} describes family {} sequence {}; the key \
                     and the record disagree, so the journal has been edited",
                    record.family, record.sequence
                )));
            }
        }
        let (journals, members) = Self::rebuild(records.into_iter().map(|(_, r)| r))?;
        Ok(Self {
            store: Some(store),
            journals,
            members,
            quarterly_budget: DEFAULT_QUARTERLY_BUDGET,
        })
    }

    /// Whether this book writes through to a store.
    pub fn is_durable(&self) -> bool {
        self.store.is_some()
    }

    /// Replay records into journals and membership, verifying as it goes.
    ///
    /// One implementation for opening and for [`Self::verify`], so what the
    /// book accepts from a store is exactly what it certifies about itself.
    #[allow(clippy::type_complexity)]
    fn rebuild(
        records: impl IntoIterator<Item = TrialRecord>,
    ) -> Result<(
        BTreeMap<StrategyFamily, Vec<TrialRecord>>,
        BTreeMap<StrategyId, StrategyFamily>,
    )> {
        let mut journals: BTreeMap<StrategyFamily, Vec<TrialRecord>> = BTreeMap::new();
        let mut members: BTreeMap<StrategyId, StrategyFamily> = BTreeMap::new();
        for record in records {
            let journal = journals.entry(record.family.clone()).or_default();
            Self::check_link(journal.last(), &record)?;
            if let TrialEvent::Enrolled { strategy } = &record.event {
                Self::check_membership(&members, strategy, &record.family)?;
                members.insert(strategy.clone(), record.family.clone());
            }
            journal.push(record);
        }
        Ok((journals, members))
    }

    /// The invariants between one record and the next.
    fn check_link(previous: Option<&TrialRecord>, record: &TrialRecord) -> Result<()> {
        let describe = || format!("family {} sequence {}", record.family, record.sequence);
        if !record.verify() {
            return Err(Error::invalid(format!(
                "trial journal {} does not hash to its contents; the record was altered",
                describe()
            )));
        }
        let (expected_sequence, expected_previous, prior_at) = match previous {
            None => (0, GENESIS.to_string(), Timestamp::EPOCH),
            Some(p) => (p.sequence + 1, p.hash.clone(), p.at),
        };
        if record.sequence != expected_sequence {
            return Err(Error::invalid(format!(
                "trial journal {} arrived where sequence {expected_sequence} was expected; a \
                 record is missing or duplicated",
                describe()
            )));
        }
        if record.previous != expected_previous {
            return Err(Error::invalid(format!(
                "trial journal {} does not chain to the record before it; the journal was \
                 rewritten",
                describe()
            )));
        }
        if record.at < prior_at {
            return Err(Error::invalid(format!(
                "trial journal {} is dated before the record it follows; a backdated charge \
                 is a rewrite",
                describe()
            )));
        }
        if let (Some(_), TrialEvent::Opened) = (previous, &record.event) {
            return Err(Error::invalid(format!(
                "trial journal {} reopens a family that already has a journal; a family opens \
                 once, at zero, and never again",
                describe()
            )));
        }
        let (expected_lifetime, expected_quarter) =
            TrialRecord::totals_after(previous, &record.event, record.at).map_err(|e| {
                Error::numeric(format!("trial journal {}: {}", describe(), e.message()))
            })?;
        if record.lifetime_after != expected_lifetime {
            return Err(Error::invalid(format!(
                "trial journal {} carries a lifetime of {} where {expected_lifetime} follows \
                 from the record before it; the count was lowered or raised by hand",
                describe(),
                record.lifetime_after
            )));
        }
        if record.quarter_after != expected_quarter {
            return Err(Error::invalid(format!(
                "trial journal {} carries {} trial(s) for {} where {expected_quarter} follows \
                 from the record before it; the quarterly count was lowered or raised by hand",
                describe(),
                record.quarter_after,
                Quarter::of(record.at)
            )));
        }
        Ok(())
    }

    fn check_membership(
        members: &BTreeMap<StrategyId, StrategyFamily>,
        strategy: &StrategyId,
        family: &StrategyFamily,
    ) -> Result<()> {
        match members.get(strategy) {
            Some(existing) if existing != family => Err(Error::denied(format!(
                "{strategy} is enrolled in family {existing} and cannot move to {family}; a \
                 strategy that changed family would leave its trials behind"
            ))),
            _ => Ok(()),
        }
    }

    /// Re-verify every journal from its first record.
    pub fn verify(&self) -> Result<()> {
        Self::rebuild(self.journals.values().flatten().cloned()).map(|_| ())
    }

    /// Open a family's account, at zero. Refuses a second opening: a family
    /// reopened at zero is a sweep laundered by renaming.
    pub fn open_family(&mut self, family: &StrategyFamily, at: Timestamp) -> Result<&TrialRecord> {
        if self.journals.contains_key(family) {
            return Err(Error::denied(format!(
                "family {family} is already open with {} lifetime trial(s); a family opens once \
                 and its count only rises",
                self.lifetime_trials(family).unwrap_or(0)
            )));
        }
        self.append(family, TrialEvent::Opened, at)
    }

    /// Make a strategy a member of a family, so its evaluations charge there.
    ///
    /// Idempotent for the same family; refuses a different one.
    pub fn enrol(
        &mut self,
        strategy: &StrategyId,
        family: &StrategyFamily,
        at: Timestamp,
    ) -> Result<()> {
        Self::check_membership(&self.members, strategy, family)?;
        if self.members.get(strategy) == Some(family) {
            return Ok(());
        }
        if !self.journals.contains_key(family) {
            return Err(Error::denied(format!(
                "family {family} is not open, so {strategy} cannot be enrolled in it; open the \
                 family with `TrialBook::open_family` first"
            )));
        }
        self.append(
            family,
            TrialEvent::Enrolled {
                strategy: strategy.clone(),
            },
            at,
        )?;
        self.members.insert(strategy.clone(), family.clone());
        Ok(())
    }

    /// The family a strategy charges to, if it has been enrolled.
    pub fn family_of(&self, strategy: &StrategyId) -> Option<&StrategyFamily> {
        self.members.get(strategy)
    }

    /// The family's lifetime trial count. `None` for a family never opened:
    /// unknown, which the caller must not read as zero.
    pub fn lifetime_trials(&self, family: &StrategyFamily) -> Option<u64> {
        self.journals
            .get(family)
            .and_then(|journal| journal.last())
            .map(|record| record.lifetime_after)
    }

    /// The family's trial count for `quarter`. `None` for a family never
    /// opened — unknown, not zero — and zero for an open family with no
    /// record in that quarter.
    ///
    /// Read from the last record filed in the quarter rather than summed, so
    /// what the book reports is what the chain verified.
    pub fn quarter_trials(&self, family: &StrategyFamily, quarter: Quarter) -> Option<u64> {
        let journal = self.journals.get(family)?;
        Some(
            journal
                .iter()
                .rev()
                .find(|record| Quarter::of(record.at) == quarter)
                .map_or(0, |record| record.quarter_after),
        )
    }

    /// Charge one evaluation to the strategy's family and return the count
    /// the evaluation must be corrected against.
    ///
    /// `trials` is what the run that produced the candidate tried — the
    /// number the evidence already carries — and it must be at least one,
    /// because the candidate itself was tried. Refuses, charging nothing,
    /// when the family's count for the calendar quarter of `at` would pass
    /// the book's budget: the five-hundredth trial of a quarter charges and
    /// the five-hundred-and-first does not, however the sweep was split.
    pub fn charge(
        &mut self,
        strategy: &StrategyId,
        trials: usize,
        at: Timestamp,
    ) -> Result<TrialAccount> {
        let family = self.members.get(strategy).cloned().ok_or_else(|| {
            Error::denied(format!(
                "the lifetime trial count for {strategy} is unknown: it is not enrolled in any \
                 family. Enrol it with `TrialBook::enrol` under the family whose sweep produced \
                 it; an unknown count is not zero"
            ))
        })?;
        if trials == 0 {
            return Err(Error::invalid(format!(
                "{strategy} reports zero trials; the candidate itself was tried, so the count \
                 is at least one"
            )));
        }
        let trials = u64::try_from(trials)
            .map_err(|_| Error::numeric(format!("{trials} trials does not fit the journal")))?;
        let quarter = Quarter::of(at);
        let charged = self.quarter_trials(&family, quarter).ok_or_else(|| {
            Error::denied(format!(
                "family {family} has no journal though {strategy} is enrolled in it; the book \
                 is inconsistent and nothing can be charged until it is reopened"
            ))
        })?;
        let would_reach = charged.checked_add(trials).ok_or_else(|| {
            Error::numeric(format!(
                "charging {trials} trial(s) to family {family} overflows its count for {quarter}"
            ))
        })?;
        if would_reach > self.quarterly_budget {
            return Err(Error::denied(format!(
                "family {family} has {charged} trial(s) charged in {quarter} and {trials} more \
                 for {strategy} would make {would_reach}, past the budget of {} per family per \
                 quarter (blueprint §20.1); nothing was charged. Cut the sweep down or wait for \
                 the next quarter — the lifetime count stays at {} either way",
                self.quarterly_budget,
                self.lifetime_trials(&family).unwrap_or(0)
            )));
        }
        let record = self.append(
            &family,
            TrialEvent::Charged {
                strategy: strategy.clone(),
                trials,
            },
            at,
        )?;
        Ok(TrialAccount {
            family,
            strategy: strategy.clone(),
            this_run: trials,
            lifetime: record.lifetime_after,
            charged_at: at,
            sequence: record.sequence,
            quarter,
            quarter_trials: record.quarter_after,
            quarterly_budget: self.quarterly_budget,
        })
    }

    /// A family's journal, oldest first.
    pub fn journal(&self, family: &StrategyFamily) -> &[TrialRecord] {
        self.journals.get(family).map_or(&[], Vec::as_slice)
    }

    pub fn families(&self) -> impl Iterator<Item = &StrategyFamily> {
        self.journals.keys()
    }

    /// Append a record, writing it to the store before the book admits it.
    ///
    /// The order matters: a record the book holds and the store does not
    /// would be a count that drops on restart, which is the failure this
    /// module exists to prevent.
    fn append(
        &mut self,
        family: &StrategyFamily,
        event: TrialEvent,
        at: Timestamp,
    ) -> Result<&TrialRecord> {
        let journal = self.journals.entry(family.clone()).or_default();
        let previous = journal.last();
        let (sequence, previous_hash) = match previous {
            None => (0, GENESIS.to_string()),
            Some(p) => (p.sequence + 1, p.hash.clone()),
        };
        let (lifetime_after, quarter_after) = TrialRecord::totals_after(previous, &event, at)
            .map_err(|e| Error::numeric(format!("family {family}: {}", e.message())))?;
        let hash = TrialRecord::digest(
            family,
            sequence,
            &event,
            lifetime_after,
            quarter_after,
            at,
            &previous_hash,
        );
        let record = TrialRecord {
            family: family.clone(),
            sequence,
            event,
            lifetime_after,
            quarter_after,
            at,
            previous: previous_hash,
            hash,
        };
        // Everything the store would refuse, the book refuses the same way,
        // so an in-memory book and a durable one accept the same journals.
        Self::check_link(previous, &record)?;
        if let Some(store) = &self.store {
            store.put_as(&record.key(), &record)?;
        }
        journal.push(record);
        journal
            .last()
            .ok_or_else(|| Error::numeric("the journal is empty immediately after an append"))
    }
}
