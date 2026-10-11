//! Capture: a disciplined, aligned market event ready for the Tick Lake
//! (R12-TICK-2049, 2051, 2089).
//!
//! [`ClockDiscipline`] returned one [`crate::clock::ClockCorrection`] per
//! adjustment and [`align`] refused an uncorrected timestamp, and neither
//! had a caller outside this crate's tests, so no stored event carried the
//! corrections between the venue's clock and the normalized one, and no
//! capture path ever asked whether two events on one stream arrived in the
//! order they happened.
//!
//! [`CaptureStream`] is that caller. Per stream (venue, feed, partition) it
//! holds a discipline, feeds it every event's venue and receive times,
//! normalizes each event through [`ClockDiscipline::discipline_traced`],
//! places it with [`Placed::from_disciplined`] once the estimate is
//! trustworthy — or with the event's own stated uncertainty before then —
//! and aligns it against the previous event on the stream. The output is a
//! [`CapturedMarketEvent`] with the three times, the uncertainty, the
//! sequence, every correction and the arrival order kept distinct, which
//! `qip_storage::tick_capture::TickLakeWriter` seals.
//!
//! Lane D wires this into `qip-edge-node`: it owns one `CaptureStream` and
//! one writer and calls [`CaptureStream::capture`] for each event its tape
//! decodes.

use crate::align::{Order, Placed, align};
use crate::clock::{ClockDiscipline, ClockObservation, CorrectionKind};
use qip_contracts::market_event::MarketEvent;
use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_storage::tick_capture::{CapturedMarketEvent, ClockCorrectionLineage};
use std::collections::BTreeMap;

/// Streams a capture path tracks before refusing a new one. A feed handler
/// sees a handful of venues and partitions; thousands means the key is
/// wrong, and an unbounded map would hide it.
pub const MAX_CAPTURE_STREAMS: usize = 1_024;

/// How each stream's clock is disciplined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisciplineSettings {
    pub window: usize,
    pub min_samples: usize,
    pub max_uncertainty: Duration,
}

impl Default for DisciplineSettings {
    fn default() -> Self {
        Self {
            window: 256,
            min_samples: 16,
            max_uncertainty: Duration::from_millis(5),
        }
    }
}

#[derive(Debug)]
struct StreamState {
    discipline: ClockDiscipline,
    previous: Option<Placed>,
}

/// Disciplines, aligns and shapes market events for the lake, per stream.
#[derive(Debug)]
pub struct CaptureStream {
    settings: DisciplineSettings,
    streams: BTreeMap<String, StreamState>,
}

impl CaptureStream {
    pub fn new(settings: DisciplineSettings) -> Result<Self> {
        // Validate once, so `capture` cannot fail on a setting.
        ClockDiscipline::new(
            settings.window,
            settings.min_samples,
            settings.max_uncertainty,
        )?;
        Ok(Self {
            settings,
            streams: BTreeMap::new(),
        })
    }

    /// The discipline for `stream`, if the stream has been seen.
    pub fn discipline(&self, stream: &str) -> Option<&ClockDiscipline> {
        self.streams.get(stream).map(|state| &state.discipline)
    }

    /// Discipline, place and align one event, and shape it for the lake.
    ///
    /// `raw` is the venue's bytes as received, if the caller kept them; the
    /// lake keeps them only where the entitlement permits and keeps their
    /// hash either way.
    pub fn capture(
        &mut self,
        event: &MarketEvent,
        raw: Option<Vec<u8>>,
    ) -> Result<CapturedMarketEvent> {
        let origin = event.origin();
        let stream = format!("{}/{}/{}", origin.venue, origin.feed, origin.partition);
        if !self.streams.contains_key(&stream) && self.streams.len() >= MAX_CAPTURE_STREAMS {
            return Err(Error::invalid(format!(
                "capture already tracks {MAX_CAPTURE_STREAMS} streams and `{stream}` would be \
                 another; a capture path keyed that widely is keyed on the wrong thing"
            )));
        }
        let settings = self.settings;
        let state = match self.streams.entry(stream) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(StreamState {
                discipline: ClockDiscipline::new(
                    settings.window,
                    settings.min_samples,
                    settings.max_uncertainty,
                )?,
                previous: None,
            }),
        };
        state.discipline.observe(ClockObservation::new(
            event.event_time(),
            event.receive_time(),
        ));
        let disciplined = state
            .discipline
            .discipline_traced(event.event_time(), event.receive_time());
        // Before the estimate is trustworthy there is no offset correction to
        // place by; the event's own stated uncertainty, widened by any
        // monotonic floor the discipline applied, is the honest placement.
        let placed = match Placed::from_disciplined(&disciplined) {
            Ok(placed) => placed,
            Err(_) => {
                let floored = disciplined
                    .corrections
                    .iter()
                    .filter(|c| c.kind == CorrectionKind::MonotonicFloor)
                    .fold(Duration::ZERO, |sum, c| sum + c.delta.abs());
                Placed::new(
                    disciplined.normalized,
                    event.uncertainty() + floored,
                    event.receive_time(),
                )?
            }
        };
        let arrival_order = state.previous.as_ref().map(|previous| {
            match align(previous, &placed).order {
                Order::Before => "before",
                Order::After => "after",
                Order::Unordered => "unordered",
            }
            .to_string()
        });
        state.previous = Some(placed);

        let corrections = disciplined
            .corrections
            .iter()
            .map(|correction| ClockCorrectionLineage {
                kind: match correction.kind {
                    CorrectionKind::OffsetEstimate => "offset_estimate",
                    CorrectionKind::MonotonicFloor => "monotonic_floor",
                }
                .to_string(),
                delta_nanos: correction.delta.as_nanos(),
                applied_at: correction.applied_at,
                samples: correction.samples,
                uncertainty_nanos: correction.uncertainty_nanos_f64,
            })
            .collect();
        let captured = CapturedMarketEvent {
            venue: origin.venue.to_string(),
            feed: origin.feed.clone(),
            instrument: event.payload().object_id.to_string(),
            sequence: Some(origin.sequence),
            event_time: event.event_time(),
            receive_time: event.receive_time(),
            normalized_time: disciplined.normalized,
            uncertainty: placed.uncertainty(),
            corrections,
            arrival_order,
            entitlement: event.entitlement().dataset().to_string(),
            canonical: serde_json::to_value(event.payload())?,
            raw_sha256: Some(event.source_hash().to_string()),
            raw,
        };
        captured.validate()?;
        Ok(captured)
    }
}
