//! The implied volatility surface: smiles, skew, term structure, forward
//! volatility and dispersion (blueprint §16.1, engine 13).
//!
//! A surface is a grid of implied volatilities over (expiry, strike), read at
//! arbitrary points. Two decisions shape everything below.
//!
//! **The grid coordinates are exact.** `expiry_years` and `strike` are
//! [`Decimal`], not `f64`. A strike is a price and must be exact for the money
//! reasons the crate documents; an expiry is not money but is an *identity* —
//! two quotes for the same expiry must collide so the duplicate can be
//! refused. [`crate::curve::TermStructure`] dedups near-equal tenors with a
//! `1e-12` tolerance and silently drops the loser; a surface must not, because
//! two different implied volatilities quoted for one strike-expiry pair is a
//! feed defect and the caller needs to hear about it rather than receive
//! whichever one sorted last.
//!
//! **Nothing is extrapolated.** [`VolatilitySurface::vol_at`] refuses a query
//! outside the observed expiry range, and refuses a strike outside the range
//! either bracketing smile actually quoted. [`qip_numerics::interpolate::Curve`]
//! flat-extrapolates by design, which is right for a yield curve read at 50y
//! and wrong here: a 10-delta wing vol returned as though it had been observed
//! is indistinguishable, downstream, from one that was — and it is the number
//! a structured payoff would be priced against.
//!
//! The volatilities themselves are `f64`. A volatility is a statistic, not an
//! amount of money; the crossing point between the two is marked at each site
//! where a [`Decimal`] strike becomes an `f64` log-moneyness.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use qip_numerics::interpolate::{Curve, Method};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One observed implied volatility.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolPoint {
    /// Time to expiry in years, as an exact grid coordinate.
    pub expiry_years: Decimal,
    /// Strike, in the underlying's currency.
    pub strike: Decimal,
    /// Implied volatility as an annualised decimal, e.g. `0.22` for 22%.
    pub implied_vol: f64,
}

impl VolPoint {
    pub fn new(expiry_years: Decimal, strike: Decimal, implied_vol: f64) -> Self {
        Self {
            expiry_years,
            strike,
            implied_vol,
        }
    }
}

/// One expiry's smile: implied volatility across strikes at a fixed expiry.
///
/// Interpolation is in log-moneyness rather than in raw strike, so that a
/// smile keeps its shape when the forward moves. Monotone cubic is used for
/// the same reason [`crate::curve::TermStructure`] uses it: a natural spline
/// overshoots between two quoted strikes and manufactures a butterfly
/// arbitrage nobody quoted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Smile {
    expiry_years: Decimal,
    forward: Decimal,
    strikes: Vec<Decimal>,
    vols: Vec<f64>,
    curve: Curve,
}

impl Smile {
    /// The expiry this smile was quoted for.
    pub fn expiry_years(&self) -> Decimal {
        self.expiry_years
    }

    /// The quoted strikes, ascending.
    pub fn strikes(&self) -> &[Decimal] {
        &self.strikes
    }

    /// The quoted volatilities, in strike order.
    pub fn vols(&self) -> &[f64] {
        &self.vols
    }

    /// The lowest and highest strike this smile can answer for.
    pub fn strike_range(&self) -> (Decimal, Decimal) {
        // `new` refuses an empty smile, so both ends exist.
        let lo = self.strikes.first().copied().unwrap_or(Decimal::ZERO);
        let hi = self.strikes.last().copied().unwrap_or(Decimal::ZERO);
        (lo, hi)
    }

