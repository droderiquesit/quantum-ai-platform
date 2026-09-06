//! The credit engine: default probability, recovery, spread decomposition and
//! covenant state.
//!
//! Before this module the platform's whole credit capability was two `f64`
//! fields on [`crate::risk_profile::RiskCharacteristics`] —
//! `default_probability` and `recovery_rate` — plus
//! [`crate::extensions::CreditRating::indicative_default_probability`]. A data
//! holder, not an engine: nothing turned a one-year default probability into a
//! survival curve, nothing derived a recovery from where a claim sat in the
//! capital structure, and nothing anywhere held a covenant, so a borrower could
//! breach its leverage test without a single line of this platform being able
//! to say so.
//!
//! # What is money and what is a statistic
//!
//! A default probability, a hazard rate, a survival probability and a spread
//! are **statistics** and stay `f64`. An exposure and an expected loss are
//! **money** and are [`Decimal`]. There is exactly one crossing point, in
//! [`CreditProfile::expected_loss`], and it is commented where it happens.
//!
//! # What this deliberately does not do
//!
//! It does not price an instrument. Discounting needs a curve, and a curve
//! lives in `qip-market`; a lib may not reach across to another lib's domain
//! to compose the two, so the composition happens in `qip-kernel`, which is
//! the only place allowed to hold both.

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::extensions::{CreditRating, Extension, Seniority};
use crate::object::FinancialObject;
use crate::risk_profile::SpreadDecomposition;

/// Headroom below which a covenant is on watch rather than merely met.
///
/// Expressed as a fraction of the threshold. A borrower at 9.9 turns of
/// leverage against a 10.0 covenant is not comfortably compliant, and a state
/// with only two arms would report it identically to one at 2.0 turns.
const WATCH_HEADROOM: f64 = 0.10;

/// Which way a covenant test runs.
///
/// Stated as an enum rather than a `bool` on [`Covenant`] because "is this a
/// ceiling?" read from a call site tells the reader nothing, and a covenant
/// tested in the wrong direction reports a breached borrower as compliant —
/// the single worst failure this module can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CovenantKind {
    /// The observed value must stay at or below the threshold. Leverage,
    /// loan-to-value, capital expenditure.
    Ceiling,
    /// The observed value must stay at or above the threshold. Interest
    /// coverage, minimum liquidity, minimum net worth.
    Floor,
}

impl CovenantKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ceiling => "ceiling",
            Self::Floor => "floor",
        }
    }
}

/// Whether a covenant is a term of the credit agreement or a level this
/// platform supplied because the agreement's own was not reported.
///
/// The distinction is in the type, not in a comment, because it decides what
/// the operator is told. A borrower above a level nobody agreed to has not
/// breached anything; reporting it in the sentence a real breach uses is the
/// inverse of the failure [`CreditProfile::covenant_state`] returns an
/// `Option` to avoid. That method exists so "nothing tested" cannot read as
/// "tested and passed"; without this enum, "nothing tested" read as **tested
/// and failed**, which is worse, because a breach is escalated and an absence
/// is investigated.
///
/// This has happened here: every non-covenant-lite loan was given a six-turn
/// leverage ceiling manufactured for it, and a borrower at 6.4 turns was
/// reported as `"net_debt_to_ebitda (ceiling 6) observed at 6.4: breached"` —
/// textually indistinguishable from a breach of a covenant the credit
/// agreement actually contains.
///
/// Modelled on [`DefaultPrior`], for the same reason: a platform assumption
/// and a counterparty's own term are not the same claim, and a register that
/// reported only the number would let the weaker one be read as the stronger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CovenantSource {
    /// A test the credit agreement contains, at the level the agreement sets.
    /// Only these can be breached.
    Agreement,
    /// A level this platform supplied because none was reported with the
    /// instrument. Exceeding one is a finding about the borrower, not a
    /// breach of anything.
    Assumed,
}

impl CovenantSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agreement => "agreement",
            Self::Assumed => "assumed",
        }
    }

    /// True only for a term somebody actually agreed to.
    pub fn is_contractual(self) -> bool {
        matches!(self, Self::Agreement)
    }
}

