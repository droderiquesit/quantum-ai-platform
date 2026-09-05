//! The valuation plane's composition point: term structures and credit.
//!
//! Two engines meet here and nowhere else. `qip-market`'s
//! [`TermStructure`] holds the discounting; `qip-financial`'s
//! [`CreditProfile`] holds the default probability, the recovery and the
//! covenant state. Neither lib may reach across to the other — a lib holds
//! shared types and pure logic, and a lib that composed another lib's domain
//! would be a service in the wrong directory — so the composition is the
//! kernel's, which is the only place allowed to hold both.
//!
//! # What this closes
//!
//! `TermStructure` was built and had no production caller anywhere: no
//! deployed process ever constructed one. Credit was two `f64` fields on a
//! risk-profile struct. This register is what calls both, from the universe
//! the platform was assembled with, and the UNDERSTAND stage is what reports
//! it — including, as stage problems, every credit claim whose terms will not
//! support a valuation and every breached covenant. A register that computed
//! an expected loss and reported nothing would be the shape of control this
//! platform already has one recorded example of.
//!
//! # Money and statistics
//!
//! Rates, discount factors, survival probabilities and spreads are statistics
//! and are `f64`. Exposures and expected losses are money and are
//! [`Decimal`]. The two crossing points are in
//! [`qip_market::curve::TermStructure::present_value`] and
//! [`qip_financial::credit::CreditProfile::expected_loss`], commented at each.

use std::collections::BTreeMap;

use qip_core::error::Result;
use qip_core::{Currency, Decimal, ObjectId, Timestamp};
use qip_financial::asset_class::InstrumentType;
use qip_financial::credit::{CovenantState, CreditProfile};
use qip_financial::universe::Universe;
use qip_market::curve::{CurvePoint, TermStructure};

/// Seconds in a year, on the 365.25-day convention the platform's tenors use.
///
/// A tenor is a statistic, not money, so the `f64` division below is the right
/// arithmetic; the convention is named here because a curve built on 360 and a
/// curve built on 365.25 disagree by a basis point at the long end, and a
/// difference nobody can attribute is worse than either.
const SECONDS_PER_YEAR: f64 = 365.25 * 24.0 * 60.0 * 60.0;

/// What the platform knows about the credit in its universe, and the
/// government curve it discounts against.
///
/// Built once at assembly from the universe, because that is when the
/// instruments are known and before anything can be sized against them.
#[derive(Clone, Debug, Default)]
pub struct CreditRegister {
    /// Object id to the profile derived from its terms. `BTreeMap` because
    /// the worst-claim sentence below is decided by iterating it, and a
    /// register that reordered would produce a different sentence from the
    /// same facts on a replay.
    profiles: BTreeMap<String, CreditProfile>,
    /// Object id to why its credit terms would not support a profile. Held
    /// rather than dropped: an unquantified credit claim is the interesting
    /// state, and one that vanished would read as an instrument with no
    /// credit risk.
    refusals: BTreeMap<String, String>,
    /// One government curve per currency, from the sovereign issues in the
    /// universe.
    curves: BTreeMap<Currency, TermStructure>,
    /// Why a currency has no curve, when sovereign issues existed but would
    /// not form one — two benchmarks quoted at the same tenor, most often.
    curve_refusals: BTreeMap<Currency, String>,
    /// The claim carrying the most discounted expected loss per unit, decided
    /// at assembly while the universe is in hand. Held rather than recomputed
    /// per cycle because the universe moves into the desk after assembly and a
    /// second reading of it could disagree with the one the log recorded.
    worst_claim: Option<(String, Decimal)>,
}

