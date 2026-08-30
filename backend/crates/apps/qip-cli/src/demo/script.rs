//! What the doubles say, written down once.
//!
//! Every body here is a pure function of the demonstration's first instant, so
//! the whole script moves with the clock instead of carrying dates that go
//! stale. That matters more than it looks: three of the four feeds decide
//! whether a record is *knowable yet* by comparing the record's own instants
//! against the poll's, and a fixture with a hard-coded year would either
//! withhold everything or withhold nothing, depending on when somebody ran it.
//!
//! # What these payloads are not
//!
//! They are not vendor data and they are not a recording of any. They are
//! shapes this platform's decoders accept, chosen so that each layer has
//! something to do. Nothing in here was observed anywhere.
//!
//! # Why the numbers are a recurrence and not a seeded RNG
//!
//! The platform's own RNG is seeded and would be reproducible, but it belongs
//! to the platform. A price series drawn from it would move whenever the
//! generator changed, and the run's output would change with it for reasons
//! that have nothing to do with the live path. A closed-form recurrence gives
//! the same closes on every machine, in every replay, for ever.

use qip_core::{Duration, Timestamp};

/// The instrument every feed here talks about.
pub(crate) const SYMBOL: &str = "NWSC";
/// The vendor's key for the issuer behind it.
pub(crate) const ISSUER: &str = "NWSC";
/// The entity the document and alternative-data feeds resolve against.
pub(crate) const ENTITY: &str = "ent-northwind";
/// The macro series the document feed carries.
pub(crate) const SERIES: &str = "US.CPI.YOY";
/// The alternative-data set and the subject it is keyed by.
pub(crate) const DATASET: &str = "satellite.parking_lot_counts";
pub(crate) const SUBJECT: &str = "NWSC-US";

/// How many daily bars the vendor serves.
///
/// Enough that the discovery stage has a distribution to call an outlier
/// against rather than three points and an opinion.
pub(crate) const BARS: usize = 120;

/// The quiet part of the series: a hundred and nineteen closes near 100.
///
/// Non-cumulative on purpose. The same window is served to every poll, so a
/// cumulative walk would put a step at the seam where one poll's last close met
/// the next poll's first, and the jump below would then be an artefact of the
/// fixture rather than a move in the price.
fn quiet_closes() -> Vec<f64> {
    let mut value = 0.37_f64;
    (0..BARS - 1)
        .map(|_| {
            value = (value * 7.13).fract();
            100.0 * (1.0 + (value - 0.5) * 0.02)
        })
        .collect()
}

/// Every close, ending in an 8.5% jump.
///
/// The jump is the reason the discovery stage has anything to find. It is on
/// the *last* bar, which is also the one that has not finished forming at the
/// first poll — so the first cycle sees the quiet series and the second sees
/// the move, with nothing changing but the clock.
fn closes() -> Vec<f64> {
    let mut closes = quiet_closes();
    let jump = closes.last().copied().unwrap_or(100.0) * 1.085;
    closes.push(jump);
    closes
}

/// The last close the vendor will ever serve, for the order's arrival price.
pub(crate) fn last_close() -> f64 {
    closes().last().copied().unwrap_or(100.0)
}

/// The price feed: a hundred and twenty daily bars and one reference change.
///
/// Bar `i` opens `BARS - 1 - i` days before `start`, so the last one opens at
/// `start` itself and closes a day later. At the first poll it is not yet
/// knowable and the adapter withholds it, counted rather than dropped; by the
/// second cycle the clock has passed its close and the same response — byte for
/// byte, from the same server — yields one more record.
pub(crate) fn market_data(start: Timestamp) -> String {
    let closes = closes();
    let mut bars = Vec::with_capacity(BARS);
    for (index, close) in closes.iter().enumerate() {
        let open = if index == 0 {
            *close
        } else {
            closes[index - 1]
        };
        let open_time = start
            .saturating_sub(Duration::from_days((BARS - 1 - index) as i64))
            .to_rfc3339();
        bars.push(format!(
            r#"{{"symbol":"{SYMBOL}","interval":"1d","open_time":"{open_time}",
                 "open":"{open:.6}","high":"{high:.6}","low":"{low:.6}","close":"{close:.6}",
                 "volume":"2500000","trade_count":8000}}"#,
            high = open.max(*close) * 1.001,
            low = open.min(*close) * 0.999,
        ));
    }
    let announced = start.saturating_sub(Duration::from_hours(1)).to_rfc3339();
    let effective = start.saturating_add(Duration::from_days(7)).to_rfc3339();
    format!(
        r#"{{"bars":[{}],"reference":[{{"symbol":"{SYMBOL}","field":"lot_size",
             "previous_value":"100","new_value":"1","effective_from":"{effective}",
             "announced_at":"{announced}","update_id":"ref-nwsc-1"}}]}}"#,
        bars.join(",")
    )
}

