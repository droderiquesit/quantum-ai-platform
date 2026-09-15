//! The decentralised-venue adapter of blueprint §34.3: the four pieces
//! assembled behind one venue, and the one place where the MEV estimate
//! stops being a number in a report and becomes a number the feasibility
//! gate reads.
//!
//! [`qip_financial::pool`] holds the arithmetic — the curve, the block-time
//! execution mode, the MEV headroom and the contract-risk flag — because it
//! is pure and belongs in a lib. This module holds what an *adapter* knows on
//! top of it: which venue, what one transaction costs regardless of size, and
//! what share of a trade the desk is willing to spend getting it done.
//!
//! # How the MEV estimate reaches a gate that already exists
//!
//! The obvious move was a new feasibility gate literal — `feasibility_mev` —
//! refused at the seam. It is the wrong move, twice over. The gate vocabulary
//! is declared in `qip_contracts::feasibility` and is the bounded label set
//! of `qip_feasibility_refusals_total{constraint}` and of the window a venue
//! is withdrawn on; a literal this crate invented would arrive at the centre
//! as `other`, which is the label meaning "a plane used a gate name this
//! build does not know", and would fire a drift alarm on a gate the build
//! declares. And a venue's cost of crossing is not a new *kind* of question:
//! it is the question the minimum-notional rule already asks, which is
//! whether this order is large enough to be worth doing here.
//!
//! So the estimate is carried into the rule that exists.
//! [`DexVenue::feasibility_model`] derives a
//! [`qip_execution_engine::feasibility::VenueFeasibility`] whose minimum
//! notional is:
//!
//! ```text
//!     gas × 10000 / (budget_bps − slippage_bps − extractable_bps)
//! ```
//!
//! and the derivation is exact rather than decorative. Gas is charged per
//! transaction whatever the size, so it is the only term that produces a
//! *minimum* at all; slippage and the adversary's headroom are proportional,
//! so what they do is eat the budget gas has to fit inside. A venue offering
//! five hundred basis points of headroom to the mempool leaves less room for
//! gas than one offering twenty, and therefore demands a larger ticket before
//! an order there is worth submitting. Refusals land under
//! `feasibility_minimum_notional`, a literal both planes and the centre
//! already know.
//!
//! When the headroom and the slippage together exhaust the budget, no size
//! works, and [`DexVenue::feasibility_model`] **refuses** rather than
//! returning a model with an astronomically large minimum. A minimum notional
//! larger than the book is a refusal wearing a threshold's clothes, and an
//! operator reading it would be told the order was too small when the truth
//! is that the venue is too expensive at any size.
//!
//! # There is no live DEX here, and there is no path to one
//!
//! [`DexVenue::adapter_class`] is [`AdapterClass::Simulated`] and takes no
//! argument. This crate has no live adapter class to name (see the crate
//! documentation's first structural decision), no chain client, and no
//! transaction signer — and this module deliberately does **not** implement
//! [`crate::adapter::VenueAdapter`], because an adapter that implemented
//! `submit_order` against a chain it cannot reach would be a stub that reads
//! as a connection. What is built here is the *model* half of the adapter,
//! which is the half §34.3 asks for and the half that can be proven correct
//! without a chain.

use crate::adapter::AdapterClass;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration};
use qip_execution_engine::feasibility::VenueFeasibility;
use qip_financial::pool::{DexModel, DexQuote};
use std::collections::BTreeMap;

/// Ten thousand, as the basis-point denominator.
const BPS_DENOMINATOR: Decimal = Decimal::from_raw(10_000 * qip_core::decimal::SCALE);

/// One decentralised venue: its §34.3 model, and what an adapter adds.
#[derive(Clone, Debug, PartialEq)]
pub struct DexVenue {
    venue: VenueId,
    model: DexModel,
    gas_cost: Decimal,
    cost_budget_bps: Decimal,
    lot_size: Decimal,
}

impl DexVenue {
    /// Refuses a negative gas cost, a non-positive budget, and a lot size
    /// that is not a grid.
    ///
    /// A zero budget is refused rather than treated as "free only": it would
    /// make [`Self::feasibility_model`] refuse every venue including a
    /// costless one, which is the gate-nobody-can-cross failure in its purest
    /// form.
    pub fn new(
        venue: VenueId,
        model: DexModel,
        gas_cost: Decimal,
        cost_budget_bps: Decimal,
        lot_size: Decimal,
    ) -> Result<Self> {
        if gas_cost.is_negative() {
            return Err(Error::invalid(format!(
                "{venue} declares a gas cost of {gas_cost}; state the fixed cost of one \
                 transaction in the pool's input units, or zero where the chain charges none"
            )));
        }
        if !cost_budget_bps.is_positive() {
            return Err(Error::invalid(format!(
                "{venue} declares a cost budget of {cost_budget_bps} basis points; state a \
                 positive share of a trade the desk will spend crossing, because a budget of \
                 zero admits no order at any size"
            )));
        }
        if !lot_size.is_positive() {
            return Err(Error::invalid(format!(
                "{venue} declares a lot size of {lot_size}; state the smallest quantity \
                 increment the pool trades in"
            )));
        }
        Ok(Self {
            venue,
            model,
            gas_cost,
            cost_budget_bps,
            lot_size,
        })
    }

