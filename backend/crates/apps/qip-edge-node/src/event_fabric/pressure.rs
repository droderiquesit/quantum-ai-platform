//! ADR 0100 §6: the node's measurement of spool bytes against budget, feeding
//! `qip_edge::pressure`'s reading-style halt wire — Narrow and Exhausted are
//! read off this measurement, not asserted independently. SLICE-54.
//!
//! # Why a gauge of atomics with two named publishers
//!
//! The first shape of this reading was one `AtomicU64` with no generation, no
//! instant and no named publisher. Whichever thread wrote it was also the
//! thread that could block, so a drain stuck in a produce to an unreachable
//! fabric left the last value in place and the reading looked fresh while it
//! described a spool nobody was watching; and once staleness was added, the
//! same stuck drain would have staled the reading and halted a healthy cell
//! for an outage its spool was built to absorb (ADR 0100 §8, proving test 3).
//! So the facts are split by who can know them, and each publisher is a
//! distinct, non-`Clone` handle returned once by [`PressureGauge::new`]:
//!
//! - [`SpoolPublisher`] — the spool writer (SLICE-24): used bytes, the
//!   unwritable bit and the heartbeat. It writes to local disk and never
//!   waits on the network, so its heartbeat stops only when the writer does.
//! - [`DrainPublisher`] — the drain (SLICE-32): the fenced and connected bits
//!   and nothing else. It cannot beat the heartbeat, so a drain blocked in a
//!   produce cannot make a dead writer look alive, and cannot make a live one
//!   look dead.
//!
//! Publisher methods take `&mut self`, so sharing one across threads needs a
//! lock a reviewer can see rather than an `Arc` nobody notices.
//!
//! # Why `read` takes no lock
//!
//! [`PressureGauge::read`] runs on the decision thread every pass. It is a
//! handful of atomic loads plus one read of the clock it was handed, and
//! waits on nothing the publishers hold: a hot path that could wait on the
//! spool writer is the stall ADR 0100 §6 exists to prevent. That holds only
//! if the clock's `now` is itself wait-free — `ManualClock`'s is an atomic
//! load; the composition root must not hand a clock that locks.
//!
//! # What each bit reads as, and why
//!
//! In order of precedence, first match wins:
//!
//! 1. Never beaten — [`Exhaustion::Stale`]: every other field is still its
//!    construction value, not a measurement.
//! 2. Fenced — [`Exhaustion::Fenced`]: a newer producer epoch owns the
//!    partition (ADR 0100 §4), so nothing this process spools will ever be
//!    accepted and new exposure would have no record.
//! 3. Unwritable, or a size the writer could not read —
//!    [`Exhaustion::Unwritable`]. An unreadable size is never read as zero:
//!    that would be `Normal` on the one spool whose fill nobody knows.
//! 4. At or above the exhaustion line — [`Exhaustion::OverBudget`].
//! 5. At or above the narrowing line — [`JournalPressure::Narrow`], by half.
//! 6. Otherwise `Normal`.
//!
//! and then a heartbeat older than the bound, or one the clock places in the
//! future, overrides a `Normal` or `Narrow` with [`Exhaustion::Stale`],
//! judged by [`Freshness::judge`] — the same rule as
//! [`JournalPressure::judged`], except that a refused judgement (a heartbeat
//! from the future) is read as stale here rather than returned, because
//! `read` has nowhere to return an error to and an error is not a reading.
//! An exhaustion cause already found is kept, as the more specific finding.
//!
//! **The connected bit is deliberately absent from that list.** A fabric
//! outage is what the producer-retained spool exists to absorb (ADR 0100 §3,
//! §8 proving test 2: identical decisions through an outage). What an outage
//! costs the cell is the spool filling, and the fill is already measured; a
//! reading that also narrowed or halted on `connected == false` would turn
//! every broker restart into a trading change and make the outage test's
//! decisions differ. It is carried on [`Reading`] for the node's
//! `connected` gauge, not for the decision.

use crate::event_fabric::telemetry::PressureState;
use qip_core::decimal::SCALE;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, Duration};
use qip_edge::pressure::{Exhaustion, Freshness, JournalPressure, Narrowing};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

