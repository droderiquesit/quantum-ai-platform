//! ADR 0100 §6: the spool-pressure reading-style halt wire. Narrow halves a
//! pass's sizing and Exhausted halts new exposure, in the same discipline as
//! the existing kill-switch and polled halt wires; cancels and confirms
//! continue regardless. SLICE-26.
//!
//! # Why a reading, and why the cell is handed it
//!
//! The spool the node writes its journal through can run out of room, lose
//! its fence, or stop being written, and a cell that keeps adding exposure
//! while the record of it has nowhere to go is trading with no record — the
//! failure the composition-root rules name as the reason storage is proven
//! writable before a process reports healthy. The node observes the spool;
//! the cell is handed the result, exactly as [`crate::cell::PolledHalt`] is,
//! so the same seam is driven by a test with no spool at all and a replay of
//! the chain sees the reading the cell acted on rather than re-deriving it
//! from a clock.
//!
//! # Why freshness is judged here and not in the cell
//!
//! [`Freshness::judge`] is a pure function of two durations. The node owns
//! the clock and the heartbeat, calls it, and hands the cell the resulting
//! reading; the cell reads no clock for this. A cell that judged staleness
//! itself would make the same chain replay differently on a slower machine.
//!
//! # Opt-in
//!
//! Only a cell built from [`crate::cell::CellConfig::with_journal_wire`]
//! reads any of this. A wire armed on every cell would fail engaged on every
//! existing cell user that has never heard of a spool, and a halt that fires
//! everywhere is a halt somebody learns to clear by reflex.

use qip_core::decimal::SCALE;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration};

/// The multiplier [`JournalPressure::narrow`] applies: half the size the
/// rest of the pass would have asked for (ADR 0100 §6, RES-062).
pub const NARROW_HALF: Decimal = Decimal::from_raw(SCALE / 2);

/// Why the journal wire reads [`JournalPressure::Exhausted`].
///
/// Each arm is a different operator action, which is why they are not one
/// boolean: a fenced spool wants the other writer found, an unwritable one
/// wants the disk, a stale heartbeat wants the writer process, and an
/// over-budget spool wants the mirror that is not draining it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhaustion {
    /// The cell was built with the wire and has never been handed a reading.
    /// The cell's own construction state: a node cannot report it, and
    /// [`crate::cell::Cell::apply_journal_pressure`] refuses a reading that
    /// claims it.
    NeverApplied,
    /// Another writer holds the spool's fence.
    Fenced,
    /// The spool refused a write.
    Unwritable,
    /// The spool's heartbeat is older than the bound — see
    /// [`Freshness::judge`].
    Stale,
    /// The spool holds more unshipped bytes than its budget allows.
    OverBudget,
}

impl Exhaustion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeverApplied => "never_applied",
            Self::Fenced => "fenced",
            Self::Unwritable => "unwritable",
            Self::Stale => "stale",
            Self::OverBudget => "over_budget",
        }
    }
}

/// A sizing narrowing the journal wire applies, strictly below one.
///
/// The field is private so a narrowing cannot be built that widens: the only
/// constructors are [`Self::half`] and [`Self::new`], and the second refuses
/// a multiplier above one rather than clamping it — a "narrowing" of 1.5 is a
/// caller bug, and clamping it to one would let the bug trade at full size
/// while the chain said the cell was narrowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Narrowing(Decimal);

impl Narrowing {
    /// The narrowing ADR 0100 §6 names: half.
    pub fn half() -> Self {
        Self(NARROW_HALF)
    }

    /// A narrowing by `multiplier`, which must lie in (0, 1].
    ///
    /// Above one is refused: that widens. Zero or below is refused too: a
    /// narrowing to nothing is an exhausted journal, and it should arrive as
    /// [`JournalPressure::Exhausted`] so that it halts under its own gate and
    /// moves the halt gauge, rather than as a pass that sizes every order to
    /// zero and reads as a quiet market.
    pub fn new(multiplier: Decimal) -> Result<Self> {
        if multiplier > Decimal::ONE {
            return Err(Error::invalid(format!(
                "a journal-pressure narrowing of {multiplier} is above one and would widen \
                 sizing; hand a multiplier no greater than one, or Normal"
            )));
        }
        if !multiplier.is_positive() {
            return Err(Error::invalid(format!(
                "a journal-pressure narrowing of {multiplier} sizes every order to nothing; \
                 hand Exhausted instead, which halts under its own gate"
            )));
        }
        Ok(Self(multiplier))
    }

