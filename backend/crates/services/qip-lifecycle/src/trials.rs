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
        at: Timestamp,
        previous: &str,
    ) -> String {
        let canonical = format!(
            "{family}|{sequence}|{}|{lifetime_after}|{}|{previous}",
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
                self.at,
                &self.previous,
            )
    }

    fn key(&self) -> String {
        format!("{JOURNAL_PREFIX}{}/{:020}", self.family, self.sequence)
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

    pub fn describe(&self) -> String {
        format!(
            "{} trial(s) this run on top of {} already charged to family {}: {} lifetime",
            self.this_run,
            self.prior(),
            self.family,
            self.lifetime
        )
    }
}

/// Every family's journal, and which strategy charges where.
#[derive(Clone, Debug)]
pub struct TrialBook {
    store: Option<Arc<dyn KeyValueStore>>,
    journals: BTreeMap<StrategyFamily, Vec<TrialRecord>>,
    members: BTreeMap<StrategyId, StrategyFamily>,
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
        }
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
        let (expected_sequence, expected_previous, prior_lifetime, prior_at) = match previous {
            None => (0, GENESIS.to_string(), 0, Timestamp::EPOCH),
            Some(p) => (p.sequence + 1, p.hash.clone(), p.lifetime_after, p.at),
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
        let expected_lifetime = match &record.event {
            TrialEvent::Opened => {
                if previous.is_some() {
                    return Err(Error::invalid(format!(
                        "trial journal {} reopens a family that already has a journal; a \
                         family opens once, at zero, and never again",
                        describe()
                    )));
                }
                0
            }
            TrialEvent::Enrolled { .. } => prior_lifetime,
            TrialEvent::Charged { trials, .. } => {
                prior_lifetime.checked_add(*trials).ok_or_else(|| {
                    Error::numeric(format!(
                        "trial journal {} overflows the lifetime count",
                        describe()
                    ))
                })?
            }
        };
        if previous.is_none() && record.event != TrialEvent::Opened {
            return Err(Error::invalid(format!(
                "trial journal {} is the first record of its family but is not an opening; \
                 the count has no origin",
                describe()
            )));
        }
        if record.lifetime_after != expected_lifetime {
            return Err(Error::invalid(format!(
                "trial journal {} carries a lifetime of {} where {expected_lifetime} follows \
                 from the record before it; the count was lowered or raised by hand",
                describe(),
                record.lifetime_after
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

    /// Charge one evaluation to the strategy's family and return the count
    /// the evaluation must be corrected against.
    ///
    /// `trials` is what the run that produced the candidate tried — the
    /// number the evidence already carries — and it must be at least one,
    /// because the candidate itself was tried.
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
        let (sequence, previous_hash, prior_lifetime) = match previous {
            None => (0, GENESIS.to_string(), 0),
            Some(p) => (p.sequence + 1, p.hash.clone(), p.lifetime_after),
        };
        let lifetime_after = match &event {
            TrialEvent::Opened => 0,
            TrialEvent::Enrolled { .. } => prior_lifetime,
            TrialEvent::Charged { trials, .. } => {
                prior_lifetime.checked_add(*trials).ok_or_else(|| {
                    Error::numeric(format!(
                        "charging {trials} to family {family} overflows its lifetime count"
                    ))
                })?
            }
        };
        let hash =
            TrialRecord::digest(family, sequence, &event, lifetime_after, at, &previous_hash);
        let record = TrialRecord {
            family: family.clone(),
            sequence,
            event,
            lifetime_after,
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
