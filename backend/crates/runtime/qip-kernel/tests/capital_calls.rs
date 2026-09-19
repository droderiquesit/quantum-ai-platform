//! The capital-call writer of blueprint §43.2, as ADR 0085 §5 designed it:
//! a fund's drawdown notice filed against one of the desk's private
//! commitments is a journalled record under an authenticated operator's
//! subject, refused by the book's own gates before anything is written,
//! rebuilt from the log at the next boot through the same gates, and read by
//! the reserve the DECIDE stage sizes against.
//!
//! Every test asserts its premise before the property. The one that matters
//! most is the reserve's: before this writer existed the penalty term of
//! `Commitment::obligation` was structurally zero on every cycle that ever
//! ran, so a test asserting the reserve "moved" against a book where nothing
//! could move it would have proved nothing. The premise there is a
//! deployable figure read before the notice, and the property is the same
//! figure read after the notice fell overdue.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::Topic;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::cashflow::CallConsequence;
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::{CapitalCallEntry, CapitalCallNotice, Platform};
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;

/// The producer the kernel writes capital-call records under. A literal
/// here rather than the kernel's constant: the test reads the log the way an
/// auditor would, by what the record says about itself.
const CAPITAL_CALL_PRODUCER: &str = "kernel/capital-call";

/// The private fund every test files against, as the universe names it.
const FUND: &str = "obj-FUND";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn listed(symbol: &str) -> Result<FinancialObject> {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .venue("XNYS")
    .geography("US")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
}

/// 400,000 promised against 150,000 drawn: 250,000 the fund may call.
fn private_fund() -> Result<FinancialObject> {
    FinancialObject::builder(
        object("FUND"),
        "FUND",
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: dec!("400000"),
        called_capital: dec!("150000"),
        distributed_capital: Decimal::ZERO,
        residual_value: dec!("160000"),
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())
}

/// A listed name and the private fund.
fn encumbered_universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(listed("AAA")?)?;
    universe.insert(private_fund()?)?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("capital-calls-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

/// A million-dollar book, so the reserve can be reasoned about in round
/// numbers.
fn config() -> PlatformConfig {
    PlatformConfig {
        initial_equity: dec!("1000000"),
        ..PlatformConfig::default()
    }
}

fn platform(universe: Universe) -> Result<Platform> {
    let config = config();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits())
}

fn operator() -> OperatorIdentity {
    OperatorIdentity::verified("ops-carol", "oidc", start())
}

/// Ten percent a year on the unmet amount, so ten days late on 100,000 is
/// exactly 1,000 — 100,000 × 3,650bp × 10 days ÷ (10,000 × 365).
fn interest() -> CallConsequence {
    CallConsequence::Interest {
        annual_rate_bps: 3_650,
    }
}

fn notice(reference: &str, amount: Decimal, due: Timestamp) -> CapitalCallNotice {
    CapitalCallNotice {
        reference: reference.to_string(),
        amount,
        due,
        consequence: interest(),
    }
}

fn ten_days_on() -> Timestamp {
    start().saturating_add(Duration::from_days(10))
}

/// Every capital-call record the *event log* holds, oldest first — read from
/// the log rather than the book, because a restarted process has a fresh
/// book and the log is the record.
fn call_records(platform: &Platform) -> Result<Vec<CapitalCallEntry>> {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == Topic::ComplianceEvaluated
                && record.event.lineage.producer == CAPITAL_CALL_PRODUCER
        })
        .map(|record| {
            Ok(
                qip_streaming::envelope::StreamEnvelope::from_frame(&record.event)?
                    .decode::<CapitalCallEntry>()?
                    .body,
            )
        })
        .collect()
}