/// The spool's budget and the two lines drawn on it, in bytes.
///
/// Built only by [`Thresholds::new`], which refuses rather than clamps: a
/// narrowing line at or above the exhaustion line would let the spool go from
/// `Normal` straight to a halt with no narrowing in between, which is the
/// failure RES-062 names, and "fixing" it by nudging one line would trade on
/// a safety parameter nobody wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    budget_bytes: u64,
    narrow_bytes: u64,
    exhaust_bytes: u64,
}

impl Thresholds {
    /// A budget of `budget_bytes`, narrowing at `narrow_at` of it and
    /// exhausted at `exhaust_at` of it.
    ///
    /// Requires `0 < narrow_at < exhaust_at <= 1`, a positive budget, and
    /// that the lines stay distinct and above zero once converted to whole
    /// bytes — a budget so small that the narrowing line floors to zero would
    /// read every empty spool as `Narrow`. Every conversion is checked; an
    /// overflow is a refusal, never a wrapped line.
    pub fn new(budget_bytes: u64, narrow_at: Decimal, exhaust_at: Decimal) -> Result<Self> {
        if budget_bytes == 0 {
            return Err(Error::invalid(
                "a spool budget of zero bytes exhausts on the first record; configure a \
                 positive budget",
            ));
        }
        if !narrow_at.is_positive() {
            return Err(Error::invalid(format!(
                "a narrowing line at {narrow_at} of the budget narrows an empty spool; hand a \
                 fraction above zero"
            )));
        }
        if exhaust_at > Decimal::ONE {
            return Err(Error::invalid(format!(
                "an exhaustion line at {exhaust_at} of the budget lets the spool exceed its \
                 budget before halting; hand a fraction no greater than one"
            )));
        }
        if narrow_at >= exhaust_at {
            return Err(Error::invalid(format!(
                "a narrowing line at {narrow_at} is not below the exhaustion line at \
                 {exhaust_at}, so the spool would halt without first narrowing; hand a \
                 narrowing line strictly below the exhaustion line"
            )));
        }
        let narrow_bytes = line_bytes(budget_bytes, narrow_at)?;
        let exhaust_bytes = line_bytes(budget_bytes, exhaust_at)?;
        if narrow_bytes == 0 {
            return Err(Error::invalid(format!(
                "a narrowing line at {narrow_at} of a {budget_bytes}-byte budget is zero whole \
                 bytes and would narrow an empty spool; raise the budget or the line"
            )));
        }
        if narrow_bytes >= exhaust_bytes {
            return Err(Error::invalid(format!(
                "at a {budget_bytes}-byte budget the narrowing line ({narrow_bytes} bytes) and \
                 the exhaustion line ({exhaust_bytes} bytes) coincide in whole bytes, so the \
                 spool would halt without first narrowing; raise the budget or separate the \
                 lines"
            )));
        }
        Ok(Self {
            budget_bytes,
            narrow_bytes,
            exhaust_bytes,
        })
    }

    pub fn budget_bytes(self) -> u64 {
        self.budget_bytes
    }

    pub fn narrow_bytes(self) -> u64 {
        self.narrow_bytes
    }

    pub fn exhaust_bytes(self) -> u64 {
        self.exhaust_bytes
    }
}

/// `floor(budget * fraction)` in whole bytes, every step checked.
///
/// The crossing from a byte count to `Decimal` and back happens here and only
/// here; a fraction is not money but `Decimal` is exact, and a line computed
/// in `f64` would move by a byte between two machines' rounding.
fn line_bytes(budget_bytes: u64, fraction: Decimal) -> Result<u64> {
    let overflow = || {
        Error::invalid(format!(
            "{fraction} of a {budget_bytes}-byte budget overflows the byte arithmetic; \
             configure a smaller budget"
        ))
    };
    let budget_raw = i128::from(budget_bytes)
        .checked_mul(SCALE)
        .ok_or_else(overflow)?;
    let line = Decimal::from_raw(budget_raw)
        .checked_mul(fraction)
        .ok_or_else(overflow)?;
    // Flooring, not rounding: a line rounded up could sit one byte past the
    // budget the operator configured.
    u64::try_from(line.raw() / SCALE).map_err(|_| overflow())
}

