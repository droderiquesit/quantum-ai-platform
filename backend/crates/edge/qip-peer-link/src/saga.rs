//! The peer cycle, driven on the session thread (R12-MESH-5046, 5061–5065,
//! 5070): origination, reservation, FIRE, execution, fill and unwind.
//!
//! Until this module the protocol's coordinating calls had no production
//! caller. The node published its touch and received its peers', and a
//! cross-region cycle could not begin, because nothing built an epoch,
//! reserved a leg, declared FIRE or sent a fill. Every rule in `qip_peer` was
//! proven and none guarded a real cycle.
//!
//! The saga runs entirely in Lane 1, on the thread that owns the endpoint.
//! Its inputs from the cell are the [`LegAuthority`] values the decision
//! loop's gate produced — what the cell will trade as a peer leg, where, at
//! what limit, until when — and it reserves, sends and fills only inside the
//! newest of them. It never reaches the cell: the cell is on another thread
//! and this crate cannot name it (ADR 0116).
//!
//! # What each step refuses
//!
//! * **Origination** pairs an authority with a peer's edge state that crosses
//!   it by at least [`MIN_EDGE_BPS`] after fees nobody stated — the touch is
//!   the price — plus the expected adverse move over the measured round trip
//!   to that peer, priced by `qip_arbitrage::netedge` with the round trip as
//!   its coordination latency (R12-MESH-5069), and opens an epoch only to a
//!   peer whose clock is measured. The two are one condition on purpose: a
//!   crossing that clears the floor at zero latency and not once the wire's
//!   own measurement is charged is a cycle that pays only on a wire nobody
//!   has.
//!   Of two nodes that see the same crossing, the one whose name sorts first
//!   coordinates; the other's view of it is the mirror image and would open a
//!   duplicate cycle on the same two books.
//! * **Reservation** puts the leg to the gate the cell's authority defines —
//!   the venue, instrument and side must match one, the size must fit, the
//!   limit must be no worse than the cell's touch, the declared unwind loss
//!   must cover crossing back out — before the book holds anything
//!   (`PeerEndpoint::reserve_own_legs`, MESH-5062).
//! * **FIRE** is declared by the coordinator only when every leg is held at
//!   full size and the loss bound is established (MESH-5063, 5070).
//! * **Execution** sends a leg through the same gate again — the authority
//!   may have narrowed since the reservation — and fills it, in paper,
//!   against the size the cell's touch still shows: a partial fill when it
//!   has shrunk (MESH-5065).
//! * **Unwind**: a node whose leg filled more than its counterparty's unwinds
//!   the excess at the spread its authority priced, inside the loss declared
//!   before FIRE, or the protocol refuses it as `loss_bound` and the refusal
//!   says the cost model was wrong.

use crate::boundary::{LegAuthority, Side};
use crate::wire::PeerSession;
use qip_arbitrage::netedge::{EdgeAssumptions, NetEdgeCalculator};
use qip_core::{Decimal, Duration, Timestamp};
use qip_peer::rebalance::{RebalanceTask, rebalancing_tasks};
use qip_peer::{
    EpochDraft, EpochLeg, LegExecution, LegSide, OpportunityEpoch, ReservationBook,
    ReservationOutcome,
};
use std::collections::{BTreeMap, BTreeSet};

/// The least a crossing must clear, in basis points of the buying price, to
/// be worth a cycle. The touch is already the executable price; this is the
/// margin left for what the touch does not show.
pub const MIN_EDGE_BPS: i64 = 5;
/// How long an originated epoch lives. Short: it is priced off a touch.
pub const EPOCH_TTL: Duration = Duration::from_secs(2);
/// A peer's edge state older than this, on the peer's own stamp corrected by
/// its measured offset, is not priced from.
pub const EDGE_FRESHNESS: Duration = Duration::from_secs(2);
/// Cycles this node coordinates at once.
pub const MAX_OPEN_CYCLES: usize = 4;
/// The detector's name and version, carried in every epoch it opens so the
/// journal can say which rule priced a cycle (MESH-5061).
pub const DETECTOR: &str = "peer-touch-crossing/1";

