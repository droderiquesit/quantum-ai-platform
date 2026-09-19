//! Settlement terms per venue, and when a leg's proceeds are usable (§56.2
//! rule 21, §32.2).
//!
//! Blueprint §32.2 puts one sentence at the centre of this: "Reservation is
//! future-tense: availability is checked at the moment each leg needs it."
//! The cell's region ledger ([`crate::reservation`]) holds a cycle gross —
//! every leg's notional out of settled capital, taken before an `Intent`
//! exists — so no leg is funded from another leg's proceeds *in the ledger*.
//! But a cycle is a chain: leg two spends what leg one bought, at the venue
//! leg one bought it. Whether that asset is usable when leg two fires is not
//! a question the ledger can answer, because the ledger counts money and the
//! question is about time. §32.2's stage table answers it: proceeds are
//! **in-settlement** until the venue's calendar says otherwise, and
//! in-settlement capital is usable as funding "only via a bridge, at a priced
//! cost". This cell has no bridge — no margin line, no broker credit, no
//! cross-margin — so the only row of the bridge table it can take is the
//! last: wait for settlement. A cycle cannot wait. It is vetoed whole.
//!
//! What this module holds is the fact that decides that veto: for each venue,
//! how long after a fill the proceeds are usable. [`SettlementTerms::instant`]
//! is a venue that credits proceeds when it reports the fill — a paper
//! simulator, or a crypto exchange booking spot internally. A venue on a
//! settlement cycle carries the convention (T+0, T+1, T+2), a cut-off minute,
//! an availability minute and the days that settle, and projects the instant
//! proceeds land by walking settlement days rather than calendar days, so a
//! Friday T+1 lands on Monday and a Thursday T+2 lands on Monday. That walk
//! is the fact `qip-capital-fabric`'s `SettlementCalendar` computes for the
//! centre; the discipline is reproduced here rather than reused for the
//! reason `reservation.rs` gives — a cell that reached a capital service
//! could widen its own bound, and the acceptance suite refuses the edge.
//!
//! # What the cell does not know, stated
//!
//! A venue the cell holds no terms for is **not projected**. It is not read
//! as instant — that is the permissive guess — and not as T+2 — that is a
//! number nobody computed. The feasibility gate takes the same position on a
//! venue with no lot size (`feasibility.rs`, "what the gate knows, and from
//! where"), and for the same reason: the fact is the composition root's to
//! supply, and inventing it would be a rounding rule wearing a refusal's
//! clothes. The count of configured venues without terms is published on a
//! gauge every pass so that the silence is a number on a chart rather than a
//! gate that appears to be passing.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The gate literal a cycle is refused under when a leg would be funded by
/// proceeds that are still in settlement when it fires. A `pub const` so the
/// `qip_edge_refusals_total{gate}` label stays bounded by source rather than
/// by a runtime string.
pub const GATE_SETTLEMENT: &str = "settlement_reservation";

/// How many settlement days after the value date proceeds are usable.
///
/// The same three arms as the centre's convention, spelled the same, so a
/// value the centre ships per venue can be read here without translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementConvention {
    /// The same settlement day, if the fill makes the cut-off.
    T0,
    /// The next settlement day.
    T1,
    /// Two settlement days on.
    T2,
}

impl SettlementConvention {
    pub const fn days(self) -> u32 {
        match self {
            Self::T0 => 0,
            Self::T1 => 1,
            Self::T2 => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::T0 => "T+0",
            Self::T1 => "T+1",
            Self::T2 => "T+2",
        }
    }

    /// Parse the spelling [`Self::as_str`] produces. Refuses anything else
    /// rather than defaulting: a convention typed wrong is a venue whose
    /// proceeds land on a day nobody chose.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "T+0" | "t+0" | "T0" | "t0" => Ok(Self::T0),
            "T+1" | "t+1" | "T1" | "t1" => Ok(Self::T1),
            "T+2" | "t+2" | "T2" | "t2" => Ok(Self::T2),
            other => Err(Error::invalid(format!(
                "settlement convention {other:?} is not one of T+0, T+1, T+2 or instant"
            ))),
        }
    }
}