    /// Implied volatility at `strike`.
    ///
    /// Refuses a strike outside the quoted range rather than flat-extrapolating.
    /// The wings are exactly where a surface is thinnest and where an invented
    /// number does the most damage.
    pub fn vol_at(&self, strike: Decimal) -> Result<f64> {
        if !strike.is_positive() {
            return Err(Error::invalid(
                "a strike must be positive; pass the contract's strike price, not a moneyness",
            ));
        }
        let (lo, hi) = self.strike_range();
        if strike < lo || strike > hi {
            return Err(Error::invalid(format!(
                "strike {strike} is outside the quoted range {lo}..{hi} at expiry \
                 {expiry}; quote a wing at that strike or read a strike inside the range \
                 — this surface does not extrapolate",
                expiry = self.expiry_years
            )));
        }
        Ok(self.curve.value_at(log_moneyness(strike, self.forward)?))
    }

    /// Skew: the slope of the smile in log-moneyness at the forward.
    ///
    /// Negative is the equity-index norm — downside strikes carry a higher
    /// implied volatility than upside ones.
    pub fn skew(&self) -> Result<f64> {
        let (lo, hi) = self.strike_range();
        if self.forward < lo || self.forward > hi {
            return Err(Error::invalid(format!(
                "the forward {forward} is outside the quoted strike range {lo}..{hi} at \
                 expiry {expiry}, so this smile has no at-the-money point to take a skew \
                 about; quote a strike bracketing the forward",
                forward = self.forward,
                expiry = self.expiry_years
            )));
        }
        Ok(self
            .curve
            .derivative_at(log_moneyness(self.forward, self.forward)?))
    }

    /// At-the-money-forward implied volatility.
    pub fn atm_vol(&self) -> Result<f64> {
        self.vol_at(self.forward)
    }
}

/// A full implied volatility surface observed at an instant.
///
/// Held as one [`Smile`] per expiry in a [`BTreeMap`] keyed by the exact
/// expiry coordinate, so iteration order is the expiry order and a replay
/// reproduces it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolatilitySurface {
    /// The object whose options these are.
    pub underlying: String,
    /// When the surface was observed — the instant the quotes were true.
    pub as_of: Timestamp,
    forward: Decimal,
    smiles: BTreeMap<Decimal, Smile>,
}

impl VolatilitySurface {
    /// Build a surface from observed points.
    ///
    /// Refuses, rather than repairs:
    ///
    /// * an empty set of points — a surface with nothing on it answers every
    ///   query with an invention;
    /// * a non-positive forward, expiry or strike;
    /// * a non-positive or non-finite implied volatility — a zero vol prices
    ///   every option at intrinsic and a negative one has no meaning;
    /// * a repeated strike-expiry pair, naming the pair. Two implied
    ///   volatilities for one grid node is a feed defect; picking one is a
    ///   choice the caller must make with the knowledge of why they differ.
    pub fn new(
        underlying: impl Into<String>,
        as_of: Timestamp,
        forward: Decimal,
        points: Vec<VolPoint>,
    ) -> Result<Self> {
        if points.is_empty() {
            return Err(Error::invalid(
                "a volatility surface needs at least one observed point; supply the quoted \
                 strikes and expiries rather than constructing an empty surface",
            ));
        }
        if !forward.is_positive() {
            return Err(Error::invalid(format!(
                "the forward must be positive, got {forward}; supply the underlying's \
                 forward price for the surface's observation instant"
            )));
        }

        // Grouped on the exact (expiry, strike) coordinates so a repeat is a
        // collision rather than a near-miss. `BTreeMap::insert` returning the
        // previous value is the duplicate detector.
        let mut grid: BTreeMap<Decimal, BTreeMap<Decimal, f64>> = BTreeMap::new();
        for point in &points {
            if !point.expiry_years.is_positive() {
                return Err(Error::invalid(format!(
                    "expiry {expiry} is not positive; an expired or zero-tenor option has \
                     no implied volatility to quote — drop it from the surface",
                    expiry = point.expiry_years
                )));
            }
            if !point.strike.is_positive() {
                return Err(Error::invalid(format!(
                    "strike {strike} at expiry {expiry} is not positive; supply the \
                     contract's strike price",
                    strike = point.strike,
                    expiry = point.expiry_years
                )));
            }
            if !point.implied_vol.is_finite() || point.implied_vol <= 0.0 {
                return Err(Error::numeric(format!(
                    "implied volatility {vol} at strike {strike}, expiry {expiry} is not a \
                     positive finite number; a quote that did not solve should be omitted, \
                     not floored",
                    vol = point.implied_vol,
                    strike = point.strike,
                    expiry = point.expiry_years
                )));
            }
            let smile = grid.entry(point.expiry_years).or_default();
            if smile.insert(point.strike, point.implied_vol).is_some() {
                return Err(Error::invalid(format!(
                    "strike {strike} at expiry {expiry} was observed twice; deduplicate the \
                     feed and decide which quote is authoritative — this surface will not \
                     choose for you",
                    strike = point.strike,
                    expiry = point.expiry_years
                )));
            }
        }

        let mut smiles = BTreeMap::new();
        for (expiry_years, quotes) in grid {
            let strikes: Vec<Decimal> = quotes.keys().copied().collect();
            let vols: Vec<f64> = quotes.values().copied().collect();
            // The crossing point: an exact `Decimal` strike becomes an `f64`
            // log-moneyness here, because interpolation is a statistical
            // operation and the strike itself is retained exactly above.
            let xs: Vec<f64> = strikes
                .iter()
                .map(|k| log_moneyness(*k, forward))
                .collect::<Result<Vec<f64>>>()?;
            let curve = Curve::new(xs, vols.clone(), Method::MonotoneCubic)?;
            smiles.insert(
                expiry_years,
                Smile {
                    expiry_years,
                    forward,
                    strikes,
                    vols,
                    curve,
                },
            );
        }

        Ok(Self {
            underlying: underlying.into(),
            as_of,
            forward,
            smiles,
        })
    }

