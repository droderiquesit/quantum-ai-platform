//! The limit in force at the kernel (ADR 0405): the operator's limit set is
//! the ceiling, and an order is checked against that ceiling narrowed by the
//! risk tolerance the mandate registry holds for the desk.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)] // a bound in force is a ceiling times a fraction, exactly
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::Topic;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::limit_narrowing::LimitsNarrowed;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                // Stated, not defaulted: a liquid listed name on five million
                // units a day, quoted at three basis points.
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

/// The operator's ceiling: two maxima and a minimum.
fn ceiling() -> LimitSet {
    LimitSet::new("tolerance-test")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 2.0 },
        ))
        .with(Limit::new(
            "cash-buffer",
            LimitKind::MinCashBuffer { limit: 0.02 },
        ))
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), ceiling())
}

/// Submit a buy of 7,000 at 100: 700,000, seven per cent of the default ten
/// million of equity.
fn buy_seven_per_cent(platform: &mut Platform) -> Result<()> {
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("7000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

fn narrowings(platform: &Platform) -> Vec<LimitsNarrowed> {
    let mut found = Vec::new();
    for record in platform.event_log().by_topic(Topic::RiskEvaluated) {
        if let Ok(envelope) = StreamEnvelope::from_frame(record)
            && let Ok(decoded) = envelope.decode::<LimitsNarrowed>()
        {
            found.push(decoded.body);
        }
    }
    found
}

// The bar decision 30 sets, at the place an order is actually refused: the
// same order, the same book and the same limits file, and only the desk
// mandate's tolerance differs. A narrowing wired to a tolerance nothing could
// set below one would pass every unit test and never refuse an order.
#[test]
fn a_desk_tolerance_below_one_refuses_at_the_kernel_an_order_the_operators_limits_admit()
-> Result<()> {
    let mut at_the_ceiling = platform(PlatformConfig::default())?;
    buy_seven_per_cent(&mut at_the_ceiling)?;
    assert_eq!(
        at_the_ceiling.orders().fills().len(),
        1,
        "the premise: under the operator's limits alone the order is sent and fills"
    );

    let mut narrowed = platform(PlatformConfig::default().with_desk_risk_tolerance(dec!("0.5")))?;
    let error = buy_seven_per_cent(&mut narrowed)
        .expect_err("seven per cent is outside half of a ten per cent cap");
    let message = error.message();
    assert!(message.contains("risk refused"), "{message}");
    assert!(message.contains("max-position-weight"), "{message}");
    // A person can say why from the refusal alone.
    for needle in [
        "the limit in force, 0.05,",
        "ceiling of 0.1 ",
        "risk tolerance of 0.5 ",
    ] {
        assert!(
            message.contains(needle),
            "{needle:?} missing from {message:?}"
        );
    }
    assert!(narrowed.orders().fills().is_empty());
    Ok(())
}

// "Do not change what any existing limit set allows when the tolerance is 1."
#[test]
fn at_a_tolerance_of_one_the_limit_in_force_is_the_operators_set_and_nothing_is_journalled()
-> Result<()> {
    let platform = platform(PlatformConfig::default())?;
    assert_eq!(
        platform
            .user_ledger()
            .registry()
            .desk_mandate()
            .risk_tolerance(),
        Decimal::ONE,
        "the premise: a platform that states no tolerance has a desk at one"
    );
    assert_eq!(platform.limits_in_force(), platform.limits());
    assert!(
        platform
            .limits()
            .limits
            .iter()
            .any(|limit| limit.name == "max-position-weight"),
        "the premise: the set compared is the one the operator handed in"
    );
    assert!(narrowings(&platform).is_empty());
    Ok(())
}

// ADR 0405 D8. The ceiling comes from a file and the tolerance from the
// registry; unless the log says what they made, a refusal cannot be
// reproduced from the log alone.
#[test]
fn a_narrowed_limit_set_is_journalled_once_with_each_ceiling_and_bound_in_force() -> Result<()> {
    let platform = platform(PlatformConfig::default().with_desk_risk_tolerance(dec!("0.5")))?;
    // The figure narrowed by is the registry's, and the ceiling is untouched.
    assert_eq!(
        platform
            .user_ledger()
            .registry()
            .desk_mandate()
            .risk_tolerance(),
        dec!("0.5")
    );
    let ceiling_weight = |set: &LimitSet| {
        set.limits
            .iter()
            .find(|limit| limit.name == "max-position-weight")
            .map(|limit| limit.kind.bound())
    };
    assert_eq!(ceiling_weight(platform.limits()), Some(0.10));
    assert_eq!(ceiling_weight(platform.limits_in_force()), Some(0.05));

    let records = narrowings(&platform);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record.tolerance, dec!("0.5"));
    assert_eq!(record.ceiling_set, "tolerance-test");
    assert_eq!(record.limits.len(), platform.limits().len());
    let bounds = |name: &str| {
        record
            .limits
            .iter()
            .find(|limit| limit.name == name)
            .map(|limit| (limit.is_minimum, limit.ceiling, limit.in_force))
    };
    assert_eq!(bounds("max-position-weight"), Some((false, 0.10, 0.05)));
    assert_eq!(bounds("max-leverage"), Some((false, 2.0, 1.0)));
    // The floor is where the operator put it.
    assert_eq!(bounds("cash-buffer"), Some((true, 0.02, 0.02)));
    Ok(())
}

// Principle 3. A tolerance nobody can read stops assembly; it does not start
// a platform at the ceiling.
#[test]
fn a_desk_tolerance_outside_zero_to_one_stops_assembly() {
    for tolerance in [dec!("1.5"), dec!("-0.1")] {
        let Err(error) = platform(PlatformConfig::default().with_desk_risk_tolerance(tolerance))
        else {
            panic!("no platform is assembled on a tolerance of {tolerance}");
        };
        assert!(
            error
                .message()
                .starts_with("risk_tolerance must be a fraction"),
            "{tolerance}: {}",
            error.message()
        );
    }
}