impl CreditRegister {
    /// Derive the register from the universe as it stood at `as_of`.
    ///
    /// Takes no clock: `as_of` is the assembly instant the caller already
    /// holds, so a replay builds the same register from the same universe.
    pub fn from_universe(universe: &Universe, as_of: Timestamp) -> Self {
        let mut register = Self::default();
        let mut points: BTreeMap<Currency, Vec<CurvePoint>> = BTreeMap::new();

        for object in universe.iter() {
            let id = object.object_id.as_str().to_string();
            match CreditProfile::from_object(object) {
                Some(Ok(profile)) => {
                    register.profiles.insert(id.clone(), profile);
                }
                Some(Err(error)) => {
                    register.refusals.insert(id.clone(), error.to_string());
                }
                None => {}
            }
            // The discount curve is the sovereign curve, not a curve through
            // every credit claim: a curve fitted through corporate yields
            // prices the credit spread into the discount factor and then the
            // credit engine charges for it a second time.
            if object.instrument_type == InstrumentType::GovernmentBond
                && let Some(maturity) = object.maturity()
                && let Some(tenor_years) = years_between(as_of, maturity)
                && let Some(yield_to_maturity) = quoted_yield(object)
            {
                points.entry(object.currency).or_default().push(CurvePoint {
                    tenor_years,
                    value: yield_to_maturity,
                });
            }
        }

        for (currency, points) in points {
            match TermStructure::new(
                format!("{currency} sovereign"),
                currency,
                as_of,
                points.clone(),
            ) {
                Ok(curve) => {
                    register.curves.insert(currency, curve);
                }
                Err(error) => {
                    register.curve_refusals.insert(currency, error.to_string());
                }
            }
        }
        register.worst_claim = register.find_worst_claim(universe, as_of);
        register
    }

    pub fn profiles(&self) -> impl Iterator<Item = (&String, &CreditProfile)> {
        self.profiles.iter()
    }

    pub fn refusals(&self) -> impl Iterator<Item = (&String, &String)> {
        self.refusals.iter()
    }

    pub fn curve(&self, currency: Currency) -> Option<&TermStructure> {
        self.curves.get(&currency)
    }

    pub fn curve_refusals(&self) -> impl Iterator<Item = (&Currency, &String)> {
        self.curve_refusals.iter()
    }

    /// Present value of the expected credit loss on `exposure` to `object_id`
    /// over `years`, discounted on that currency's sovereign curve.
    ///
    /// This is where the two engines actually compose: the credit profile
    /// produces the loss in money, and the term structure discounts it. Both
    /// halves are exact decimal arithmetic; only the survival probability and
    /// the discount factor are `f64`.
    ///
    /// Refuses rather than substituting a flat rate when no curve exists for
    /// the currency — a discounted loss quoted off a rate nobody observed is
    /// a number that reads as a measurement.
    pub fn discounted_expected_loss(
        &self,
        object_id: &str,
        currency: Currency,
        exposure: Decimal,
        years: f64,
    ) -> Result<Decimal> {
        let profile = self.profiles.get(object_id).ok_or_else(|| {
            qip_core::error::Error::not_found(format!(
                "no credit profile for {object_id}; the register names why every credit claim \
                 it refused was refused"
            ))
        })?;
        let curve = self.curves.get(&currency).ok_or_else(|| {
            qip_core::error::Error::not_found(format!(
                "no sovereign curve in {currency} to discount the expected loss on \
                 {object_id}; supply a sovereign issue in that currency rather than \
                 discounting at a rate nobody quoted"
            ))
        })?;
        curve.present_value(profile.expected_loss(exposure, years)?, years)
    }

    /// Every breached covenant across the register, as a sentence naming the
    /// object, the obligor and the test.
    pub fn breaches(&self) -> Vec<String> {
        let mut breaches = Vec::new();
        for (id, profile) in &self.profiles {
            for covenant in profile.breached_covenants() {
                breaches.push(format!(
                    "{id} ({}): {}",
                    profile.obligor(),
                    covenant.describe()
                ));
            }
        }
        breaches
    }

    /// How many obligors are within touching distance of a covenant without
    /// having breached one.
    pub fn on_watch(&self) -> usize {
        self.profiles
            .values()
            .filter(|profile| profile.covenant_state() == Some(CovenantState::Watch))
            .count()
    }

    /// The credit claim carrying the most expected loss per unit held, and how
    /// much, discounted on its own currency's curve.
    ///
    /// Per unit of the object's own price rather than per position, because a
    /// register built at assembly has no positions yet. A ranking rather than
    /// an aggregate for the same reason: summing unheld claims would produce a
    /// number describing a portfolio nobody owns.
    ///
    /// `None` when nothing in the register can be valued — no profile, no
    /// curve in its currency, or no maturity to discount to. Reported as
    /// absent rather than as zero: the two are different, and a zero would
    /// read as a universe with no credit risk in it.
    pub fn worst_claim(&self) -> Option<&(String, Decimal)> {
        self.worst_claim.as_ref()
    }

