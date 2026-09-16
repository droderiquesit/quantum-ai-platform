//! What a user contributed, when, and under which jurisdiction — blueprint
//! §43.3's `TaxLot`.
//!
//! # The fact the ledger could not state
//!
//! A [`super::StrategyBook`] holds a [`super::CashBalance`], and a balance is
//! a single number that funding, realised profit and realised loss all move.
//! So the ledger could say what a user has *at work* and could never say what
//! that user *put in*: a book holding 150 is a 100 contribution that earned 50
//! and a 200 contribution that lost 50, and nothing in the tree could tell
//! those apart. Two questions the platform is required to answer depend on the
//! distinction and neither could be answered:
//!
//! * **How much of the desk's capital is this user's own money?**
//!   [`super::UserLedger::fund`] limits what a user may place *at work* to the
//!   mandate's investable capital, which is the right limit for that question
//!   and the wrong one for authority over contributions. A user who funds
//!   their whole investable capital, loses half of it and funds again has a
//!   smaller settled total and so passes that check — while having placed
//!   more of their own money under management than the mandate they signed
//!   says the desk may manage. The settled check cannot see it, because by
//!   the time it looks the loss has already made room.
//! * **Over what horizon is this platform actually holding things?** ADR
//!   0008 makes the edge-cell architecture reversible on evidence and names
//!   the evidence in its own words: "holding-period distribution and
//!   sensitivity of realised edge to execution delay". The first half had no
//!   producer. A balance carries one [`Timestamp`], `last_entry_at`, which is
//!   when it last moved — not when any particular tranche of capital arrived —
//!   so no distribution could be computed from the books at all.
//!
//! A lot records the contribution itself: its basis, the instant it was
//! acquired, and the jurisdiction the mandate placed it under at that instant.
//! None of those is recoverable from a balance, so this is not a second claim
//! about a fact the books already hold — `contributed` and `settled` are
//! different quantities, and their difference is realised profit and loss,
//! which is a third thing the ledger could not previously state either.
//!
//! # Holding-period state is declared, never inferred
//!
//! Blueprint §43.3 asks a lot to carry "holding-period state". Whether a lot
//! is held long enough to be long-term is a determination of tax law in the
//! jurisdiction concerned, and this file is not the place anyone should
//! discover what this platform believes the law to be. So there is no default
//! threshold and none is guessed: [`HoldingPeriodRules`] starts empty, a
//! jurisdiction nobody has declared a rule for yields
//! [`HoldingPeriod::Undetermined`], and `Undetermined` is reported as its own
//! share of the distribution rather than folded into `Short`.
//!
//! That follows `ProductCatalogue` next door, which is empty by default for
//! the same reason: an empty table is the honest record of nobody having taken
//! the determination, and a table pre-filled with a plausible guess is a
//! compliance position nobody signed. Folding the undeclared case into `Short`
//! would be the worse failure of the two — it reads as a measurement, and an
//! operator looking at a distribution that is 100% short-term cannot tell
//! whether the platform is trading fast or whether nobody ever wrote the rules
//! down.
//!
//! # A lot serialises out and does not come back
//!
//! [`TaxLot`] carries money, so it follows the rule `ledger/cash.rs` states
//! and `ledger/entitlement.rs` argues: a record is evidence of what was
//! decided, never an input that decides. A lot is created in exactly one
//! place, [`super::UserLedger::fund`], after the mandate registry, the
//! eligibility registry, the investable ceiling and the contribution ceiling
//! have all admitted the funding. A `Deserialize` would be a sixth way for a
//! contribution to appear, past every one of them — and because the
//! contribution ceiling is enforced *from* the lots, a document that minted
//! lots could also move the ceiling that governs them.
//!
//! ```compile_fail
//! # use qip_capital::ledger::TaxLot;
//! // There is no `Deserialize for TaxLot`, and by the orphan rules no crate
//! // but this one can add one. A contribution minted from a document is
//! // unrepresentable rather than refused.
//! let minted: TaxLot = serde_json::from_str(
//!     r#"{"strategy":"momentum-v3","currency":"USD","basis":"999999999"}"#,
//! )
//! .expect("a document is not a contribution");
//! ```