    /// The forward the surface's moneyness is measured against.
    pub fn forward(&self) -> Decimal {
        self.forward
    }

    /// The quoted expiries, ascending.
    pub fn expiries(&self) -> Vec<Decimal> {
        self.smiles.keys().copied().collect()
    }

    /// The smile at an exactly quoted expiry, if there is one.
    pub fn smile_at(&self, expiry_years: Decimal) -> Option<&Smile> {
        self.smiles.get(&expiry_years)
    }

    /// The shortest and longest quoted expiry.
    pub fn expiry_range(&self) -> (Decimal, Decimal) {
        let lo = self.smiles.keys().next().copied().unwrap_or(Decimal::ZERO);
        let hi = self
            .smiles
            .keys()
            .next_back()
            .copied()
            .unwrap_or(Decimal::ZERO);
        (lo, hi)
    }

    /// Implied volatility at an arbitrary (expiry, strike) inside the data.
    ///
    /// Between expiries the interpolation is linear in *total variance*
    /// `σ²T` rather than in `σ`. Interpolating volatility directly between a
    /// one-month and a one-year quote produces a term structure whose forward
    /// variance can be negative — a calendar arbitrage invented by the
    /// interpolator rather than observed in the market.
    ///
    /// Refuses an expiry outside the quoted range, and a strike outside the
    /// range of *either* bracketing smile. The second half matters: a surface
    /// quoted with wide wings at one month and narrow wings at one year has a
    /// hole between them, and reading across that hole returns a number nobody
    /// quoted.
    pub fn vol_at(&self, expiry_years: Decimal, strike: Decimal) -> Result<f64> {
        let (lo_t, hi_t) = self.expiry_range();
        if expiry_years < lo_t || expiry_years > hi_t {
            return Err(Error::invalid(format!(
                "expiry {expiry_years} is outside the quoted range {lo_t}..{hi_t}; quote an \
                 expiry covering it — this surface does not extrapolate in time"
            )));
        }
        if let Some(smile) = self.smiles.get(&expiry_years) {
            return smile.vol_at(strike);
        }

        let (lo_key, lo_smile) = self
            .smiles
            .range(..expiry_years)
            .next_back()
            .ok_or_else(|| Error::invalid("no quoted expiry below the requested one"))?;
        let (hi_key, hi_smile) = self
            .smiles
            .range(expiry_years..)
            .next()
            .ok_or_else(|| Error::invalid("no quoted expiry above the requested one"))?;

        let vol_lo = lo_smile.vol_at(strike)?;
        let vol_hi = hi_smile.vol_at(strike)?;

        // The crossing point: exact expiry coordinates become `f64` here, to
        // combine with volatilities that are already statistics.
        let t_lo = lo_key.to_f64();
        let t_hi = hi_key.to_f64();
        let t = expiry_years.to_f64();
        let span = t_hi - t_lo;
        if span <= 0.0 {
            return Err(Error::numeric(
                "the bracketing expiries do not straddle the requested one",
            ));
        }
        let w_lo = vol_lo * vol_lo * t_lo;
        let w_hi = vol_hi * vol_hi * t_hi;
        let w = w_lo + (w_hi - w_lo) * (t - t_lo) / span;
        if w < 0.0 || t <= 0.0 {
            return Err(Error::numeric(format!(
                "interpolating between expiries {t_lo} and {t_hi} at strike {strike} implies \
                 a negative total variance; the quotes are calendar-arbitrageable and must \
                 be corrected at the source"
            )));
        }
        Ok((w / t).sqrt())
    }

