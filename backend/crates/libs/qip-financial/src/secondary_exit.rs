//! Secondary-sale exit planning for illiquid positions (blueprint ASSET-013).
//!
//! A private position with no primary liquidity event (redemption window,
//! distribution, listing) before its horizon would otherwise be planned as an
//! exit that cannot happen. This offers the one route left, a sale on the
//! secondary market, at an *estimated* discount to the mark. The discount is a
//! caller-supplied, evidenced input and is never defaulted: a plan carrying a
//! guessed 10% reads as an estimate and is a fiction. Planning only: the sale
//! is carried out against the simulator, never a live counterparty (ADR 0003).

use crate::position_record::{ExitPlan, ExitRoute};
use qip_core::{Decimal, Error, Result, Timestamp};

const BPS_DENOMINATOR: i64 = 10_000;

/// A discount to mark in basis points, with the evidence it rests on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscountToMark {
    bps: Decimal,
    evidence: String,
}

impl DiscountToMark {
    /// Refuses a discount outside `[0, 10000)` (a 100% discount is a gift,
    /// not a price) and one with no stated evidence.
    pub fn new(bps: Decimal, evidence: impl Into<String>) -> Result<Self> {
        let evidence = evidence.into();
        if bps.is_negative() || bps >= Decimal::from(BPS_DENOMINATOR) {
            return Err(Error::invalid(
                "a secondary discount to mark must lie in [0, 10000) basis points; state the \
                 observed or quoted discount rather than a clamped one",
            ));
        }
        if evidence.trim().is_empty() {
            return Err(Error::invalid(
                "a secondary discount needs the evidence it rests on (a quote, a comparable \
                 trade, a fund-level discount survey); name it",
            ));
        }
        Ok(Self { bps, evidence })
    }
}

/// A secondary-sale route: when, at what indicative price, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondarySale {
    plan: ExitPlan,
    mark: Decimal,
    discount: DiscountToMark,
    indicative_price: Decimal,
}

impl SecondarySale {
    pub fn plan(&self) -> ExitPlan {
        self.plan
    }
    pub fn mark(&self) -> Decimal {
        self.mark
    }
    pub fn discount_bps(&self) -> Decimal {
        self.discount.bps
    }
    pub fn discount_evidence(&self) -> &str {
        &self.discount.evidence
    }
    /// `mark * (1 - bps / 10000)`.
    pub fn indicative_price(&self) -> Decimal {
        self.indicative_price
    }
}

/// Plan a secondary sale, or return `None` when a primary liquidity event in
/// `[now, horizon_end]` makes one unnecessary (no secondary route by default
/// for a position that will be liquid anyway).
///
/// Refuses a non-positive mark, a horizon already past, and a lockup that
/// outlasts the horizon (no sale is legal in time; extend the horizon or
/// accept the position cannot exit within it).
pub fn plan_secondary_exit(
    mark: Decimal,
    now: Timestamp,
    horizon_end: Timestamp,
    primary_events: &[Timestamp],
    lockup_expires: Option<Timestamp>,
    discount: DiscountToMark,
) -> Result<Option<SecondarySale>> {
    if !mark.is_positive() {
        return Err(Error::invalid(
            "a secondary sale is priced off a positive mark; mark the position first",
        ));
    }
    if horizon_end < now {
        return Err(Error::invalid(
            "the horizon has already passed; set a horizon at or after now",
        ));
    }
    if primary_events
        .iter()
        .any(|e| *e >= now && *e <= horizon_end)
    {
        return Ok(None);
    }
    let exit_at = lockup_expires.map_or(now, |l| l.max(now));
    if exit_at > horizon_end {
        return Err(Error::invalid(
            "the lockup outlasts the horizon, so no sale is permitted in time; extend the \
             horizon",
        ));
    }
    let keep = Decimal::from(BPS_DENOMINATOR)
        .checked_sub(discount.bps)
        .and_then(|k| mark.checked_mul(k))
        .and_then(|n| n.checked_div(Decimal::from(BPS_DENOMINATOR)))
        .ok_or_else(|| Error::numeric("secondary price overflowed; check the mark"))?;
    Ok(Some(SecondarySale {
        plan: ExitPlan::new(exit_at, ExitRoute::SecondarySale, lockup_expires)?,
        mark,
        discount,
        indicative_price: keep,
    }))
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
mod tests {
    use super::*;

    fn t(s: i64) -> Timestamp {
        Timestamp::from_secs(s)
    }
    fn d(bps: i64) -> Result<DiscountToMark> {
        DiscountToMark::new(Decimal::from(bps), "comparable fund trade")
    }

    #[test]
    fn a_position_with_no_primary_event_before_its_horizon_gets_a_discounted_secondary_route()
    -> Result<()> {
        // A primary event exists but only after the horizon: it does not count.
        let s = plan_secondary_exit(Decimal::from(100), t(0), t(100), &[t(101)], None, d(1500)?)?
            .ok_or_else(|| Error::invalid("expected a secondary route"))?;
        assert_eq!(s.plan().route(), ExitRoute::SecondarySale);
        assert_eq!(s.indicative_price(), Decimal::from(85));
        assert_eq!(s.discount_bps(), Decimal::from(1500));
        Ok(())
    }

    #[test]
    fn a_position_with_a_scheduled_primary_event_inside_the_horizon_gets_no_secondary_route()
    -> Result<()> {
        let with =
            plan_secondary_exit(Decimal::from(100), t(0), t(100), &[t(100)], None, d(1500)?)?;
        assert!(with.is_none());
        // Premise: the same position without the event does get one.
        let without = plan_secondary_exit(Decimal::from(100), t(0), t(100), &[], None, d(1500)?)?;
        assert!(without.is_some());
        Ok(())
    }

    #[test]
    fn the_secondary_exit_waits_for_lockup_expiry_and_is_refused_if_that_is_past_the_horizon()
    -> Result<()> {
        let s = plan_secondary_exit(Decimal::from(100), t(0), t(100), &[], Some(t(40)), d(500)?)?
            .ok_or_else(|| Error::invalid("expected a route"))?;
        assert_eq!(s.plan().exit_at(), t(40));
        assert!(
            plan_secondary_exit(Decimal::from(100), t(0), t(100), &[], Some(t(101)), d(500)?)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn a_discount_with_no_evidence_or_outside_zero_to_ten_thousand_bps_is_refused() {
        assert!(DiscountToMark::new(Decimal::from(10), "  ").is_err());
        assert!(DiscountToMark::new(Decimal::from(-1), "x").is_err());
        assert!(DiscountToMark::new(Decimal::from(10_000), "x").is_err());
        assert!(DiscountToMark::new(Decimal::ZERO, "x").is_ok());
    }
}