/// What one step did, for the status cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SagaStep {
    pub originated: u64,
    pub reservations: u64,
    pub declines: u64,
    pub fires: u64,
    pub legs_sent: u64,
    pub fills: u64,
    pub unwinds: u64,
    pub rejected: u64,
    /// Cycles whose every leg's fill is known, closed this step (ADR 0154
    /// decision 6).
    pub closed: u64,
    /// Cycles this node originated that the endpoint let go of without FIRE
    /// this step: the counterparty never reserved, declined, or was lost
    /// before it could, and the epoch reached its TTL (R12-RES-7095).
    pub expired: u64,
}

/// The session thread's cycle state.
#[derive(Debug, Default)]
pub struct Saga {
    authorities: Vec<LegAuthority>,
    /// One reservation book per authorised book, its grant the authority's
    /// size: the protocol's book counts in leg units, and units of two
    /// instruments do not add.
    books: BTreeMap<(String, String, Side), ReservationBook>,
    /// Cycles this node opened and still holds: opportunity to (instrument,
    /// peer), so one crossing is not opened twice while the first is live.
    open: BTreeMap<String, (String, String)>,
    /// Opportunities whose own legs this node has put to its gate.
    attempted: BTreeSet<String>,
    /// Own legs sent, with the spread the authority priced at fill time.
    executed: BTreeMap<(String, String), Decimal>,
    unwound: BTreeSet<(String, String)>,
    /// Fired cycles already closed: their drift was raised once and is not
    /// raised again while the epoch is still held.
    closed: BTreeSet<String>,
    /// Cycles this node originated and declared FIRE on, while the endpoint
    /// still holds them. The endpoint forgets whether an epoch fired the
    /// moment it drops the epoch, so the saga keeps the fact it needs to
    /// tell a cycle that ran from one that lapsed.
    fired: BTreeSet<String>,
    /// Rebalancing tasks closed cycles raised, until the host carries them
    /// to the port.
    rebalance: Vec<RebalanceTask>,
    /// How many refusals have been read for `guarantee_unmet`.
    refusals_read: usize,
    started_ns: i64,
    next: u64,
}

impl Saga {
    pub fn new(started: Timestamp) -> Self {
        Self {
            started_ns: started.as_nanos(),
            ..Self::default()
        }
    }

    /// Replace the authorities with the cell's newest, and every book's
    /// grant with its authority's size — zero for a book the cell no longer
    /// authorises, which refuses new holds and keeps the ones a coordinator
    /// was already told about.
    pub fn authorise(&mut self, authorities: Vec<LegAuthority>) {
        for ((venue, instrument, side), book) in &mut self.books {
            let grant = authorities
                .iter()
                .find(|a| &a.venue == venue && &a.instrument == instrument && a.side == *side)
                .map_or(Decimal::ZERO, |a| a.max_size);
            book.set_grant(grant);
        }
        self.authorities = authorities;
    }

    /// One pass over every held cycle. Frames go out through `session`,
    /// each to the nodes holding a leg of its epoch and never to every peer.
    pub fn step(&mut self, session: &mut PeerSession, now: Timestamp) -> SagaStep {
        let mut step = SagaStep::default();
        self.forget_gone(session, now, &mut step);
        self.originate(session, now, &mut step);
        self.reserve(session, now, &mut step);
        self.fire(session, now, &mut step);
        self.execute(session, now, &mut step);
        self.unwind(session, &mut step);
        self.close(session, &mut step);
        let refusals = session.refusals();
        step.rejected = refusals[self.refusals_read.min(refusals.len())..]
            .iter()
            .filter(|r| r.kind == "guarantee_unmet")
            .count() as u64;
        self.refusals_read = refusals.len();
        step
    }

    fn authority(&self, execution: &LegExecution, now: Timestamp) -> Option<&LegAuthority> {
        let side = match execution.side {
            LegSide::Buy => Side::Buy,
            LegSide::Sell => Side::Sell,
        };
        self.authorities.iter().find(|a| {
            a.venue == execution.venue
                && a.instrument == execution.instrument
                && a.side == side
                && a.valid_until_ns >= now.as_nanos()
        })
    }

