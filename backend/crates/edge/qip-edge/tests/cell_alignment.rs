//! R12-TICK-2062: the cell tells the feature engine how far each venue's
//! stamp may be from the truth, and the engine declines to order two samples
//! the clocks cannot separate.
//!
//! `qip-feature-dag`'s own suite proves the engine, given a doubt. Nothing
//! proved the cell gives it one: `apply_batch` could pass `Duration::ZERO`
//! and every test in the tree stayed green, because the default simulated
//! feed stamps venue time as the gateway's now and the doubt is zero on that
//! path anyway. This suite drives the cell through its own entry, `on_bytes`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::feature::FeatureValue;
use qip_contracts::message::{MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::LeadLagCorrelation;
use qip_feature_dag::state::MarketState;
use qip_protocols::FeedKey;
use qip_protocols::decoder::{Decoder, Diagnostics};

const FEED: &str = "feed-a";

fn venue() -> VenueId {
    VenueId::new("XLON")
}

/// One quote per line: `sequence \t instrument \t bid \t ask \t lead_ms`,
/// where the lead is how far the venue's stamp runs ahead of receipt.
#[derive(Debug)]
struct QuoteDecoder {
    consumed: usize,
    diagnostics: Diagnostics,
}

impl Decoder for QuoteDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        let text = std::str::from_utf8(bytes).map_err(|e| Error::invalid(e.to_string()))?;
        let mut out = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let [sequence, name, bid, ask, lead_ms] = f[..] else {
                return Err(Error::invalid("a line has five fields"));
            };
            let price = |text: &str| Decimal::parse(text).ok_or_else(|| Error::invalid("a price"));
            let lead: i64 = lead_ms.parse().map_err(|_| Error::invalid("a lead"))?;
            out.push(MarketMessage::new(
                ObjectId::from_string(name),
                Origin::new(
                    venue(),
                    FEED,
                    0,
                    sequence.parse().map_err(|_| Error::invalid("a sequence"))?,
                ),
                MessageBody::Quote {
                    bid: Some((price(bid)?, Decimal::from_int(10))),
                    ask: Some((price(ask)?, Decimal::from_int(11))),
                },
                captured_at.saturating_add(Duration::from_millis(lead)),
                captured_at,
            ));
        }
        self.consumed = bytes.len();
        Ok(out)
    }
    fn protocol(&self) -> &str {
        "quotes"
    }
    fn consumed(&self) -> usize {
        self.consumed
    }
    fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

#[derive(Debug)]
struct Paper;
impl Placer for Paper {
    fn is_simulated(&self) -> bool {
        true
    }
    fn place(
        &mut self,
        _: &str,
        _: &ObjectId,
        _: &VenueId,
        _: qip_contracts::message::BookSide,
        _: Decimal,
        _: Decimal,
        _: Timestamp,
    ) -> Result<()> {
        Ok(())
    }
}

/// The leader's mids, and a follower that copies them one step (100 ms) late:
/// the tape `qip-feature-dag`'s own alignment test uses.
const BIDS: [&str; 10] = [
    "100.00", "101.00", "99.00", "102.00", "100.00", "103.00", "98.00", "101.00", "104.00",
    "100.50",
];

/// The lead/lag correlation the cell computed, with every stamp running
/// `lead_ms` ahead of receipt.
fn computed_with_lead(lead_ms: i64) -> Result<Option<FeatureValue>> {
    let follower = ObjectId::from_string("obj-FOLLOW");
    let leader = ObjectId::from_string("obj-LEAD");
    let mut cell = Cell::new(
        CellConfig::new("london-1", "europe-west2").with_venue(venue()),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(30)),
    )?;
    cell.protocols_mut().register(
        venue(),
        FEED,
        Box::new(QuoteDecoder {
            consumed: 0,
            diagnostics: Diagnostics::default(),
        }),
    )?;
    let key = LeadLagCorrelation::key(&follower, &leader, 5, 1);
    cell.register_features(vec![Box::new(LeadLagCorrelation::new(
        follower.clone(),
        leader.clone(),
        5,
        1,
    ))])?;
    let start = Timestamp::from_secs(1_760_000_000);
    let ask =
        |bid: &str| (Decimal::parse(bid).unwrap() + Decimal::parse("0.02").unwrap()).to_string();
    let mut sequence = 0u64;
    let mut at = start;
    for (step, bid) in BIDS.iter().enumerate() {
        at = start.saturating_add(Duration::from_millis(100 * step as i64));
        let previous = if step == 0 { bid } else { &BIDS[step - 1] };
        let lines = format!(
            "{}\tobj-LEAD\t{bid}\t{}\t{lead_ms}\n{}\tobj-FOLLOW\t{previous}\t{}\t{lead_ms}",
            sequence + 1,
            ask(bid),
            sequence + 2,
            ask(previous),
        );
        sequence += 2;
        cell.on_bytes(&FeedKey::new(venue(), FEED), lines.as_bytes(), at)?;
    }
    cell.work(at, &mut Paper)?;
    Ok(cell.features().value(&key))
}

/// Mutation: pass `Duration::ZERO` instead of `uncertainty` to
/// `ingest_placed` in `Cell::apply_batch`: the doubted run computes a
/// correlation and the first assertion fails.
#[test]
fn a_venue_whose_stamps_run_ahead_of_receipt_leaves_the_pair_unordered_at_the_cell() -> Result<()> {
    let clear = computed_with_lead(0)?;
    assert!(
        matches!(clear, Some(FeatureValue::Statistic(c)) if (c - 1.0).abs() < 1e-9),
        "premise: with no clock doubt the cell computes the tape's correlation of 1, got {clear:?}"
    );
    let doubted = computed_with_lead(60)?;
    assert!(
        matches!(doubted, Some(FeatureValue::Undefined)),
        "a 60 ms lead puts the 100 ms step inside the combined doubt; the cell asserted an order \
         the clocks cannot support: {doubted:?}"
    );
    Ok(())
}
