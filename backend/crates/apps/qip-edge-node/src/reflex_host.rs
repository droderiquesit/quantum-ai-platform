//! The node's host for the microsecond row's issuer (R12-WORLD-3084,
//! R12-WORLD-7197, ADR 0506, ADR 0633).
//!
//! `qip_edge::reflex_forecast::ReflexIssuer` stated and scored the lattice's
//! microsecond row and had no caller outside its tests: no pass fed it and
//! no composition root built it, so the row was still never issued on any
//! path a binary runs. This module is that caller. It watches what a pass
//! already reported — the orders the venue accepted and the fills it
//! confirmed — and feeds the issuer from them; it reads nothing the cell
//! decides on and the cell reads nothing it issues (ADR 0506 decision 5,
//! ADR 0195 decision 1).
//!
//! # What is fed, and what is not
//!
//! Only [`ReflexTarget::FillProbability`] is fed, from the one outcome the
//! pass sees honestly: whether an order the venue accepted was filled inside
//! the horizon. The forecast for an order is issued **when the order is
//! placed**, before its outcome exists, and scored when the horizon has
//! elapsed; a forecast issued after the outcome would be calibrated by
//! construction and prove nothing. Queue movement, adverse selection and
//! executable edge need observations the pass does not carry (queue
//! position, post-fill price, net edge at decision) and are not issued.
//!
//! # The horizon is the operator's
//!
//! No default is chosen here. [`REFLEX_HORIZON_VARIABLE`] declares it; unset,
//! the node hosts no issuer ([`parse_reflex_horizon`] returns `None`). A
//! value outside the microsecond row is refused at start, not clamped.
//!
//! # Bounds
//!
//! [`MAX_PENDING`] orders await their horizon and [`MAX_RECENT`] issued
//! forecasts are kept for inspection. An order past the first is counted in
//! [`ReflexHost::untracked`] and not tracked, rather than evicting a pending
//! one whose score would then silently never arrive.

use crate::pass::PassOutcome;
use qip_contracts::reflex_forecast::{MAX_REFLEX_HORIZON_US, ReflexForecast, ReflexTarget};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_edge::reflex_forecast::ReflexIssuer;
use std::collections::{BTreeMap, VecDeque};

/// The variable declaring the microsecond row's horizon, in microseconds.
pub const REFLEX_HORIZON_VARIABLE: &str = "QIP_REFLEX_HORIZON_US";

/// Orders awaiting their horizon.
pub const MAX_PENDING: usize = 4_096;

/// Issued forecasts kept for inspection.
pub const MAX_RECENT: usize = 256;

/// Read the declared horizon. Unset or blank is `None` (no issuer); anything
/// else must be an integer inside the microsecond row.
pub fn parse_reflex_horizon(value: Option<&str>) -> Result<Option<u64>> {
    let Some(text) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let horizon: u64 = text.parse().map_err(|_| {
        Error::invalid(format!(
            "configuration: {REFLEX_HORIZON_VARIABLE}={text:?} is not a whole number of \
             microseconds; give an integer from 1 to {MAX_REFLEX_HORIZON_US}"
        ))
    })?;
    if horizon == 0 || horizon > MAX_REFLEX_HORIZON_US {
        return Err(Error::invalid(format!(
            "configuration: {REFLEX_HORIZON_VARIABLE}={horizon} is outside the microsecond row; \
             give an integer from 1 to {MAX_REFLEX_HORIZON_US}"
        )));
    }
    Ok(Some(horizon))
}

#[derive(Debug)]
struct Pending {
    venue: VenueId,
    instrument: String,
    placed_at: Timestamp,
    filled_at: Option<Timestamp>,
    forecast: Option<ReflexForecast>,
}

/// The issuer, fed from the passes.
#[derive(Debug)]
pub struct ReflexHost {
    issuer: ReflexIssuer,
    horizon: Duration,
    pending: BTreeMap<String, Pending>,
    recent: VecDeque<ReflexForecast>,
    issued: u64,
    resolved: u64,
    untracked: u64,
}

impl ReflexHost {
    pub fn new(cell: impl Into<String>, horizon_us: u64) -> Result<Self> {
        let issuer = ReflexIssuer::new(cell, horizon_us)?;
        let horizon = Duration::from_micros(i64::try_from(horizon_us).unwrap_or(i64::MAX));
        Ok(Self {
            issuer,
            horizon,
            pending: BTreeMap::new(),
            recent: VecDeque::new(),
            issued: 0,
            resolved: 0,
            untracked: 0,
        })
    }

    /// Take one turn's pass outcome: book the fills it confirmed, resolve
    /// every order whose horizon has now elapsed, then issue a forecast for
    /// each order the pass placed. In that order, so an order placed this
    /// pass is forecast from outcomes that existed before it.
    pub fn observe(&mut self, outcome: &PassOutcome, now: Timestamp) -> Result<()> {
        let (fills, orders): (&[_], &[_]) = match outcome {
            PassOutcome::Ran { report, .. } => (&report.fills, &report.orders),
            PassOutcome::Halted { fills, .. } => (fills, &[]),
        };
        for fill in fills {
            if let Some(pending) = self.pending.get_mut(&fill.order_id) {
                pending.filled_at = Some(pending.filled_at.map_or(fill.at, |t| t.min(fill.at)));
            }
        }
        self.resolve_due(now)?;
        for order in orders {
            if self.pending.len() >= MAX_PENDING {
                self.untracked = self.untracked.saturating_add(1);
                continue;
            }
            let instrument = order.object_id.as_str().to_string();
            let forecast = self.issuer.issue(
                &order.venue,
                &instrument,
                ReflexTarget::FillProbability,
                now,
            )?;
            if let Some(forecast) = &forecast {
                self.issued = self.issued.saturating_add(1);
                self.recent.push_back(forecast.clone());
                if self.recent.len() > MAX_RECENT {
                    self.recent.pop_front();
                }
            }
            self.pending.insert(
                order.order_id.clone(),
                Pending {
                    venue: order.venue.clone(),
                    instrument,
                    placed_at: now,
                    filled_at: None,
                    forecast,
                },
            );
        }
        Ok(())
    }

    fn resolve_due(&mut self, now: Timestamp) -> Result<()> {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.placed_at.saturating_add(self.horizon))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(pending) = self.pending.remove(&id) else {
                continue;
            };
            let deadline = pending.placed_at.saturating_add(self.horizon);
            let occurred = pending.filled_at.is_some_and(|at| at <= deadline);
            if let Some(forecast) = &pending.forecast {
                self.issuer.resolve_event(forecast, occurred)?;
                self.resolved = self.resolved.saturating_add(1);
            }
            self.issuer.observe_event(
                &pending.venue,
                &pending.instrument,
                ReflexTarget::FillProbability,
                occurred,
            )?;
        }
        Ok(())
    }

    /// Forecasts issued since the node started.
    pub fn issued(&self) -> u64 {
        self.issued
    }

    /// Forecasts scored against what the venue then did.
    pub fn resolved(&self) -> u64 {
        self.resolved
    }

    /// Orders not tracked because [`MAX_PENDING`] were already waiting.
    pub fn untracked(&self) -> u64 {
        self.untracked
    }

    /// The most recent forecasts, oldest first.
    pub fn recent(&self) -> impl Iterator<Item = &ReflexForecast> {
        self.recent.iter()
    }

    /// The issuer's own measured score for fill probability.
    pub fn brier(&self) -> Option<qip_core::Decimal> {
        self.issuer.brier(ReflexTarget::FillProbability)
    }
}
