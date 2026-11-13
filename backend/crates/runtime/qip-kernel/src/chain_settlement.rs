//! Digital-asset proceeds settling on paper through a simulated chain
//! (ADR 0396, R12-COVERAGE-5103).
//!
//! # What this closes
//!
//! A digital-asset sale's proceeds were released on the venue's settlement
//! calendar, the same as an equity's. A chain does not settle on a calendar:
//! proceeds are usable once the including block is buried to the depth the
//! holder requires, and a transfer across a bridge arrives later still and
//! smaller by the bridge's fee. Releasing them on a T+0 calendar let survival
//! sizing spend proceeds a real chain would not yet have made final.
//!
//! # What it is, and what it is not
//!
//! Arithmetic over terms an operator declares. It reads no chain, holds no
//! key, signs nothing and broadcasts nothing; there is no I/O in this module
//! and no order path. It can only delay a release and shrink it, never bring
//! one forward or enlarge it.
//!
//! # Where every figure comes from
//!
//! Nothing here is a constant of this module. The finality depth, the block
//! interval, and a bridge's delay and fee are each declared by the operator
//! in [`ChainSettlementTerms::declare`]. A venue with no declared terms is
//! not given any: the platform settles it as it did before this existed.

use qip_chain::{Confirmations, FeeBps};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};

/// A bridge as the operator declares it: how long a transfer takes and what
/// it costs. No route, no counterparty and no transfer is modelled here;
/// `qip_chain::BridgeRoute` is the fuller model and is not driven by this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredBridge {
    delay: Duration,
    fee: FeeBps,
}

impl DeclaredBridge {
    /// Refuses a negative delay: a bridge cannot deliver before it is asked.
    pub fn declare(delay: Duration, fee: FeeBps) -> Result<Self> {
        if delay.as_nanos() < 0 {
            return Err(Error::invalid(
                "a bridge delay is negative, which would release proceeds before finality; \
                 state the time a transfer takes",
            ));
        }
        Ok(Self { delay, fee })
    }
}

/// The terms one venue's digital-asset proceeds settle on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainSettlementTerms {
    finality: Confirmations,
    block_interval: Duration,
    bridge: Option<DeclaredBridge>,
}

/// One sale's proceeds as the simulated chain releases them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainSettlement {
    /// When the proceeds are final and, if bridged, delivered.
    pub available_at: Timestamp,
    /// The proceeds that arrive: the sale's, less the bridge fee.
    pub proceeds: Decimal,
    /// What the bridge kept. Zero when no bridge is declared.
    pub bridge_fee: Decimal,
}

impl ChainSettlementTerms {
    /// Declare a venue's terms.
    ///
    /// Refused rather than defaulted: a finality depth of zero (the chain
    /// head is `Confirmations::AT_RISK`, and proceeds at risk of a reorg are
    /// not settled) and a block interval that is not positive (depth would
    /// then take no time, which is the same claim).
    pub fn declare(
        finality: Confirmations,
        block_interval: Duration,
        bridge: Option<DeclaredBridge>,
    ) -> Result<Self> {
        if finality.depth() == 0 {
            return Err(Error::invalid(
                "a finality depth of zero confirmations is the chain head, which a reorg can \
                 still void; declare the depth at which proceeds are treated as final",
            ));
        }
        if block_interval.as_nanos() <= 0 {
            return Err(Error::invalid(
                "the block interval is not positive, so no depth would take any time to reach; \
                 state the chain's time per block",
            ));
        }
        Ok(Self {
            finality,
            block_interval,
            bridge,
        })
    }

    /// Whether the proceeds cross a declared bridge: what makes the bridge a
    /// node of the counterparty graph (R12-WORLD-3049).
    pub const fn bridged(&self) -> bool {
        self.bridge.is_some()
    }

    /// When proceeds from a sale filled at `at` are final and, if bridged,
    /// delivered: `at + depth × block_interval`, plus the bridge delay.
    ///
    /// An instant that cannot be represented is refused: wrapping it would
    /// release proceeds at once.
    pub fn available_at(&self, at: Timestamp) -> Result<Timestamp> {
        let overflow = || Error::numeric("the chain settlement instant overflowed");
        let finality_nanos = self
            .block_interval
            .as_nanos()
            .checked_mul(i64::from(self.finality.depth()))
            .ok_or_else(overflow)?;
        let bridge_nanos = self.bridge.map_or(0, |bridge| bridge.delay.as_nanos());
        let available = finality_nanos
            .checked_add(bridge_nanos)
            .and_then(|wait| at.as_nanos().checked_add(wait))
            .ok_or_else(overflow)?;
        Ok(Timestamp::from_nanos(available))
    }

    /// What the declared bridge keeps of `proceeds`; zero with no bridge.
    pub fn bridge_fee(&self, proceeds: Decimal) -> Result<Decimal> {
        match self.bridge {
            Some(bridge) => proceeds
                .checked_mul(Decimal::from_int(i64::from(bridge.fee.bps())))
                .and_then(|scaled| scaled.checked_div(Decimal::from_int(10_000)))
                .ok_or_else(|| Error::numeric("the bridge fee arithmetic overflowed")),
            None => Ok(Decimal::ZERO),
        }
    }

    /// Settle `proceeds` from a sale filled at `at`: the release instant and
    /// the amount that arrives, `proceeds` less the bridge fee.
    pub fn settle(&self, proceeds: Decimal, at: Timestamp) -> Result<ChainSettlement> {
        let bridge_fee = self.bridge_fee(proceeds)?;
        Ok(ChainSettlement {
            available_at: self.available_at(at)?,
            proceeds: proceeds
                .checked_sub(bridge_fee)
                .ok_or_else(|| Error::numeric("the bridge fee exceeds the proceeds"))?,
            bridge_fee,
        })
    }

    pub fn finality(&self) -> Confirmations {
        self.finality
    }

    pub fn is_bridged(&self) -> bool {
        self.bridge.is_some()
    }
}