/// Where a covenant stands: met with room, met without room, or breached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CovenantState {
    Compliant,
    /// Met, but within [`WATCH_HEADROOM`] of the threshold.
    Watch,
    Breached,
}

impl CovenantState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compliant => "compliant",
            Self::Watch => "watch",
            Self::Breached => "breached",
        }
    }

    pub fn is_breached(self) -> bool {
        matches!(self, Self::Breached)
    }
}

/// One covenant test, with the level the borrower most recently reported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Covenant {
    pub name: String,
    pub kind: CovenantKind,
    pub threshold: f64,
    /// The borrower's most recently reported level for this test.
    pub observed: f64,
    /// Whether the threshold above is the agreement's or this platform's.
    pub source: CovenantSource,
}

impl Covenant {
    /// A test the credit agreement contains, at the level it sets.
    ///
    /// There is deliberately no `new`. A covenant's provenance decides whether
    /// exceeding it is escalated as a breach or investigated as a finding, and
    /// a constructor that did not ask would hand every caller a provenance it
    /// never chose — the shape of defect this enum was added to close. Naming
    /// the two constructors makes the choice structural rather than a fifth
    /// positional argument a reader has to decode.
    pub fn agreed(
        name: impl Into<String>,
        kind: CovenantKind,
        threshold: f64,
        observed: f64,
    ) -> Result<Self> {
        Self::build(name, kind, threshold, observed, CovenantSource::Agreement)
    }

    /// A test at a level this platform supplied because the instrument
    /// reported none.
    ///
    /// Exceeding one is not a breach and [`Self::describe`] says so in the
    /// sentence an operator reads, not only in this doc.
    pub fn assumed(
        name: impl Into<String>,
        kind: CovenantKind,
        threshold: f64,
        observed: f64,
    ) -> Result<Self> {
        Self::build(name, kind, threshold, observed, CovenantSource::Assumed)
    }

