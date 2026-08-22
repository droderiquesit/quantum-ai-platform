//! Cross-chain transfers and the exposure they create.
//!
//! A bridge turns an asset on one chain into an asset on another, and in
//! between it is neither. For the duration of the transfer the position is
//! unhedgeable, unsellable, and dependent on a set of counterparties that the
//! position report does not otherwise mention. Netting the two legs and
//! calling the exposure zero is the standard mistake, and it is why bridge
//! failures show up as surprises rather than as breaches of a limit.
//!
//! So an in-flight transfer here is a position with its own line, its own
//! clock and its own failure modes, and it is not credited on the destination
//! until the source side is confirmed to the depth the route requires.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use crate::amm::FeeBps;
use crate::block::{BlockHash, BlockNumber, ChainId};
use crate::finality::{Confirmations, Finality};
use crate::state::Reorg;

/// A transfer's identity, assigned by the bridge protocol.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TransferId(String);

impl TransferId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a bridge transfer fails, as opposed to how it is slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BridgeFailure {
    /// The source deposit was reorganised out. The destination must never
    /// credit against it, and a credit that already happened is a loss for
    /// whoever fronted it.
    SourceReorg,
    /// The relayer set stopped relaying: paused, upgraded, or gone.
    RelayerHalt,
    /// The destination side does not hold enough of the asset to release it.
    LiquidityShortfall,
    /// The destination chain is congested past the route's timeout.
    DestinationCongestion,
    /// The transfer was deliberately not relayed.
    Censored,
}

impl BridgeFailure {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceReorg => "source_reorg",
            Self::RelayerHalt => "relayer_halt",
            Self::LiquidityShortfall => "liquidity_shortfall",
            Self::DestinationCongestion => "destination_congestion",
            Self::Censored => "censored",
        }
    }

    /// Whether the value comes back to the sender rather than being lost.
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::SourceReorg | Self::DestinationCongestion)
    }
}

/// A bridge route, with the properties that decide whether to use it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BridgeRoute {
    pub name: String,
    pub source: ChainId,
    pub destination: ChainId,
    /// How long the transfer usually takes once the source side is confirmed.
    pub expected_latency: Duration,
    /// Past this, the transfer is late rather than in flight, and lateness is
    /// the first symptom of every failure mode below.
    pub timeout: Duration,
    /// Source confirmations the route waits for before relaying.
    pub source_confirmations: Confirmations,
    pub fee: FeeBps,
    pub failure_modes: Vec<BridgeFailure>,
}

impl BridgeRoute {
    pub fn new(
        name: impl Into<String>,
        source: ChainId,
        destination: ChainId,
        expected_latency: Duration,
        timeout: Duration,
        source_confirmations: Confirmations,
        fee: FeeBps,
        failure_modes: Vec<BridgeFailure>,
    ) -> Result<Self> {
        let name = name.into();
        if source == destination {
            return Err(Error::invalid(format!(
                "bridge route {name} has the same chain on both sides"
            )));
        }
        if timeout < expected_latency {
            return Err(Error::invalid(format!(
                "bridge route {name} times out before it is expected to complete"
            )));
        }
        Ok(Self {
            name,
            source,
            destination,
            expected_latency,
            timeout,
            source_confirmations,
            fee,
            failure_modes,
        })
    }

    /// What arrives on the destination for a given amount sent.
    pub fn delivered(&self, amount: Decimal) -> Result<Decimal> {
        if !amount.is_positive() {
            return Err(Error::invalid("a bridge transfer must be positive"));
        }
        let retained = Decimal::from_int(i64::from(10_000 - self.fee.bps()))
            .checked_div(Decimal::from_int(10_000))
            .ok_or_else(|| Error::numeric("bridge fee is undefined"))?;
        amount
            .checked_mul(retained)
            .ok_or_else(|| Error::numeric("bridge amount overflowed"))
    }
}

/// Where a transfer has got to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TransferStatus {
    /// Locked on the source chain, waiting for the route's confirmations.
    AwaitingSourceFinality,
    /// Source side confirmed, destination side not yet credited.
    InFlight { since: Timestamp },
    /// Delivered.
    Credited {
        at: Timestamp,
        destination_block: BlockNumber,
    },
    /// Failed and not delivered.
    Failed {
        at: Timestamp,
        failure: BridgeFailure,
    },
}

impl TransferStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingSourceFinality => "awaiting_source_finality",
            Self::InFlight { .. } => "in_flight",
            Self::Credited { .. } => "credited",
            Self::Failed { .. } => "failed",
        }
    }

    /// Whether value is still at risk in the transfer.
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::AwaitingSourceFinality | Self::InFlight { .. })
    }
}

/// One cross-chain transfer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BridgeTransfer {
    pub id: TransferId,
    pub route: BridgeRoute,
    pub object_id: ObjectId,
    pub amount: Decimal,
    pub initiated_at: Timestamp,
    pub source_block: BlockNumber,
    pub source_hash: BlockHash,
    status: TransferStatus,
}

