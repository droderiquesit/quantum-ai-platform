//! What the reference-rate connector refuses, and what it still admits.
//!
//! These exist because of two defects found together, both on a live path: the
//! platform genuinely fetches this source, and `qip_transport::http` speaks
//! plaintext HTTP/1.1 by design, so whoever answers that hop chose both the
//! keys and the values below.
//!
//! 1. The connector took the base currency from the response body and the
//!    quote currencies from the response's own `rates` keys, and checked
//!    neither for length, charset, or against the `base=EUR&symbols=USD,GBP,JPY`
//!    the manifest asked for. A measured run of one hostile response minted
//!    3,840,704 bytes of permanent feature-store key — 64 series of 60 KB
//!    each — with nothing quarantined, because `max_events_per_batch: 64`
//!    bounds how many events a response may become and nothing bounded how
//!    large one may be.
//! 2. Only `!rate.is_finite() || rate <= 0.0` was refused, so `EUR/USD = 1e300`
//!    was admitted end to end. It is finite until something squares it, and a
//!    second-moment statistic over it is `inf`; `inf - inf` is `NaN`, whose
//!    comparisons answer `false` in both directions, which is a risk check
//!    that neither passes nor fails.
//!
//! Every test here asserts its own premise — that the well-formed table it is
//! contrasted against is *admitted* — because a connector that refused
//! everything would satisfy the refusals alone and carry no data at all.

#![allow(clippy::panic_in_result_fn)]

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_financial::intelligence::MacroObservation;
use qip_financial::quality::{DataQuality, Provenance};
use qip_market_ingestion::adapter::{MAX_STATISTIC_MAGNITUDE, MAX_SUBJECT_KEY_CHARS, SensedRecord};
use qip_market_ingestion::connector::emulator::{RecordedAnswer, RecordedExchange, SourceEmulator};
use qip_market_ingestion::connector::{
    ConnectorRuntime, Cursor, QuarantineReason, RuntimeConfig, SourceConnector,
    transport::SourceTransport,
};
use qip_market_ingestion::connectors::FrankfurterRatesConnector;
use qip_transport::RecordingSleeper;
use std::sync::Arc;

/// After the recorded reference date plus the ECB's sixteen-hour publication
/// delay, so a well-formed table is knowable rather than correctly withheld.
fn horizon() -> Timestamp {
    Timestamp::parse_rfc3339("2026-09-05T09:00:00Z").expect("a fixture instant is valid RFC 3339")
}

