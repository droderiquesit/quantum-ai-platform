//! Trading calendars and market hours.
//!
//! Used to decide whether a venue is open, to age data correctly across
//! weekends, and to stop a backtest from trading on a day the market was shut.

use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One contiguous trading window within a day, in UTC minutes from midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub open_minute: u32,
    pub close_minute: u32,
}

impl Session {
    pub fn new(open_minute: u32, close_minute: u32) -> Self {
        Self {
            open_minute,
            close_minute,
        }
    }

    pub fn contains(&self, minute_of_day: u32) -> bool {
        if self.open_minute <= self.close_minute {
            minute_of_day >= self.open_minute && minute_of_day < self.close_minute
        } else {
            // A session that wraps past midnight UTC, as Asian hours do.
            minute_of_day >= self.open_minute || minute_of_day < self.close_minute
        }
    }

    pub fn duration(&self) -> Duration {
        let minutes = if self.open_minute <= self.close_minute {
            self.close_minute - self.open_minute
        } else {
            (1440 - self.open_minute) + self.close_minute
        };
        Duration::from_mins(i64::from(minutes))
    }
}

/// When a venue is open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketHours {
    pub venue: String,
    /// Sessions, in UTC. Multiple entries model a lunch break.
    pub sessions: Vec<Session>,
    /// Days of the week the venue trades, Monday = 0.
    pub trading_days: BTreeSet<u32>,
    /// Dates the venue is closed, as days since the Unix epoch.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub holidays: BTreeSet<i64>,
    /// True for venues that never close, such as crypto.
    pub is_continuous: bool,
}

impl MarketHours {
    /// Continuous trading, seven days a week.
    pub fn continuous(venue: impl Into<String>) -> Self {
        Self {
            venue: venue.into(),
            sessions: vec![Session::new(0, 1440)],
            trading_days: (0..7).collect(),
            holidays: BTreeSet::new(),
            is_continuous: true,
        }
    }

    /// A weekday venue with one session, given in UTC.
    pub fn weekday_session(venue: impl Into<String>, open: u32, close: u32) -> Self {
        Self {
            venue: venue.into(),
            sessions: vec![Session::new(open, close)],
            trading_days: (0..5).collect(),
            holidays: BTreeSet::new(),
            is_continuous: false,
        }
    }

    /// US equities regular hours: 14:30-21:00 UTC.
    pub fn us_equities() -> Self {
        Self::weekday_session("XNYS", 14 * 60 + 30, 21 * 60)
    }

    /// London equities regular hours: 08:00-16:30 UTC.
    pub fn london_equities() -> Self {
        Self::weekday_session("XLON", 8 * 60, 16 * 60 + 30)
    }

    pub fn with_holiday(mut self, date: Timestamp) -> Self {
        self.holidays
            .insert(date.as_nanos().div_euclid(qip_core::time::NANOS_PER_DAY));
        self
    }

    /// Whether the venue is open at `at`.
    pub fn is_open(&self, at: Timestamp) -> bool {
        if self.is_continuous {
            return true;
        }
        if !self.is_trading_day(at) {
            return false;
        }
        let (hour, minute, _, _) = at.civil_time();
        let minute_of_day = hour * 60 + minute;
        self.sessions.iter().any(|s| s.contains(minute_of_day))
    }

    /// Whether the venue trades at all on the date containing `at`.
    pub fn is_trading_day(&self, at: Timestamp) -> bool {
        if self.is_continuous {
            return true;
        }
        let day = at.as_nanos().div_euclid(qip_core::time::NANOS_PER_DAY);
        self.trading_days.contains(&at.weekday()) && !self.holidays.contains(&day)
    }

    /// The next instant the venue opens at or after `from`.
    ///
    /// Scans forward a day at a time and then to the session boundary, which is
    /// bounded and cheap for the horizons the scheduler uses.
    pub fn next_open(&self, from: Timestamp) -> Option<Timestamp> {
        if self.is_continuous {
            return Some(from);
        }
        if self.sessions.is_empty() {
            return None;
        }
        let mut day_start = from.start_of_day();
        for _ in 0..400 {
            if self.is_trading_day(day_start) {
                let mut candidates: Vec<u32> =
                    self.sessions.iter().map(|s| s.open_minute).collect();
                candidates.sort_unstable();
                for open_minute in candidates {
                    let candidate =
                        day_start.saturating_add(Duration::from_mins(i64::from(open_minute)));
                    if candidate >= from {
                        return Some(candidate);
                    }
                }
            }
            day_start = day_start.saturating_add(Duration::from_days(1));
        }
        None
    }
}

/// A named set of venue calendars.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TradingCalendar {
    venues: Vec<MarketHours>,
}

impl TradingCalendar {
    pub fn new() -> Self {
        Self::default()
    }

    /// The calendars the platform ships with.
    pub fn standard() -> Self {
        Self {
            venues: vec![
                MarketHours::us_equities(),
                MarketHours::london_equities(),
                MarketHours::continuous("CRYPTO"),
                MarketHours::weekday_session("XETR", 7 * 60, 15 * 60 + 30),
                MarketHours::weekday_session("XTKS", 0, 6 * 60),
                // FX trades continuously from Sunday evening to Friday evening;
                // modelled as a weekday-continuous venue.
                MarketHours {
                    venue: "FX".into(),
                    sessions: vec![Session::new(0, 1440)],
                    trading_days: (0..5).collect(),
                    holidays: BTreeSet::new(),
                    is_continuous: false,
                },
            ],
        }
    }

    pub fn add(&mut self, hours: MarketHours) {
        self.venues.retain(|v| v.venue != hours.venue);
        self.venues.push(hours);
    }

    pub fn get(&self, venue: &str) -> Option<&MarketHours> {
        self.venues.iter().find(|v| v.venue == venue)
    }

    /// Whether a venue is open, defaulting to closed for an unknown venue.
    ///
    /// Failing closed is deliberate: an unknown venue must not become an
    /// accidental licence to trade around the clock.
    pub fn is_open(&self, venue: &str, at: Timestamp) -> bool {
        self.get(venue).is_some_and(|v| v.is_open(at))
    }

    pub fn venues(&self) -> impl Iterator<Item = &str> {
        self.venues.iter().map(|v| v.venue.as_str())
    }
}
