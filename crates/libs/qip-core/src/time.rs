//! Deterministic time.
//!
//! [`Timestamp`] is nanoseconds since the Unix epoch in UTC. Every component
//! reads the current time through a [`Clock`] so a simulation, a replay and a
//! live run differ only in which clock is injected.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};

pub const NANOS_PER_MICRO: i64 = 1_000;
pub const NANOS_PER_MILLI: i64 = 1_000_000;
pub const NANOS_PER_SEC: i64 = 1_000_000_000;
pub const NANOS_PER_MIN: i64 = 60 * NANOS_PER_SEC;
pub const NANOS_PER_HOUR: i64 = 60 * NANOS_PER_MIN;
pub const NANOS_PER_DAY: i64 = 24 * NANOS_PER_HOUR;

/// A signed span of time in nanoseconds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Duration(i64);

impl Duration {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(n: i64) -> Self {
        Self(n)
    }
    pub const fn from_micros(n: i64) -> Self {
        Self(n * NANOS_PER_MICRO)
    }
    pub const fn from_millis(n: i64) -> Self {
        Self(n * NANOS_PER_MILLI)
    }
    pub const fn from_secs(n: i64) -> Self {
        Self(n * NANOS_PER_SEC)
    }
    pub const fn from_mins(n: i64) -> Self {
        Self(n * NANOS_PER_MIN)
    }
    pub const fn from_hours(n: i64) -> Self {
        Self(n * NANOS_PER_HOUR)
    }
    pub const fn from_days(n: i64) -> Self {
        Self(n * NANOS_PER_DAY)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / NANOS_PER_SEC as f64
    }
    pub fn as_millis(self) -> i64 {
        self.0 / NANOS_PER_MILLI
    }
    pub fn as_days_f64(self) -> f64 {
        self.0 as f64 / NANOS_PER_DAY as f64
    }
    /// Fraction of a 365-day year — the convention used by every annualisation
    /// in the platform. Documented in `docs/research/conventions.md`.
    pub fn as_years_f64(self) -> f64 {
        self.as_days_f64() / 365.0
    }
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
    pub fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        if n.abs() >= NANOS_PER_DAY {
            write!(f, "{:.3}d", self.as_days_f64())
        } else if n.abs() >= NANOS_PER_HOUR {
            write!(f, "{:.3}h", n as f64 / NANOS_PER_HOUR as f64)
        } else if n.abs() >= NANOS_PER_SEC {
            write!(f, "{:.3}s", self.as_secs_f64())
        } else if n.abs() >= NANOS_PER_MILLI {
            write!(f, "{:.3}ms", n as f64 / NANOS_PER_MILLI as f64)
        } else {
            write!(f, "{n}ns")
        }
    }
}

