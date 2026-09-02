//! The node's mesh seam: where the cell stops being a process on its own.
//!
//! `qip_edge::mesh` is the cell's half of ADR 0011's spine — an uplink that
//! publishes state deltas to the central plane and a downlink that pulls signed
//! capital envelopes back. This module is what a *deployed* cell does with
//! them: read the peer out of the environment, assemble both halves, and run
//! one exchange per tick.
//!
//! # A cell with no peer still runs
//!
//! `QIP_MESH_PEER` is optional and its absence is not a misconfiguration. A
//! cell decides alone — that is the whole of ADR 0008 — so a node whose central
//! plane is unreachable keeps trading inside the envelope it already holds
//! until that envelope expires. Refusing to start without a peer would turn a
//! partition into an outage, which is the failure the cellular architecture
//! exists to avoid. What the node does instead is *say* that it has no peer, in
//! the same list as every other production requirement it cannot satisfy, so
//! "this cell is detached" is a line somebody reads rather than a silence.
//!
//! What is **not** optional is the capital envelope key. A cell that cannot
//! verify a grant must not trade, and the downlink refuses to be built without
//! one — see [`qip_edge::mesh::CapitalDownlink::connect`]. Arriving over the
//! mesh does not make an envelope trustworthy; the signature does.
//!
//! # One tick, in this order
//!
//! [`MeshLink::exchange`] takes capital *first* and publishes state *second*.
//! That way the delta the centre receives already reflects any grant installed
//! in the same tick, so the expiry the centre reads back is its own
//! confirmation that the envelope landed. The other order would leave the
//! centre a full tick behind on the one fact it needs to decide whether to
//! re-issue.
//!
//! # A tick does not fail
//!
//! [`MeshLink::exchange`] returns a report rather than a `Result`. Every way
//! the exchange can go wrong — an unreachable peer, an envelope that does not
//! verify, a grant for a strategy this cell does not run — is a fact the health
//! surface has to publish, and a `Result` would make the node choose between
//! crashing and throwing that fact away. Neither is the right answer for a
//! process whose job is to keep trading inside a bound somebody already
//! approved.

use std::sync::Arc;

use crate::arbitrage::ArbitrageInstaller;
use crate::strategies::StrategyInstaller;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_edge::cell::{Cell, WorkReport};
use qip_edge::mesh::{
    CapitalDownlink, CellUplink, Dispatch, DownlinkConfig, DownlinkStats, PolicyDownlink,
    PolicyDownlinkStats, UplinkConfig, UplinkStats,
};
use qip_transport::breaker::BreakerState;
use qip_transport::retry::{Sleeper, ThreadSleeper};
use qip_transport::{ClientLimits, MemoryDeadLetters, MeshConfig, RetryPolicy};

/// The environment variable naming the central plane.
pub const PEER_VARIABLE: &str = "QIP_MESH_PEER";
/// The environment variable overriding the derived jitter seed.
pub const SEED_VARIABLE: &str = "QIP_MESH_SEED";

/// How many dead letters a node keeps in memory.
///
/// In memory because this end of the wire is the *delta* path, where a lost
/// message is superseded by the next one. The capital path is the one that is
/// spooled, and it is spooled at the sending end, which is the central plane.
const DEAD_LETTER_CAPACITY: usize = 64;

/// What the node needs to reach the central plane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshSettings {
    pub cell: String,
    pub region: String,
    /// The central plane's base URL, `http://host:port`.
    pub peer: String,
    /// Seed for the retry and cooldown jitter.
    pub seed: u64,
}

impl MeshSettings {
    /// Read the mesh configuration, or `None` when this node has no peer.
    ///
    /// The seed defaults to a value derived from the cell's own identity rather
    /// than to zero. Nine cells that all rolled out together, all failed
    /// against the same central plane and all drew the same jitter would retry
    /// on the same millisecond and probe the recovering plane in lockstep —
    /// which is the thundering herd the jitter exists to break up. Deriving it
    /// from the cell id makes the spread automatic and still reproducible: the
    /// same cell replays the same schedule.
    pub fn from_env(cell: &str, region: &str) -> Result<Option<Self>> {
        let Some(peer) = std::env::var(PEER_VARIABLE)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let seed = match std::env::var(SEED_VARIABLE) {
            Ok(value) => value.trim().parse::<u64>().map_err(|_| {
                Error::invalid(format!(
                    "configuration: {SEED_VARIABLE} is not a number: {value}"
                ))
            })?,
            Err(_) => seed_from(cell),
        };
        Ok(Some(Self {
            cell: cell.to_string(),
            region: region.to_string(),
            peer: peer.trim().to_string(),
            seed,
        }))
    }