/// The atomics both publishers write and the gauge reads.
#[derive(Debug)]
struct Shared {
    used_bytes: AtomicU64,
    /// Whether `used_bytes` is a measurement. False until the writer's first
    /// report and whenever it reports a size it could not read.
    size_known: AtomicBool,
    unwritable: AtomicBool,
    fenced: AtomicBool,
    connected: AtomicBool,
    /// Beats so far. Zero means never beaten. Written after `beat_nanos`
    /// with `Release`, read first with `Acquire`, so a non-zero generation
    /// guarantees the instant beside it is at least that beat's.
    generation: AtomicU64,
    beat_nanos: AtomicI64,
}

/// The node's spool-pressure gauge: the read side.
///
/// `Clone` because more than one reader is harmless; the publishers are not,
/// and are returned once, by [`Self::new`].
#[derive(Clone, Debug)]
pub struct PressureGauge {
    shared: Arc<Shared>,
    thresholds: Thresholds,
    clock: Arc<dyn Clock>,
    bound: Duration,
}

/// One observation of the gauge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// What to hand `Cell::apply_journal_pressure`.
    pub pressure: JournalPressure,
    /// Bytes on the spool, or `None` where the writer has not reported a
    /// size it could read — for the `spool_bytes` gauge, which should show a
    /// gap rather than a zero nobody measured.
    pub used_bytes: Option<u64>,
    /// Whether the drain last reported a connection. Not an input to
    /// `pressure`; see the module documentation.
    pub connected: bool,
    /// Heartbeats so far; zero means never.
    pub generation: u64,
}

impl Reading {
    /// The `state` label `OutboxTelemetry::pressure` records for this
    /// reading — the three `JournalPressure` arms, one to one.
    pub fn state(self) -> PressureState {
        match self.pressure {
            JournalPressure::Normal => PressureState::Normal,
            JournalPressure::Narrow(_) => PressureState::Narrow,
            JournalPressure::Exhausted(_) => PressureState::Exhausted,
        }
    }
}

impl PressureGauge {
    /// A gauge over `thresholds`, judging the heartbeat against `bound` on
    /// `clock`, with its two publishers.
    ///
    /// `clock` must be monotonic, never the wall clock: a wall clock stepped
    /// back by NTP makes the heartbeat read as from the future, and one
    /// stepped forward halts a healthy cell. A bound of zero or below is
    /// refused here, at construction, by the same [`Freshness::judge`] rule
    /// every read applies — rather than discovered as a halt on the first
    /// pass.
    pub fn new(
        thresholds: Thresholds,
        clock: Arc<dyn Clock>,
        bound: Duration,
    ) -> Result<(Self, SpoolPublisher, DrainPublisher)> {
        Freshness::judge(Duration::ZERO, bound)?;
        let shared = Arc::new(Shared {
            used_bytes: AtomicU64::new(0),
            size_known: AtomicBool::new(false),
            unwritable: AtomicBool::new(false),
            fenced: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            beat_nanos: AtomicI64::new(0),
        });
        let spool = SpoolPublisher {
            shared: Arc::clone(&shared),
            clock: Arc::clone(&clock),
        };
        let drain = DrainPublisher {
            shared: Arc::clone(&shared),
        };
        Ok((
            Self {
                shared,
                thresholds,
                clock,
                bound,
            },
            spool,
            drain,
        ))
    }

    pub fn thresholds(&self) -> Thresholds {
        self.thresholds
    }