/// When one venue's proceeds become usable as funding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementTerms {
    /// `None` is a venue that credits proceeds when it reports the fill.
    cycle: Option<SettlementCycle>,
}

/// A venue that settles on a calendar rather than on the fill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SettlementCycle {
    convention: SettlementConvention,
    /// A fill at or after this UTC minute of the day is dated to the next
    /// settlement day.
    cutoff_minute: u32,
    /// Proceeds are usable from this UTC minute of the settlement day.
    availability_minute: u32,
    /// Weekdays that settle, Monday = 0. Saturday and Sunday are absent from
    /// the weekday constructor, which is the one fact this module exists to
    /// stop a cycle forgetting.
    settlement_weekdays: BTreeSet<u32>,
    /// Non-settlement days, as day numbers since the epoch.
    holidays: BTreeSet<i64>,
}

/// Minutes in a day; a cut-off or an availability at or past it names no
/// instant.
const MINUTES_PER_DAY: u32 = 1440;

/// The furthest ahead a projection walks before refusing. Bounded rather
/// than a loop that trusts its input: terms with every weekday a holiday
/// would otherwise spin on the order path.
const MAXIMUM_ROLL_DAYS: u32 = 400;

impl SettlementTerms {
    /// A venue whose proceeds are usable the instant it reports the fill.
    pub const fn instant() -> Self {
        Self { cycle: None }
    }

    /// A venue settling Monday to Friday on the given convention, with a
    /// 16:00 UTC cut-off and proceeds usable at 09:00 UTC on the settlement
    /// day — 17:00 for T+0, which must hand over after its own cut-off.
    ///
    /// The same figures the centre's weekday calendar uses, on purpose: a
    /// cell and the centre projecting one fill to two different days would
    /// be two claims about one fact.
    pub fn weekday(convention: SettlementConvention) -> Result<Self> {
        let availability = match convention {
            SettlementConvention::T0 => 17 * 60,
            _ => 9 * 60,
        };
        Self::on_cycle(convention, 16 * 60, availability, [0, 1, 2, 3, 4])
    }

    /// A venue settling on a calendar.
    ///
    /// Refused when either minute names no instant in a day, when no weekday
    /// settles at all — terms under which no proceeds ever land are not
    /// terms, they are a venue that should not be configured — and when a
    /// T+0 venue would hand proceeds over before its own cut-off, which
    /// models settlement before instruction.
    pub fn on_cycle(
        convention: SettlementConvention,
        cutoff_minute: u32,
        availability_minute: u32,
        settlement_weekdays: impl IntoIterator<Item = u32>,
    ) -> Result<Self> {
        if cutoff_minute >= MINUTES_PER_DAY || availability_minute >= MINUTES_PER_DAY {
            return Err(Error::invalid(format!(
                "a settlement cut-off ({cutoff_minute}) and availability ({availability_minute}) \
                 are minutes within a day, so both must be below {MINUTES_PER_DAY}"
            )));
        }
        if convention == SettlementConvention::T0 && availability_minute < cutoff_minute {
            return Err(Error::invalid(format!(
                "T+0 terms cannot make proceeds usable at minute {availability_minute} against a \
                 cut-off at minute {cutoff_minute}; that is settlement before the fill"
            )));
        }
        let settlement_weekdays: BTreeSet<u32> = settlement_weekdays.into_iter().collect();
        if settlement_weekdays.is_empty() {
            return Err(Error::invalid(
                "settlement terms name no weekday that settles, so no proceeds would ever be \
                 usable; name the settling weekdays or configure the venue as instant",
            ));
        }
        if let Some(day) = settlement_weekdays.iter().find(|day| **day > 6) {
            return Err(Error::invalid(format!(
                "settlement weekday {day} is not a weekday; Monday is 0 and Sunday is 6"
            )));
        }
        Ok(Self {
            cycle: Some(SettlementCycle {
                convention,
                cutoff_minute,
                availability_minute,
                settlement_weekdays,
                holidays: BTreeSet::new(),
            }),
        })
    }

