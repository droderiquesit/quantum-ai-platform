//! What a source's payloads are worth, and what they cost.
//!
//! Quality figures are `f64` because they are statistics. Money is
//! [`Decimal`] because it is money: a monthly fee compared with a budget must
//! agree to the cent with the invoice, and a `f64` fee is a fee that
//! disagrees with the invoice by a fraction nobody can explain.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// How good the payloads themselves are.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceQuality {
    field_completeness: f64,
    parse_success_rate: f64,
    duplicate_rate: f64,
    observations: usize,
}

impl SourceQuality {
    /// Build a quality reading.
    ///
    /// Every rate is in `[0, 1]` and checked, because a completeness of 1.4
    /// arriving from a miscounted denominator would otherwise flow straight
    /// into a routing class.
    pub fn new(
        field_completeness: f64,
        parse_success_rate: f64,
        duplicate_rate: f64,
        observations: usize,
    ) -> Result<Self> {
        for (name, value) in [
            ("field completeness", field_completeness),
            ("parse success rate", parse_success_rate),
            ("duplicate rate", duplicate_rate),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(Error::invalid(format!(
                    "{name} must be a fraction in [0, 1], not {value}"
                )));
            }
        }
        if observations == 0 {
            return Err(Error::invalid(
                "a quality reading over zero observations is a guess, not a reading",
            ));
        }
        Ok(Self {
            field_completeness,
            parse_success_rate,
            duplicate_rate,
            observations,
        })
    }

    /// Fraction of expected fields present, in `[0, 1]`.
    pub fn field_completeness(&self) -> f64 {
        self.field_completeness
    }

    /// Fraction of payloads that parsed, in `[0, 1]`.
    pub fn parse_success_rate(&self) -> f64 {
        self.parse_success_rate
    }

    /// Fraction of records already seen, in `[0, 1]`.
    pub fn duplicate_rate(&self) -> f64 {
        self.duplicate_rate
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    /// A single figure in `[0, 1]`, weighting parse success highest because a
    /// payload that does not parse contributes nothing at all.
    pub fn composite(&self) -> f64 {
        0.5 * self.parse_success_rate
            + 0.3 * self.field_completeness
            + 0.2 * (1.0 - self.duplicate_rate)
    }
}

/// What a source charges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCost {
    monthly_fee: Decimal,
    per_request: Decimal,
    included_requests: u64,
    currency: qip_core::Currency,
}

impl SourceCost {
    pub fn free(currency: qip_core::Currency) -> Self {
        Self {
            monthly_fee: Decimal::ZERO,
            per_request: Decimal::ZERO,
            included_requests: u64::MAX,
            currency,
        }
    }

    pub fn new(
        monthly_fee: Decimal,
        per_request: Decimal,
        included_requests: u64,
        currency: qip_core::Currency,
    ) -> Result<Self> {
        if monthly_fee.is_negative() || per_request.is_negative() {
            return Err(Error::invalid(
                "a negative price is a rebate, and no data source offers one",
            ));
        }
        Ok(Self {
            monthly_fee,
            per_request,
            included_requests,
            currency,
        })
    }

    pub fn monthly_fee(&self) -> Decimal {
        self.monthly_fee
    }

    pub fn per_request(&self) -> Decimal {
        self.per_request
    }

    pub fn included_requests(&self) -> u64 {
        self.included_requests
    }

    pub fn currency(&self) -> qip_core::Currency {
        self.currency
    }

    /// Exact monthly cost at a request volume.
    ///
    /// Exact, not estimated: this figure is what a data budget is checked
    /// against, and a rounding error repeated across a hundred sources is a
    /// budget nobody can reconcile.
    pub fn monthly_cost_at(&self, requests: u64) -> Result<Decimal> {
        let billable = requests.saturating_sub(self.included_requests);
        let billable = i64::try_from(billable).map_err(|_| {
            Error::numeric(format!(
                "{billable} billable requests exceeds what a monthly cost can be computed over"
            ))
        })?;
        let metered = self
            .per_request
            .checked_mul(Decimal::from_int(billable))
            .ok_or_else(|| Error::numeric("metered request cost overflowed"))?;
        self.monthly_fee
            .checked_add(metered)
            .ok_or_else(|| Error::numeric("total monthly cost overflowed"))
    }
}