impl BridgeTransfer {
    pub fn open(
        id: TransferId,
        route: BridgeRoute,
        object_id: ObjectId,
        amount: Decimal,
        initiated_at: Timestamp,
        source_block: BlockNumber,
        source_hash: BlockHash,
    ) -> Result<Self> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "bridge transfer {id} must move a positive amount"
            )));
        }
        Ok(Self {
            id,
            route,
            object_id,
            amount,
            initiated_at,
            source_block,
            source_hash,
            status: TransferStatus::AwaitingSourceFinality,
        })
    }

    pub const fn status(&self) -> &TransferStatus {
        &self.status
    }

    /// Feed the source block's finality in, and let the transfer react.
    ///
    /// An orphaned source block fails the transfer outright: whatever the
    /// destination believes, the value that was supposed to back it no longer
    /// exists.
    pub fn observe_source(&mut self, finality: Finality, now: Timestamp) -> Result<()> {
        if finality.is_void() {
            self.status = TransferStatus::Failed {
                at: now,
                failure: BridgeFailure::SourceReorg,
            };
            return Ok(());
        }
        if matches!(self.status, TransferStatus::AwaitingSourceFinality) && finality.is_actionable()
        {
            self.status = TransferStatus::InFlight { since: now };
        }
        Ok(())
    }

    /// Credit the destination side, refusing to do so before the source side
    /// is confirmed.
    pub fn credit(&mut self, at: Timestamp, destination_block: BlockNumber) -> Result<Decimal> {
        match self.status {
            TransferStatus::InFlight { .. } => {
                self.status = TransferStatus::Credited {
                    at,
                    destination_block,
                };
                self.route.delivered(self.amount)
            }
            TransferStatus::AwaitingSourceFinality => Err(Error::denied(format!(
                "transfer {} cannot be credited before its source block reaches {}",
                self.id, self.route.source_confirmations
            ))),
            TransferStatus::Credited { .. } | TransferStatus::Failed { .. } => Err(Error::denied(
                format!("transfer {} is already {}", self.id, self.status.as_str()),
            )),
        }
    }

    pub fn fail(&mut self, at: Timestamp, failure: BridgeFailure) -> Result<()> {
        if !self.status.is_open() {
            return Err(Error::denied(format!(
                "transfer {} is already {}",
                self.id,
                self.status.as_str()
            )));
        }
        self.status = TransferStatus::Failed { at, failure };
        Ok(())
    }

    /// Whether the route's own timeout has passed.
    pub fn is_overdue(&self, now: Timestamp) -> bool {
        self.status.is_open() && now.since(self.initiated_at) > self.route.timeout
    }

    /// The exposure this transfer currently represents, if any.
    ///
    /// `None` once the transfer has settled one way or the other, which is the
    /// only time a bridged position is genuinely flat.
    pub fn exposure(&self, now: Timestamp) -> Option<BridgeExposure> {
        if !self.status.is_open() {
            return None;
        }
        let elapsed = now.since(self.initiated_at);
        Some(BridgeExposure {
            transfer: self.id.clone(),
            route: self.route.name.clone(),
            object_id: self.object_id.clone(),
            amount: self.amount,
            elapsed,
            expected_remaining: self.route.expected_latency - elapsed,
            overdue: self.is_overdue(now),
            failure_modes: self.route.failure_modes.clone(),
        })
    }
}

/// A position that exists on neither chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BridgeExposure {
    pub transfer: TransferId,
    pub route: String,
    pub object_id: ObjectId,
    pub amount: Decimal,
    pub elapsed: Duration,
    /// Negative once the transfer is past its expected latency.
    pub expected_remaining: Duration,
    pub overdue: bool,
    pub failure_modes: Vec<BridgeFailure>,
}

/// Every transfer the platform has open, and what they add up to.
#[derive(Clone, Debug, Default)]
pub struct BridgeLedger {
    transfers: BTreeMap<TransferId, BridgeTransfer>,
}

impl BridgeLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, transfer: BridgeTransfer) -> Result<()> {
        if self.transfers.contains_key(&transfer.id) {
            return Err(Error::invalid(format!(
                "transfer {} is already open",
                transfer.id
            )));
        }
        self.transfers.insert(transfer.id.clone(), transfer);
        Ok(())
    }

    pub fn get(&self, id: &TransferId) -> Option<&BridgeTransfer> {
        self.transfers.get(id)
    }

    pub fn get_mut(&mut self, id: &TransferId) -> Option<&mut BridgeTransfer> {
        self.transfers.get_mut(id)
    }

    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    /// Fail every open transfer whose source block was reorganised away.
    ///
    /// Returns what it failed, because a bridged position disappearing is not
    /// something a caller should have to diff two snapshots to notice.
    pub fn on_reorg(&mut self, reorg: &Reorg, now: Timestamp) -> Vec<TransferId> {
        let mut failed = Vec::new();
        for transfer in self.transfers.values_mut() {
            if !transfer.status().is_open() {
                continue;
            }
            if reorg.reverted.contains(&transfer.source_hash)
                && transfer
                    .fail(now, BridgeFailure::SourceReorg)
                    .is_ok()
            {
                failed.push(transfer.id.clone());
            }
        }
        failed
    }

    /// Everything currently in flight.
    pub fn exposures(&self, now: Timestamp) -> Vec<BridgeExposure> {
        self.transfers
            .values()
            .filter_map(|transfer| transfer.exposure(now))
            .collect()
    }

    /// In-flight amount per asset — the line that a position report has to
    /// carry for the exposure to be visible at all.
    pub fn exposure_by_object(&self, now: Timestamp) -> BTreeMap<ObjectId, Decimal> {
        let mut totals: BTreeMap<ObjectId, Decimal> = BTreeMap::new();
        for exposure in self.exposures(now) {
            let entry = totals.entry(exposure.object_id).or_insert(Decimal::ZERO);
            *entry += exposure.amount;
        }
        totals
    }

    /// Transfers past their route's timeout.
    pub fn overdue(&self, now: Timestamp) -> Vec<&BridgeTransfer> {
        self.transfers
            .values()
            .filter(|transfer| transfer.is_overdue(now))
            .collect()
    }
}
