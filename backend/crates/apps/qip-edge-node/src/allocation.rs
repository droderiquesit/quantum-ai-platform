//! The capital this whole cell may commit, as the deployment states it.
//!
//! `qip-edge` holds the discipline — `Cell::with_region_allocation`, one hold
//! per strategy per pass, refused whole when the region has nothing left —
//! and the library's own tests prove it. What the library cannot do is give
//! itself the number: a cell that chose how much it may risk would be
//! deciding the one thing ADR 0008 says it never does, so the amount is
//! *given* by a composition root. Until this module existed no composition
//! root gave one, and every node this binary could build ran with
//! `region_allocation: None` — each deployed strategy bounded by its own
//! signed envelope and nothing bounding their sum. Two strategies in one
//! disconnected cell could each spend the whole envelope, which is exactly
//! the double-spend the reservation was written to close.
//!
//! # Refused, never defaulted
//!
//! [`RegionCapital::read`] is the only way to obtain a [`RegionCapital`], and
//! [`crate::assemble`] takes one by value, so a node cannot be assembled
//! without an amount that passed here. Absent, blank, unparseable and
//! non-positive are all refused at start with `EX_CONFIG`. A default would
//! be a number nobody chose — a large one is the double-spend with a
//! different face, and a zero is a node that decides and sends nothing
//! while looking healthy. The library admits a zero allocation because it
//! is a coherent ledger state; the node refuses it because a region with no
//! capital does not need a process deciding what to do with it, and an
//! operator who meant "stop this cell" has the halt flag for that.
//!
//! # What this number is, and is not
//!
//! It is the operator's local backstop, not the centre's authority. Nothing
//! on the mesh carries a per-region amount — a `CapitalEnvelope` is keyed on
//! (strategy, cell) — so this can only ever narrow what the signed envelopes
//! already permit, never widen it. See `qip_edge::reservation` for the
//! argument in full.

use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// Names the capital the whole cell may commit, as a decimal amount in the
/// envelope's currency.
pub const ALLOCATION_VARIABLE: &str = "QIP_REGION_ALLOCATION";

/// A positive amount the deployment stated, and the only way to get one.
///
/// The field is private and there is no `From<Decimal>`: a `RegionCapital`
/// in hand is proof its amount was read from configuration and passed the
/// refusals in [`Self::read`], which is what lets [`crate::assemble`] take
/// it as a fact rather than re-checking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionCapital {
    amount: Decimal,
}

impl RegionCapital {
    /// Interpret the variable's value.
    ///
    /// Every refusal starts with `configuration:` so `main` exits with
    /// `EX_CONFIG` — an orchestrator should read this as "deployed wrong"
    /// and stop restarting it, not as a crash.
    pub fn read(value: Option<&str>) -> Result<Self> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Err(Error::invalid(format!(
                "configuration: {ALLOCATION_VARIABLE} must be set to the capital this cell may \
                 commit in total; without it every deployed strategy is bounded only by its own \
                 envelope and nothing bounds their sum. There is no default: a number nobody \
                 chose is not a limit"
            )));
        };
        let Some(amount) = Decimal::parse(value) else {
            return Err(Error::invalid(format!(
                "configuration: {ALLOCATION_VARIABLE}={value} is not a decimal amount; write the \
                 region's capital as digits with an optional fraction, such as 250000 or \
                 250000.50, and nothing else"
            )));
        };
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "configuration: {ALLOCATION_VARIABLE}={value} is not a positive amount. A cell \
                 with no capital has nothing to decide and should not be started; to stop a \
                 running cell engage its halt flag rather than starving it"
            )));
        }
        Ok(Self { amount })
    }

    /// The amount the deployment stated.
    pub fn amount(self) -> Decimal {
        self.amount
    }
}