impl std::ops::Add for Duration {
    type Output = Duration;
    fn add(self, rhs: Duration) -> Duration {
        Duration(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::Sub for Duration {
    type Output = Duration;
    fn sub(self, rhs: Duration) -> Duration {
        Duration(self.0.saturating_sub(rhs.0))
    }
}

impl std::ops::Mul<i64> for Duration {
    type Output = Duration;
    fn mul(self, rhs: i64) -> Duration {
        Duration(self.0.saturating_mul(rhs))
    }
}

/// Nanoseconds since 1970-01-01T00:00:00Z.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const EPOCH: Self = Self(0);
    /// Sentinel used by point-in-time views meaning "no upper bound".
    pub const MAX: Self = Self(i64::MAX);

    pub const fn from_nanos(n: i64) -> Self {
        Self(n)
    }
    pub const fn from_millis(n: i64) -> Self {
        Self(n * NANOS_PER_MILLI)
    }
    pub const fn from_secs(n: i64) -> Self {
        Self(n * NANOS_PER_SEC)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }
    pub const fn as_millis(self) -> i64 {
        self.0 / NANOS_PER_MILLI
    }
    pub const fn as_secs(self) -> i64 {
        self.0.div_euclid(NANOS_PER_SEC)
    }

    pub fn saturating_add(self, d: Duration) -> Self {
        Self(self.0.saturating_add(d.0))
    }
    pub fn saturating_sub(self, d: Duration) -> Self {
        Self(self.0.saturating_sub(d.0))
    }

    /// Signed distance `self - earlier`.
    pub fn since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }

    /// Truncate to the start of the containing UTC day.
    pub fn start_of_day(self) -> Self {
        Self(self.0.div_euclid(NANOS_PER_DAY) * NANOS_PER_DAY)
    }

    /// Truncate to a bucket boundary, e.g. one-minute bars.
    pub fn floor_to(self, bucket: Duration) -> Self {
        if bucket.0 <= 0 {
            return self;
        }
        Self(self.0.div_euclid(bucket.0) * bucket.0)
    }

    /// Civil date as `(year, month, day)` in UTC.
    pub fn civil_date(self) -> (i32, u32, u32) {
        civil_from_days(self.0.div_euclid(NANOS_PER_DAY))
    }

    /// Time of day as `(hour, minute, second, nanos)` in UTC.
    pub fn civil_time(self) -> (u32, u32, u32, u32) {
        let ns_of_day = self.0.rem_euclid(NANOS_PER_DAY);
        let secs = ns_of_day / NANOS_PER_SEC;
        (
            (secs / 3600) as u32,
            ((secs % 3600) / 60) as u32,
            (secs % 60) as u32,
            (ns_of_day % NANOS_PER_SEC) as u32,
        )
    }

    /// Day of week, Monday = 0. 1970-01-01 was a Thursday.
    pub fn weekday(self) -> u32 {
        let days = self.0.div_euclid(NANOS_PER_DAY);
        (days + 3).rem_euclid(7) as u32
    }

    /// RFC 3339 / ISO 8601 in UTC with millisecond precision.
    pub fn to_rfc3339(self) -> String {
        let (y, m, d) = self.civil_date();
        let (hh, mm, ss, ns) = self.civil_time();
        format!(
            "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{:03}Z",
            ns / 1_000_000
        )
    }

    /// Date-only rendering, `YYYY-MM-DD`.
    pub fn to_date_string(self) -> String {
        let (y, m, d) = self.civil_date();
        format!("{y:04}-{m:02}-{d:02}")
    }

    /// Parse `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SS[.fff][Z]`.
    pub fn parse_rfc3339(s: &str) -> Option<Self> {
        let s = s.trim().trim_end_matches('Z');
        let (date, time) = match s.split_once(['T', ' ']) {
            Some((d, t)) => (d, Some(t)),
            None => (s, None),
        };
        let mut dp = date.split('-');
        let y: i32 = dp.next()?.parse().ok()?;
        let m: u32 = dp.next()?.parse().ok()?;
        let d: u32 = dp.next()?.parse().ok()?;
        if dp.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        let days = days_from_civil(y, m, d);
        let mut nanos = days.checked_mul(NANOS_PER_DAY)?;

        if let Some(t) = time {
            let t = t.split(['+', '-']).next()?; // ignore offsets; inputs are UTC
            let mut tp = t.split(':');
            let hh: i64 = tp.next()?.parse().ok()?;
            let mm: i64 = tp.next().unwrap_or("0").parse().ok()?;
            let sec_part = tp.next().unwrap_or("0");
            let (ss, frac) = match sec_part.split_once('.') {
                Some((a, b)) => (a.parse::<i64>().ok()?, b),
                None => (sec_part.parse::<i64>().ok()?, ""),
            };
            if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..61).contains(&ss) {
                return None;
            }
            let mut frac_ns: i64 = 0;
            for i in 0..9 {
                let digit = frac.as_bytes().get(i).map_or(0, |c| i64::from(c - b'0'));
                frac_ns = frac_ns * 10 + digit;
            }
            nanos += hh * NANOS_PER_HOUR + mm * NANOS_PER_MIN + ss * NANOS_PER_SEC + frac_ns;
        }
        Some(Self(nanos))
    }

    /// Construct from a civil UTC date at midnight.
    pub fn from_civil(y: i32, m: u32, d: u32) -> Self {
        Self(days_from_civil(y, m, d) * NANOS_PER_DAY)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.to_rfc3339())
    }
}

/// Serialized as RFC 3339 so event logs stay human-auditable.
impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as DeError;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Nanos(i64),
        }
        match Repr::deserialize(d)? {
            Repr::Str(s) => Timestamp::parse_rfc3339(&s)
                .ok_or_else(|| D::Error::custom(format!("invalid timestamp: {s}"))),
            Repr::Nanos(n) => Ok(Timestamp::from_nanos(n)),
        }
    }
}

/// Days since the Unix epoch for a civil UTC date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = i64::from(m);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// Source of the current time.
pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> Timestamp;
}

/// Reads the host wall clock. Used only in live deployments.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Timestamp::from_nanos(d.as_nanos() as i64)
    }
}

/// A clock advanced explicitly by the caller. Backs simulation, replay and tests.
#[derive(Debug)]
pub struct ManualClock {
    nanos: AtomicI64,
}

impl ManualClock {
    pub fn new(start: Timestamp) -> Self {
        Self { nanos: AtomicI64::new(start.as_nanos()) }
    }

    pub fn advance(&self, d: Duration) {
        self.nanos.fetch_add(d.as_nanos(), AtomicOrdering::SeqCst);
    }

    /// Move the clock to `t`. Never moves backwards — monotonicity is a
    /// precondition of the event log's ordering guarantees.
    pub fn set(&self, t: Timestamp) {
        self.nanos.fetch_max(t.as_nanos(), AtomicOrdering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_nanos(self.nanos.load(AtomicOrdering::SeqCst))
    }
}

/// A clock that hands out a strictly increasing timestamp on every read.
///
/// Useful where distinct records must be totally ordered even when generated
/// inside the same nanosecond.
#[derive(Debug)]
pub struct MonotonicClock {
    inner: Mutex<i64>,
}

impl MonotonicClock {
    pub fn new(start: Timestamp) -> Self {
        Self { inner: Mutex::new(start.as_nanos()) }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Timestamp {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard += 1;
        Timestamp::from_nanos(*guard)
    }
}
