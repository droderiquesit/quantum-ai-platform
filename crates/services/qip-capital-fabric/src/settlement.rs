//! Settlement timing: capital in flight is not capital available.
//!
//! This module exists because of one class of plan that looks correct and is
//! not. The forecast says euro collateral is needed in Frankfurt on Monday
//! morning. The transfer costs less than the shortfall it avoids. Every limit
//! has headroom. The plan is approved on Friday afternoon, the instruction
//! misses the cut-off, the value date rolls over a weekend that has no
//! settlement days in it, and the collateral arrives on Tuesday — by which time
//! the margin call it was for has already been met by somebody selling a
//! position.
//!
//! Nothing in that sequence is a bug in the forecast, the cost model or the
//! limits. It is a bug in assuming money is available when it is sent.
//!
//! So a [`SettlementCalendar`] answers one question — given an instruction at
//! this instant, when are the funds usable at the far end — and it answers it
//! by walking actual settlement days. Three things move the answer:
//!
//! * **The cut-off.** An instruction after it is a next-day instruction, on the
//!   same day it was typed.
//! * **Non-settlement days.** Weekends and holidays are skipped, not counted.
//!   [`qip_financial::calendar::MarketHours`] already models trading days,
//!   holidays and weekday sets, so it is reused rather than restated — a
//!   settlement calendar and a trading calendar are the same shape of object
//!   and keeping two would guarantee they drift.
//! * **The convention.** T+0, T+1 and T+2 count settlement days, not calendar
//!   days, which is the whole reason a Friday T+2 lands on Tuesday.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_financial::calendar::MarketHours;
use serde::{Deserialize, Serialize};

/// How many settlement days after the value date funds arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementConvention {
    /// Same settlement day, if the instruction makes the cut-off.
    T0,
    /// The next settlement day.
    T1,
    /// Two settlement days on — the equity and FX spot convention in most
    /// markets, and the one that quietly turns a Thursday into a Monday.
    T2,
}

impl SettlementConvention {
    /// Settlement days added to the value date.
    pub const fn days(&self) -> u32 {
        match self {
            Self::T0 => 0,
            Self::T1 => 1,
            Self::T2 => 2,
        }
    }

    /// A stable label for logs and refusal messages.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::T0 => "T+0",
            Self::T1 => "T+1",
            Self::T2 => "T+2",
        }
    }
}

/// When an instruction placed at a given instant produces usable funds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettlementQuote {
    /// When the instruction was given.
    pub instructed_at: Timestamp,
    /// The convention applied.
    pub convention: SettlementConvention,
    /// Whether the instruction made that day's cut-off.
    ///
    /// False is not an error. It is the single most common reason a plan that
    /// arithmetically works does not, and it is reported rather than absorbed.
    pub made_cutoff: bool,
    /// Start of the day the instruction is dated to, after any cut-off roll.
    pub value_date: Timestamp,
    /// The instant funds are usable at the destination.
    pub available_at: Timestamp,
    /// Settlement days between the value date and availability.
    pub settlement_days: u32,
    /// Calendar time the capital spends in flight, in days.
    ///
    /// Calendar rather than settlement days because funding and opportunity
    /// cost accrue over a weekend exactly as they do over a Tuesday. A
    /// statistic used for costing, hence `f64`.
    pub days_in_flight_stat: f64,
}

impl SettlementQuote {
    /// Whether funds land at or before an instant the plan depends on.
    pub fn arrives_by(&self, needed_by: Timestamp) -> bool {
        self.available_at <= needed_by
    }

    /// How late the funds are against an instant they were needed at.
    pub fn lateness(&self, needed_by: Timestamp) -> Duration {
        if self.available_at <= needed_by {
            Duration::ZERO
        } else {
            self.available_at.since(needed_by)
        }
    }

    /// A sentence naming every reason the funds land when they do.
    pub fn describe(&self) -> String {
        format!(
            "instructed {} ({} the cut-off), {} value {} settling {} settlement day(s) later, \
             usable {} after {:.2} calendar day(s) in flight",
            self.instructed_at.to_rfc3339(),
            if self.made_cutoff {
                "inside"
            } else {
                "after"
            },
            self.convention.as_str(),
            self.value_date.to_date_string(),
            self.settlement_days,
            self.available_at.to_rfc3339(),
            self.days_in_flight_stat,
        )
    }
}

/// Settlement days, cut-off times and when money becomes usable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettlementCalendar {
    /// Which days settle. Reused from the trading calendar, which already
    /// carries weekday sets and holiday exclusions.
    days: MarketHours,
    convention: SettlementConvention,
    cutoff_minute: u32,
    availability_minute: u32,
}

impl SettlementCalendar {
    /// The furthest ahead the calendar will walk before giving up.
    ///
    /// A bound rather than a loop that trusts its input. A calendar configured
    /// with no settlement days at all — every weekday a holiday, say — would
    /// otherwise spin forever looking for the next one, inside a planner that
    /// is on the path of a margin call.
    const MAXIMUM_ROLL_DAYS: u32 = 400;