use super::identity::Jurisdiction;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, Timestamp};
use serde::Serialize;
use std::collections::BTreeMap;

/// How long a lot has been held, against the rule its jurisdiction declares.
///
/// Ordered `Undetermined`, `Short`, `Long` so that a report built on a
/// [`BTreeMap`] lists the unknown share first. That is deliberate: the share
/// nobody has a rule for is the one an operator most needs to see, and a
/// reader who skims only the first row of a table should be reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldingPeriod {
    /// No rule has been declared for the lot's jurisdiction, so the platform
    /// does not know and says so. Never folded into [`Self::Short`].
    Undetermined,
    /// Held for less than the declared threshold.
    Short,
    /// Held for at least the declared threshold.
    Long,
}

impl HoldingPeriod {
    /// Every state, in the order a report lists them.
    pub const ALL: [Self; 3] = [Self::Undetermined, Self::Short, Self::Long];

    /// The stable token a report or a journal entry carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undetermined => "undetermined",
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

/// The operator's declaration of when a holding becomes long-term, per
/// jurisdiction.
///
/// Empty on construction and grown only by [`Self::declare`]. There is no
/// default threshold, because a default here would be this repository
/// asserting a tax position on behalf of a jurisdiction nobody consulted.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HoldingPeriodRules {
    thresholds: BTreeMap<Jurisdiction, Duration>,
}

impl HoldingPeriodRules {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the span at or beyond which a lot in `jurisdiction` is
    /// long-term.
    ///
    /// Refuses a non-positive span, and refuses to overwrite a declaration
    /// silently. A threshold of zero would make every lot long-term at the
    /// instant it was acquired, which distinguishes nothing; a second
    /// declaration under one jurisdiction is either a retry or a changed
    /// position and this type cannot tell which, so it names what to do
    /// instead rather than believing the later one.
    pub fn declare(&mut self, jurisdiction: Jurisdiction, threshold: Duration) -> Result<()> {
        if threshold <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "the long-term threshold for {jurisdiction} is {} nanoseconds; declare a \
                 positive span, because a threshold of zero or less makes every lot \
                 long-term at the instant it is acquired and distinguishes nothing",
                threshold.as_nanos()
            )));
        }
        if let Some(declared) = self.thresholds.get(&jurisdiction) {
            return Err(Error::invalid(format!(
                "a long-term threshold of {} nanoseconds is already declared for \
                 {jurisdiction}; a holding-period rule is not replaced in place — withdraw \
                 the declaration before recording a different one",
                declared.as_nanos()
            )));
        }
        self.thresholds.insert(jurisdiction, threshold);
        Ok(())
    }

    /// Withdraw a jurisdiction's declaration, so lots under it report
    /// [`HoldingPeriod::Undetermined`] again.
    ///
    /// Refuses a jurisdiction that has no declaration, so an operator who
    /// mistypes a code is told rather than silently changing nothing.
    pub fn withdraw(&mut self, jurisdiction: &Jurisdiction) -> Result<()> {
        if self.thresholds.remove(jurisdiction).is_none() {
            return Err(Error::invalid(format!(
                "no long-term threshold is declared for {jurisdiction}, so there is nothing \
                 to withdraw; check the jurisdiction code against the declarations"
            )));
        }
        Ok(())
    }

    /// The declared threshold, or `None` where nobody has taken the
    /// determination.
    pub fn threshold(&self, jurisdiction: &Jurisdiction) -> Option<Duration> {
        self.thresholds.get(jurisdiction).copied()
    }

    /// Every declaration, in jurisdiction order.
    pub fn declared(&self) -> &BTreeMap<Jurisdiction, Duration> {
        &self.thresholds
    }
}

/// One contribution of a user's capital into one strategy.
///
/// Created only by [`super::UserLedger::fund`]. Serialises and does not
/// deserialise — see this module's documentation for why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaxLot {
    /// The strategy the capital went into.
    pub strategy: StrategyId,
    pub currency: Currency,
    /// What was contributed. Money, so [`Decimal`] — never `f64`.
    pub basis: Decimal,
    /// When the contribution was made, as the caller stated it. Nothing here
    /// reads a clock.
    pub acquired_at: Timestamp,
    /// The jurisdiction the user's mandate placed them in at the instant of
    /// acquisition, copied onto the lot rather than looked up later. A
    /// mandate is never replaced in place — `MandateRegistry::register`
    /// refuses a user who already holds one — so today the two cannot
    /// disagree; the copy is here so the lot stays a statement about the past
    /// on the day a superseding mandate is recorded under a new id.
    pub jurisdiction: Jurisdiction,
}