    /// Parse the composition root's spelling: `instant`, or a convention as
    /// [`SettlementConvention::as_str`] writes it, on the weekday calendar.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "instant" | "INSTANT" | "Instant" => Ok(Self::instant()),
            other => Self::weekday(SettlementConvention::parse(other)?),
        }
    }

    /// Add a non-settlement day. A no-op for instant terms, because a venue
    /// that credits on the fill has no settlement day to skip.
    #[must_use]
    pub fn with_holiday(mut self, date: Timestamp) -> Self {
        if let Some(cycle) = self.cycle.as_mut() {
            cycle.holidays.insert(day_number(date));
        }
        self
    }

    pub const fn is_instant(&self) -> bool {
        self.cycle.is_none()
    }

    /// The convention, or `None` for instant terms.
    pub fn convention(&self) -> Option<SettlementConvention> {
        self.cycle.as_ref().map(|cycle| cycle.convention)
    }

    /// A stable label for refusal messages: `instant` or the convention.
    pub fn describe(&self) -> &'static str {
        match self.cycle.as_ref() {
            None => "instant",
            Some(cycle) => cycle.convention.as_str(),
        }
    }

    /// The instant proceeds of a fill at `filled_at` are usable as funding.
    ///
    /// Instant terms return `filled_at` itself. Terms on a cycle date the
    /// fill to today if today settles and the cut-off has not passed, else
    /// to the next settlement day, then advance the convention's number of
    /// *settlement* days, then apply the availability minute. Monotone in
    /// `filled_at`: a later fill never lands earlier, which is what lets a
    /// caller that does not know exactly when a leg will fill project from
    /// the earliest instant it could.
    pub fn available_at(&self, filled_at: Timestamp) -> Result<Timestamp> {
        let Some(cycle) = self.cycle.as_ref() else {
            return Ok(filled_at);
        };
        let today = filled_at.start_of_day();
        let (hour, minute, _, _) = filled_at.civil_time();
        let inside_cutoff = hour * 60 + minute < cycle.cutoff_minute;
        let value_date = if cycle.settles_on(today) && inside_cutoff {
            today
        } else {
            cycle.settlement_day_at_or_after(today.saturating_add(Duration::from_days(1)))?
        };
        let mut settlement_date = value_date;
        for _ in 0..cycle.convention.days() {
            settlement_date = cycle.settlement_day_at_or_after(
                settlement_date.saturating_add(Duration::from_days(1)),
            )?;
        }
        Ok(settlement_date.saturating_add(Duration::from_secs(
            i64::from(cycle.availability_minute) * 60,
        )))
    }
}

impl SettlementCycle {
    fn settles_on(&self, day_start: Timestamp) -> bool {
        self.settlement_weekdays.contains(&day_start.weekday())
            && !self.holidays.contains(&day_number(day_start))
    }

    fn settlement_day_at_or_after(&self, from: Timestamp) -> Result<Timestamp> {
        let mut cursor = from.start_of_day();
        for _ in 0..MAXIMUM_ROLL_DAYS {
            if self.settles_on(cursor) {
                return Ok(cursor);
            }
            cursor = cursor.saturating_add(Duration::from_days(1));
        }
        Err(Error::unavailable(format!(
            "no settlement day within {MAXIMUM_ROLL_DAYS} days of {}; the venue's terms have \
             no settlement days left in them",
            from.to_date_string()
        )))
    }
}

fn day_number(at: Timestamp) -> i64 {
    at.as_nanos().div_euclid(qip_core::time::NANOS_PER_DAY)
}