    pub const fn venue(&self) -> &VenueId {
        &self.venue
    }

    /// Always [`VenueClass::DecentralisedExchange`]. Stated as a method
    /// rather than taken as a constructor argument, so a decentralised venue
    /// cannot be registered under a class whose `settles_atomically` and
    /// `quotes_are_firm` are true.
    pub const fn class(&self) -> VenueClass {
        VenueClass::DecentralisedExchange
    }

    /// Always [`AdapterClass::Simulated`], and there is no argument that
    /// changes it.
    pub const fn adapter_class(&self) -> AdapterClass {
        AdapterClass::Simulated
    }

    pub const fn model(&self) -> &DexModel {
        &self.model
    }

    pub const fn gas_cost(&self) -> Decimal {
        self.gas_cost
    }

    pub const fn cost_budget_bps(&self) -> Decimal {
        self.cost_budget_bps
    }

    /// Why this venue is observe-only, or `None` when it is not.
    ///
    /// §34.3's fallback, worded for an operator: the pieces still owed, by
    /// the names [`qip_financial::pool::DEX_MODEL_PIECES`] uses.
    pub fn observe_only_reason(&self) -> Option<String> {
        let missing = self.model.missing();
        if missing.is_empty() {
            return None;
        }
        Some(format!(
            "{} is observe-only: it still owes {}",
            self.venue,
            missing.join(", ")
        ))
    }

    /// Price one clip end to end, through all four pieces.
    pub fn quote(&self, amount_in: Decimal) -> Result<DexQuote> {
        self.model.quote(amount_in)
    }

    /// How long a fill here is unconfirmed: block-time execution, stated so a
    /// caller does not have to reach through the model for it.
    pub fn settlement_window(&self) -> Result<Duration> {
        let execution = self.model.execution().ok_or_else(|| {
            Error::denied(format!(
                "{} has no block-time execution mode; it is observe-only",
                self.venue
            ))
        })?;
        execution.settlement_window()
    }

    /// The exposure axis and bucket every position at this venue is charged
    /// to, for the `axes` map an order carries into pre-trade risk.
    ///
    /// Returned as a map rather than a pair so a caller merges it in one
    /// call, and empty for an observe-only venue, because a contract nobody
    /// named cannot be a bucket.
    pub fn exposure_axes(&self) -> BTreeMap<String, String> {
        let mut axes = BTreeMap::new();
        if let Some(contract) = self.model.contract() {
            let (axis, bucket) = contract.axis();
            axes.insert(axis, bucket);
        }
        axes
    }