fn table(base: &str, rates: &str) -> String {
    format!(r#"{{"amount":1.0,"base":"{base}","date":"2026-09-04","rates":{{{rates}}}}}"#)
}

/// The requested table, exactly as the vendor serves it.
const HONEST_RATES: &str = r#""GBP":0.85898,"JPY":181.59,"USD":1.1622"#;

/// What one poll of `body` produced: the series ids admitted, and the
/// quarantine reasons for whatever was not.
struct Polled {
    series: Vec<String>,
    values: Vec<f64>,
    refusals: Vec<String>,
}

fn poll(body: &str) -> Result<Polled> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let mut connector = FrankfurterRatesConnector::new(manifest.clone())?;
    let mut runtime = ConnectorRuntime::new(
        manifest,
        RuntimeConfig::seeded(11).with_sleeper(Arc::new(RecordingSleeper::new())),
    )?;
    let request = connector.fetch_request(&Cursor::beginning())?;
    let mut emulator = SourceEmulator::new(vec![RecordedExchange::always(
        request.path.clone(),
        RecordedAnswer::json(200, body),
    )]);
    let transport: &mut dyn SourceTransport = &mut emulator;
    let report = runtime.poll(&mut connector, transport, horizon())?;

    let mut series = Vec::new();
    let mut values = Vec::new();
    for envelope in &report.admitted {
        match envelope.record() {
            SensedRecord::Macro(observation) => {
                series.push(observation.series_id.clone());
                values.push(observation.value);
            }
            other => panic!("a reference rate decoded into {other:?}"),
        }
    }
    let refusals = runtime
        .quarantine()
        .recent(4)
        .iter()
        .map(|held| match &held.reason {
            QuarantineReason::DecodeFailure { detail } => detail.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    Ok(Polled {
        series,
        values,
        refusals,
    })
}

#[test]
fn the_requested_table_is_still_admitted_in_full() -> Result<()> {
    // The premise every refusal below rests on. A gate that refused this too
    // would pass every other test in this file and carry no rates at all.
    let admitted = poll(&table("EUR", HONEST_RATES))?;
    assert_eq!(
        admitted.series,
        ["FX.EUR.GBP", "FX.EUR.JPY", "FX.EUR.USD"],
        "the three requested pairs must still arrive: {:?}",
        admitted.refusals
    );
    assert!(
        admitted.refusals.is_empty(),
        "nothing about the honest table is refusable: {:?}",
        admitted.refusals
    );
    Ok(())
}

#[test]
fn a_table_whose_base_is_not_the_one_requested_is_refused_whole() -> Result<()> {
    // The connector asks `base=EUR`. A table claiming another base is an
    // answer to a question nobody asked, and admitting it would file every
    // rate in it under a series id naming the wrong currency — permanently,
    // in the event log.
    let hostile = format!("NOT-THE-BASE-WE-ASKED-FOR{}", "!".repeat(40));
    let refused = poll(&table(&hostile, HONEST_RATES))?;
    assert!(
        refused.series.is_empty(),
        "a table about another base produced series {:?}",
        refused.series
    );
    let [detail] = refused.refusals.as_slice() else {
        panic!("the page was not quarantined: {:?}", refused.refusals);
    };
    assert!(
        detail.contains("this connector asked for EUR"),
        "the refusal must name what was asked for: {detail}"
    );
    // Bounded and escaped: the refusal travels to a DataQualityFailure on the
    // bus and to an operator's terminal, so it carries a prefix and the
    // length, never the vendor's whole string.
    assert!(
        !detail.contains(&hostile),
        "the refusal repeated the whole rejected value: {detail}"
    );
    assert!(
        detail.contains("(65 characters)"),
        "the refusal must give the length, which is the part that says what happened: {detail}"
    );
    Ok(())
}

#[test]
fn currencies_the_manifest_never_requested_cannot_mint_a_single_series() -> Result<()> {
    // The measured attack: 64 keys of 60,000 characters, one under the
    // manifest's own `max_events_per_batch`, so no existing bound is touched.
    // Before the fix this admitted 64 records carrying 3,840,704 bytes of key.
    let long = "Z".repeat(60_000);
    let rates: Vec<String> = (0..64)
        .map(|index| format!(r#""{long}{index:04}":1.1"#))
        .collect();
    let refused = poll(&table("EUR", &rates.join(",")))?;
    assert!(
        refused.series.is_empty(),
        "{} hostile series were minted",
        refused.series.len()
    );
    let [detail] = refused.refusals.as_slice() else {
        panic!("the page was not quarantined: {:?}", refused.refusals);
    };
    assert!(
        detail.contains("not one of the 3 currencies this connector requested (GBP, JPY, USD)"),
        "the refusal must name the set that was asked for: {detail}"
    );
    assert!(
        detail.len() < 500,
        "the refusal is {} bytes; a message carrying the rejected key is the same unbounded \
         allocation one step later",
        detail.len()
    );
    Ok(())
}

#[test]
fn a_currency_absent_from_the_table_is_not_treated_as_an_attack() -> Result<()> {
    // The other direction, and it must behave differently. The ECB does stop
    // publishing a currency; taking USD and GBP down because JPY went away
    // would be an outage manufactured out of a vendor's edit.
    let short = poll(&table("EUR", r#""GBP":0.85898,"USD":1.1622"#))?;
    assert_eq!(
        short.series,
        ["FX.EUR.GBP", "FX.EUR.USD"],
        "a subset of the requested currencies is a smaller answer, not a wrong one: {:?}",
        short.refusals
    );
    Ok(())
}

#[test]
fn a_rate_outside_the_plausibility_band_is_refused_and_the_refusal_names_it() -> Result<()> {
    let refused = poll(&table("EUR", r#""USD":1e300,"GBP":0.85898"#))?;
    assert!(
        refused.values.is_empty(),
        "an absurd rate was admitted: {:?}",
        refused.values
    );
    let [detail] = refused.refusals.as_slice() else {
        panic!("the page was not quarantined: {:?}", refused.refusals);
    };
    assert!(
        detail.contains("1e300"),
        "a refusal that does not name the value leaves an operator guessing: {detail}"
    );
    assert!(
        detail.contains("outside the 1e-6..=1e9 band"),
        "the refusal must name the band it applied: {detail}"
    );
    Ok(())
}

#[test]
fn the_band_admits_every_rate_the_ecb_has_ever_actually_published() -> Result<()> {
    // The half that distinguishes a working gate from one that refuses
    // everything, and the argument for the numbers, written as a test rather
    // than only as prose. The band is deliberately loose: it exists to refuse
    // a number that cannot be an exchange rate, not one that is wrong by ten
    // per cent.
    let strongest = 0.26; // the Kuwaiti dinar, the strongest currency per unit
    let hyperinflation = 1_800_000.0; // the Turkish lira before the 2005 redenomination
    assert!(
        FrankfurterRatesConnector::MIN_RATE < strongest,
        "the floor would refuse the strongest currency in existence"
    );
    assert!(
        FrankfurterRatesConnector::MAX_RATE > hyperinflation,
        "the ceiling would refuse a rate the ECB has actually published, which is a band that \
         takes the feed down for a true event"
    );
    // And the feed carries them, rather than the band merely being wide.
    let extremes = poll(&table("EUR", r#""USD":0.26,"JPY":1800000.0,"GBP":0.85898"#))?;
    assert_eq!(
        extremes.series,
        ["FX.EUR.GBP", "FX.EUR.JPY", "FX.EUR.USD"],
        "the band refused a rate the source has published: {:?}",
        extremes.refusals
    );
    Ok(())
}

fn macro_record(series_id: &str, value: f64) -> SensedRecord {
    SensedRecord::Macro(Box::new(MacroObservation {
        series_id: series_id.to_string(),
        region: "EA".into(),
        value,
        unit: "USD per EUR".into(),
        reference_date: horizon(),
        consensus: None,
        previous: None,
        is_revision: false,
        provenance: Provenance::synthetic("test-macro", horizon()),
        quality: DataQuality::clean(),
    }))
}

#[test]
fn a_macro_series_id_too_long_to_be_an_identifier_is_refused_at_the_record_gate() {
    // The connector is one source; this is the gate every macro adapter goes
    // through — `service.rs::sift` and `runtime.rs::admit` both call it, and a
    // record that fails here never reaches the bus and so never reaches the
    // hash-chained event log, where it would be permanent by design.
    let honest = macro_record("FX.EUR.USD", 1.1622);
    assert!(
        honest.validate().is_empty(),
        "premise: a well-formed macro record passes: {:?}",
        honest.validate()
    );

    let long = "Z".repeat(60_000);
    let issues = macro_record(&long, 1.1622).validate();
    let [issue] = issues.as_slice() else {
        panic!("a 60,000-character series id was accepted as a key: {issues:?}");
    };
    assert!(
        issue.contains("60000 characters") && issue.contains(&MAX_SUBJECT_KEY_CHARS.to_string()),
        "the refusal must name the length and the bound: {issue}"
    );
    assert!(
        !issue.contains(&long),
        "the issue string repeated the key, which is published on the bus: {issue}"
    );
}

#[test]
fn a_series_id_carrying_an_escape_sequence_never_reaches_prose_unescaped() {
    // The series id is interpolated into the UNDERSTAND stage's operator
    // detail and rendered in the console. A raw ESC or newline there is a
    // terminal doing what the response told it to.
    let hostile = "FX.EUR.\u{1b}[2JUSD\nGBP";
    let issues = macro_record(hostile, 1.1622).validate();
    let [issue] = issues.as_slice() else {
        panic!("a control character in a series id was accepted: {issues:?}");
    };
    assert!(
        !issue.contains('\u{1b}') && !issue.contains('\n'),
        "the refusal itself carried the raw control characters: {issue:?}"
    );
    assert!(
        issue.contains("\\u{1b}"),
        "the refusal must show what was there, escaped: {issue}"
    );
}

#[test]
fn a_finite_but_absurd_macro_value_is_refused_at_the_record_gate() {
    let honest = macro_record("FX.EUR.USD", 1.1622);
    assert!(
        honest.validate().is_empty(),
        "premise: an ordinary rate passes the same gate"
    );

    let issues = macro_record("FX.EUR.USD", 1e300).validate();
    let [issue] = issues.as_slice() else {
        panic!("1e300 was admitted as a macro value: {issues:?}");
    };
    assert!(
        issue.contains("1e300"),
        "the refusal must name the value: {issue}"
    );

    // The edge of the bound, admitted, so the bound is a bound and not a
    // blanket refusal.
    assert!(
        macro_record("FX.EUR.USD", MAX_STATISTIC_MAGNITUDE)
            .validate()
            .is_empty(),
        "the largest admitted magnitude must be admitted"
    );
}

#[test]
fn the_magnitude_bound_is_the_arithmetic_it_claims_to_be() {
    // The constant is argued from what a second-moment statistic does to it,
    // so the argument is asserted rather than left in a doc comment. A
    // variance over the deepest feature series the world model retains sums
    // 512 squares: at the bound that sum must be finite, and for the value
    // this defect was found with it must not be. The first draft of this test
    // claimed the overflow began one decade above the bound; it begins at
    // sqrt(f64::MAX / 512) ≈ 5.9e152, and the test failing is what corrected
    // the comment.
    let deepest_series = 512.0_f64;
    let at_bound = deepest_series * MAX_STATISTIC_MAGNITUDE * MAX_STATISTIC_MAGNITUDE;
    assert!(
        at_bound.is_finite(),
        "a sum of squares at the bound already overflows, so the bound admits values that break \
         the statistics it exists to protect"
    );
    assert!(
        MAX_STATISTIC_MAGNITUDE < (f64::MAX / deepest_series).sqrt(),
        "the bound is at or above the point where the sum overflows, so it guarantees nothing"
    );
    let refused = 1e300_f64;
    let over = deepest_series * refused * refused;
    assert!(
        over.is_infinite(),
        "premise: the value this defect was found with does overflow the statistic"
    );
    // And this is why infinity is not merely a large number here: the
    // difference of two of them is not orderable against anything, so a
    // threshold check drawn from it is neither passed nor failed.
    let gap = over - over;
    assert!(gap.is_nan(), "premise: inf - inf is NaN");
    assert!(
        gap.partial_cmp(&0.0).is_none(),
        "a statistic that reaches a comparison must be comparable, and this one is not"
    );
}