    /// The reading now. Wait-free given a wait-free clock; never fails,
    /// because every way it could is itself a reading — `Exhausted`.
    pub fn read(&self) -> Reading {
        let s = &self.shared;
        let generation = s.generation.load(Ordering::Acquire);
        let beat_nanos = s.beat_nanos.load(Ordering::Acquire);
        let fenced = s.fenced.load(Ordering::Acquire);
        let unwritable = s.unwritable.load(Ordering::Acquire);
        let connected = s.connected.load(Ordering::Acquire);
        let size_known = s.size_known.load(Ordering::Acquire);
        let used = s.used_bytes.load(Ordering::Acquire);
        let used_bytes = size_known.then_some(used);

        let spool = if generation == 0 {
            JournalPressure::Exhausted(Exhaustion::Stale)
        } else if fenced {
            JournalPressure::Exhausted(Exhaustion::Fenced)
        } else if unwritable {
            JournalPressure::Exhausted(Exhaustion::Unwritable)
        } else {
            match used_bytes {
                None => JournalPressure::Exhausted(Exhaustion::Unwritable),
                Some(used) if used >= self.thresholds.exhaust_bytes => {
                    JournalPressure::Exhausted(Exhaustion::OverBudget)
                }
                Some(used) if used >= self.thresholds.narrow_bytes => {
                    JournalPressure::Narrow(Narrowing::half())
                }
                Some(_) => JournalPressure::Normal,
            }
        };

        // An age that does not fit in the arithmetic, or one `judge` refuses
        // because the clock has moved behind the heartbeat, is a clock the
        // gauge cannot believe: stale, not fresh.
        let now = self.clock.now().as_nanos();
        let fresh = now.checked_sub(beat_nanos).is_some_and(|age| {
            matches!(
                Freshness::judge(Duration::from_nanos(age), self.bound),
                Ok(Freshness::Fresh)
            )
        });
        // A stale heartbeat overrides Normal and Narrow — the last good
        // reading is exactly what a dead writer leaves behind — and keeps an
        // exhaustion cause already found, which is the more specific finding.
        let pressure = match spool {
            JournalPressure::Normal | JournalPressure::Narrow(_) if !fresh => {
                JournalPressure::Exhausted(Exhaustion::Stale)
            }
            _ => spool,
        };

        Reading {
            pressure,
            used_bytes,
            connected,
            generation,
        }
    }
}

/// The spool writer's handle: used bytes, the unwritable bit, the heartbeat.
#[derive(Debug)]
pub struct SpoolPublisher {
    shared: Arc<Shared>,
    clock: Arc<dyn Clock>,
}

impl SpoolPublisher {
    /// The bytes on the spool, or `None` when the writer could not read its
    /// size. `None` reads as exhausted until a size is reported again.
    pub fn set_used_bytes(&mut self, used: Option<u64>) {
        match used {
            Some(bytes) => {
                self.shared.used_bytes.store(bytes, Ordering::Release);
                self.shared.size_known.store(true, Ordering::Release);
            }
            None => self.shared.size_known.store(false, Ordering::Release),
        }
    }

    /// Whether the spool last refused a write.
    pub fn set_unwritable(&mut self, unwritable: bool) {
        self.shared.unwritable.store(unwritable, Ordering::Release);
    }

    /// Stamp the heartbeat at the gauge's clock and return its generation.
    ///
    /// Refuses once the generation cannot advance, and leaves the heartbeat
    /// where it was, so the gauge goes stale and halts rather than wrapping
    /// to zero and reading as a writer that never beat — or, worse, as one
    /// generation that repeats.
    pub fn beat(&mut self) -> Result<u64> {
        let next = self
            .shared
            .generation
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or_else(|| {
                Error::invalid(
                    "the spool heartbeat's generation is exhausted; restart the writer, which \
                     starts a new gauge",
                )
            })?;
        self.shared
            .beat_nanos
            .store(self.clock.now().as_nanos(), Ordering::Release);
        self.shared.generation.store(next, Ordering::Release);
        Ok(next)
    }
}

/// The drain's handle: the fenced and connected bits, and nothing else.
#[derive(Debug)]
pub struct DrainPublisher {
    shared: Arc<Shared>,
}

impl DrainPublisher {
    /// A newer epoch has fenced this producer (ADR 0100 §4).
    ///
    /// There is no unfence: a fenced incarnation never regains its partition,
    /// and a restarted node builds a new gauge. A clear here would let one
    /// misread broker reply resume exposure whose record the broker refuses.
    pub fn mark_fenced(&mut self) {
        self.shared.fenced.store(true, Ordering::Release);
    }

    /// Whether the drain holds a connection to the fabric. Carried for the
    /// node's gauge; it does not move the reading.
    pub fn set_connected(&mut self, connected: bool) {
        self.shared.connected.store(connected, Ordering::Release);
    }
}
