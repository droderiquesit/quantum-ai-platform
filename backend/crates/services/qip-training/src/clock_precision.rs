//! The clock precision a consumer of recorded market events declares, and
//! the refusal of a window whose events cannot meet it (R12-RES-7103).
//!
//! When clock synchronisation degrades, the capture path keeps the raw
//! timestamps and widens each event's clock uncertainty instead of guessing a
//! correction. That widening was written down and then read by nothing: a
//! label whose instant was uncertain by two seconds was labelled at a
//! one-second horizon as if its instant were exact, and a replay that orders
//! venues against each other sequenced events whose order no clock could
//! vouch for. Both look like ground truth once promoted, and neither is.
//!
//! So a consumer states how precise its clock must be — a [`ClockPrecision`]
//! — and a window of events is admitted for promotion only if every event's
//! uncertainty is within it. A window that is not is refused **whole**, as an
//! [`ImpreciseWindow`] naming the window, its span, how many events failed
//! and the widest doubt it held. Not trimmed: dropping the imprecise events
//! and labelling the rest leaves a window with holes exactly where the clock
//! was worst, which is where a cross-venue label is most wrong, and the hole
//! is invisible downstream.
//!
//! Plain arithmetic over `(knowable instant, uncertainty)` pairs, so any
//! producer of recorded events — the lake's history reader, a capture stream
//! — feeds it without this crate linking either.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use std::fmt;

/// How many times finer than its shortest horizon a label's instant must be
/// known. A house default and not a calibrated figure: at ten, a label whose
/// instant is uncertain by its whole allowance can misplace a tenth of its
/// window, which is already a different label on a busy book; looser than
/// that and the label's horizon is mostly clock error. A plan that needs a
/// different figure states its own precision through [`ClockPrecision::new`].
pub const LABEL_HORIZON_PRECISION_DIVISOR: i64 = 10;

/// The widest clock uncertainty a consumer will accept on any event it
/// promotes. Validated at construction: positive, because a precision of zero
/// refuses every event a capture path ever widened and so declares nothing a
/// degraded clock could change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockPrecision {
    max_uncertainty: Duration,
}

impl ClockPrecision {
    /// A precision of at most `max_uncertainty` either side of each event's
    /// instant. Refuses zero or less rather than reading it as "exact".
    pub fn new(max_uncertainty: Duration) -> Result<Self> {
        if max_uncertainty <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a clock precision of {} ns admits no event a degraded clock ever touched and \
                 declares nothing; state the widest uncertainty the consumer can label or \
                 order across, which is positive",
                max_uncertainty.as_nanos()
            )));
        }
        Ok(Self { max_uncertainty })
    }

    /// The precision a label over `shortest_horizon` needs:
    /// `shortest_horizon / LABEL_HORIZON_PRECISION_DIVISOR`. Refuses a
    /// horizon too short to leave a positive precision, rather than rounding
    /// it up to one nanosecond.
    pub fn for_label_horizon(shortest_horizon: Duration) -> Result<Self> {
        let nanos = shortest_horizon.as_nanos() / LABEL_HORIZON_PRECISION_DIVISOR;
        Self::new(Duration::from_nanos(nanos)).map_err(|_| {
            Error::invalid(format!(
                "a label horizon of {} ns leaves no positive clock precision at one part in \
                 {LABEL_HORIZON_PRECISION_DIVISOR}; label at a longer horizon or declare the \
                 precision explicitly",
                shortest_horizon.as_nanos()
            ))
        })
    }

    pub const fn max_uncertainty(&self) -> Duration {
        self.max_uncertainty
    }

    /// Admit `events` — each an instant and the clock uncertainty it was
    /// captured with — as the window named `window`, or refuse the window.
    ///
    /// An event is out of precision when its uncertainty exceeds
    /// [`Self::max_uncertainty`], and also when it is negative: a negative
    /// half-width is a subtraction that ran the wrong way upstream, and an
    /// event carrying one has a clock nobody can place. An empty window is
    /// admitted with nothing in it; there is nothing to promote and nothing
    /// was mis-timed.
    pub fn admit<I>(
        &self,
        window: &str,
        events: I,
    ) -> std::result::Result<Admitted, ImpreciseWindow>
    where
        I: IntoIterator<Item = (Timestamp, Duration)>,
    {
        let mut span: Option<(Timestamp, Timestamp)> = None;
        let mut count = 0_usize;
        let mut imprecise = 0_usize;
        let mut widest = Duration::ZERO;
        for (at, uncertainty) in events {
            count += 1;
            span = Some(match span {
                Some((from, to)) => (from.min(at), to.max(at)),
                None => (at, at),
            });
            if uncertainty < Duration::ZERO || uncertainty > self.max_uncertainty {
                imprecise += 1;
            }
            widest = widest.max(uncertainty);
        }
        match span {
            Some((from, to)) if imprecise > 0 => Err(ImpreciseWindow {
                window: window.to_string(),
                from,
                to,
                events: count,
                imprecise,
                widest,
                required: self.max_uncertainty,
            }),
            _ => Ok(Admitted {
                events: count,
                widest,
            }),
        }
    }
}

/// A window every event of which met the declared precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub events: usize,
    /// The widest uncertainty admitted, for the card that records the round.
    pub widest: Duration,
}

/// A window refused for promotion because some of its events were captured
/// under a clock less precise than the consumer declared it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImpreciseWindow {
    /// The window's name as the caller gave it — a manifest, a session.
    pub window: String,
    /// The earliest and latest instant in the window.
    pub from: Timestamp,
    pub to: Timestamp,
    pub events: usize,
    /// Events whose uncertainty exceeded `required` or was negative.
    pub imprecise: usize,
    /// The widest uncertainty any event in the window carried.
    pub widest: Duration,
    /// The precision the consumer declared.
    pub required: Duration,
}

impl fmt::Display for ImpreciseWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "window {} ({} to {}) is not promoted: {} of its {} event(s) were captured with a \
             clock uncertainty past the {} ns this consumer needs, the widest {} ns; the \
             raw timestamps are kept, and the window may be promoted once a capture under a \
             recovered clock replaces it or a consumer that needs less precision reads it",
            self.window,
            self.from,
            self.to,
            self.imprecise,
            self.events,
            self.required.as_nanos(),
            self.widest.as_nanos()
        )
    }
}

impl From<ImpreciseWindow> for Error {
    /// Denied rather than invalid: nothing about the window is malformed,
    /// and the refusal is a policy holding, not an input to correct.
    fn from(refusal: ImpreciseWindow) -> Self {
        Error::denied(refusal.to_string())
    }
}