    /// Refuses rather than clamps: a non-finite threshold or observation is a
    /// reporting error upstream, and a covenant carrying one would test
    /// `NaN <= threshold`, which is `false` in the ceiling direction and
    /// `false` in the floor direction — so the same missing number reads as a
    /// breach either way, and an operator would be paged on arithmetic rather
    /// than on a borrower.
    fn build(
        name: impl Into<String>,
        kind: CovenantKind,
        threshold: f64,
        observed: f64,
        source: CovenantSource,
    ) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a covenant needs a name; name the test as the credit agreement names it",
            ));
        }
        if !threshold.is_finite() {
            return Err(Error::invalid(format!(
                "covenant {name} has a threshold of {threshold}, which is not a finite level; \
                 supply the level the agreement sets"
            )));
        }
        if !observed.is_finite() {
            return Err(Error::invalid(format!(
                "covenant {name} has an observed level of {observed}, which is not a finite \
                 number; omit the covenant until the borrower reports rather than testing \
                 against a placeholder"
            )));
        }
        Ok(Self {
            name,
            kind,
            threshold,
            observed,
            source,
        })
    }

    /// Signed room to the threshold, as a fraction of the threshold.
    ///
    /// Positive is compliant in both directions, which is the point: a caller
    /// comparing headroom across a leverage ceiling and a coverage floor is
    /// comparing the same quantity. `None` when the threshold is zero, because
    /// a fraction of zero is not a measure of anything — the state is still
    /// determinable and [`Covenant::state`] still returns it.
    pub fn headroom(&self) -> Option<f64> {
        if self.threshold == 0.0 {
            return None;
        }
        let room = match self.kind {
            CovenantKind::Ceiling => self.threshold - self.observed,
            CovenantKind::Floor => self.observed - self.threshold,
        };
        Some(room / self.threshold.abs())
    }

    /// Where the observation sits relative to the threshold.
    ///
    /// Arithmetic, and so the same question for either
    /// [`CovenantSource`]. What differs is what the answer *means*, which is
    /// [`Self::is_breached`]'s and [`Self::describe`]'s business: an
    /// observation past an assumed level is `Breached` here and is not a
    /// breach of anything.
    pub fn state(&self) -> CovenantState {
        let breached = match self.kind {
            CovenantKind::Ceiling => self.observed > self.threshold,
            CovenantKind::Floor => self.observed < self.threshold,
        };
        if breached {
            return CovenantState::Breached;
        }
        match self.headroom() {
            Some(room) if room < WATCH_HEADROOM => CovenantState::Watch,
            _ => CovenantState::Compliant,
        }
    }

    /// True only when a term somebody agreed to has been broken.
    ///
    /// The pairing with [`Self::state`] is the whole point of
    /// [`CovenantSource`]: an assumed level that has been exceeded returns
    /// `CovenantState::Breached` from `state` and `false` from here, because
    /// only one of the two is a fact about the credit agreement.
    pub fn is_breached(&self) -> bool {
        self.source.is_contractual() && self.state().is_breached()
    }

    /// True when an assumed level has been exceeded — a finding about the
    /// borrower, and never a breach.
    pub fn exceeds_assumed_level(&self) -> bool {
        !self.source.is_contractual() && self.state().is_breached()
    }

    /// The refusal-shaped sentence a caller puts in front of a person.
    ///
    /// The two arms do not share a template on purpose. A shared one would
    /// differ by a single word, and the reader who most needs the distinction
    /// is the one scanning a list of stage problems at speed. An assumed test
    /// therefore says, in the same sentence, that nobody agreed the level and
    /// that exceeding it is not a breach.
    pub fn describe(&self) -> String {
        match self.source {
            CovenantSource::Agreement => format!(
                "{} (agreement {} {}) observed at {}: {}",
                self.name,
                self.kind.label(),
                self.threshold,
                self.observed,
                self.state().label()
            ),
            CovenantSource::Assumed => format!(
                "{} (no agreement level supplied; tested against this platform's assumed {} {}) \
                 observed at {}: {}",
                self.name,
                self.kind.label(),
                self.threshold,
                self.observed,
                self.assumed_verdict()
            ),
        }
    }

    /// The verdict word for an assumed test. Never `breached`: that word is
    /// reserved for a term of a credit agreement, and the sentence spends the
    /// extra clause saying so rather than leaving the reader to infer it.
    fn assumed_verdict(&self) -> &'static str {
        match self.state() {
            CovenantState::Breached => "past the assumed level, which is not a covenant breach",
            CovenantState::Watch => "near the assumed level",
            CovenantState::Compliant => "within the assumed level",
        }
    }
}

/// Where a profile's default probability came from.
///
/// Recorded because a rating-implied through-the-cycle prior and an
/// issuer-specific estimate are not the same claim, and a register that
/// reported only the number would let the weaker one be read as the stronger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultPrior {
    /// Stated directly by whoever estimated it.
    Stated,
    /// Derived from an agency rating's through-the-cycle default rate.
    Rated,
}

impl DefaultPrior {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stated => "stated",
            Self::Rated => "rated",
        }
    }
}

/// Through-the-cycle recovery prior by position in the capital structure.
///
/// Priors, not measurements: they are used only where nobody has supplied a
/// recovery, and [`CreditProfile::recovery_rate`] reports whichever was used
/// so a caller can tell a prior from an estimate.
pub fn indicative_recovery_rate(seniority: Seniority) -> f64 {
    match seniority {
        Seniority::SecuredFirstLien => 0.70,
        Seniority::SecuredSecondLien => 0.45,
        Seniority::SeniorUnsecured => 0.40,
        Seniority::Subordinated => 0.25,
        Seniority::JuniorSubordinated => 0.15,
        // An equity claim recovers after every creditor is made whole, which
        // in a default it is not. Zero is the modelling assumption, stated.
        Seniority::Equity => 0.0,
    }
}

/// An obligor's credit standing: how likely it is to default, what is
/// recovered when it does, and where its covenants stand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreditProfile {
    obligor: String,
    seniority: Seniority,
    rating: Option<CreditRating>,
    prior: DefaultPrior,
    one_year_default_probability: f64,
    recovery_rate: f64,
    /// Keyed by name so iteration order reaches output identically on a
    /// replay. A covenant register that reordered would produce a different
    /// "worst covenant" sentence from the same facts.
    covenants: BTreeMap<String, Covenant>,
}