    fn mesh_config(&self, name: &str) -> MeshConfig {
        MeshConfig::new(format!("{name}:{}", self.cell), &self.peer)
            .with_seed(self.seed)
            .with_retry(RetryPolicy::default())
            .with_limits(ClientLimits::default())
    }
}

/// A stable per-cell seed, from the cell's own name.
fn seed_from(cell: &str) -> u64 {
    let digest = qip_core::hash::sha256_hex(cell.as_bytes());
    u64::from_str_radix(digest.get(..16).unwrap_or("0"), 16).unwrap_or(0)
}

/// What one exchange with the central plane did.
///
/// Every field is something an operator or a health probe has to be able to
/// see. A tick that quietly did nothing and a tick that could not reach the
/// centre look identical from outside unless the difference is published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshTick {
    /// What happened to the delta: `delivered`, `circuit_open`, `dead_lettered`
    /// or `refused`.
    pub delta: Option<String>,
    /// Grants verified and installed on a deployed strategy.
    pub renewed: Vec<String>,
    /// Grants that arrived and were not accepted, with the reason. Includes
    /// both the ones that failed verification and the ones the cell refused —
    /// a grant for a strategy it does not run, for instance, which it will not
    /// deploy on the strength of an envelope.
    pub refused: Vec<String>,
    /// Grants recognised as ones already applied.
    pub duplicates: usize,
    /// Set when the poll itself failed, rather than any grant in it.
    pub poll_error: Option<String>,
    /// Policy sequences applied this tick, in application order.
    pub policies: Vec<u64>,
    /// Halt commands applied this tick.
    pub halts: usize,
    /// Set when the policy poll itself failed.
    pub policy_poll_error: Option<String>,
    /// What the arbitrage installer did this tick, when the node has one.
    pub desk: Option<String>,
    /// What the strategy installer did this tick, when the node has one.
    pub plan: Option<String>,
    /// Whether `plan` is something an operator needs to look at. Carried
    /// as a flag rather than re-read from the text, so a reworded outcome
    /// cannot make a deployment go quiet.
    pub plan_is_quiet: bool,
}

impl Default for MeshTick {
    /// A tick that has done nothing yet. The plan flag starts quiet because
    /// a node with no strategy installer has no plan outcome to look at.
    fn default() -> Self {
        Self {
            delta: None,
            renewed: Vec::new(),
            refused: Vec::new(),
            duplicates: 0,
            poll_error: None,
            policies: Vec::new(),
            halts: 0,
            policy_poll_error: None,
            desk: None,
            plan: None,
            plan_is_quiet: true,
        }
    }
}

impl MeshTick {
    /// Whether anything happened that an operator needs to look at.
    pub fn is_quiet(&self) -> bool {
        self.refused.is_empty()
            && self.poll_error.is_none()
            && self.delta.as_deref().is_none_or(|code| code == "delivered")
            && self
                .desk
                .as_deref()
                .is_none_or(|desk| !(desk.starts_with("installed") || desk.starts_with("refused")))
            && self.plan_is_quiet
    }
}

/// Counters the health surface publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshHealth {
    pub uplink: UplinkStats,
    pub downlink: DownlinkStats,
    pub policy: PolicyDownlinkStats,
    /// The circuit to the central plane, as the uplink sees it. Published
    /// because "this cell has stopped talking to the centre" is invisible from
    /// a counter that merely stopped increasing.
    pub circuit: BreakerState,
}

/// Both halves of one node's link to the central plane.
#[derive(Debug)]
pub struct MeshLink {
    peer: String,
    uplink: CellUplink,
    downlink: CapitalDownlink,
    /// The policy half. Same inbox as capital — each downlink ignores the
    /// other's topics — so one deployment variable serves both.
    policy: PolicyDownlink,
}

impl MeshLink {
    /// Assemble the link.
    ///
    /// The sleeper is a real [`ThreadSleeper`] here and an injected fake in the
    /// tests, which is the whole reason `qip-transport` takes one: a test
    /// asserts the retry ladder instead of spending it, and a deployment
    /// actually waits.
    pub fn connect(
        settings: &MeshSettings,
        envelope_key: &[u8],
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        Self::connect_with(settings, envelope_key, clock, Arc::new(ThreadSleeper))
    }