impl TaxLot {
    /// How long this lot has been held at `at`, against `rules`.
    ///
    /// A jurisdiction with no declared rule is [`HoldingPeriod::Undetermined`]
    /// whatever the elapsed time. An `at` before [`Self::acquired_at`] is
    /// [`HoldingPeriod::Short`]: a negative age has not reached any positive
    /// threshold, and reporting a lot as long-term because a caller asked
    /// about a moment before it existed would be the more surprising of the
    /// two answers.
    pub fn holding_period(&self, at: Timestamp, rules: &HoldingPeriodRules) -> HoldingPeriod {
        let Some(threshold) = rules.threshold(&self.jurisdiction) else {
            return HoldingPeriod::Undetermined;
        };
        if at.since(self.acquired_at) >= threshold {
            HoldingPeriod::Long
        } else {
            HoldingPeriod::Short
        }
    }
}

/// Contributed basis split by holding-period state.
///
/// The evidence ADR 0008's first reversal condition names. Built by
/// [`super::UserLedger::holding_period_distribution`], which is the only way
/// to obtain one.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HoldingPeriodDistribution {
    /// The instant the distribution was taken at. Carried so a report of it
    /// cannot be read as a statement about any other moment.
    pub at: Timestamp,
    basis: BTreeMap<HoldingPeriod, Decimal>,
    lots: BTreeMap<HoldingPeriod, u64>,
}

impl HoldingPeriodDistribution {
    pub(super) fn empty(at: Timestamp) -> Self {
        Self {
            at,
            basis: BTreeMap::new(),
            lots: BTreeMap::new(),
        }
    }

    /// Add one lot's basis to its state's share, refusing an overflow rather
    /// than panicking inside an arithmetic operator.
    pub(super) fn add(&mut self, period: HoldingPeriod, basis: Decimal) -> Result<()> {
        let running = self.basis.entry(period).or_insert(Decimal::ZERO);
        let Some(next) = running.checked_add(basis) else {
            return Err(Error::numeric(format!(
                "the {} contributed basis overflows past {running} when {basis} is added; \
                 take the distribution over a narrower set of books",
                period.as_str()
            )));
        };
        *running = next;
        *self.lots.entry(period).or_insert(0) += 1;
        Ok(())
    }

    /// Contributed basis in one state; zero where no lot is in it.
    pub fn basis(&self, period: HoldingPeriod) -> Decimal {
        self.basis.get(&period).copied().unwrap_or(Decimal::ZERO)
    }

    /// How many lots are in one state.
    pub fn lots(&self, period: HoldingPeriod) -> u64 {
        self.lots.get(&period).copied().unwrap_or(0)
    }

    /// Total contributed basis across every state.
    ///
    /// Saturates rather than refusing: every component was admitted by
    /// [`Self::add`], so a total that cannot be represented is a problem with
    /// the report's breadth and not with any book.
    pub fn total_basis(&self) -> Decimal {
        self.basis
            .values()
            .copied()
            .fold(Decimal::ZERO, |a, b| a.checked_add(b).unwrap_or(a))
    }

    /// Total lots across every state.
    pub fn total_lots(&self) -> u64 {
        self.lots.values().copied().sum()
    }

    /// The share of contributed basis in one state, as a fraction of the
    /// total.
    ///
    /// **This is the one place in this file where money becomes a
    /// statistic.** A share is a ratio of two [`Decimal`] amounts, and it is
    /// reported as `f64` because it is read by a person deciding whether ADR
    /// 0008's reversal condition has been met — never by anything that sizes
    /// a position or moves a book. No caller may multiply money by it; the
    /// amounts themselves stay [`Decimal`] in [`Self::basis`].
    pub fn share(&self, period: HoldingPeriod) -> f64 {
        let total = self.total_basis();
        if total == Decimal::ZERO {
            return 0.0;
        }
        self.basis(period).to_f64() / total.to_f64()
    }
}