impl CreditProfile {
    /// Build a profile from a stated one-year default probability.
    ///
    /// # Refusals
    ///
    /// * An empty obligor. A credit fact attributable to nobody is not a
    ///   credit fact.
    /// * A default probability outside `[0, 1]`, or non-finite. Refused rather
    ///   than clamped, because a probability of 1.4 is an upstream unit error
    ///   — most often a percentage fed in as a fraction — and clamping it to 1
    ///   turns a caller bug into a plausible-looking certainty of default.
    /// * A recovery rate outside `[0, 1]`, or non-finite. A recovery above one
    ///   claims a defaulted claim pays more than it owed.
    pub fn new(
        obligor: impl Into<String>,
        seniority: Seniority,
        one_year_default_probability: f64,
        recovery_rate: f64,
    ) -> Result<Self> {
        let obligor = obligor.into();
        if obligor.trim().is_empty() {
            return Err(Error::invalid(
                "a credit profile needs an obligor; name the issuer, borrower or reference \
                 entity the claim is on",
            ));
        }
        Self::check_unit_interval(
            "default probability",
            one_year_default_probability,
            &obligor,
        )?;
        Self::check_unit_interval("recovery rate", recovery_rate, &obligor)?;
        Ok(Self {
            obligor,
            seniority,
            rating: None,
            prior: DefaultPrior::Stated,
            one_year_default_probability,
            recovery_rate,
            covenants: BTreeMap::new(),
        })
    }

    /// Build a profile from an agency rating, using the rating's
    /// through-the-cycle default rate and the seniority's recovery prior.
    pub fn from_rating(
        obligor: impl Into<String>,
        seniority: Seniority,
        rating: CreditRating,
    ) -> Result<Self> {
        let mut profile = Self::new(
            obligor,
            seniority,
            rating.indicative_default_probability(),
            indicative_recovery_rate(seniority),
        )?;
        profile.rating = Some(rating);
        profile.prior = DefaultPrior::Rated;
        Ok(profile)
    }