    /// The same, with the sleeper supplied.
    pub fn connect_with(
        settings: &MeshSettings,
        envelope_key: &[u8],
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Result<Self> {
        let uplink = CellUplink::connect(
            UplinkConfig::new(
                &settings.cell,
                &settings.region,
                settings.mesh_config("uplink"),
            )
            .with_breaker(
                qip_transport::breaker::BreakerPolicy::default(),
                settings.seed,
            ),
            Arc::clone(&clock),
            Arc::clone(&sleeper),
            Box::new(MemoryDeadLetters::new(DEAD_LETTER_CAPACITY)),
        )?;
        let downlink = CapitalDownlink::connect(
            DownlinkConfig::new(&settings.cell, settings.mesh_config("downlink")).with_breaker(
                qip_transport::breaker::BreakerPolicy::default(),
                // Offset so the two halves of one link do not draw the same
                // cooldown: the uplink and the downlink address the same peer
                // and would otherwise probe it on the same millisecond.
                settings.seed.wrapping_add(1),
            ),
            envelope_key,
            Arc::clone(&clock),
            Arc::clone(&sleeper),
        )?;
        let policy = PolicyDownlink::connect(
            DownlinkConfig::new(&settings.cell, settings.mesh_config("downlink")).with_breaker(
                qip_transport::breaker::BreakerPolicy::default(),
                // A third offset for the third consumer of the same peer, for
                // the same reason the second got one.
                settings.seed.wrapping_add(2),
            ),
            envelope_key,
            clock,
            sleeper,
        )?;
        Ok(Self {
            peer: settings.peer.clone(),
            uplink,
            downlink,
            policy,
        })
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    pub fn health(&self) -> MeshHealth {
        MeshHealth {
            uplink: self.uplink.stats(),
            downlink: self.downlink.stats(),
            policy: self.policy.stats(),
            circuit: self.uplink.circuit(),
        }
    }

    /// Take whatever capital arrived, then tell the centre where the cell
    /// stands.
    ///
    /// `report` is the last pass of the cell's own work. A node with no venue
    /// feed configured passes an empty one, and the delta is then a statement
    /// about the cell's authority and halt state rather than about its trading
    /// — which is exactly what such a node has to report.
    pub fn exchange(&mut self, cell: &mut Cell, report: &WorkReport, now: Timestamp) -> MeshTick {
        self.exchange_with(cell, report, now, None)
    }

    /// The same tick, with the arbitrage installer given its two inputs.
    ///
    /// A grant for the desk's strategy is held by the installer rather than
    /// handed to `renew_capital`, which would refuse it while no desk is
    /// deployed — and would be right to, since a cell does not deploy a
    /// strategy because capital arrived for it. The installer is the one
    /// place that grant may wait, and it waits for a whitelist the centre
    /// signed. Once a desk is installed, its renewals go through
    /// `renew_capital` like any strategy's.
    pub fn exchange_with(
        &mut self,
        cell: &mut Cell,
        report: &WorkReport,
        now: Timestamp,
        installer: Option<&mut ArbitrageInstaller>,
    ) -> MeshTick {
        self.exchange_with_installers(cell, report, now, installer, None)
    }

    /// The same tick, with the strategy installer given its inputs too.
    ///
    /// A grant for a strategy the cell does not run is refused by
    /// `renew_capital` — the cell does not deploy a strategy because capital
    /// arrived — and, when the node has a strategy installer, is held there
    /// instead of reported as refused, to be spent when a verified plan
    /// names the strategy. Without an installer the refusal is reported as
    /// it always was. The desk's grant is still the desk's: it is offered to
    /// the arbitrage installer first, so a node running both never holds
    /// the desk's capital where a plan could spend it on a strategy.
    pub fn exchange_with_installers(
        &mut self,
        cell: &mut Cell,
        report: &WorkReport,
        now: Timestamp,
        mut installer: Option<&mut ArbitrageInstaller>,
        mut strategies: Option<&mut StrategyInstaller>,
    ) -> MeshTick {
        let mut tick = MeshTick::default();

        match self.downlink.poll(now) {
            Ok(batch) => {
                tick.duplicates = batch.duplicates.len();
                for refusal in batch.refused {
                    tick.refused
                        .push(format!("{}: {}", refusal.event_id, refusal.reason));
                }
                for envelope in batch.verified {
                    let strategy = envelope.strategy().as_str().to_string();
                    if let Some(installer) = installer.as_deref_mut()
                        && cell.arbitrage().is_none()
                        && envelope.strategy() == installer.strategy()
                    {
                        match installer.offer(envelope) {
                            Ok(()) => tick.renewed.push(format!("{strategy} (held for the desk)")),
                            Err(error) => tick
                                .refused
                                .push(format!("{strategy}: {}", error.message())),
                        }
                        continue;
                    }
                    // Held for the plan only when the cell refuses the grant
                    // *because nothing runs under that name*. Any other
                    // refusal — another cell's envelope, most likely — is
                    // reported as it always was; holding it would be
                    // keeping a grant the cell has already said is not its
                    // to spend.
                    let not_deployed = !cell.deployed_strategies().contains(&strategy.as_str())
                        && cell
                            .arbitrage()
                            .is_none_or(|desk| desk.strategy().as_str() != strategy);
                    if not_deployed && let Some(strategies) = strategies.as_deref_mut() {
                        match strategies.offer(envelope) {
                            Ok(()) => tick.renewed.push(format!("{strategy} (held for the plan)")),
                            Err(error) => tick
                                .refused
                                .push(format!("{strategy}: {}", error.message())),
                        }
                        continue;
                    }
                    match cell.renew_capital(envelope, now) {
                        Ok(()) => tick.renewed.push(strategy),
                        // The cell refusing a grant it verified is a
                        // disagreement between the centre and this cell about
                        // what runs here. It is reported, never resolved by
                        // deploying something.
                        Err(error) => tick
                            .refused
                            .push(format!("{strategy}: {}", error.message())),
                    }
                }
            }
            Err(error) => tick.poll_error = Some(error.message().to_string()),
        }

        // Policy after capital, halts before payloads. Halts first because a
        // halt is never improved by waiting, and a batch that carried both an
        // engage and a releasing payload must end halted only if the release
        // predates the halt — which the cell's barrier decides, not arrival
        // order.
        match self.policy.poll(now) {
            Ok(batch) => {
                for halt in batch.halts {
                    cell.apply_halt(halt, now);
                    tick.halts += 1;
                }
                for payload in batch.verified {
                    let sequence = payload.sequence();
                    match cell.apply_policy(payload, now) {
                        Ok(()) => tick.policies.push(sequence),
                        // A sequence the cell refuses is a disagreement about
                        // ordering, reported and never resolved by forcing it.
                        Err(error) => tick
                            .refused
                            .push(format!("policy {sequence}: {}", error.message())),
                    }
                }
                for refusal in batch.refused {
                    tick.refused
                        .push(format!("{}: {}", refusal.event_id, refusal.reason));
                }
            }
            Err(error) => {
                tick.policy_poll_error = Some(error.message().to_string());
            }
        }

        // After both polls, so a whitelist and a grant that arrived in the
        // same tick install in the same tick, and the delta below reports
        // the desk's utilisation from its first pass.
        if let Some(installer) = installer {
            let outcome = installer.install(cell, now);
            tick.desk = Some(outcome.describe());
        }
        // And the plan, for the same reason: a plan and the grants that fund
        // it may arrive in one tick, and the delta below then reports the
        // strategies as deployed with their envelopes' expiry.
        if let Some(strategies) = strategies {
            let outcome = strategies.install(cell, now);
            tick.plan_is_quiet = outcome.is_quiet();
            tick.plan = Some(outcome.describe());
        }

        let delta = cell.state_delta(report, now);
        tick.delta = Some(match self.uplink.publish(delta, now) {
            Ok(Dispatch::Delivered(_)) => "delivered".to_string(),
            Ok(Dispatch::CircuitOpen(_)) => "circuit_open".to_string(),
            Ok(Dispatch::DeadLettered { .. }) => "dead_lettered".to_string(),
            // A delta the transport would not take at all. Recorded rather
            // than raised: it is a fact about this build's own framing, and a
            // node that exited on it would stop trading over a reporting bug.
            Err(error) => format!("refused: {}", error.message()),
        });
        tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_cells_draw_different_jitter_from_their_own_names() {
        // The thundering herd this prevents is nine cells probing a recovering
        // central plane on the same millisecond after a shared rollout.
        assert_ne!(seed_from("london-1"), seed_from("tokyo-2"));
        assert_eq!(
            seed_from("london-1"),
            seed_from("london-1"),
            "the same cell must replay the same schedule"
        );
    }
}
