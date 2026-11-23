//! The commerce plane's own inventory ledger, journalled and resumed
//! (R12-COMMERCE-5096).
//!
//! `qip_financial::commerce::Ledger` is event-sourced, but the plane that runs
//! it is in memory: before this a restart opened a fresh ledger, so the first
//! purchase after it re-issued unit ids (`{product}-{purchase}-{index}`) the
//! previous process had already received and booked at the desk ledger as
//! `PhysicalInventory` movements. The same id then named two physical units,
//! and the movement's operation id (`...commerce-received:{unit}`) collided.
//!
//! Each SIMULATE pass journals the ledger events it applied since the last
//! journalled one, with the purchase counter, before the pass's movements are
//! queued. Installing a commerce plan replays every such record, in order,
//! into the plane's ledger and resumes the counter, so a replay of the log
//! alone reconstructs the ledger and no unit id is issued twice.
//!
//! The completed purchase-and-resale cycles are resumed too, from the
//! journalled `CommerceSimulated` passes, so SKU economics learned before a
//! restart are learned after it (R12-COMMERCE-1171).
//!
//! What is not resumed: the plane's sales, settlements, listings and held
//! units. They are not ledger facts; a unit received before the restart stays
//! in the ledger and is not re-listed.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_events::{EventBody, Topic};
use qip_financial::commerce::LedgerEvent;
use serde::{Deserialize, Serialize};

use crate::commerce_research::{CommerceResearch, CommerceSimulated};
use crate::platform::Platform;
use qip_streaming::envelope::StreamEnvelope;

/// The producer of [`CommerceLedgerJournalled`], its own origin so a reader
/// of the other `ScenarioEvaluated` producers never meets this body.
pub const COMMERCE_LEDGER_ORIGIN: &str = "kernel/commerce-ledger";

/// The producer of the plane's own pass records, `CommerceDiscovered` and
/// `CommerceSimulated`, on two topics.
pub const COMMERCE_ORIGIN: &str = "kernel/commerce";

/// The ledger events one SIMULATE pass applied, in the order applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommerceLedgerJournalled {
    pub events: Vec<LedgerEvent>,
    /// The plane's purchase counter after the pass; unit ids embed it.
    pub purchases: u32,
}

impl EventBody for CommerceLedgerJournalled {
    const TOPIC: Topic = Topic::ScenarioEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

impl Platform {
    /// Journal the ledger events the commerce plane applied since the last
    /// journalled one. The cursor advances only once the log has taken the
    /// record, so a refused write is retried by the next pass and not lost.
    /// Returns a problem line when the log refused it.
    pub(crate) fn journal_commerce_ledger(&mut self, now: Timestamp) -> Option<String> {
        let research = self.breadth.commerce.as_ref()?;
        let events = research.unjournaled_ledger_events().to_vec();
        if events.is_empty() {
            return None;
        }
        let purchases = research.purchases();
        let count = events.len();
        match self.journal_record(
            CommerceLedgerJournalled { events, purchases },
            COMMERCE_LEDGER_ORIGIN,
            now,
        ) {
            Ok(()) => {
                if let Some(research) = self.breadth.commerce.as_mut() {
                    research.mark_ledger_journalled(count);
                }
                None
            }
            Err(error) => Some(format!(
                "commerce ledger: {count} event(s) were not journalled, so a restart would \
                 reissue their unit ids: {}",
                error.message()
            )),
        }
    }

    /// Replay every journalled commerce ledger record into `research`, which
    /// must be freshly opened. A record the ledger refuses stops the install:
    /// a log that does not replay is not resumed from.
    pub(crate) fn resume_commerce_ledger(&self, research: &mut CommerceResearch) -> Result<()> {
        let mut events = Vec::new();
        let mut purchases = 0;
        for record in self.event_log().records() {
            if record.event.lineage.producer == COMMERCE_LEDGER_ORIGIN
                && record.event.topic == CommerceLedgerJournalled::TOPIC
            {
                let body = StreamEnvelope::from_frame(&record.event)?
                    .decode::<CommerceLedgerJournalled>()?
                    .body;
                events.extend(body.events);
                purchases = purchases.max(body.purchases);
            }
        }
        // The completed cycles, from every journalled SIMULATE pass, so a
        // restarted plane learns from the sales it made before the restart
        // (R12-COMMERCE-1171).
        let mut cycles = Vec::new();
        for record in self.event_log().records() {
            if record.event.lineage.producer == COMMERCE_ORIGIN
                && record.event.topic == CommerceSimulated::TOPIC
            {
                let body = StreamEnvelope::from_frame(&record.event)?
                    .decode::<CommerceSimulated>()?
                    .body;
                cycles.extend(body.sold);
            }
        }
        if !events.is_empty() {
            research.resume_ledger(events, purchases)?;
        }
        if !cycles.is_empty() {
            research.resume_cycles(cycles)?;
        }
        Ok(())
    }
}