    fn find_worst_claim(&self, universe: &Universe, as_of: Timestamp) -> Option<(String, Decimal)> {
        let mut worst: Option<(String, Decimal)> = None;
        for id in self.profiles.keys() {
            let Some(object) = universe.get(&ObjectId::from_string(id.clone())) else {
                continue;
            };
            let Some(maturity) = object.maturity() else {
                continue;
            };
            let Some(years) = years_between(as_of, maturity) else {
                continue;
            };
            let Ok(loss) =
                self.discounted_expected_loss(id, object.currency, object.price.abs(), years)
            else {
                continue;
            };
            if worst.as_ref().is_none_or(|(_, seen)| loss > *seen) {
                worst = Some((id.clone(), loss));
            }
        }
        worst
    }

    /// The one-line summary the UNDERSTAND stage reports.
    ///
    /// Empty when the universe holds no credit claim at all — a clause reading
    /// "0 credit claim(s)" on an equity-only universe is noise, and noise in a
    /// stage detail is what stops the interesting clauses being read.
    pub fn summary(&self) -> String {
        if self.profiles.is_empty() && self.refusals.is_empty() {
            return String::new();
        }
        let mut clause = format!(
            "; credit register holds {} priced claim(s) and {} unquantified",
            self.profiles.len(),
            self.refusals.len()
        );
        if !self.curves.is_empty() {
            let inverted = self
                .curves
                .values()
                .filter(|curve| curve.is_inverted())
                .count();
            clause.push_str(&format!(
                ", against {} sovereign curve(s) of which {inverted} inverted",
                self.curves.len()
            ));
        }
        let breaches = self.breaches().len();
        let watch = self.on_watch();
        if breaches > 0 || watch > 0 {
            clause.push_str(&format!(
                ", {breaches} breached covenant(s) and {watch} obligor(s) on watch"
            ));
        }
        if let Some((id, loss)) = self.worst_claim() {
            clause.push_str(&format!(
                ", worst claim {id} at {loss} of discounted expected loss per unit"
            ));
        }
        clause
    }

    /// Everything the register found that a person should be told about, as
    /// stage problems: a credit claim the platform cannot quantify, a
    /// currency whose sovereign curve would not form, and a breached
    /// covenant.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .refusals
            .iter()
            .map(|(id, reason)| format!("credit claim {id} is unquantified: {reason}"))
            .collect();
        problems.extend(
            self.curve_refusals
                .iter()
                .map(|(currency, reason)| format!("no {currency} sovereign curve: {reason}")),
        );
        problems.extend(
            self.breaches()
                .into_iter()
                .map(|breach| format!("covenant breached: {breach}")),
        );
        problems
    }
}

/// Years from `from` to `to`, or `None` when `to` is not after `from`.
///
/// `None` rather than a negative tenor: a matured claim is settled, and both
/// the curve and the credit profile refuse a negative horizon by name, so
/// handing them one would turn a stale universe into a refusal per instrument
/// per cycle.
fn years_between(from: Timestamp, to: Timestamp) -> Option<f64> {
    if to <= from {
        return None;
    }
    let years = to.since(from).as_secs_f64() / SECONDS_PER_YEAR;
    (years > 0.0 && years.is_finite()).then_some(years)
}

/// The yield the object's own terms quote, where it quotes one.
///
/// Only a bond quotes a yield to maturity, and only a finite positive-or-zero
/// one is a curve point: a zero-filled default would anchor the curve's front
/// end at zero and flat-extrapolate every shorter tenor onto it.
fn quoted_yield(object: &qip_financial::object::FinancialObject) -> Option<f64> {
    match &object.extension {
        qip_financial::extensions::Extension::Bond(details) => {
            let y = details.yield_to_maturity;
            (y.is_finite() && y != 0.0).then_some(y)
        }
        _ => None,
    }
}