/// The document feed: one news item, one filing carrying two figures, one macro
/// release.
///
/// The filing's period ended thirteen weeks before it was filed, and every
/// document states whether it is an original or a revision. Both are refusals
/// waiting to happen if the fixture gets them wrong — the decoder will not
/// accept a document that says nothing about restatement — which is exactly why
/// they are here: a demonstration whose documents could not have come from a
/// vendor proves nothing about the decoder that reads them.
pub(crate) fn narrative(start: Timestamp) -> String {
    let published = start.saturating_sub(Duration::from_mins(15)).to_rfc3339();
    let filed = start.saturating_sub(Duration::from_mins(30)).to_rfc3339();
    let period_end = start.saturating_sub(Duration::from_days(91)).to_rfc3339();
    let released = start.saturating_sub(Duration::from_hours(2)).to_rfc3339();
    let reference = start.saturating_sub(Duration::from_days(24)).to_rfc3339();
    format!(
        r#"{{
  "news":[{{"document_id":"wire-nwsc-4471",
    "headline":"Northwind Semiconductor warns on fourth-quarter volumes",
    "body":"The company said output at its northern fabrication site would be reduced.",
    "source":"newswire","published_at":"{published}",
    "entities":[{{"text":"Northwind Semiconductor Corporation","issuer":"{ISSUER}",
                  "confidence":0.94,"is_primary":true}}],
    "sentiment":{{"polarity":-0.42,"confidence":0.81,"novelty":0.66}},
    "topics":["guidance","supply_chain"],
    "licensing":"restricted","revision":{{"status":"original"}}}}],
  "filings":[{{"document_id":"0000912057-26-000431","issuer":"{ISSUER}",
    "filed_at":"{filed}","period_end":"{period_end}","period":"quarter",
    "figures":[{{"metric":"revenue","value":"4310.5","unit":"USD_millions",
                 "consensus":"4200.0","prior_value":"3980.0"}},
               {{"metric":"operating_margin","value":"0.294","unit":"ratio",
                 "consensus":"0.310"}}],
    "licensing":"licensed","revision":{{"status":"original"}}}}],
  "macro":[{{"document_id":"stat-cpi-1","series_id":"{SERIES}","region":"US",
    "value":2.9,"unit":"percent","reference_date":"{reference}",
    "released_at":"{released}","consensus":3.1,"previous":3.0,
    "licensing":"public","revision":{{"status":"original"}}}}]
}}"#
    )
}

/// A two-sided book, open, complete as of sequence 1000.
pub(crate) fn depth_snapshot(start: Timestamp) -> String {
    let stamp = start.saturating_sub(Duration::from_secs(10)).to_rfc3339();
    format!(
        r#"{{"symbol":"{SYMBOL}","sequence":1000,"at":"{stamp}","status":"open",
             "bids":[{{"price":"99.98","size":"500","orders":3}},
                     {{"price":"99.97","size":"300","orders":2}}],
             "asks":[{{"price":"100.02","size":"400","orders":4}},
                     {{"price":"100.03","size":"600","orders":1}}]}}"#
    )
}

/// The two increments that follow it: a new best bid inside the spread, and the
/// old best offer deleted.
///
/// Served once. A second poll is answered with an empty list, because replaying
/// the same increments would be the vendor re-sending sequence numbers the book
/// has already applied — a duplicate the sequence tracker is right to drop, and
/// a thing this demonstration has no reason to make it do.
pub(crate) fn depth_updates(start: Timestamp) -> String {
    let first = start.saturating_sub(Duration::from_secs(5)).to_rfc3339();
    let second = start.saturating_sub(Duration::from_secs(4)).to_rfc3339();
    format!(
        r#"{{"updates":[
             {{"sequence":1001,"at":"{first}","type":"level_set","side":"bid",
               "price":"99.99","size":"250","orders":1}},
             {{"sequence":1002,"at":"{second}","type":"level_set","side":"ask",
               "price":"100.02","size":"0"}}]}}"#
    )
}

/// No increments at all.
pub(crate) const NO_DEPTH_UPDATES: &str = r#"{"updates":[]}"#;

/// One satellite reading: captured, processed and published weeks apart.
///
/// The three instants are in that order on purpose. The decoder refuses a
/// reading processed before it was captured or published before it was
/// processed, and it keys the record on the *published* instant, because that
/// is the only one at which anybody could have acted on the number.
pub(crate) fn alternative(start: Timestamp) -> String {
    let captured = start.saturating_sub(Duration::from_days(21)).to_rfc3339();
    let processed = start.saturating_sub(Duration::from_days(15)).to_rfc3339();
    let published = start.saturating_sub(Duration::from_days(12)).to_rfc3339();
    format!(
        r#"{{"readings":[{{"observation_id":"obs-8801","dataset":"{DATASET}",
             "subject":"{SUBJECT}","metric":"vehicles","value":1842.0,"unit":"vehicles",
             "captured_at":"{captured}","processed_at":"{processed}","published_at":"{published}",
             "lead_days":21.0,"proxy_correlation":0.62,"proxies_for":"revenue",
             "licensing":"restricted",
             "quality":{{"completeness":0.94,"confidence":0.88,"basis":"observed"}}}}]}}"#
    )
}