    /// The cell's gate for one leg, as its newest authority defines it.
    fn gate(&self, leg: &EpochLeg, now: Timestamp) -> qip_core::error::Result<()> {
        use qip_core::error::Error;
        let execution = leg.execution.as_ref().ok_or_else(|| {
            Error::denied("the leg names no venue, instrument, side or limit to judge it by")
        })?;
        let authority = self.authority(execution, now).ok_or_else(|| {
            Error::denied(format!(
                "this cell authorises no {} of {} at {} now",
                execution.side.as_str(),
                execution.instrument,
                execution.venue
            ))
        })?;
        if leg.size > authority.max_size {
            return Err(Error::denied(format!(
                "a leg of {} exceeds the {} this cell authorises",
                leg.size, authority.max_size
            )));
        }
        let priced_worse = match execution.side {
            LegSide::Buy => execution.limit_price < authority.limit_price,
            LegSide::Sell => execution.limit_price > authority.limit_price,
        };
        if priced_worse {
            return Err(Error::denied(format!(
                "the leg's limit {} is worse than this cell's touch {}",
                execution.limit_price, authority.limit_price
            )));
        }
        let unwind = leg
            .size
            .checked_mul(authority.unwind_per_unit)
            .ok_or_else(|| Error::denied("the unwind cost of the leg overflows"))?;
        if leg.unwind_loss < unwind {
            return Err(Error::denied(format!(
                "the declared unwind loss {} does not cover crossing back out at this book, {unwind}",
                leg.unwind_loss
            )));
        }
        Ok(())
    }

    /// Drop what the endpoint no longer holds, counting each cycle this node
    /// originated that left without FIRE.
    ///
    /// This is the one place an originated epoch that lapsed unfired is
    /// seen: the endpoint journals a `guarantee_unmet` refusal for every
    /// unfired epoch it expires, but it does so for epochs this node merely
    /// joined as well, so that count cannot say whether a cycle *this cell
    /// started* was refused. Until R12-RES-7095 the saga dropped such a cycle
    /// here in silence, and a peer lost mid-epoch was indistinguishable, in
    /// the status, from a cycle that was never offered.
    fn forget_gone(&mut self, session: &PeerSession, now: Timestamp, step: &mut SagaStep) {
        let held: BTreeSet<String> = session.endpoint().opportunities().into_iter().collect();
        let fired = &self.fired;
        let mut lapsed = 0u64;
        self.open.retain(|opportunity, _| {
            let still = held.contains(opportunity);
            if !still && !fired.contains(opportunity) {
                lapsed += 1;
            }
            still
        });
        step.expired = lapsed;
        self.fired.retain(|opportunity| held.contains(opportunity));
        self.attempted
            .retain(|opportunity| held.contains(opportunity));
        self.executed
            .retain(|(opportunity, _), _| held.contains(opportunity));
        self.unwound
            .retain(|(opportunity, _)| held.contains(opportunity));
        self.closed.retain(|opportunity| held.contains(opportunity));
        for book in self.books.values_mut() {
            book.expire(now);
        }
    }