/// The references standing against the fund, in the book's own order.
fn standing(platform: &Platform) -> Vec<String> {
    platform
        .commitments()
        .get(FUND)
        .map(|commitment| {
            commitment
                .calls()
                .map(|call| call.reference().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_capital_call_is_refused_before_anything_is_journalled_and_an_admitted_one_is_on_the_log()
-> Result<()> {
    // Refusals first, and each leaves no record: a refusal the log heard of
    // would be indistinguishable, on replay, from a notice that stood. Then
    // the admitting half, and the record it leaves is attributed to the
    // operator's subject and never to anything a body could carry.
    let mut platform = platform(encumbered_universe()?)?;
    // Premise: the fund reached the book with 250,000 undrawn, and nothing
    // is filed or on the log.
    assert_eq!(
        platform.commitments().unfunded_total(start())?,
        dec!("250000"),
        "the premise failed: the universe's private record did not reach the commitment book"
    );
    assert!(call_records(&platform)?.is_empty());
    assert!(standing(&platform).is_empty());

    // 1. A commitment the universe does not hold.
    let unknown = platform
        .record_capital_call(
            "obj-NOBODY",
            notice("call-1", dec!("1"), ten_days_on()),
            &operator(),
            start(),
        )
        .expect_err("a call against no commitment");
    assert!(
        unknown
            .message()
            .contains("no commitment is recorded for obj-NOBODY"),
        "{}",
        unknown.message()
    );
    // 2. An amount past the undrawn balance.
    let too_much = platform
        .record_capital_call(
            FUND,
            notice("call-1", dec!("250001"), ten_days_on()),
            &operator(),
            start(),
        )
        .expect_err("250,001 against 250,000 undrawn");
    assert!(
        too_much.message().contains("unfunded balance of 250000"),
        "{}",
        too_much.message()
    );
    // 3. A due instant before the notice's own.
    let backdated = platform
        .record_capital_call(
            FUND,
            notice(
                "call-1",
                dec!("1000"),
                start().saturating_sub(Duration::from_days(1)),
            ),
            &operator(),
            start(),
        )
        .expect_err("due yesterday, issued today");
    assert!(
        backdated.message().contains("already late when it arrived"),
        "{}",
        backdated.message()
    );
    // 4. A stale credential, refused before the book is asked.
    let stale = OperatorIdentity::verified(
        "ops-carol",
        "oidc",
        start().saturating_sub(Duration::from_mins(60)),
    );
    assert!(
        platform
            .record_capital_call(
                FUND,
                notice("call-1", dec!("1000"), ten_days_on()),
                &stale,
                start()
            )
            .is_err(),
        "an hour-old credential cannot file"
    );
    // 5. A withdrawal of a call that is not open.
    let nothing_to_withdraw = platform
        .withdraw_capital_call(FUND, "call-1", &operator(), start())
        .expect_err("nothing stands under call-1");
    assert!(
        nothing_to_withdraw
            .message()
            .contains("holds no notice call-1 to withdraw"),
        "{}",
        nothing_to_withdraw.message()
    );
    assert!(
        call_records(&platform)?.is_empty(),
        "a refused notice reached the event log"
    );
    assert!(standing(&platform).is_empty());

    // The admitting half.
    platform.record_capital_call(
        FUND,
        notice("call-1", dec!("100000"), ten_days_on()),
        &operator(),
        start(),
    )?;
    let records = call_records(&platform)?;
    assert_eq!(records.len(), 1);
    let CapitalCallEntry::Noticed {
        commitment,
        reference,
        amount,
        issued_at,
        due_at,
        consequence,
        filed_by,
    } = &records[0]
    else {
        panic!("the one record is not a notice: {:?}", records[0]);
    };
    assert_eq!(commitment, FUND);
    assert_eq!(reference, "call-1");
    assert_eq!(*amount, dec!("100000"));
    assert_eq!(*issued_at, start(), "issued at the server's instant");
    assert_eq!(*due_at, ten_days_on());
    assert_eq!(*consequence, interest(), "the consequence is on the record");
    assert_eq!(
        filed_by, "ops-carol",
        "the operator's subject, from the identity and not from anything a body could carry"
    );
    assert_eq!(standing(&platform), vec!["call-1".to_string()]);

    // 6. A duplicate reference, now that one stands — refused by the book,
    // and unrecorded.
    let duplicate = platform
        .record_capital_call(
            FUND,
            notice("call-1", dec!("1"), ten_days_on()),
            &operator(),
            start(),
        )
        .expect_err("call-1 already stands");
    assert!(
        duplicate.message().contains("already holds notice call-1"),
        "{}",
        duplicate.message()
    );
    // And a second notice that with the first would pass the balance.
    assert!(
        platform
            .record_capital_call(
                FUND,
                notice("call-2", dec!("150001"), ten_days_on()),
                &operator(),
                start()
            )
            .is_err(),
        "100,000 standing plus 150,001 passes 250,000"
    );
    assert_eq!(call_records(&platform)?.len(), 1);

    // Withdrawing the one that stands is recorded and drops it, and the
    // reference is free again.
    platform.withdraw_capital_call(FUND, "call-1", &operator(), start())?;
    let records = call_records(&platform)?;
    assert_eq!(records.len(), 2);
    assert!(
        matches!(
            &records[1],
            CapitalCallEntry::Withdrawn { commitment, reference, amount, withdrawn_by, .. }
                if commitment == FUND
                    && reference == "call-1"
                    && *amount == dec!("100000")
                    && withdrawn_by == "ops-carol"
        ),
        "{:?}",
        records[1]
    );
    assert!(standing(&platform).is_empty());
    platform.record_capital_call(
        FUND,
        notice("call-1", dec!("50000"), ten_days_on()),
        &operator(),
        start(),
    )?;
    assert_eq!(standing(&platform), vec!["call-1".to_string()]);
    Ok(())
}

#[test]
fn an_overdue_capital_call_shrinks_the_capital_the_platform_will_deploy_by_what_missing_it_cost()
-> Result<()> {
    // The reserve's reading of a filed call, and the reason the writer is
    // not a spare part: `Platform::deployable_capital` subtracts
    // `CommitmentBook::unfunded_total`, which is each commitment's
    // `obligation` — the unfunded balance plus what failing an overdue
    // notice has cost. With no writer the penalty term was zero on every
    // cycle that ever ran. A notice ahead of its date changes nothing, which
    // is the arithmetic's own promise; one past its date adds exactly its
    // penalty; and a withdrawn one stops charging.
    let mut platform = platform(encumbered_universe()?)?;
    let before = platform.deployable_capital(start())?;
    assert_eq!(
        before,
        dec!("750000"),
        "the premise failed: a million less the 250,000 undrawn is not what the book deploys"
    );

    platform.record_capital_call(
        FUND,
        notice("call-1", dec!("100000"), ten_days_on()),
        &operator(),
        start(),
    )?;
    assert_eq!(
        platform.deployable_capital(start())?,
        before,
        "a notice ahead of its due date reserves nothing beyond the balance already held back"
    );

    // Ten days past due: 100,000 at 3,650bp for ten days is 1,000.
    let late = ten_days_on().saturating_add(Duration::from_days(10));
    assert_eq!(
        platform.commitments().accrued_default_penalty(late)?,
        dec!("1000"),
        "the premise failed: the penalty the book accrues is not the 1,000 this test reasons about"
    );
    assert_eq!(
        platform.deployable_capital(late)?,
        dec!("749000"),
        "the overdue notice's penalty did not come off what the platform will deploy"
    );

    // Withdrawn, the charge stops — and nothing else moved: the unfunded
    // balance is what it was, because a withdrawal meets nothing.
    platform.withdraw_capital_call(
        FUND,
        "call-1",
        &OperatorIdentity::verified("ops-carol", "oidc", late),
        late,
    )?;
    assert_eq!(platform.deployable_capital(late)?, before);
    assert_eq!(
        platform.commitments().unfunded_total(late)?,
        dec!("250000"),
        "a withdrawal moved the unfunded balance"
    );
    Ok(())
}

/// A file-backed log in a directory of this test's own, so two processes
/// can be assembled over it in turn.
fn log_path(tag: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-capital-calls-{tag}-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory.join("events.jsonl")
}

/// Assemble a platform over `path` at `at`, over `universe`, as the
/// composition root would after a restart.
fn platform_over(path: &std::path::Path, at: Timestamp, universe: Universe) -> Result<Platform> {
    let config = config().with_event_log_file(path);
    let (context, _clock) = Context::deterministic(at, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits())
}

#[test]
fn a_filed_capital_call_survives_a_restart_and_a_withdrawn_one_does_not() -> Result<()> {
    // The lifetime finding the register made against §43.2 on 2026-09-19:
    // the commitment book is derived state rebuilt from the universe at
    // every boot, so a notice filed on `Platform` would have been erased at
    // the next process start, taking the reserve it raised down with it. A
    // second process over the first's log must hold what stood — the notice
    // that was not withdrawn — and not what was.
    let path = log_path("restart");
    {
        let mut first = platform_over(&path, start(), encumbered_universe()?)?;
        first.record_capital_call(
            FUND,
            notice("call-1", dec!("100000"), ten_days_on()),
            &operator(),
            start(),
        )?;
        first.record_capital_call(
            FUND,
            notice("call-2", dec!("50000"), ten_days_on()),
            &operator(),
            start(),
        )?;
        first.withdraw_capital_call(FUND, "call-2", &operator(), start())?;
        assert_eq!(
            standing(&first),
            vec!["call-1".to_string()],
            "premise: one notice stands in the first process"
        );
        assert_eq!(call_records(&first)?.len(), 3);
    }

    // An hour on, as a restart is in life (and so the id stream does not
    // mint the first process's ids again).
    let later = start().saturating_add(Duration::from_hours(1));
    let mut second = platform_over(&path, later, encumbered_universe()?)?;
    assert_eq!(
        call_records(&second)?.len(),
        3,
        "premise: the second process read the first's log back"
    );
    assert_eq!(
        standing(&second),
        vec!["call-1".to_string()],
        "the notice that stood is back, and the withdrawn one is not"
    );
    let resumed = second
        .commitments()
        .get(FUND)
        .and_then(|commitment| commitment.call("call-1"))
        .cloned()
        .expect("the restarted book holds call-1");
    assert_eq!(resumed.amount(), dec!("100000"));
    assert_eq!(
        resumed.issued_at(),
        start(),
        "issued when it was, not when it was resumed"
    );
    assert_eq!(resumed.due_at(), ten_days_on());
    assert_eq!(resumed.consequence(), interest());
    // The resumed state is the live state, read by the reserve: the notice
    // is overdue by the same instant it would have been, and the standing
    // reference is still taken while the withdrawn one is free.
    let late = ten_days_on().saturating_add(Duration::from_days(10));
    assert_eq!(second.deployable_capital(late)?, dec!("749000"));
    assert!(
        second
            .record_capital_call(
                FUND,
                notice("call-1", dec!("1"), later),
                &OperatorIdentity::verified("ops-carol", "oidc", later),
                later
            )
            .is_err(),
        "call-1 still stands after the restart"
    );
    second.record_capital_call(
        FUND,
        notice("call-2", dec!("1000"), later),
        &OperatorIdentity::verified("ops-carol", "oidc", later),
        later,
    )?;
    assert_eq!(
        standing(&second),
        vec!["call-1".to_string(), "call-2".to_string()]
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the log has a directory"));
    Ok(())
}

#[test]
fn a_log_filing_a_call_against_a_commitment_this_universe_no_longer_holds_stops_assembly()
-> Result<()> {
    // Fail closed, as the ledger's resume does. A record the book this boot
    // built will not take — here, a fund the universe no longer carries —
    // is not skipped: skipping is the erasure the seam exists to end, and
    // the reserve would then disagree with the log about what the desk
    // owes. The refusal names the record and what to do.
    let path = log_path("dropped-fund");
    {
        let mut first = platform_over(&path, start(), encumbered_universe()?)?;
        first.record_capital_call(
            FUND,
            notice("call-1", dec!("100000"), ten_days_on()),
            &operator(),
            start(),
        )?;
        assert_eq!(
            call_records(&first)?.len(),
            1,
            "premise: a notice is on the log"
        );
    }

    let later = start().saturating_add(Duration::from_hours(1));
    let mut listed_only = Universe::new();
    listed_only.insert(listed("AAA")?)?;
    let refused = platform_over(&path, later, listed_only).expect_err(
        "assembly over a log that files a call against a fund the universe dropped is refused",
    );
    assert!(
        refused
            .message()
            .contains("cannot be resumed from this log under this universe")
            && refused.message().contains("call-1")
            && refused.message().contains(FUND),
        "the refusal names the record and the remedy: {}",
        refused.message()
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the log has a directory"));
    Ok(())
}