    /// The feasibility model the order manager installs for this venue, with
    /// the cost of crossing — slippage *and* the MEV headroom — inside the
    /// minimum notional.
    ///
    /// `reference_clip` is the size the cost is measured at, because a cost
    /// in basis points is a function of size on a curve and a figure with no
    /// size attached is not a figure.
    ///
    /// Three refusals, each naming what to do instead:
    ///
    /// * an observe-only venue has no model to price against;
    /// * a clip the pool cannot price — too large for a concentrated range,
    ///   or too small to receive anything — is the pool's own refusal,
    ///   carried up;
    /// * a venue whose slippage and headroom exhaust the budget on their own
    ///   is refused, rather than handed back as a minimum notional no order
    ///   will ever reach.
    pub fn feasibility_model(&self, reference_clip: Decimal) -> Result<VenueFeasibility> {
        if let Some(reason) = self.observe_only_reason() {
            return Err(Error::denied(format!(
                "{reason}; an observe-only venue has no feasibility model because it has nothing \
                 to price against"
            )));
        }
        let quote = self.model.quote(reference_clip)?;
        let cost_bps = quote.total_cost_bps()?;
        let remaining = self
            .cost_budget_bps
            .checked_sub(cost_bps)
            .ok_or_else(|| Error::numeric("the remaining cost budget is not representable"))?;
        if !remaining.is_positive() {
            return Err(Error::denied(format!(
                "{} costs {cost_bps} basis points to cross {reference_clip} — {} of slippage and \
                 {} of headroom left to the mempool — against a budget of {}. No size clears \
                 that, so the venue is refused rather than given a minimum notional no order \
                 would reach: narrow the slippage tolerance, reduce the clip, or register the \
                 venue observe-only",
                self.venue,
                quote.pool.slippage_bps,
                quote.mev.extractable_bps,
                self.cost_budget_bps
            )));
        }
        let minimum_notional = self
            .gas_cost
            .checked_mul(BPS_DENOMINATOR)
            .and_then(|scaled| scaled.checked_div(remaining))
            .ok_or_else(|| {
                Error::numeric(format!(
                    "a gas cost of {} against {remaining} basis points of remaining budget is \
                     not a representable minimum notional",
                    self.gas_cost
                ))
            })?;
        // No tick grid: an automated market maker has no price increment to
        // be on, and a tick invented here would refuse prices the venue would
        // have accepted. `None` is the honest answer and the one
        // `VenueFeasibility` was given for exactly this case.
        VenueFeasibility::new(self.lot_size, None, Decimal::ZERO, minimum_notional)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_financial::pool::{BlockExecution, ContractRisk, PoolCurve, PoolState};

    fn model(tolerance_bps: &str) -> DexModel {
        DexModel::new()
            .with_pool(
                PoolState::new(
                    PoolCurve::ConstantProduct,
                    dec!("1000000"),
                    dec!("1000000"),
                    dec!("5"),
                )
                .expect("a valid pool"),
            )
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(Decimal::parse(tolerance_bps).expect("a decimal literal"))
            .expect("a tolerance")
            .with_contract(ContractRisk::new("0xpool", None, true).expect("a flag"))
    }

    /// Gas of one input unit, a budget of two hundred basis points, lot 0.01.
    fn venue(tolerance_bps: &str) -> DexVenue {
        DexVenue::new(
            VenueId::new("XPOOL"),
            model(tolerance_bps),
            dec!("1"),
            dec!("200"),
            dec!("0.01"),
        )
        .expect("a valid venue")
    }

    #[test]
    fn a_wider_slippage_tolerance_raises_the_minimum_notional_the_feasibility_gate_enforces() {
        // The §34.3 claim this module exists to make good: the MEV estimate
        // is a number a gate reads. Two venues identical in pool, gas, budget
        // and lot — only the tolerance orders are submitted under differs, so
        // the curve charges both the same and the only thing that moved is
        // the headroom an adversary can take.
        let tight = venue("20");
        let wide = venue("120");
        let clip = dec!("100");
        let tight_quote = tight.quote(clip).expect("a quote");
        let wide_quote = wide.quote(clip).expect("a quote");
        assert_eq!(
            tight_quote.pool.slippage_bps, wide_quote.pool.slippage_bps,
            "the premise: the curve charges both trades identically"
        );
        assert!(wide_quote.mev.extractable_bps > tight_quote.mev.extractable_bps);

        let tight_model = tight.feasibility_model(clip).expect("a feasibility model");
        let wide_model = wide.feasibility_model(clip).expect("a feasibility model");
        assert!(
            wide_model.minimum_notional() > tight_model.minimum_notional(),
            "the headroom left to the mempool did not reach the minimum notional: {} vs {}",
            wide_model.minimum_notional(),
            tight_model.minimum_notional()
        );
        // Both are grids an order can actually be on, so neither is a
        // threshold nothing clears.
        assert!(tight_model.minimum_notional().is_positive());
        assert!(wide_model.minimum_notional() < dec!("1000"));
    }

    #[test]
    fn a_venue_whose_cost_exactly_consumes_the_budget_is_denied_and_not_reported_as_unrepresentable()
     {
        // The boundary a mutation survived: weakening the guard from
        // `!remaining.is_positive()` to `remaining.is_negative()` admits
        // exactly zero, and every other test still passed. The venue is
        // still refused either way — `checked_div` returns `None` on a zero
        // divisor — so this is not a capital hole. What is lost is the
        // refusal's class and its message: an operator-actionable `denied`
        // naming what to do instead becomes a `numeric` reading as an
        // arithmetic defect in the platform, and the core-Rust rule requires
        // every error here to name the alternative.
        //
        // The budget is measured rather than hand-solved, so the premise
        // holds if the curve is ever recalibrated.
        let clip = dec!("100");
        let measured = venue("20").quote(clip).expect("a quote");
        let exact_cost = measured.total_cost_bps().expect("a total cost");
        let exhausted = DexVenue::new(
            VenueId::new("XPOOL"),
            model("20"),
            dec!("1"),
            exact_cost,
            dec!("0.01"),
        )
        .expect("a valid venue");
        // The premise: cost and budget really are equal, so `remaining` is
        // exactly zero and not merely small.
        assert_eq!(
            exhausted
                .quote(clip)
                .expect("a quote")
                .total_cost_bps()
                .expect("a total cost"),
            exact_cost
        );

        let refused = exhausted.feasibility_model(clip);
        let error = refused.expect_err("a venue with no headroom is refused");
        assert!(
            error.message().contains("No size clears that"),
            "a budget exactly consumed must be denied by the headroom gate, naming what to do \
             instead, rather than falling through to the division and reporting the minimum \
             notional as unrepresentable: {}",
            error.message()
        );
    }

    #[test]
    fn a_venue_whose_headroom_exhausts_the_budget_is_refused_rather_than_priced_out_of_reach() {
        // The failure this refusal prevents: a minimum notional of ten to the
        // twelve reads to an operator as "your order was too small" when the
        // truth is that the venue is too expensive at every size.
        let ruinous = DexVenue::new(
            VenueId::new("XPOOL"),
            model("500"),
            dec!("1"),
            dec!("200"),
            dec!("0.01"),
        )
        .expect("a valid venue");
        let refused = ruinous.feasibility_model(dec!("100"));
        assert!(refused.is_err());
        let message = refused
            .err()
            .map(|error| error.message().to_string())
            .unwrap_or_default();
        assert!(message.contains("No size clears that"), "{message}");
        // The premise: the same venue at a tolerance inside the budget yields
        // a model, so the refusal is about the budget and not about the
        // constructor refusing everything.
        assert!(venue("20").feasibility_model(dec!("100")).is_ok());
    }

    #[test]
    fn an_observe_only_venue_has_no_feasibility_model_and_says_which_piece_it_owes() {
        let incomplete = DexModel::new()
            .with_pool(
                PoolState::new(
                    PoolCurve::ConstantProduct,
                    dec!("1000000"),
                    dec!("1000000"),
                    dec!("5"),
                )
                .expect("a pool"),
            )
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(dec!("20"))
            .expect("a tolerance");
        let venue = DexVenue::new(
            VenueId::new("XPOOL"),
            incomplete,
            dec!("1"),
            dec!("200"),
            dec!("0.01"),
        )
        .expect("a valid venue");
        let reason = venue.observe_only_reason().unwrap_or_default();
        assert!(reason.contains("contract_risk"), "{reason}");
        // Not merely that it errored. A code-review finding: `DexModel::quote`
        // refuses an observe-only model too, so deleting this function's own
        // early refusal left every assertion here passing on the inner one —
        // and the inner one cannot say *why a feasibility model in particular*
        // was refused, which is the sentence an operator installing venue
        // models needs. The refusal is asserted by its own wording.
        let refusal = venue
            .feasibility_model(dec!("100"))
            .err()
            .map(|error| error.message().to_string())
            .unwrap_or_default();
        assert!(
            refusal.contains("has nothing to price against"),
            "the feasibility model was refused by something other than its own observe-only \
             check: {refusal}"
        );
        assert!(
            venue.exposure_axes().is_empty(),
            "a venue with no contract named produced an exposure bucket anyway"
        );
        // The premise: a complete venue is not observe-only and does produce
        // a model and an axis.
        let complete = self::venue("20");
        assert_eq!(complete.observe_only_reason(), None);
        assert!(complete.feasibility_model(dec!("100")).is_ok());
        assert_eq!(
            complete.exposure_axes().get("contract").map(String::as_str),
            Some("0xpool")
        );
    }

    #[test]
    fn a_decentralised_venue_is_simulated_and_settles_over_block_time_rather_than_at_once() {
        let venue = venue("20");
        assert_eq!(venue.adapter_class(), AdapterClass::Simulated);
        assert!(venue.adapter_class().is_paper());
        assert_eq!(venue.class(), VenueClass::DecentralisedExchange);
        assert!(
            !venue.class().settles_atomically(),
            "a chain venue was treated as settling atomically, so a failed leg would be assumed \
             impossible"
        );
        assert!(!venue.class().quotes_are_firm());
        assert_eq!(
            venue.settlement_window().expect("a window"),
            Duration::from_secs(24)
        );
    }

    #[test]
    fn a_venue_with_a_zero_cost_budget_is_refused_at_construction() {
        // A zero budget would make every feasibility model refuse, including
        // a costless venue's, which is a gate nothing can cross.
        assert!(
            DexVenue::new(
                VenueId::new("XPOOL"),
                model("20"),
                dec!("1"),
                dec!("0"),
                dec!("0.01"),
            )
            .is_err()
        );
        assert!(
            DexVenue::new(
                VenueId::new("XPOOL"),
                model("20"),
                dec!("-1"),
                dec!("200"),
                dec!("0.01"),
            )
            .is_err()
        );
        assert!(
            DexVenue::new(
                VenueId::new("XPOOL"),
                model("20"),
                dec!("1"),
                dec!("200"),
                dec!("0"),
            )
            .is_err()
        );
        // The premise: a well-formed venue is admitted.
        assert!(
            DexVenue::new(
                VenueId::new("XPOOL"),
                model("20"),
                dec!("1"),
                dec!("200"),
                dec!("0.01"),
            )
            .is_ok()
        );
    }
}