    pub fn multiplier(self) -> Decimal {
        self.0
    }
}

/// What the journal spool reads as, on one observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalPressure {
    /// The spool has room and a live writer. Sizing is not narrowed.
    Normal,
    /// The spool is filling. New exposure is sized down by the narrowing,
    /// before the budget is exhausted rather than at it (RES-062).
    Narrow(Narrowing),
    /// The spool cannot take the record of new exposure. New exposure halts;
    /// cancels, withdrawals and fill confirmations continue, because each of
    /// them reduces what the cell holds or tells it what it already holds
    /// (RES-013).
    Exhausted(Exhaustion),
}

impl JournalPressure {
    /// Whether this reading halts new exposure.
    pub fn halts(self) -> bool {
        matches!(self, Self::Exhausted(_))
    }

    /// The multiplier this reading applies to a pass's sizing.
    ///
    /// One for `Normal`, the narrowing for `Narrow`, and zero for
    /// `Exhausted` — though an exhausted cell never reaches sizing, because
    /// the halt gate returns first; zero is the answer here so that no future
    /// caller reading this outside the gate can size at full on an exhausted
    /// spool.
    pub fn sizing_multiplier(self) -> Decimal {
        match self {
            Self::Normal => Decimal::ONE,
            Self::Narrow(narrowing) => narrowing.multiplier(),
            Self::Exhausted(_) => Decimal::ZERO,
        }
    }

    /// The reading, as the node should hand it after judging the heartbeat.
    ///
    /// A stale heartbeat overrides whatever the spool last said: the last
    /// good reading is exactly the one that must not be trusted, because it
    /// is what a writer that died would leave behind. An already-exhausted
    /// reading keeps its own cause, which is the more specific finding.
    pub fn judged(self, heartbeat_age: Duration, bound: Duration) -> Result<Self> {
        Ok(match Freshness::judge(heartbeat_age, bound)? {
            Freshness::Fresh => self,
            Freshness::Stale => match self {
                Self::Exhausted(_) => self,
                Self::Normal | Self::Narrow(_) => Self::Exhausted(Exhaustion::Stale),
            },
        })
    }

    /// A short, fixed description for the chain. Never carries text from
    /// outside the process.
    pub fn describe(self) -> String {
        match self {
            Self::Normal => "normal".to_string(),
            Self::Narrow(narrowing) => format!("narrow to {}", narrowing.multiplier()),
            Self::Exhausted(cause) => format!("exhausted ({})", cause.as_str()),
        }
    }
}

/// Whether the spool's heartbeat is recent enough to believe its reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
}

impl Freshness {
    /// Judge a heartbeat `heartbeat_age` old against `bound`.
    ///
    /// Stale when strictly older than the bound. A bound of zero or below is
    /// refused rather than clamped: it would judge every heartbeat stale and
    /// halt the cell for a configuration typo, and clamping it to some small
    /// positive value would substitute a safety parameter nobody wrote. A
    /// negative age is refused too: a heartbeat from the future is a clock
    /// the node cannot trust, and reading it as fresh would let the skew
    /// release the halt. The node treats either refusal as
    /// [`Exhaustion::Stale`].
    pub fn judge(heartbeat_age: Duration, bound: Duration) -> Result<Self> {
        if bound.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a journal heartbeat bound of {} nanoseconds judges every heartbeat stale; \
                 configure a positive bound",
                bound.as_nanos()
            )));
        }
        if heartbeat_age.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "a journal heartbeat {} nanoseconds in the future cannot be judged; the clock \
                 that stamped it is not the node's, so treat the spool as stale",
                heartbeat_age.as_nanos().unsigned_abs()
            )));
        }
        Ok(if heartbeat_age > bound {
            Self::Stale
        } else {
            Self::Fresh
        })
    }
}