    /// Build a calendar.
    ///
    /// Refuses one whose funds become available before the cut-off it settles
    /// against: a calendar that hands over money in the morning for an
    /// instruction accepted that afternoon models a machine that does not
    /// exist, and every plan built on it would be early by exactly one day.
    pub fn new(
        days: MarketHours,
        convention: SettlementConvention,
        cutoff_minute: u32,
        availability_minute: u32,
    ) -> Result<Self> {
        if cutoff_minute >= 1440 || availability_minute >= 1440 {
            return Err(Error::invalid(
                "a cut-off and an availability time are minutes within a day, so both must be \
                 below 1440",
            ));
        }
        if convention == SettlementConvention::T0 && availability_minute < cutoff_minute {
            return Err(Error::invalid(format!(
                "a T+0 calendar cannot make funds available at minute {availability_minute} \
                 against a cut-off at minute {cutoff_minute}; that is settlement before \
                 instruction"
            )));
        }
        Ok(Self {
            days,
            convention,
            cutoff_minute,
            availability_minute,
        })
    }

    /// A weekday calendar with the given convention.
    ///
    /// 16:00 UTC cut-off, funds usable at 09:00 UTC on the settlement day.
    /// Saturday and Sunday do not settle, which is the single fact this module
    /// exists to stop a planner forgetting.
    pub fn weekday(convention: SettlementConvention) -> Result<Self> {
        let mut days = MarketHours::weekday_session("SETTLEMENT", 0, 1440);
        days.is_continuous = false;
        let availability = match convention {
            // A same-day calendar has to hand funds over after its own cut-off.
            SettlementConvention::T0 => 17 * 60,
            _ => 9 * 60,
        };
        Self::new(days, convention, 16 * 60, availability)
    }

    /// The convention this calendar settles on.
    pub fn convention(&self) -> SettlementConvention {
        self.convention
    }

    /// The cut-off, as a UTC minute of the day.
    pub fn cutoff_minute(&self) -> u32 {
        self.cutoff_minute
    }

    /// Whether the date containing `at` settles at all.
    pub fn is_settlement_day(&self, at: Timestamp) -> bool {
        self.days.is_trading_day(at)
    }

    /// Whether an instant is inside its day's cut-off.
    pub fn is_inside_cutoff(&self, at: Timestamp) -> bool {
        let (hour, minute, _, _) = at.civil_time();
        hour * 60 + minute < self.cutoff_minute
    }

    /// Add a non-settlement day.
    pub fn with_holiday(mut self, date: Timestamp) -> Self {
        self.days = self.days.with_holiday(date);
        self
    }

    /// The next settlement day at or after a date, as a start-of-day instant.
    fn settlement_day_at_or_after(&self, from: Timestamp) -> Result<Timestamp> {
        let mut cursor = from.start_of_day();
        for _ in 0..Self::MAXIMUM_ROLL_DAYS {
            if self.is_settlement_day(cursor) {
                return Ok(cursor);
            }
            cursor = cursor.saturating_add(Duration::from_days(1));
        }
        Err(Error::unavailable(format!(
            "no settlement day found within {} days of {}; the calendar has no settlement days",
            Self::MAXIMUM_ROLL_DAYS,
            from.to_date_string()
        )))
    }

    /// Quote an instruction: value date, settlement date and availability.
    ///
    /// The value date is the first settlement day the instruction can be dated
    /// to — today if today settles and the cut-off has not passed, otherwise
    /// the next one. The settlement date then advances the convention's number
    /// of *settlement* days from there, so a Thursday T+2 is Monday and a
    /// Friday T+1 is Monday rather than Saturday.
    pub fn quote(&self, instructed_at: Timestamp) -> Result<SettlementQuote> {
        let today = instructed_at.start_of_day();
        let made_cutoff = self.is_settlement_day(instructed_at)
            && self.is_inside_cutoff(instructed_at);
        let value_date = if made_cutoff {
            today
        } else {
            self.settlement_day_at_or_after(today.saturating_add(Duration::from_days(1)))?
        };

        let mut settlement_date = value_date;
        for _ in 0..self.convention.days() {
            settlement_date = self
                .settlement_day_at_or_after(settlement_date.saturating_add(Duration::from_days(1)))?;
        }

        let available_at =
            settlement_date.saturating_add(Duration::from_mins(i64::from(self.availability_minute)));
        // Guard against a calendar whose availability time precedes the
        // instruction on the same day; the funds cannot be usable before they
        // were asked for, and reporting that they are is worse than being late.
        let available_at = available_at.max(instructed_at);

        Ok(SettlementQuote {
            instructed_at,
            convention: self.convention,
            made_cutoff,
            value_date,
            available_at,
            settlement_days: self.convention.days(),
            days_in_flight_stat: available_at.since(instructed_at).as_days_f64(),
        })
    }

    /// Whether an instruction given now produces funds by an instant.
    pub fn delivers_by(&self, instructed_at: Timestamp, needed_by: Timestamp) -> Result<bool> {
        Ok(self.quote(instructed_at)?.arrives_by(needed_by))
    }
}