    /// At-the-money-forward volatility at an expiry inside the quoted range.
    pub fn atm_vol(&self, expiry_years: Decimal) -> Result<f64> {
        self.vol_at(expiry_years, self.forward)
    }

    /// The volatility term structure slope between two expiries, in
    /// volatility points (`0.01` is one point).
    ///
    /// Positive is the calm-market norm: longer-dated implied volatility above
    /// shorter-dated. An inversion is one of the stress signals the macro
    /// agent watches, mirroring [`crate::curve::TermStructure::is_inverted`].
    pub fn term_slope(&self, short_years: Decimal, long_years: Decimal) -> Result<f64> {
        if long_years <= short_years {
            return Err(Error::invalid(format!(
                "the long expiry {long_years} must exceed the short expiry {short_years}; \
                 swap the arguments"
            )));
        }
        Ok(self.atm_vol(long_years)? - self.atm_vol(short_years)?)
    }

    /// True where any quoted expiry's at-the-money volatility sits below the
    /// one before it — an inverted volatility term structure.
    pub fn is_term_inverted(&self) -> Result<bool> {
        let mut previous: Option<f64> = None;
        for expiry in self.expiries() {
            let atm = self.atm_vol(expiry)?;
            if let Some(before) = previous
                && atm < before - 1e-12
            {
                return Ok(true);
            }
            previous = Some(atm);
        }
        Ok(false)
    }

    /// Forward volatility between two quoted expiries.
    ///
    /// From total variances `w = σ²T`: `σ_fwd = sqrt((w₂ − w₁) / (T₂ − T₁))`.
    ///
    /// Refuses when the implied forward variance is negative. That is not a
    /// numerical edge case to be floored at zero — it is the surface telling
    /// the caller that a calendar spread on it has a riskless profit, which
    /// means the quotes are wrong and every payoff priced off them is wrong
    /// too.
    pub fn forward_vol(&self, from_years: Decimal, to_years: Decimal) -> Result<f64> {
        if to_years <= from_years {
            return Err(Error::invalid(format!(
                "the far expiry {to_years} must exceed the near expiry {from_years}; swap \
                 the arguments"
            )));
        }
        let v1 = self.atm_vol(from_years)?;
        let v2 = self.atm_vol(to_years)?;
        // The crossing point: exact expiry coordinates become `f64` to combine
        // with volatilities, which are statistics.
        let t1 = from_years.to_f64();
        let t2 = to_years.to_f64();
        let numerator = v2 * v2 * t2 - v1 * v1 * t1;
        if numerator < 0.0 {
            return Err(Error::numeric(format!(
                "the surface implies a negative forward variance between {from_years} and \
                 {to_years} ({v1} then {v2}); this is a calendar arbitrage in the quotes — \
                 correct them at the source rather than reading a forward volatility off it"
            )));
        }
        Ok((numerator / (t2 - t1)).sqrt())
    }
}

