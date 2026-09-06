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

use qip_core::error::{Error, Result};
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

/// The magnitude at or above which a quoted yield is a unit error rather than
/// a yield.
///
/// A yield on this platform is a fraction: `0.0435` is 4.35%. A value of
/// `4.35` therefore claims 435%, which no sovereign benchmark has ever been
/// quoted at — a claim that distressed trades on price, not on yield. The
/// overwhelmingly likelier cause is a vendor publishing the field in percent,
/// which is the same unit error
/// [`qip_financial::credit::CreditProfile::new`] already refuses for a default
/// probability with the same sentence.
///
/// One hundred percent, not something tighter, so the gate refuses the error
/// and admits every rate a curve is actually built from: deeply negative
/// sovereign yields are real and are accepted, and so is a distressed
/// sovereign at 90%. What it catches is a decimal point in the wrong place.
/// Left unrefused it does not produce a wrong number — it produces
/// `exp(-4.35 × 10)`, a discount factor that rounds to zero at the scale money
/// is held at, and every credit claim in the universe reported as carrying
/// exactly zero discounted expected loss.
const IMPLAUSIBLE_YIELD_MAGNITUDE: f64 = 1.0;

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
    /// Object id to why a sovereign issue was kept out of its currency's
    /// curve. Held rather than dropped for the same reason `refusals` is: a
    /// benchmark that silently vanished takes the curve's shape with it, and
    /// the remaining points still fit a curve that answers every query.
    excluded_curve_points: BTreeMap<String, String>,
    /// Object id to why a profiled claim could not be discounted at all.
    ///
    /// [`Self::find_worst_claim`] used to swallow this error and move on, so a
    /// universe in which nothing could be valued was indistinguishable from
    /// one in which nothing was worth much. Recorded at the seam where the
    /// failure is known, and reported through [`Self::problems`].
    valuation_refusals: BTreeMap<String, String>,
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
            {
                match quoted_yield(object) {
                    Some(Ok(yield_to_maturity)) => {
                        points.entry(object.currency).or_default().push(CurvePoint {
                            tenor_years,
                            value: yield_to_maturity,
                        });
                    }
                    Some(Err(error)) => {
                        register
                            .excluded_curve_points
                            .insert(id.clone(), error.to_string());
                    }
                    None => {}
                }
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
        let (worst, unvalued) = register.find_worst_claim(universe, as_of);
        register.worst_claim = worst;
        register.valuation_refusals = unvalued;
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
    ///
    /// # Why this guards the curve's domain and the curve does not
    ///
    /// [`TermStructure::rate_at`] flat-extrapolates outside its quoted tenors
    /// and that is right for what it is: a market-data primitive read at 50y
    /// off a 2y-30y fit is a marginal extension of an observed shape, and the
    /// macro path's slope and inversion readings depend on it. It is not right
    /// *here*. A credit claim's tenor is the claim's own fact, not a curve
    /// reading, and the two arrive from different objects: a universe holding
    /// one 10y benchmark answered a 40y claim with the 10y rate, which is not
    /// an extension of a shape but an invention carrying an observation's
    /// provenance. This is the composition point — the only place that knows
    /// both the claim's maturity and the curve's range — so the guard belongs
    /// here, exactly as
    /// [`qip_market::volatility::VolatilitySurface::vol_at`] refuses an expiry
    /// outside its own grid rather than asking the interpolator to.
    pub fn discounted_expected_loss(
        &self,
        object_id: &str,
        currency: Currency,
        exposure: Decimal,
        years: f64,
    ) -> Result<Decimal> {
        let profile = self.profiles.get(object_id).ok_or_else(|| {
            Error::not_found(format!(
                "no credit profile for {object_id}; the register names why every credit claim \
                 it refused was refused"
            ))
        })?;
        let curve = self.curves.get(&currency).ok_or_else(|| {
            Error::not_found(format!(
                "no sovereign curve in {currency} to discount the expected loss on \
                 {object_id}; supply a sovereign issue in that currency rather than \
                 discounting at a rate nobody quoted"
            ))
        })?;
        let (shortest, longest) = curve.tenor_range();
        if years < shortest || years > longest {
            return Err(Error::invalid(format!(
                "the claim on {object_id} runs {years} years, outside the {shortest}..{longest} \
                 years the {currency} sovereign curve is quoted at; the curve flat-extrapolates \
                 beyond its own points, so discounting there would price the claim at a rate \
                 nobody quoted — supply a sovereign issue at that tenor rather than reading \
                 past the last one"
            )));
        }
        curve.present_value(profile.expected_loss(exposure, years)?, years)
    }

    pub fn excluded_curve_points(&self) -> impl Iterator<Item = (&String, &String)> {
        self.excluded_curve_points.iter()
    }

    pub fn valuation_refusals(&self) -> impl Iterator<Item = (&String, &String)> {
        self.valuation_refusals.iter()
    }

    /// Every breached **agreement** covenant across the register, as a
    /// sentence naming the object, the obligor and the test.
    ///
    /// A borrower past a level this platform assumed is not here; it is in
    /// [`Self::leverage_above_assumed_levels`], under its own sentence.
    /// Merging the two is what produced
    /// `"covenant breached: obj-X (Borrower): net_debt_to_ebitda (ceiling 6)
    /// observed at 6.4: breached"` for a loan whose agreement nobody had
    /// captured — an operator escalating a contract term that did not exist.
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

    /// Every borrower past a level this platform assumed, as a sentence that
    /// says so.
    pub fn leverage_above_assumed_levels(&self) -> Vec<String> {
        let mut findings = Vec::new();
        for (id, profile) in &self.profiles {
            for covenant in profile.assumed_tests_exceeded() {
                findings.push(format!(
                    "{id} ({}): {}",
                    profile.obligor(),
                    covenant.describe()
                ));
            }
        }
        findings
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

    /// The worst claim, and why every claim that could not be valued could
    /// not be.
    ///
    /// The second half is not bookkeeping. This loop used to drop the refusal
    /// on the floor, so a universe whose curve was unusable produced a worst
    /// claim of `None` or — worse, once the discount factor underflowed — a
    /// ranking decided by which claim's arithmetic collapsed last, with
    /// nothing anywhere saying why.
    fn find_worst_claim(
        &self,
        universe: &Universe,
        as_of: Timestamp,
    ) -> (Option<(String, Decimal)>, BTreeMap<String, String>) {
        let mut worst: Option<(String, Decimal)> = None;
        let mut unvalued: BTreeMap<String, String> = BTreeMap::new();
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
            match self.discounted_expected_loss(id, object.currency, object.price.abs(), years) {
                Ok(loss) => {
                    if worst.as_ref().is_none_or(|(_, seen)| loss > *seen) {
                        worst = Some((id.clone(), loss));
                    }
                }
                Err(error) => {
                    unvalued.insert(id.clone(), error.to_string());
                }
            }
        }
        (worst, unvalued)
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
        // Counted separately from the breaches above and never folded into
        // them: one is a term somebody agreed to, the other is this
        // platform's assumption, and a single total would let the second be
        // read as the first — which is the defect this split closes.
        let assumed = self.leverage_above_assumed_levels().len();
        if assumed > 0 {
            clause.push_str(&format!(
                ", {assumed} borrower(s) past an assumed level with no covenant supplied"
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
    /// currency whose sovereign curve would not form, a benchmark kept out of
    /// a curve, a claim nothing could discount, a breached covenant, and —
    /// under its own leading words — a borrower past a level this platform
    /// assumed.
    ///
    /// The last two are separate lines rather than one, because the leading
    /// words are what an operator scanning a list acts on. `covenant
    /// breached:` means a term of a credit agreement has been broken and
    /// begins an escalation; `leverage above an assumed level` means the
    /// agreement was never captured and begins a data request. They were the
    /// same sentence, and the second was being read as the first.
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
        problems.extend(self.excluded_curve_points.iter().map(|(id, reason)| {
            format!("sovereign issue {id} is not on its currency's curve: {reason}")
        }));
        problems.extend(
            self.valuation_refusals
                .iter()
                .map(|(id, reason)| format!("credit claim {id} could not be discounted: {reason}")),
        );
        problems.extend(
            self.breaches()
                .into_iter()
                .map(|breach| format!("covenant breached: {breach}")),
        );
        problems.extend(
            self.leverage_above_assumed_levels()
                .into_iter()
                .map(|finding| format!("leverage above an assumed level: {finding}")),
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
/// `None` means the object quotes no yield at all — only a bond does — and is
/// not a failure. `Some(Err(_))` means it quotes one the curve will not take,
/// and the caller records it against the object rather than dropping the
/// benchmark, because a point that silently vanished takes the curve's shape
/// with it while the remaining points still fit a curve that answers every
/// query.
///
/// # What is refused, and what the doc used to say
///
/// This doc previously read "only a finite positive-or-zero one is a curve
/// point", and the code did neither of those things: it *rejected* zero and
/// *accepted* negative. The code was right on both counts and the sentence was
/// wrong, so the sentence is what changed.
///
/// * **Negative is accepted.** Negative sovereign yields are not a defect;
///   they were the quoted level across the EUR, CHF and JPY curves for years,
///   and a gate refusing them would refuse the bunds.
/// * **Exactly zero is refused.** Not because a zero yield is impossible — a
///   JGB under yield-curve control printed one — but because `0.0` is
///   indistinguishable from a `yield_to_maturity` field the feed never
///   populated, and a zero-filled default would anchor the curve's front end
///   at zero and flat-extrapolate every shorter tenor onto it. The refusal
///   names the ambiguity so a genuine zero can be quoted as a hair either side
///   of it.
/// * **A magnitude at or above [`IMPLAUSIBLE_YIELD_MAGNITUDE`] is refused**,
///   as a percent-for-fraction unit error, which is the finding this arm was
///   added for.
///
/// Refused, never corrected: dividing a suspicious `4.35` by a hundred would
/// build a curve out of a guess about what a vendor meant.
fn quoted_yield(object: &qip_financial::object::FinancialObject) -> Option<Result<f64>> {
    let qip_financial::extensions::Extension::Bond(details) = &object.extension else {
        return None;
    };
    let y = details.yield_to_maturity;
    let id = object.object_id.as_str();
    if !y.is_finite() {
        return Some(Err(Error::invalid(format!(
            "the yield to maturity on {id} is {y}, which is not a finite rate; quote the yield \
             the benchmark trades at or leave it off the curve"
        ))));
    }
    if y == 0.0 {
        return Some(Err(Error::invalid(format!(
            "the yield to maturity on {id} is exactly zero, which this platform cannot tell \
             apart from a field the feed never filled in; quote the benchmark's own yield, and \
             if it really is zero quote it as such a hair either side rather than as the value \
             an empty field also takes"
        ))));
    }
    if y.abs() >= IMPLAUSIBLE_YIELD_MAGNITUDE {
        return Some(Err(Error::invalid(format!(
            // Two decimal places because the exact binary expansion of
            // `4.35 * 100.0` is 434.99999999999994, and a refusal whose own
            // arithmetic looks broken is a refusal an operator argues with
            // rather than acts on.
            "the yield to maturity on {id} is {y}, which as a fraction is {percent:.2}%; supply \
             it as a fraction rather than a percentage. Discounting on a curve through this \
             point would return a present value of exactly zero for every claim in the \
             currency, which reads as a universe carrying no credit risk",
            percent = y * 100.0
        ))));
    }
    Some(Ok(y))
}
