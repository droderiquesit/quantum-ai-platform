//! Company records a fundamental view needs beyond the reported figure:
//! what management said to expect, and how the company is financed
//! (R12-REASON-3013, ADR 0458).
//!
//! A reported metric arrives as a [`crate::intelligence::FundamentalUpdate`]
//! and its consensus surprise is the world model's. Two facts a company
//! analyst reads had no record anywhere in the tree: management's own
//! guidance, which is a different expectation from the street's consensus,
//! and the capital structure — the debt, the cash and when the debt falls
//! due. Without a type for either, the only honest finding about them was
//! "nothing", and the only dishonest one was a default.
//!
//! Both constructors refuse rather than repair. A guidance range whose low
//! sits above its high is an upstream transposition, and a maturity ladder
//! that owes more than the stated total debt is two records that disagree;
//! clamping either would hand the analyst a plausible number nobody stated.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// Management's stated expectation for one metric over one period.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManagementGuidance {
    entity_id: String,
    metric: String,
    low: Decimal,
    high: Decimal,
    /// End of the period the guidance is about: the period a later reported
    /// figure for the same metric is matched against.
    period_end: Timestamp,
    /// When management said it. Guidance not yet issued at a reader's as-of
    /// is not readable.
    issued_at: Timestamp,
    source: String,
}

impl ManagementGuidance {
    /// # Refusals
    ///
    /// An empty entity, metric or source; a low above the high. A point
    /// guidance is stated as `low == high`.
    pub fn new(
        entity_id: impl Into<String>,
        metric: impl Into<String>,
        low: Decimal,
        high: Decimal,
        period_end: Timestamp,
        issued_at: Timestamp,
        source: impl Into<String>,
    ) -> Result<Self> {
        let (entity_id, metric, source) = (entity_id.into(), metric.into(), source.into());
        for (field, value) in [
            ("entity", &entity_id),
            ("metric", &metric),
            ("source", &source),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "management guidance needs a {field}; a range attributable to nobody, about \
                     nothing, or from no source is not guidance"
                )));
            }
        }
        if low > high {
            return Err(Error::invalid(format!(
                "guidance for {entity_id} {metric} has its low {low} above its high {high}; state \
                 the range low-to-high rather than have it reordered"
            )));
        }
        Ok(Self {
            entity_id,
            metric,
            low,
            high,
            period_end,
            issued_at,
            source,
        })
    }

    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }
    pub fn metric(&self) -> &str {
        &self.metric
    }
    pub fn low(&self) -> Decimal {
        self.low
    }
    pub fn high(&self) -> Decimal {
        self.high
    }
    pub fn period_end(&self) -> Timestamp {
        self.period_end
    }
    pub fn issued_at(&self) -> Timestamp {
        self.issued_at
    }
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The middle of the range, the figure a reported value is judged
    /// against.
    pub fn midpoint(&self) -> Decimal {
        (self.low + self.high) / Decimal::from_int(2)
    }

    /// How far `reported` landed from the midpoint, as a fraction of it;
    /// positive when the company beat its own guidance. `None` for a zero
    /// midpoint, where no fraction exists — a zero is not a miss of zero.
    /// A statistic (`f64`) from money (`Decimal`); the crossing is here.
    pub fn gap(&self, reported: f64) -> Option<f64> {
        let mid = self.midpoint().to_f64();
        (mid.is_finite() && mid != 0.0 && reported.is_finite())
            .then(|| (reported - mid) / mid.abs())
    }
}

/// One rung of the maturity ladder: an amount of debt and when it is due.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DebtMaturity {
    pub due: Timestamp,
    pub amount: Decimal,
}

/// How a company is financed at a point in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalStructure {
    entity_id: String,
    /// The balance-sheet date the figures describe.
    as_of: Timestamp,
    /// When the figures became knowable.
    published_at: Timestamp,
    total_debt: Decimal,
    cash: Decimal,
    /// Trailing earnings before interest, tax, depreciation and
    /// amortisation, where stated. Absent is not zero: leverage is then not
    /// computed rather than computed as infinite.
    ebitda: Option<Decimal>,
    maturities: Vec<DebtMaturity>,
    source: String,
}