    /// Open a cycle on each crossing this node coordinates.
    fn originate(&mut self, session: &mut PeerSession, now: Timestamp, step: &mut SagaStep) {
        let node = session.endpoint().node().to_string();
        let mut drafts = Vec::new();
        for authority in self.authorities.clone() {
            if authority.valid_until_ns < now.as_nanos() {
                continue;
            }
            for peer in session.endpoint().edge_peers() {
                if peer <= node || self.open.len() + drafts.len() >= MAX_OPEN_CYCLES {
                    continue;
                }
                let already = self
                    .open
                    .values()
                    .any(|(i, p)| *i == authority.instrument && *p == peer);
                let Some(clock) = session.endpoint().peer_clock(&peer, now) else {
                    continue;
                };
                if already {
                    continue;
                }
                let Some(edges) = session.endpoint().peer_edges(&peer) else {
                    continue;
                };
                for edge in edges
                    .iter()
                    .filter(|e| e.instrument == authority.instrument)
                {
                    // The peer stamps its touch on its own clock; on ours it
                    // is that stamp less the measured offset.
                    let seen = edge.as_of_ns.saturating_sub(clock.offset_nanos);
                    if now.as_nanos().saturating_sub(seen) > EDGE_FRESHNESS.as_nanos() {
                        continue;
                    }
                    let (buy, sell, peer_side, peer_limit, peer_spread, peer_touch) =
                        match authority.side {
                            Side::Buy => (
                                authority.limit_price,
                                edge.bid,
                                LegSide::Sell,
                                edge.bid,
                                edge.ask - edge.bid,
                                edge.bid_size,
                            ),
                            Side::Sell => (
                                edge.ask,
                                authority.limit_price,
                                LegSide::Buy,
                                edge.ask,
                                edge.ask - edge.bid,
                                edge.ask_size,
                            ),
                        };
                    // The round trip the wire measured to this peer is the
                    // coordination latency the crossing is exposed over.
                    if !clears(
                        buy,
                        sell,
                        Duration::from_nanos(clock.round_trip_nanos.max(0)),
                    ) {
                        continue;
                    }
                    let size = authority.max_size.min(authority.touch_size).min(peer_touch);
                    if !size.is_positive() {
                        continue;
                    }
                    if let Some(epoch) = self.draft(
                        &node,
                        &peer,
                        &authority,
                        size,
                        LegExecution {
                            venue: edge.venue.clone(),
                            instrument: edge.instrument.clone(),
                            side: peer_side,
                            limit_price: peer_limit,
                        },
                        peer_spread,
                        now,
                    ) {
                        drafts.push((epoch, authority.instrument.clone(), peer.clone()));
                        break;
                    }
                }
            }
        }
        for (epoch, instrument, peer) in drafts {
            let opportunity = epoch.opportunity.clone();
            let recipients = vec![peer.clone()];
            if let Ok(token) = session.endpoint_mut().open_epoch(epoch, now) {
                session.send_to(recipients, token);
                self.open.insert(opportunity, (instrument, peer));
                step.originated += 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draft(
        &mut self,
        node: &str,
        peer: &str,
        authority: &LegAuthority,
        size: Decimal,
        peer_execution: LegExecution,
        peer_spread: Decimal,
        now: Timestamp,
    ) -> Option<OpportunityEpoch> {
        let own_unwind = size.checked_mul(authority.unwind_per_unit)?;
        let peer_unwind = size.checked_mul(peer_spread)?;
        let own_side = match authority.side {
            Side::Buy => LegSide::Buy,
            Side::Sell => LegSide::Sell,
        };
        self.next = self.next.wrapping_add(1);
        EpochDraft {
            opportunity: Some(format!("{node}-{}-{}", self.started_ns, self.next)),
            sequence: Some(1),
            cycle: Some(format!(
                "{}:{}>{}",
                authority.instrument, authority.venue, peer_execution.venue
            )),
            legs: Some(vec![
                EpochLeg {
                    leg: format!("{node}:{}", own_side.as_str()),
                    node: node.to_string(),
                    size,
                    unwind_loss: own_unwind,
                    execution: Some(LegExecution {
                        venue: authority.venue.clone(),
                        instrument: authority.instrument.clone(),
                        side: own_side,
                        limit_price: authority.limit_price,
                    }),
                },
                EpochLeg {
                    leg: format!("{peer}:{}", peer_execution.side.as_str()),
                    node: peer.to_string(),
                    size,
                    unwind_loss: peer_unwind,
                    execution: Some(peer_execution),
                },
            ]),
            size: Some(size),
            min_edge_bps: Some(Decimal::from_int(MIN_EDGE_BPS)),
            model_versions: Some(vec![DETECTOR.to_string()]),
            opened_at_nanos: Some(now.as_nanos()),
            ttl: Some(EPOCH_TTL),
            // The unwind policy: recovering both legs at their spreads. A
            // stranded cycle costs at most what crossing back out costs, and
            // FIRE is refused if the legs' bounds do not fit inside this.
            max_recovery_loss: own_unwind.checked_add(peer_unwind),
        }
        .build(Duration::from_secs(30))
        .ok()
    }

    /// Put this node's legs of every newly held epoch to its gate, then its
    /// book, and tell the coordinator either way.
    fn reserve(&mut self, session: &mut PeerSession, now: Timestamp, step: &mut SagaStep) {
        let node = session.endpoint().node().to_string();
        for opportunity in session.endpoint().opportunities() {
            if self.attempted.contains(&opportunity) || session.endpoint().is_fired(&opportunity) {
                continue;
            }
            let Some(epoch) = session.endpoint().epoch(&opportunity).cloned() else {
                continue;
            };
            let own: Vec<&EpochLeg> = epoch.legs.iter().filter(|l| l.node == node).collect();
            if own.is_empty() {
                continue;
            }
            self.attempted.insert(opportunity.clone());
            // The protocol reserves a node's legs against one book; legs on
            // two books are declined by the gate rather than mixed in one.
            let key = own[0].execution.as_ref().map(|e| {
                let side = match e.side {
                    LegSide::Buy => Side::Buy,
                    LegSide::Sell => Side::Sell,
                };
                (e.venue.clone(), e.instrument.clone(), side)
            });
            let one_book = own.iter().all(|l| {
                l.execution
                    .as_ref()
                    .map(|e| (&e.venue, &e.instrument, e.side))
                    == own[0]
                        .execution
                        .as_ref()
                        .map(|e| (&e.venue, &e.instrument, e.side))
            });
            let grant = key
                .as_ref()
                .and_then(|(v, i, s)| {
                    self.authorities
                        .iter()
                        .find(|a| &a.venue == v && &a.instrument == i && a.side == *s)
                })
                .map_or(Decimal::ZERO, |a| a.max_size);
            let mut book = key
                .as_ref()
                .and_then(|k| self.books.remove(k))
                .unwrap_or_else(|| ReservationBook::new(grant));
            book.set_grant(grant);
            let outcome = {
                let saga: &Saga = self;
                let gate = |leg: &EpochLeg| {
                    if !one_book {
                        return Err(qip_core::error::Error::denied(
                            "this node's legs of the epoch are on more than one book",
                        ));
                    }
                    saga.gate(leg, now)
                };
                session
                    .endpoint_mut()
                    .reserve_own_legs(&opportunity, now, &mut book, &gate)
            };
            if let Some(k) = key {
                self.books.insert(k, book);
            }
            self.record_outcome(session, &opportunity, outcome, step);
        }
    }

    fn record_outcome(
        &self,
        session: &PeerSession,
        opportunity: &str,
        outcome: qip_core::error::Result<ReservationOutcome>,
        step: &mut SagaStep,
    ) {
        let recipients = session
            .endpoint()
            .recipients(opportunity)
            .unwrap_or_default();
        match outcome {
            Ok(ReservationOutcome::Reserved(frames)) => {
                step.reservations += 1;
                for frame in frames {
                    session.send_to(recipients.clone(), frame);
                }
            }
            Ok(ReservationOutcome::Declined(frame)) => {
                step.declines += 1;
                session.send_to(recipients, frame);
            }
            // Journaled by the endpoint; nothing to send.
            Err(_) => {}
        }
    }

    /// Declare FIRE on every cycle this node coordinates whose legs are all
    /// held. The endpoint refuses, by name, one whose loss bound cannot be
    /// established.
    fn fire(&mut self, session: &mut PeerSession, now: Timestamp, step: &mut SagaStep) {
        for opportunity in session.endpoint().opportunities() {
            let endpoint = session.endpoint();
            if !endpoint.coordinates(&opportunity)
                || endpoint.is_fired(&opportunity)
                || !endpoint.every_leg_reserved(&opportunity)
            {
                continue;
            }
            let recipients = endpoint.recipients(&opportunity).unwrap_or_default();
            if let Ok(frame) = session.endpoint_mut().declare_fire(&opportunity, now) {
                session.send_to(recipients, frame);
                self.fired.insert(opportunity);
                step.fires += 1;
            }
        }
    }

    /// Send and fill this node's legs of every fired cycle, in paper.
    fn execute(&mut self, session: &mut PeerSession, now: Timestamp, step: &mut SagaStep) {
        let node = session.endpoint().node().to_string();
        for opportunity in session.endpoint().opportunities() {
            if !session.endpoint().is_fired(&opportunity) {
                continue;
            }
            let Some(epoch) = session.endpoint().epoch(&opportunity).cloned() else {
                continue;
            };
            let recipients = session
                .endpoint()
                .recipients(&opportunity)
                .unwrap_or_default();
            for leg in epoch.legs.iter().filter(|l| l.node == node) {
                let key = (opportunity.clone(), leg.leg.clone());
                if self.executed.contains_key(&key)
                    || session.endpoint().leg_sent(&opportunity, &leg.leg)
                {
                    continue;
                }
                let saga: &Saga = self;
                let gate = |l: &EpochLeg| saga.gate(l, now);
                if session
                    .endpoint_mut()
                    .send_leg(&opportunity, &leg.leg, now, &gate)
                    .is_err()
                {
                    continue;
                }
                step.legs_sent += 1;
                // Paper: the leg fills against the size the cell's touch
                // still shows, which is less than the leg when it shrank.
                let Some((available, spread)) = leg
                    .execution
                    .as_ref()
                    .and_then(|e| self.authority(e, now))
                    .map(|a| (a.touch_size, a.unwind_per_unit))
                else {
                    continue;
                };
                let quantity = leg.size.min(available);
                self.executed.insert(key.clone(), spread);
                if let Some((v, i, s)) = leg.execution.as_ref().map(|e| {
                    let side = match e.side {
                        LegSide::Buy => Side::Buy,
                        LegSide::Sell => Side::Sell,
                    };
                    (e.venue.clone(), e.instrument.clone(), side)
                }) && let Some(book) = self.books.get_mut(&(v, i, s))
                {
                    // Executed: the hold has become the trade.
                    book.release_leg(&opportunity, &leg.leg);
                }
                if quantity.is_positive()
                    && let Ok(frame) =
                        session
                            .endpoint_mut()
                            .record_own_fill(&opportunity, &leg.leg, quantity)
                {
                    session.send_to(recipients.clone(), frame);
                    step.fills += 1;
                }
            }
        }
    }

    /// Unwind what this node filled beyond its counterparties, once their
    /// fills are known (MESH-5065).
    fn unwind(&mut self, session: &mut PeerSession, step: &mut SagaStep) {
        let executed: Vec<((String, String), Decimal)> =
            self.executed.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for ((opportunity, leg), spread) in executed {
            if self.unwound.contains(&(opportunity.clone(), leg.clone())) {
                continue;
            }
            let endpoint = session.endpoint();
            let (Some(epoch), Some(own)) = (
                endpoint.epoch(&opportunity),
                endpoint.filled(&opportunity, &leg),
            ) else {
                continue;
            };
            let others: Option<Vec<Decimal>> = epoch
                .legs
                .iter()
                .filter(|l| l.leg != leg)
                .map(|l| endpoint.filled(&opportunity, &l.leg))
                .collect();
            let Some(least) = others.and_then(|fills| fills.into_iter().min()) else {
                continue;
            };
            self.unwound.insert((opportunity.clone(), leg.clone()));
            if own <= least {
                continue;
            }
            let Some(loss) = (own - least).checked_mul(spread) else {
                continue;
            };
            let recipients = endpoint.recipients(&opportunity).unwrap_or_default();
            if let Ok(frame) = session
                .endpoint_mut()
                .record_own_unwind(&opportunity, &leg, loss)
            {
                session.send_to(recipients, frame);
                step.unwinds += 1;
            }
        }
    }
    /// The tasks closed cycles have raised since the last call, in the order
    /// they closed.
    pub fn take_rebalance_tasks(&mut self) -> Vec<RebalanceTask> {
        std::mem::take(&mut self.rebalance)
    }

    /// Raise the drift a fired cycle left, once every leg's fill is known
    /// (ADR 0154 decision 6, MESH-033).
    ///
    /// Legs land wherever their venues are, and a leg that filled less than
    /// planned leaves its place short while the protocol unwinds the others
    /// down to the least fill. Without this the next cycle starts from a
    /// position nobody chose. Held is each leg's fill capped at the least
    /// fill, because that is where the unwind leaves it whichever node does
    /// the unwinding; wanted is the leg's planned size. Places are one per
    /// node, venue and instrument, because units of two instruments do not
    /// add. A cycle with an unknown fill is not closed: drift measured
    /// against a fill nobody has reported would be a guess.
    fn close(&mut self, session: &PeerSession, step: &mut SagaStep) {
        let endpoint = session.endpoint();
        for opportunity in endpoint.opportunities() {
            if self.closed.contains(&opportunity) || !endpoint.is_fired(&opportunity) {
                continue;
            }
            let Some(epoch) = endpoint.epoch(&opportunity) else {
                continue;
            };
            let fills: Option<Vec<(&EpochLeg, Decimal)>> = epoch
                .legs
                .iter()
                .map(|leg| endpoint.filled(&opportunity, &leg.leg).map(|f| (leg, f)))
                .collect();
            let Some(fills) = fills else {
                continue;
            };
            let Some(least) = fills.iter().map(|(_, filled)| *filled).min() else {
                continue;
            };
            self.closed.insert(opportunity.clone());
            let mut target: BTreeMap<String, Decimal> = BTreeMap::new();
            let mut actual: BTreeMap<String, Decimal> = BTreeMap::new();
            let mut overflow = false;
            for (leg, filled) in fills {
                let place = match &leg.execution {
                    Some(e) => format!("{}/{}/{}", leg.node, e.venue, e.instrument),
                    None => format!("{}/{}", leg.node, leg.leg),
                };
                for (book, add) in [(&mut target, leg.size), (&mut actual, filled.min(least))] {
                    let held = book.entry(place.clone()).or_insert(Decimal::ZERO);
                    match held.checked_add(add) {
                        Some(sum) => *held = sum,
                        None => overflow = true,
                    }
                }
            }
            if overflow {
                continue;
            }
            // Zero tolerance: this only reports, it never moves anything, so
            // reporting every unit short is the cautious reading.
            if let Ok(tasks) = rebalancing_tasks(epoch, &target, &actual, Decimal::ZERO) {
                step.closed += 1;
                self.rebalance.extend(tasks);
            }
        }
    }
}

/// Whether selling at `sell` what was bought at `buy` clears
/// [`MIN_EDGE_BPS`] and the expected adverse move over `coordination`, the
/// measured round trip to the peer whose touch `sell` (or `buy`) is.
///
/// The move is `qip_arbitrage::netedge`'s own latency deduction with that
/// round trip as its coordination latency, on one unit bought at `buy` — the
/// same figure the cell's desk charges a cycle of its own, so the peer
/// crossing is not priced by a kinder model than the local one. A deduction
/// the calculator refuses (a statistic it cannot carry) clears nothing.
fn clears(buy: Decimal, sell: Decimal, coordination: Duration) -> bool {
    if !buy.is_positive() || sell <= buy {
        return false;
    }
    let Some(edge) = (sell - buy).checked_mul(Decimal::from_int(10_000)) else {
        return false;
    };
    let Some(floor) = buy.checked_mul(Decimal::from_int(MIN_EDGE_BPS)) else {
        return false;
    };
    let calculator = NetEdgeCalculator::new(EdgeAssumptions {
        coordination_latency: coordination,
        ..EdgeAssumptions::default()
    });
    let Ok(move_per_unit) = calculator.latency_deduction(buy) else {
        return false;
    };
    let Some(move_bps) = move_per_unit.checked_mul(Decimal::from_int(10_000)) else {
        return false;
    };
    edge >= floor + move_bps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crossing_clears_only_at_or_above_the_minimum_edge() {
        let d = |s: &str| Decimal::parse(s).expect("a literal");
        let none = Duration::from_nanos(0);
        // At zero coordination latency the default assumptions still charge
        // the venue round trip, so exactly five basis points no longer clears;
        // the floor the venue round trip adds is under a third of one.
        assert!(clears(d("100"), d("100.06"), none), "six basis points");
        assert!(
            !clears(d("100"), d("100.05"), none),
            "five, with the venue trip"
        );
        assert!(!clears(d("100"), d("100.049"), none));
        assert!(!clears(d("100"), d("99"), none));
        assert!(!clears(d("0"), d("1"), none));
    }

    /// R12-MESH-5069: a crossing that clears at zero coordination latency is
    /// declined once the round trip the wire measured to the peer is priced.
    /// Five and a half basis points clears the floor plus the venue round
    /// trip's own move (about 0.28 bps at the default 250 ms and 40%
    /// annualised volatility); a 900 ms measured round trip to the peer
    /// raises that move to about 0.61 bps and the same crossing is refused.
    ///
    /// Mutation (run, failed here, restored): `clears` passes
    /// `Duration::from_nanos(0)` to the calculator in place of
    /// `coordination` — the second assertion fails, the crossing clearing
    /// whatever the wire measured.
    #[test]
    fn a_peer_crossing_clearing_at_zero_latency_is_declined_once_the_measured_round_trip_is_priced()
    {
        let d = |s: &str| Decimal::parse(s).expect("a literal");
        let (buy, sell) = (d("100"), d("100.055"));
        assert!(
            clears(buy, sell, Duration::from_nanos(0)),
            "premise: the crossing clears with no coordination latency"
        );
        assert!(
            !clears(buy, sell, Duration::from_millis(900)),
            "a 900 ms measured round trip did not lower the net edge below the floor"
        );
        // And the deduction is monotone in the measurement: a wider trip
        // never admits what a narrower one refused.
        assert!(!clears(buy, sell, Duration::from_millis(1000)));
    }
}