/// Log-moneyness `ln(K/F)`.
///
/// The crossing point between exact prices and statistics: a strike and a
/// forward are money and arrive as [`Decimal`]; their ratio is a dimensionless
/// coordinate and leaves as `f64`.
fn log_moneyness(strike: Decimal, forward: Decimal) -> Result<f64> {
    let f = forward.to_f64();
    let k = strike.to_f64();
    if f <= 0.0 || k <= 0.0 {
        return Err(Error::numeric(format!(
            "cannot take a log-moneyness of strike {strike} against forward {forward}; both \
             must be positive"
        )));
    }
    Ok((k / f).ln())
}

/// Index-implied correlation: the dispersion trade's headline number.
///
/// With index variance `σ_I²` and component weights `wᵢ` and volatilities
/// `σᵢ`, the implied average correlation is
///
/// ```text
/// ρ = (σ_I² − Σ wᵢ² σᵢ²) / (Σᵢ Σⱼ≠ᵢ wᵢ wⱼ σᵢ σⱼ)
/// ```
///
/// Refuses rather than repairs: weights that do not sum to one, a component
/// list shorter than two, and a result outside `[-1, 1]`. The last is the one
/// worth stating — a correlation above one is not a stressed market, it is
/// arithmetic proof that the index and the components were not observed at the
/// same instant or not on the same expiry, and returning it clamped to one
/// would hide exactly that.
pub fn implied_correlation(
    index: &VolatilitySurface,
    components: &[(f64, &VolatilitySurface)],
    expiry_years: Decimal,
) -> Result<f64> {
    if components.len() < 2 {
        return Err(Error::invalid(format!(
            "dispersion needs at least two components, got {}; a one-name 'index' has no \
             correlation to imply",
            components.len()
        )));
    }
    let mut weight_sum = 0.0;
    for (weight, _) in components {
        if !weight.is_finite() || *weight <= 0.0 {
            return Err(Error::invalid(format!(
                "component weight {weight} is not a positive finite number; supply each \
                 component's index weight"
            )));
        }
        weight_sum += *weight;
    }
    if (weight_sum - 1.0).abs() > 1e-6 {
        return Err(Error::invalid(format!(
            "component weights sum to {weight_sum}, not 1.0; supply the full index \
             composition or renormalise it deliberately before calling"
        )));
    }

    let index_vol = index.atm_vol(expiry_years)?;
    let mut vols = Vec::with_capacity(components.len());
    for (weight, surface) in components {
        vols.push((*weight, surface.atm_vol(expiry_years)?));
    }

    let own: f64 = vols.iter().map(|(w, v)| w * w * v * v).sum();
    let mut cross = 0.0;
    for (i, (wi, vi)) in vols.iter().enumerate() {
        for (j, (wj, vj)) in vols.iter().enumerate() {
            if i != j {
                cross += wi * wj * vi * vj;
            }
        }
    }
    if cross <= 0.0 {
        return Err(Error::numeric(
            "the component volatilities imply no cross term; check that every component has \
             a positive volatility at this expiry",
        ));
    }

    let rho = (index_vol * index_vol - own) / cross;
    if !(-1.0..=1.0).contains(&rho) {
        return Err(Error::numeric(format!(
            "the inputs imply a correlation of {rho}, which is outside [-1, 1]; the index \
             and its components were not observed on the same instant or the same expiry — \
             re-read them together rather than accepting a clamped value"
        )));
    }
    Ok(rho)
}