    fn check_unit_interval(what: &str, value: f64, obligor: &str) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "the {what} of {obligor} is {value}, which is not a finite number; supply the \
                 estimate or omit the obligor from the register"
            )));
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::invalid(format!(
                "the {what} of {obligor} is {value}, outside [0, 1]; supply it as a fraction \
                 rather than a percentage"
            )));
        }
        Ok(())
    }

    /// Record a covenant.
    ///
    /// Refuses a second covenant under a name already registered, naming it.
    /// Overwriting would let a stale test silently replace a live one, and the
    /// register would then report a state nobody could reproduce from the
    /// credit agreement.
    pub fn with_covenant(mut self, covenant: Covenant) -> Result<Self> {
        if self.covenants.contains_key(&covenant.name) {
            return Err(Error::invalid(format!(
                "covenant {} is already registered for {}; reconcile the two tests upstream \
                 rather than letting the later one replace the earlier",
                covenant.name, self.obligor
            )));
        }
        self.covenants.insert(covenant.name.clone(), covenant);
        Ok(self)
    }

    pub fn obligor(&self) -> &str {
        &self.obligor
    }

    pub fn seniority(&self) -> Seniority {
        self.seniority
    }

    pub fn rating(&self) -> Option<CreditRating> {
        self.rating
    }

    pub fn prior(&self) -> DefaultPrior {
        self.prior
    }

    pub fn one_year_default_probability(&self) -> f64 {
        self.one_year_default_probability
    }

    pub fn recovery_rate(&self) -> f64 {
        self.recovery_rate
    }

    pub fn covenants(&self) -> impl Iterator<Item = &Covenant> {
        self.covenants.values()
    }

    /// Fraction of notional lost when the obligor defaults.
    pub fn loss_given_default(&self) -> f64 {
        1.0 - self.recovery_rate
    }

    /// The constant hazard rate implied by the one-year default probability.
    ///
    /// `h = -ln(1 - p₁)`, the intensity of a Poisson default process whose
    /// first-year probability is `p₁`.
    ///
    /// Refuses an obligor already certain to default: `-ln(0)` is infinite,
    /// and every survival probability, spread and expected loss computed from
    /// an infinite hazard is a number nobody can act on. Book the recovery
    /// instead.
    pub fn hazard_rate(&self) -> Result<f64> {
        if self.one_year_default_probability >= 1.0 {
            return Err(Error::numeric(format!(
                "{} has a one-year default probability of 1, so it has no hazard rate; book the \
                 recovery on the claim rather than projecting a survival curve",
                self.obligor
            )));
        }
        Ok(-(1.0 - self.one_year_default_probability).ln())
    }

    /// Probability the obligor survives `years` from now.
    ///
    /// Refuses a negative or non-finite horizon: survival backwards in time is
    /// not a quantity, and the exponential would happily return a number
    /// greater than one for it.
    pub fn survival_probability(&self, years: f64) -> Result<f64> {
        self.check_horizon(years)?;
        Ok((-self.hazard_rate()? * years).exp())
    }

    /// Probability the obligor defaults at some point within `years`.
    pub fn cumulative_default_probability(&self, years: f64) -> Result<f64> {
        Ok(1.0 - self.survival_probability(years)?)
    }

    fn check_horizon(&self, years: f64) -> Result<()> {
        if !years.is_finite() {
            return Err(Error::invalid(format!(
                "the credit horizon for {} is {years} years, which is not a finite horizon; \
                 supply the claim's own time to maturity",
                self.obligor
            )));
        }
        if years < 0.0 {
            return Err(Error::invalid(format!(
                "the credit horizon for {} is {years} years; a claim already matured is \
                 settled, not projected",
                self.obligor
            )));
        }
        Ok(())
    }

    /// The credit-risk component of a spread over `years`, in basis points.
    ///
    /// `s = LGD × (−ln S(t)) / t`, the continuously-compounded spread that
    /// makes a risky claim's expected value equal the risk-free one.
    ///
    /// This is a statistic and stays `f64`. It is the credit component only:
    /// a quoted market spread also pays for liquidity and term premia this
    /// identity does not separate out, so it is not comparable to a screen
    /// spread without saying so.
    ///
    /// Refuses a zero horizon, where the arithmetic divides by zero and the
    /// answer would be infinite rather than large.
    pub fn credit_spread_bps(&self, years: f64) -> Result<f64> {
        self.check_horizon(years)?;
        if years == 0.0 {
            return Err(Error::invalid(format!(
                "a spread over zero years is undefined for {}; supply the claim's time to \
                 maturity",
                self.obligor
            )));
        }
        Ok(self.hazard_rate()? * self.loss_given_default() * 10_000.0)
    }

    /// The named components of the credit-spread identity over `years`, in
    /// exact decimal arithmetic.
    ///
    /// The term-aware sibling of
    /// [`crate::risk_profile::RiskCharacteristics::spread_decomposition`],
    /// which decomposes a single period. Returns the same type so a caller
    /// reporting either reports one shape.
    pub fn spread_decomposition(&self, years: f64) -> Result<SpreadDecomposition> {
        let cumulative = self.cumulative_default_probability(years)?;
        let default_probability = Decimal::from_f64(cumulative).ok_or_else(|| {
            Error::numeric(format!(
                "the cumulative default probability {cumulative} of {} over {years} years is \
                 not representable",
                self.obligor
            ))
        })?;
        let recovery = Decimal::from_f64(self.recovery_rate).ok_or_else(|| {
            Error::numeric(format!(
                "the recovery rate {} of {} is not representable",
                self.recovery_rate, self.obligor
            ))
        })?;
        let loss_given_default = Decimal::ONE
            .checked_sub(recovery)
            .ok_or_else(|| Error::numeric("loss given default overflowed"))?;
        let spread = default_probability
            .checked_mul(loss_given_default)
            .ok_or_else(|| Error::numeric("the spread computation overflowed"))?;
        Ok(SpreadDecomposition {
            default_probability,
            loss_given_default,
            spread,
        })
    }

    /// Expected credit loss on `exposure` over `years`.
    ///
    /// **This is the crossing point between the statistical half of this
    /// module and the money half.** Everything above is `f64`; `exposure` and
    /// the answer are money and are [`Decimal`]. The two probabilities cross
    /// once, here, and the multiplication that produces the loss is exact — so
    /// the register's aggregate is the sum of its parts to the cent rather
    /// than to within a rounding nobody can attribute.
    pub fn expected_loss(&self, exposure: Decimal, years: f64) -> Result<Decimal> {
        if exposure.is_negative() {
            return Err(Error::invalid(format!(
                "the exposure to {} is {exposure}; an expected credit loss is measured on a \
                 positive claim, so supply the absolute exposure and record the direction \
                 separately",
                self.obligor
            )));
        }
        let decomposition = self.spread_decomposition(years)?;
        exposure.checked_mul(decomposition.spread).ok_or_else(|| {
            Error::numeric(format!(
                "the expected loss on {exposure} of {} overflowed",
                self.obligor
            ))
        })
    }

    /// The worst state across the obligor's **agreement** covenants, or `None`
    /// when it has none.
    ///
    /// `None` rather than `Compliant` on an empty register, deliberately. A
    /// covenant state of "compliant" asserts that tests were run and passed;
    /// an obligor nobody wrote a covenant for has had nothing tested, and
    /// reporting the two identically is precisely the shape of control that
    /// reads as protection and is not.
    ///
    /// An assumed test is excluded for exactly that argument, one step on. A
    /// level this platform supplied is not a test anybody ran, so counting it
    /// here would answer `Some(Compliant)` for every borrower whose agreement
    /// nobody captured — reinstating, through the back door, the reading this
    /// method returns an `Option` to prevent.
    /// [`Self::assumed_tests_exceeded`] is where those are reported instead.
    pub fn covenant_state(&self) -> Option<CovenantState> {
        self.covenants
            .values()
            .filter(|covenant| covenant.source.is_contractual())
            .map(Covenant::state)
            .max()
    }

    /// Every breached agreement covenant, in name order.
    ///
    /// An assumed level that has been exceeded is deliberately absent: it is
    /// not a breach, and the caller that reports breaches reports it under
    /// [`Self::assumed_tests_exceeded`] with its own sentence.
    pub fn breached_covenants(&self) -> Vec<&Covenant> {
        self.covenants
            .values()
            .filter(|covenant| covenant.is_breached())
            .collect()
    }

    /// Every assumed test the borrower is past, in name order.
    ///
    /// Reported rather than dropped. `net_debt_to_ebitda` is a number the
    /// borrower actually filed, and a platform that stayed silent about nine
    /// turns of leverage because nobody captured the agreement would have
    /// swapped a false breach for a missing control.
    pub fn assumed_tests_exceeded(&self) -> Vec<&Covenant> {
        self.covenants
            .values()
            .filter(|covenant| covenant.exceeds_assumed_level())
            .collect()
    }

    /// Derive a profile from an instrument's own credit terms.
    ///
    /// `None` means the instrument carries no credit claim — an equity, a
    /// future, a cash balance — and is not a failure. `Some(Err(_))` means the
    /// instrument *is* a credit claim and its terms will not support one, and
    /// the message names what to supply; the caller reports that rather than
    /// substituting a prior, because a default probability nobody estimated is
    /// the number an expected-loss control would silently read as zero.
    pub fn from_object(object: &FinancialObject) -> Option<Result<Self>> {
        match &object.extension {
            Extension::Bond(details) => Some(Self::from_rated_claim(
                &details.issuer,
                details.seniority,
                details.credit_rating,
                object,
            )),
            Extension::StructuredCredit(details) => Some(Self::from_rated_claim(
                &format!("{} {}", details.deal_name, details.tranche),
                // A tranche's position in its own deal is the subordination
                // below it; the object model carries no seniority for one, and
                // a tranche's recovery is a deal-level assumption rather than a
                // capital-structure prior. Subordinated is the conservative
                // reading and is stated rather than assumed silently.
                Seniority::Subordinated,
                details.credit_rating,
                object,
            )),
            Extension::Loan(details) => {
                // A loan carries no rating, so the stated estimate is the only
                // source. Its leverage test is a covenant the platform can
                // actually hold, and a covenant-lite loan is recorded as
                // having no test rather than as passing one.
                let stated = object.risk.default_probability;
                let mut profile = match Self::new(
                    details.borrower.clone(),
                    details.seniority,
                    stated,
                    indicative_recovery_rate(details.seniority),
                ) {
                    Ok(profile) => profile,
                    Err(error) => return Some(Err(error)),
                };
                if stated <= 0.0 {
                    return Some(Err(Error::invalid(format!(
                        "the loan to {} states no default probability, and a loan is a credit \
                         claim; supply risk.default_probability or leave the borrower out of \
                         the credit register rather than booking it as riskless",
                        details.borrower
                    ))));
                }
                if !details.covenant_lite {
                    // The agreement's own ceiling where it was captured, and
                    // this platform's assumption where it was not — labelled
                    // as which, because the two produce different sentences
                    // and only one of them can be breached.
                    let covenant = match details.leverage_covenant {
                        Some(agreed) => Covenant::agreed(
                            "net_debt_to_ebitda",
                            CovenantKind::Ceiling,
                            agreed,
                            details.net_debt_to_ebitda,
                        ),
                        None => Covenant::assumed(
                            "net_debt_to_ebitda",
                            CovenantKind::Ceiling,
                            LOAN_LEVERAGE_COVENANT,
                            details.net_debt_to_ebitda,
                        ),
                    };
                    let covenant = match covenant {
                        Ok(covenant) => covenant,
                        Err(error) => return Some(Err(error)),
                    };
                    profile = match profile.with_covenant(covenant) {
                        Ok(profile) => profile,
                        Err(error) => return Some(Err(error)),
                    };
                }
                Some(Ok(profile))
            }
            Extension::CreditDerivative(details) => {
                let stated = object.risk.default_probability;
                if stated <= 0.0 {
                    return Some(Err(Error::invalid(format!(
                        "the credit derivative on {} states no default probability; supply \
                         risk.default_probability or leave the reference entity out of the \
                         credit register rather than booking protection as free",
                        details.reference_entity
                    ))));
                }
                // The contract's own recovery assumption is what it settles
                // against, so it is used in preference to the seniority prior.
                Some(Self::new(
                    details.reference_entity.clone(),
                    details.seniority,
                    stated,
                    details.recovery_assumption,
                ))
            }
            _ => None,
        }
    }

    fn from_rated_claim(
        obligor: &str,
        seniority: Seniority,
        rating: Option<CreditRating>,
        object: &FinancialObject,
    ) -> Result<Self> {
        if let Some(rating) = rating {
            return Self::from_rating(obligor, seniority, rating);
        }
        let stated = object.risk.default_probability;
        if stated <= 0.0 {
            return Err(Error::invalid(format!(
                "{obligor} carries neither a credit rating nor a stated default probability, so \
                 its credit risk is unquantified; supply a rating or \
                 risk.default_probability rather than valuing the claim as riskless"
            )));
        }
        Self::new(
            obligor,
            seniority,
            stated,
            indicative_recovery_rate(seniority),
        )
    }
}

/// The leverage ceiling a loan's `net_debt_to_ebitda` is tested against where
/// [`crate::extensions::LoanDetails::leverage_covenant`] is absent.
///
/// Six turns is the level above which the US and European regulators' leveraged
/// lending guidance asks a lender to justify the credit. It is a default, not a
/// contract — and that sentence used to live only here, in the source, while
/// the sentence the operator read said `"net_debt_to_ebitda (ceiling 6)
/// observed at 6.4: breached"`, which is what a real covenant breach looks
/// like. A disclosure only the implementer sees is not a disclosure.
///
/// The default is kept rather than deleted because deleting it would make the
/// platform silent about a leverage number the borrower actually filed, and a
/// missing control is not an improvement on a mislabelled one. It earns its
/// place by being labelled: every covenant this constant produces is a
/// [`CovenantSource::Assumed`] one, which cannot be breached, is excluded from
/// [`CreditProfile::covenant_state`], and renders a sentence that names both
/// the absence of an agreed level and the fact that being past it is not a
/// breach.
pub const LOAN_LEVERAGE_COVENANT: f64 = 6.0;