impl CapitalStructure {
    /// # Refusals
    ///
    /// An empty entity or source; negative debt, cash or maturity amount; a
    /// ladder whose rungs sum above the total debt, which is two records
    /// disagreeing; a publication before the balance-sheet date.
    #[allow(clippy::too_many_arguments)] // each figure is a separate fact on the record
    pub fn new(
        entity_id: impl Into<String>,
        as_of: Timestamp,
        published_at: Timestamp,
        total_debt: Decimal,
        cash: Decimal,
        ebitda: Option<Decimal>,
        mut maturities: Vec<DebtMaturity>,
        source: impl Into<String>,
    ) -> Result<Self> {
        let (entity_id, source) = (entity_id.into(), source.into());
        if entity_id.trim().is_empty() || source.trim().is_empty() {
            return Err(Error::invalid(
                "a capital structure needs an entity and a source; name the issuer and where the \
                 balance sheet came from",
            ));
        }
        if total_debt.is_negative() || cash.is_negative() {
            return Err(Error::invalid(format!(
                "{entity_id}'s capital structure states negative debt or cash; a net position is \
                 stated as gross debt and gross cash"
            )));
        }
        if maturities.iter().any(|m| m.amount.is_negative()) {
            return Err(Error::invalid(format!(
                "{entity_id}'s maturity ladder has a negative rung; a repayment owed is positive"
            )));
        }
        let laddered: Decimal = maturities.iter().map(|m| m.amount).sum();
        if laddered > total_debt {
            return Err(Error::invalid(format!(
                "{entity_id}'s maturity ladder owes {laddered} against a total debt of \
                 {total_debt}; the two records disagree and neither is preferred"
            )));
        }
        if published_at < as_of {
            return Err(Error::invalid(format!(
                "{entity_id}'s capital structure was published before the balance-sheet date it \
                 describes"
            )));
        }
        maturities.sort_by_key(|m| m.due);
        Ok(Self {
            entity_id,
            as_of,
            published_at,
            total_debt,
            cash,
            ebitda,
            maturities,
            source,
        })
    }

    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }
    pub fn as_of(&self) -> Timestamp {
        self.as_of
    }
    pub fn published_at(&self) -> Timestamp {
        self.published_at
    }
    pub fn total_debt(&self) -> Decimal {
        self.total_debt
    }
    pub fn cash(&self) -> Decimal {
        self.cash
    }
    pub fn ebitda(&self) -> Option<Decimal> {
        self.ebitda
    }
    pub fn maturities(&self) -> &[DebtMaturity] {
        &self.maturities
    }
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Debt less cash.
    pub fn net_debt(&self) -> Decimal {
        self.total_debt - self.cash
    }

    /// Net debt over EBITDA. `None` without a stated, positive EBITDA: a
    /// company earning nothing has no leverage multiple, and a number
    /// standing in for "undefined" would rank it beside a healthy one.
    pub fn net_leverage(&self) -> Option<f64> {
        let ebitda = self.ebitda?;
        (ebitda > Decimal::ZERO).then(|| self.net_debt().to_f64() / ebitda.to_f64())
    }

    /// Debt falling due after `from` and no later than `until`.
    pub fn due_between(&self, from: Timestamp, until: Timestamp) -> Decimal {
        self.maturities
            .iter()
            .filter(|m| m.due > from && m.due <= until)
            .map(|m| m.amount)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn at(days: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + days * 86_400)
    }

    #[test]
    fn a_guidance_range_stated_high_to_low_is_refused_rather_than_reordered() {
        let refused = ManagementGuidance::new(
            "ent",
            "revenue",
            dec!("12"),
            dec!("10"),
            at(90),
            at(0),
            "ir",
        );
        assert!(refused.is_err());
        let point = ManagementGuidance::new(
            "ent",
            "revenue",
            dec!("10"),
            dec!("10"),
            at(90),
            at(0),
            "ir",
        );
        assert!(point.is_ok(), "a point guidance is a range of one");
    }

    #[test]
    fn a_guidance_gap_is_measured_from_the_midpoint_and_is_absent_at_zero() {
        let g = ManagementGuidance::new(
            "ent",
            "revenue",
            dec!("90"),
            dec!("110"),
            at(90),
            at(0),
            "ir",
        )
        .unwrap();
        assert_eq!(g.gap(110.0), Some(0.1));
        let zero =
            ManagementGuidance::new("ent", "eps", dec!("-1"), dec!("1"), at(90), at(0), "ir")
                .unwrap();
        assert_eq!(zero.gap(0.5), None);
    }

    #[test]
    fn a_ladder_owing_more_than_the_total_debt_is_refused() {
        let ladder = vec![
            DebtMaturity {
                due: at(100),
                amount: dec!("60"),
            },
            DebtMaturity {
                due: at(800),
                amount: dec!("50"),
            },
        ];
        let refused = CapitalStructure::new(
            "ent",
            at(0),
            at(1),
            dec!("100"),
            dec!("0"),
            None,
            ladder.clone(),
            "filing",
        );
        assert!(refused.is_err());
        let held = CapitalStructure::new(
            "ent",
            at(0),
            at(1),
            dec!("110"),
            dec!("10"),
            Some(dec!("50")),
            ladder,
            "filing",
        )
        .unwrap();
        assert_eq!(held.net_debt(), dec!("100"));
        assert_eq!(held.net_leverage(), Some(2.0));
        assert_eq!(held.due_between(at(0), at(365)), dec!("60"));
    }

    #[test]
    fn leverage_is_absent_without_a_positive_ebitda() {
        let cs = CapitalStructure::new(
            "ent",
            at(0),
            at(1),
            dec!("100"),
            dec!("0"),
            Some(dec!("0")),
            Vec::new(),
            "filing",
        )
        .unwrap();
        assert_eq!(cs.net_leverage(), None);
    }
}
