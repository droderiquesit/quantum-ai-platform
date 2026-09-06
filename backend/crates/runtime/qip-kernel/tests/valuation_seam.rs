//! The valuation plane and the liquidity floor **at the seam a cycle crosses**,
//! not at the accessor beside it.
//!
//! `tests/valuation_plane.rs` proves the ingredients: `deployable_capital`
//! returns free capital less the unfunded commitments, `sizing_confidence`
//! refuses an unmarkable instrument and narrows a marked one. Every one of its
//! assertions calls the public accessor directly. That left the headline claim
//! of `1107305` — that these engines "are reached by a deployed binary rather
//! than sitting in a library" — with no test behind it, and it was not
//! hypothetical: an adversarial review deleted the narrowing from
//! `Platform::mark_confidence_multiplier`, keeping only the refusal
//!
//! ```text
//! -    weakest = weakest.min(self.sizing_confidence(thesis.object_id.as_str(), now)?);
//! +    let _ = self.sizing_confidence(thesis.object_id.as_str(), now)?;
//! ```
//!
//! and the whole crate's suite still passed: 230 passed, 0 failed. This is the
//! `MaxExpectedShortfall` shape moved one level up — not a control that cannot
//! fire, but a control whose firing nothing checks.
//!
//! So every test here drives `Platform::run_cycle` and asserts the *observable
//! consequence*: the notional the cycle actually proposed, the construction it
//! actually refused, the risk monitor that actually withheld its signature.
//! Nothing here reads a valuation accessor to decide whether the test passed;
//! the accessors appear only where a test derives an expectation from the
//! platform's own number instead of restating a constant, which keeps these
//! tests about the seam rather than about `0.4`.
//!
//! **On the fixture, honestly.** Two settings are chosen rather than defaulted,
//! and both are stated here because a fixture that quietly tunes the thing
//! under test proves nothing:
//!
//! * [`REVIEW_FLOOR`] lowers `ReviewPolicy::minimum_surviving_confidence` from
//!   the shipped 0.50. The adversarial panel's confidence on a synthetic tape
//!   tops out near 0.37 for a listed name and near 0.17 for a private fund, so
//!   at the shipped floor no thesis is ever approved, `construct_from` is never
//!   called, and the valuation seam is never crossed — which is part of how it
//!   went unguarded. The floor is a deployment configuration
//!   (`PlatformConfig::review`), not a safety control: it governs whether the
//!   red team's verdict admits a hypothesis, and everything these tests assert
//!   happens strictly downstream of that verdict. No risk limit, no autonomy
//!   ceiling and no paper-trading layer is touched.
//!
//!   **Read this before quoting anything below as evidence about a
//!   deployment.** Nothing in *this file* runs under a shipped configuration.
//!   Raise [`REVIEW_FLOOR`] to 0.50 and all five tests here fail on their own
//!   premise — `the listed cycle proposed no legs ... rationale: no thesis
//!   cleared the action bar this cycle` — so what they prove is that the
//!   narrowing arithmetic is right *if reached*. That it is reached is proved
//!   somewhere else, and it has to be, because no synthetic tape this file can
//!   build clears the shipped bar:
//!   `qip-fastbrain/tests/tape.rs::the_shipped_review_policy_admits_a_thesis_that_reaches_the_narrowed_sizing_budget`
//!   drives the committed demonstration tape through `PlatformConfig::default()`
//!   with the review policy untouched, and 107 of its 600 cycles enter
//!   `construct_from` — so `deployable_capital`, `central_degradation` and
//!   `mark_confidence_multiplier` all run in the configuration `qip-fastbrain`
//!   deploys. That test pins the budget they produce at 3,750,000 and then
//!   5,625,000 of a 10,000,000 book. If it is ever deleted, this file is back
//!   to proving arithmetic nobody has shown a deployment performs, which was
//!   finding M4.
//!
//!   One honest limit remains, and it is the tape's rather than this file's:
//!   under the shipped policy the seam is reached and no *leg* is sized,
//!   because the only theses the review approves on that tape are
//!   `Claim::Overvalued` — an upward structural break — and
//!   `conservative_default`'s long-only mandate has no feasible solution for a
//!   short. Sizing a leg under the shipped floor needs a tape carrying a
//!   downward dislocation whose panel still clears 0.50; the jump section
//!   reaches 0.34 today. That is a fixture that does not exist, not a control
//!   that cannot fire.
//! * The tape's jump sits on the **last** bar. A jump two thirds of the way
//!   through leaves a volatility-shift anomaly at the head of the queue, whose
//!   claim is about the option rather than the underlying, so the thesis's
//!   conviction is negative, the long-only mandate drops it, and the optimiser
//!   returns an infeasible zero-weight solution. That produces a proposal with
//!   no legs — the precise state in which a test asserting "a small notional"
//!   passes while proving nothing. Every test below therefore asserts its
//!   premise first: a proposal exists, it has a leg, and its notional is
//!   positive.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion aborting a `Result`-returning function is a bug. In a test the
// assertion is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_financial::valuation::ValuationMethod;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_portfolio_engine::proposal::Proposal;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// See the module note. Low enough that the panel's verdict on a synthetic
/// tape is `Approved` for a listed name *and* for a private fund, because the
/// seam under test is reached only through an approved thesis.
///
/// **This is not the shipped value.** `ReviewPolicy::default()` requires 0.50
/// and no app overrides it. Every test in this file is therefore a statement
/// about arithmetic and not about a deployment; the reachability of that
/// arithmetic under the shipped 0.50 is proved in
/// `qip-fastbrain/tests/tape.rs`, and the module note says how.
const REVIEW_FLOOR: f64 = 0.10;

/// The book every test sizes against, so a notional can be reasoned about in
/// round numbers.
fn book() -> Decimal {
    dec!("1000000")
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A leverage cap and nothing else, so that a proposal these tests size is not
/// also being judged by a concentration or notional control. The liquidity test
/// at the bottom of this file deliberately uses `conservative_default` instead —
/// it is about a shipped limit.
fn leverage_only() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn config(equity: Decimal) -> PlatformConfig {
    PlatformConfig {
        initial_equity: equity,
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
}

fn platform_over(universe: Universe, limits: LimitSet) -> Result<Platform> {
    platform_of(universe, limits, book())
}

/// The same platform on a stated book size. Only the liquidity test uses a
/// book other than [`book`]: its positions are judged by
/// `conservative_default`'s ten-percent-of-equity position cap as well as by
/// the liquidity floor, and a book small enough to trip the concentration cap
/// first would prove the wrong control.
fn platform_of(universe: Universe, limits: LimitSet, equity: Decimal) -> Result<Platform> {
    let config = config(equity);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

fn listed(symbol: &str) -> Result<FinancialObject> {
    FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
        .venue("XNYS")
        .geography("US")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())
}

/// A private fund the valuation plane will have a view on, or refuse to.
///
/// The four cash-flow figures decide both the unfunded commitment
/// (`committed - called`) and the mark's method: a reported residual is a
/// last-round mark, called capital not yet returned is a cost mark, and
/// nothing left of either is nothing anybody observed.
fn private_fund(
    symbol: &str,
    committed: Decimal,
    called: Decimal,
    distributed: Decimal,
    residual: Decimal,
) -> Result<FinancialObject> {
    private_fund_reported_at(symbol, committed, called, distributed, residual, start())
}

/// The same fund with the administrator's report dated.
///
/// `observed` becomes the record's `provenance.event_time`, which since
/// `9fe3df4` is the instant a mark decays and falls due for review from — not
/// the instant it was read. A fund whose report is older than the mark's review
/// interval is the routine case, not an exotic one: private administrators
/// report quarterly with a 45-to-90-day lag.
fn private_fund_reported_at(
    symbol: &str,
    committed: Decimal,
    called: Decimal,
    distributed: Decimal,
    residual: Decimal,
    observed: Timestamp,
) -> Result<FinancialObject> {
    FinancialObject::builder(object(symbol), symbol, InstrumentType::PrivateEquityFund)
        .venue("OTC")
        .geography("US")
        .price(dec!("100"))
        .extension(Extension::PrivateAsset(PrivateAssetDetails {
            vintage_year: 2024,
            committed_capital: committed,
            called_capital: called,
            distributed_capital: distributed,
            residual_value: residual,
            stage: "buyout".to_string(),
            lockup_years: 7.0,
            capital_call_notice_days: 10,
        }))
        .provenance(Provenance::synthetic("administrator", observed))
        .build(start())
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// How many bars the tape carries, and the drop on its last one.
const TAPE: usize = 120;
const DROP: f64 = -0.09;

/// A quiet series that drops nine percent on its final bar.
///
/// The drop is last on purpose — see the module note. It gives the return
/// detector a `PriceMove` at roughly minus thirty sigma, whose negative
/// z-score reads as `Claim::Undervalued`, which has a positive implied sign,
/// which is a long the mandate permits.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// Drive one whole cycle over `universe`, with the tape on `subject`, and
/// return the platform beside the proposal the DECIDE stage recorded.
///
/// Every test goes through here rather than reaching into a stage, because a
/// stage called directly is not the production path — which is the entire
/// finding these tests close.
fn cycle_over(universe: Universe, subject: &str) -> Result<(Platform, Proposal)> {
    let mut platform = platform_over(universe, leverage_only())?;
    platform.observe(tape(subject));
    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage did not run, so the proposal below is not a whole cycle's:\n{}",
        report.summarise()
    );
    let proposal = platform
        .proposals()
        .last()
        .cloned()
        .expect("every cycle records a proposal, even a quiet one");
    Ok((platform, proposal))
}

/// Assert that a cycle actually sized something, and return what it sized.
///
/// This is the premise guard the testing rules name directly. A cycle that
/// produced no proposal, or a proposal with no legs, has a traded notional of
/// zero — and zero is smaller than every number these tests would like to see
/// it be smaller than.
fn sized_notional(proposal: &Proposal, what: &str) -> Decimal {
    assert!(
        !proposal.is_empty(),
        "the premise failed: the {what} cycle proposed no legs, so its notional is zero for a \
         reason that has nothing to do with the valuation plane — rationale: {}",
        proposal.rationale
    );
    let notional = proposal.traded_notional();
    assert!(
        notional.is_positive(),
        "the premise failed: the {what} cycle proposed {} leg(s) worth nothing",
        proposal.len()
    );
    notional
}

// --- 1. the budget the cycle deploys ----------------------------------------

#[test]
fn an_unfunded_commitment_shrinks_the_notional_a_whole_cycle_actually_proposes() -> Result<()> {
    // What this guards, exactly: `construct_from` reads
    // `Platform::deployable_capital` rather than the raw free balance. Delete
    // that read and every accessor test in `valuation_plane.rs` still passes,
    // because each of them calls `deployable_capital` itself. This test never
    // calls it: it compares two cycles that differ only in whether the
    // universe carries an undrawn commitment, and asserts the *proposal*
    // shrank. The failure prevented is a book holding a large undrawn
    // commitment deploying as though the commitment were somebody else's
    // problem — the first capital call would forfeit the position.
    let unencumbered_universe = {
        let mut universe = Universe::new();
        universe.insert(listed("AAA")?)?;
        universe
    };
    let (unencumbered_platform, unencumbered) = cycle_over(unencumbered_universe, "AAA")?;
    let full = sized_notional(&unencumbered, "unencumbered");
    // Premise: nothing is held back in the unencumbered run, so the reduction
    // below is the commitment and not some other claim on capital.
    assert_eq!(
        unencumbered_platform.commitments().len(),
        0,
        "the premise failed: the unencumbered book already carries a commitment"
    );

    let encumbered_universe = {
        let mut universe = Universe::new();
        universe.insert(listed("AAA")?)?;
        // 400,000 promised against 150,000 drawn: 250,000 the fund may call on
        // its own schedule, which is a quarter of this book.
        universe.insert(private_fund(
            "FUND",
            dec!("400000"),
            dec!("150000"),
            Decimal::ZERO,
            dec!("160000"),
        )?)?;
        universe
    };
    let (encumbered_platform, encumbered) = cycle_over(encumbered_universe, "AAA")?;
    let narrowed = sized_notional(&encumbered, "encumbered");

    // Premise: the commitment is real and is exactly the quarter of the book
    // the expectation below is derived from. Read from the platform rather
    // than restated, so a change to how a commitment is measured moves the
    // expectation with it instead of failing this test for the wrong reason.
    assert_eq!(
        encumbered_platform.commitments().len(),
        1,
        "the premise failed: the universe's private record did not reach the commitment book"
    );
    let unfunded = encumbered_platform.commitments().unfunded_total(start())?;
    assert_eq!(
        unfunded,
        dec!("250000"),
        "the premise failed: the undrawn commitment is not the 250,000 this test reasons about"
    );

    // The consequence. Strict, not approximate: a cycle that ignored the
    // commitment would propose the same notional to the last digit, because
    // everything else about the two runs is identical.
    assert!(
        narrowed < full,
        "the undrawn commitment did not shrink what the cycle proposed: {narrowed} against \
         {full}. `construct_from` is sizing against free capital rather than deployable capital."
    );
    // And by the right amount. The budget `construct` was handed is the seam
    // itself — equity less every hold, less the unfunded commitments, narrowed
    // by the degradation table — so the identity is stated against the
    // unencumbered run's own budget rather than against a literal.
    let deployable = book()
        .checked_sub(unfunded)
        .expect("the commitment is smaller than the book");
    assert_eq!(
        encumbered.equity.amount * book(),
        unencumbered.equity.amount * deployable,
        "the budget did not fall by exactly the undrawn commitment: {} against {} on a book of {}",
        encumbered.equity.amount,
        unencumbered.equity.amount,
        book()
    );
    Ok(())
}

// --- 2. an instrument nobody can mark ---------------------------------------

#[test]
fn a_thesis_on_an_unmarkable_private_asset_stops_the_construction_and_the_cycle_says_why()
-> Result<()> {
    // The refusal half of the mark seam, driven through the cycle. The failure
    // prevented is the one the valuation plane exists to prevent: a number
    // nobody observed, believed downstream because it arrived in the shape of
    // a price. Sizing an unmarkable instrument at some reduced fraction of a
    // fabricated mark would still be sizing against that number, so the
    // construction is refused outright rather than narrowed.
    //
    // Premise first, and it is the important one here. The same tape on a
    // markable fund of the same instrument type sizes a real position, so the
    // refusal below is about the mark and not about private assets, the tape,
    // or a cycle that was never going to propose anything.
    let markable_universe = {
        let mut universe = Universe::new();
        universe.insert(private_fund(
            "PRIV",
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
        )?)?;
        universe
    };
    let (_, markable) = cycle_over(markable_universe, "PRIV")?;
    sized_notional(&markable, "markable-control");

    let barren_universe = {
        let mut universe = Universe::new();
        // Fully called, fully distributed, no residual: the administrator's
        // record holds nothing anybody observed a value from.
        universe.insert(private_fund(
            "PRIV",
            dec!("100000"),
            dec!("100000"),
            dec!("100000"),
            Decimal::ZERO,
        )?)?;
        universe
    };
    let (platform, refused) = cycle_over(barren_universe, "PRIV")?;

    // Premise: the plane refused this exact instrument, keyed on the whole id
    // rather than matched inside a longer one.
    assert_eq!(
        platform.illiquid_unmarkable().keys().collect::<Vec<_>>(),
        vec!["obj-PRIV"],
        "the premise failed: the barren record is not the one and only unmarkable instrument"
    );
    // The consequence, first, because it is the one an operator cares about:
    // nothing was sized into an instrument nobody could mark. Stated before
    // the premise below so that a mutation which makes the plane *stop*
    // refusing fails here, with a message naming what went wrong, rather than
    // on a premise assertion that would read as a broken fixture.
    assert!(
        refused.is_empty(),
        "an instrument the valuation plane refused to mark was sized into anyway: {} leg(s), \
         rationale: {}",
        refused.len(),
        refused.rationale
    );
    assert_eq!(refused.traded_notional(), Decimal::ZERO);
    // Premise: a thesis really was approved and really did reach construction.
    // Without this the assertions around it would pass on a cycle where REASON
    // rejected the hypothesis and DECIDE never called the valuation plane at
    // all — a green test guarding nothing, which is the finding this file
    // closes.
    assert!(
        refused
            .rationale
            .starts_with("1 thesis(es) approved and none sized:"),
        "the premise failed: no approved thesis reached the construction, so the valuation plane \
         was never asked — rationale: {}",
        refused.rationale
    );
    assert!(
        refused.rationale.contains(
            "obj-PRIV cannot be sized because the valuation plane holds no defensible mark for it"
        ),
        "the cycle did not name the instrument or the reason: {}",
        refused.rationale
    );
    assert!(
        refused
            .rationale
            .contains("this plane refuses to invent a mark"),
        "the cycle did not carry the valuation plane's own refusal: {}",
        refused.rationale
    );

    // Refused, not merely unsized: nothing reached a venue, and the platform
    // is paper on the way in and on the way out.
    assert!(platform.orders().fills().is_empty());
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());
    Ok(())
}

// --- 3. the mark's confidence -----------------------------------------------

#[test]
fn the_weaker_of_two_marks_produces_the_smaller_notional_from_an_otherwise_identical_cycle()
-> Result<()> {
    // This is the test whose absence let the reviewer's mutation survive.
    // `mark_confidence_multiplier` narrows the construction budget by the
    // weakest mark any thesis rests on; deleting the narrowing while keeping
    // the refusal left every accessor assertion in `valuation_plane.rs`
    // passing, because none of them looked at a proposal.
    //
    // Three cycles, identical but for the record the thesis's instrument
    // carries: an instrument the plane has no view on, one marked at its last
    // round, and one marked at cost. Committed equals called in both private
    // records, so neither carries an unfunded commitment and this test cannot
    // pass on the budget engine from test 1 by accident.
    let listed_universe = {
        let mut universe = Universe::new();
        universe.insert(listed("AAA")?)?;
        universe
    };
    let (listed_platform, unnarrowed) = cycle_over(listed_universe, "AAA")?;
    let full = sized_notional(&unnarrowed, "listed");
    // Premise: a listed equity is priced by the market and takes no haircut,
    // so it is the unnarrowed reference the two private runs are measured
    // against.
    assert_eq!(
        listed_platform.sizing_confidence("obj-AAA", start())?,
        Decimal::ONE,
        "the premise failed: the listed reference is itself narrowed"
    );
    // "Unnarrowed" means unnarrowed *by a mark*. The centre still narrows the
    // budget by §6.2, and this is the only assertion in this crate that pins
    // that product at the seam a cycle crosses rather than at the accessor
    // beside it — `central_degradation` is proved by unit tests in
    // `platform.rs`, and a reviewer deleted the multiplier from
    // `construct_from` and then deleted only its self-model row, and both
    // survived every test in this file.
    //
    // The figure is stated here rather than read back off the platform, which
    // is the whole point: an expectation derived from `central_degradation`
    // moves with the mutation and pins nothing. On a platform assembled this
    // cycle the self-model has absorbed no graded outcome, so §6.2 row 6 reads
    // `Unavailable` and halves; the causal graph has absorbed no claim, so
    // row 2 reads `Unavailable` and takes 0.75; the belief state was written
    // by this cycle's own REASON stage, so row 4 is fresh and takes nothing.
    // 1 x 0.75 x 0.5 = 0.375 of a 1,000,000 book. Dropping the narrowing
    // altogether gives 1,000,000; dropping only the self-model row gives
    // 750,000.
    assert_eq!(
        unnarrowed.equity.amount,
        book() * dec!("0.375"),
        "the budget a whole cycle handed the optimiser is not the free capital narrowed by §6.2 \
         as the centre reads it: {} against a book of {}",
        unnarrowed.equity.amount,
        book()
    );

    let reported_universe = {
        let mut universe = Universe::new();
        universe.insert(private_fund(
            "PRIV",
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
        )?)?;
        universe
    };
    let (reported_platform, reported) = cycle_over(reported_universe, "PRIV")?;
    let reported_notional = sized_notional(&reported, "last-round");

    let cost_universe = {
        let mut universe = Universe::new();
        universe.insert(private_fund(
            "PRIV",
            dec!("400000"),
            dec!("400000"),
            dec!("100000"),
            Decimal::ZERO,
        )?)?;
        universe
    };
    let (cost_platform, at_cost) = cycle_over(cost_universe, "PRIV")?;
    let cost_notional = sized_notional(&at_cost, "at-cost");

    // Premise: the two private records really were marked by different
    // methods. Without this the test could pass on two identical marks and
    // some other difference between the runs.
    assert_eq!(
        reported_platform
            .illiquid_mark("obj-PRIV")
            .expect("a reported residual is markable")
            .method(),
        ValuationMethod::LastRound,
    );
    assert_eq!(
        cost_platform
            .illiquid_mark("obj-PRIV")
            .expect("called capital not yet returned is markable at cost")
            .method(),
        ValuationMethod::Cost,
    );
    // Premise: neither private record encumbers the budget, so what differs
    // below is the mark and only the mark.
    for (label, platform) in [
        ("last-round", &reported_platform),
        ("at-cost", &cost_platform),
    ] {
        assert_eq!(
            platform.commitments().unfunded_total(start())?,
            Decimal::ZERO,
            "the premise failed: the {label} record carries an unfunded commitment, so its \
             smaller notional could be the commitment engine rather than the mark"
        );
    }

    // The consequences. First: a private mark narrows the position rather than
    // passing it through, which is the assertion the reviewer's mutation
    // breaks — with the narrowing deleted, both private runs size at the full
    // budget and both of these fail.
    assert!(
        reported_notional < full,
        "a last-round mark did not narrow what the cycle proposed: {reported_notional} against \
         the unnarrowed {full}"
    );
    assert!(
        cost_notional < full,
        "a cost mark did not narrow what the cycle proposed: {cost_notional} against the \
         unnarrowed {full}"
    );
    // Second: the weaker mark narrows harder. A single constant haircut
    // applied to every private record would pass the two assertions above and
    // fail this one.
    assert!(
        cost_notional < reported_notional,
        "a cost mark — the lowest-confidence method in the table — sized at least as large as a \
         reported round: {cost_notional} against {reported_notional}"
    );

    // And by exactly the mark's own confidence. Derived from each platform's
    // own reading rather than restated as a literal, so a change to how the
    // mark decays moves the expectation with it instead of failing here.
    for (label, platform, proposal) in [
        ("last-round", &reported_platform, &reported),
        ("at-cost", &cost_platform, &at_cost),
    ] {
        let confidence = platform.sizing_confidence("obj-PRIV", start())?;
        assert!(
            confidence.is_positive() && confidence < Decimal::ONE,
            "the premise failed: the {label} mark's sizing confidence {confidence} is not a \
             narrowing"
        );
        assert_eq!(
            proposal.equity.amount,
            unnarrowed.equity.amount * confidence,
            "the {label} cycle's construction budget is not the unnarrowed budget times the \
             mark's confidence {confidence}"
        );
    }

    // Finally, the magnitude — and this is the one thing every assertion above
    // fails to hold, because each of them has the platform's own reading on
    // both sides. `assert_eq!(budget, unnarrowed * confidence)` is a statement
    // about the *shape* of the haircut and says nothing about its size: halve
    // every private mark's haircut (`ValuationMethod::base_confidence`,
    // `LastRound` 0.40 -> 0.80 and `Cost` 0.20 -> 0.40) and the budget doubles,
    // the confidence doubles with it, the ordering between the two methods
    // holds, and every assertion in this file still passes while every private
    // position the platform would take has doubled. That mutation was applied
    // and no test in this crate caught it.
    //
    // So these two bounds are stated here and derived from nothing the platform
    // computed. Both ends matter:
    //
    // * A `LastRound` mark is the price of one negotiated primary transaction
    //   that may stand for a year before review. It is evidence, and it is not
    //   a market, so the platform may stand behind **at most half** of what it
    //   would deploy behind a quoted price.
    // * `ValuationMethod::Cost` documents itself as "an admission of
    //   ignorance": the number is what was paid, and nothing since has been
    //   observed. **At most a quarter.**
    // * And each has a floor, because a haircut severe enough that a private
    //   position rounds to nothing is a refusal wearing a haircut's clothes.
    //   This plane has a refusal path and it is the right place for a refusal —
    //   the test above this one drives it — so a mark the plane was willing to
    //   strike must still support a position an operator can see.
    for (label, proposal, most, least) in [
        ("last-round", &reported, dec!("2"), dec!("10")),
        ("at-cost", &at_cost, dec!("4"), dec!("20")),
    ] {
        assert!(
            proposal.equity.amount * most <= unnarrowed.equity.amount,
            "the {label} cycle deployed {} of the {} a quoted price supports, which is more than \
             one part in {most}; a private mark is standing behind more capital than the method \
             can carry",
            proposal.equity.amount,
            unnarrowed.equity.amount
        );
        assert!(
            proposal.equity.amount * least >= unnarrowed.equity.amount,
            "the {label} cycle deployed {} of the {} a quoted price supports, less than one part \
             in {least}; a mark the plane was willing to strike is being refused by arithmetic \
             instead of by the refusal path",
            proposal.equity.amount,
            unnarrowed.equity.amount
        );
    }
    Ok(())
}

// --- 3b. a mark nobody has refreshed ----------------------------------------

/// How long before [`start`] the administrator's report is dated in the stale
/// run.
///
/// Past the 365-day review interval a `LastRound` mark carries, and not so far
/// past it that the mark decays to nothing: at 400 days the decayed confidence
/// is still about 0.086, so a platform with the staleness refusal deleted would
/// happily size a small position rather than fail for some unrelated reason.
/// That is the mutation this test exists for, and a fixture that made it fail
/// for the wrong reason would prove nothing.
const REPORT_AGE_DAYS: i64 = 400;

#[test]
fn a_cycle_whose_thesis_rests_on_a_mark_past_its_review_date_sizes_nothing_and_says_when_it_fell_due()
-> Result<()> {
    // The staleness half of `sizing_confidence`, driven through a whole cycle.
    // Deleting the `is_stale` block was caught only by an accessor test in
    // `valuation_plane.rs`, which calls `sizing_confidence` itself; nothing
    // asserted the consequence, which is that a cycle refuses to size against
    // an expired mark. The distinction from the test above it is the point: the
    // plane *did* strike a mark here and the platform still will not deploy
    // against it, because a mark nobody has refreshed for longer than its own
    // review interval is a number about a world that has moved.
    //
    // Premise and control: the identical record, reported today, sizes a real
    // position. Without it, "nothing was sized" below would pass on a cycle
    // that was never going to size anything.
    let fresh_universe = {
        let mut universe = Universe::new();
        universe.insert(private_fund(
            "PRIV",
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
        )?)?;
        universe
    };
    let (_, fresh) = cycle_over(fresh_universe, "PRIV")?;
    sized_notional(&fresh, "freshly-reported control");

    let stale_universe = {
        let mut universe = Universe::new();
        universe.insert(private_fund_reported_at(
            "PRIV",
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
            start().saturating_sub(Duration::from_days(REPORT_AGE_DAYS)),
        )?)?;
        universe
    };
    let (platform, refused) = cycle_over(stale_universe, "PRIV")?;

    // Premise, and the one that separates this test from the unmarkable one:
    // the plane had no trouble marking this record. It is the age of the
    // evidence and nothing else that stops the sizing.
    assert!(
        platform.illiquid_unmarkable().is_empty(),
        "the premise failed: the plane refused to mark the record at all, so this test is the \
         unmarkable test again: {:?}",
        platform.illiquid_unmarkable()
    );
    let mark = platform
        .illiquid_mark("obj-PRIV")
        .expect("the premise failed: a reported residual is markable whatever its age");
    assert!(
        mark.is_stale(start()),
        "the premise failed: a report dated {REPORT_AGE_DAYS} days back is not past the {} mark's \
         review date of {}",
        mark.method().label(),
        mark.next_review().to_rfc3339()
    );

    // The consequence, first: nothing was sized against it.
    assert!(
        refused.is_empty(),
        "a mark {REPORT_AGE_DAYS} days past its review date was sized into anyway: {} leg(s), \
         rationale: {}",
        refused.len(),
        refused.rationale
    );
    assert_eq!(refused.traded_notional(), Decimal::ZERO);
    // Premise: a thesis really was approved and really did reach construction,
    // so the zero above is a refusal and not a quiet cycle.
    assert!(
        refused
            .rationale
            .starts_with("1 thesis(es) approved and none sized:"),
        "the premise failed: no approved thesis reached the construction — rationale: {}",
        refused.rationale
    );
    // And the cycle says which instrument, and when the mark fell due, so an
    // operator is told what to refresh rather than that something went wrong.
    assert!(
        refused.rationale.contains("obj-PRIV")
            && refused.rationale.contains("due for review")
            && refused.rationale.contains(&mark.next_review().to_rfc3339()),
        "the cycle does not name the instrument and the date the mark fell due: {}",
        refused.rationale
    );

    assert!(platform.orders().fills().is_empty());
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());
    Ok(())
}

// --- 4. the liquidity floor, end to end -------------------------------------

/// Two names, one on each side of a week, in a universe that differs only in
/// whether `SLOW` carries a negotiated liquidity profile.
///
/// Nothing about the instrument *class* differs — both are common stock — so
/// neither half of the test can pass on an asset-class comparison that ignored
/// the liquidity record.
fn liquidity_universe(slow: LiquidityProfile) -> Result<Universe> {
    let mut universe = Universe::new();
    for (symbol, profile) in [
        (
            "FAST",
            LiquidityProfile::listed(Decimal::from_int(10_000_000), 5.0),
        ),
        ("SLOW", slow),
    ] {
        universe.insert(
            FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                .venue("XNYS")
                .geography("US")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .liquidity(profile)
                .provenance(Provenance::synthetic("test", start()))
                .build(start())?,
        )?;
    }
    Ok(universe)
}

/// Offer the same ten orders to a platform: one lot of `FAST`, nine of `SLOW`,
/// each 1,000 shares at 100 — a tenth of the conservative default's 250,000
/// single-order cap, so a refusal here is never the notional control.
///
/// Returns how many the control path accepted, and the refusals it gave.
fn offer_the_same_book(platform: &mut Platform) -> (usize, Vec<String>) {
    let mut accepted = 0;
    let mut refusals = Vec::new();
    for (symbol, lots) in [("FAST", 1), ("SLOW", 9)] {
        for _ in 0..lots {
            let order = platform.order_from(
                object(symbol),
                Side::Buy,
                dec!("1000"),
                dec!("100"),
                "prop-liquidity",
                vec!["hyp-liquidity".to_string()],
                start(),
            );
            match platform.submit_order(order, start()) {
                Ok(()) => accepted += 1,
                Err(error) => refusals.push(error.message().to_string()),
            }
        }
    }
    (accepted, refusals)
}

/// The fraction of a book exitable inside the horizon the shipped limit names.
fn within_a_week(state: &qip_risk::limits::RiskState) -> f64 {
    state
        .liquidatable_within
        .get("5")
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "the fraction exitable within five days was never computed for the horizon the \
                 shipped limit names; the map holds {:?} — the limit is taking its None arm and \
                 cannot fire",
                state.liquidatable_within.keys().collect::<Vec<_>>()
            )
        })
}

#[test]
fn a_cycle_over_a_book_that_cannot_be_exited_within_the_week_is_refused_new_risk() -> Result<()> {
    // `b060df2` wired `LimitKind::MinLiquidity` — the `liquidity` limit in
    // `LimitSet::conservative_default` — to the liquidity ladder, closing a
    // limit that had shipped unable to fire. The kernel's own unit tests prove
    // the seam: `risk_state` fills `liquidatable_within` and a check against it
    // breaches. Nothing proved the consequence, which is what an operator
    // actually experiences: an illiquid book stops the platform taking new
    // risk. This drives it end to end, both halves — the floor must refuse a
    // book it should refuse *and admit one it should admit*, because a floor
    // that refuses everything is an outage rather than a control.
    // Ten million, so that nine hundred thousand in one name sits at nine
    // percent of equity — inside `conservative_default`'s ten-percent position
    // cap. On the million-dollar book the other tests use, the same order is
    // refused by the concentration control before the liquidity floor is ever
    // consulted, and this test would then pass while proving the wrong limit
    // fires. That is not hypothetical: it is what this test did on its first
    // run.
    let desk = Decimal::from_int(10_000_000);
    let mut illiquid = platform_of(
        // 900bps is this record's own measured exit spread, stated rather than
        // taken from a constructor default: `illiquid` no longer invents one,
        // because the figure it used to invent became a ceiling on what every
        // listed name above it could be quoted at. It has to stay wider than
        // `FAST`'s 5bps above it or the two records cannot sit on one ladder
        // and `Platform::new` refuses them by name.
        liquidity_universe(LiquidityProfile::illiquid(30.0, 900.0))?,
        LimitSet::conservative_default(),
        desk,
    )?;
    let mut liquid = platform_of(
        liquidity_universe(LiquidityProfile::listed(Decimal::from_int(10_000_000), 5.0))?,
        LimitSet::conservative_default(),
        desk,
    )?;
    illiquid.observe(tape("FAST"));
    liquid.observe(tape("FAST"));

    let (illiquid_accepted, illiquid_refusals) = offer_the_same_book(&mut illiquid);
    let (liquid_accepted, liquid_refusals) = offer_the_same_book(&mut liquid);

    // Premise, and the admitting half of the gate: the identical ten orders
    // are all accepted where `SLOW` is listed. A floor that refused this book
    // too would make everything below meaningless.
    assert_eq!(
        liquid_accepted, 10,
        "the premise failed: the liquid book was refused as well, so the refusal below is not \
         about liquidity — refusals: {liquid_refusals:?}"
    );
    // And the refusing half, at the pre-trade seam: further accumulation into
    // an instrument that takes months to leave is stopped before the order
    // exists.
    assert!(
        illiquid_accepted < 10,
        "every one of ten orders into a book that takes months to exit was accepted; the \
         liquidity floor is taking its None arm again — `liquidatable_within` holds nothing for \
         the horizon the limit names, so the limit reads as satisfied by a figure nobody computed"
    );
    assert!(
        illiquid_refusals
            .iter()
            .any(|refusal| refusal.contains("risk refused: liquidity:")),
        "an order was refused for some other reason than the liquidity floor: \
         {illiquid_refusals:?}"
    );

    // The book each platform ended up holding, as the limit set reads it. The
    // limit is matched on its whole name — `conservative_default` also ships
    // `value-at-risk`, `position-weight` and `sector-concentration`, and a
    // substring match would not tell `liquidity` apart from a longer name
    // containing it.
    let illiquid_state = illiquid.risk_state_from(illiquid.risk_figures());
    let liquid_state = liquid.risk_state_from(liquid.risk_figures());
    assert!(
        within_a_week(&illiquid_state) < 0.80,
        "the premise failed: the illiquid book is exitable within the week after all ({})",
        within_a_week(&illiquid_state)
    );
    assert!(
        within_a_week(&liquid_state) >= 0.80,
        "the premise failed: the liquid book is not exitable within the week ({})",
        within_a_week(&liquid_state)
    );

    let illiquid_breach = illiquid
        .risk_limits()
        .check(&illiquid_state)
        .breaches
        .into_iter()
        .find(|breach| breach.limit_name == "liquidity")
        .expect("the liquidity floor recorded no breach against a book it should refuse");
    assert!(
        illiquid_breach.blocks(),
        "the liquidity floor warned rather than blocked: {illiquid_breach:?}"
    );
    assert!(
        !liquid
            .risk_limits()
            .check(&liquid_state)
            .breaches
            .iter()
            .any(|breach| breach.limit_name == "liquidity"),
        "the liquidity floor breached on a book entirely exitable within a day"
    );

    // Now the whole cycle, on both. Premise: each one actually sized
    // something, so "nothing was released" below is a veto rather than a quiet
    // cycle with nothing to release.
    let fills_before = illiquid.orders().fills().len();
    let illiquid_report = illiquid.run_cycle(start().saturating_add(Duration::from_mins(5)));
    let liquid_report = liquid.run_cycle(start().saturating_add(Duration::from_mins(5)));
    for (label, platform) in [("illiquid", &illiquid), ("liquid", &liquid)] {
        let proposal = platform
            .proposals()
            .last()
            .expect("every cycle records a proposal");
        sized_notional(proposal, label);
    }

    let illiquid_act = illiquid_report.stage(Stage::Act).expect("ACT ran");
    assert!(
        illiquid_act
            .problems
            .iter()
            .any(|problem| problem.starts_with("new risk is blocked: reduce_only")),
        "a cycle over a book that cannot be exited within the week took new risk anyway: {} — \
         {:?}",
        illiquid_act.detail,
        illiquid_act.problems
    );
    assert!(
        illiquid_act
            .problems
            .iter()
            .any(|problem| problem
                == "no proposal was signed off: the risk monitor is at reduce_only"),
        "the sized proposal was signed off despite the floor: {:?}",
        illiquid_act.problems
    );
    assert_eq!(illiquid_act.produced, 0, "an order was released anyway");
    assert_eq!(
        illiquid.orders().fills().len(),
        fills_before,
        "the vetoed cycle still filled something"
    );

    // The admitting half again, at the cycle seam: the liquid book's proposal
    // is signed and reaches the release path. Without this the test would pass
    // against a monitor that blocked every cycle for any reason at all.
    let liquid_act = liquid_report.stage(Stage::Act).expect("ACT ran");
    assert!(
        !liquid_act
            .problems
            .iter()
            .any(|problem| problem.starts_with("new risk is blocked")),
        "new risk was blocked on a book entirely exitable within a day: {} — {:?}",
        liquid_act.detail,
        liquid_act.problems
    );
    assert!(
        liquid_act.detail.contains("from 1 approved proposal(s)"),
        "the liquid book's proposal was never signed, so this test proves only that something \
         refuses: {}",
        liquid_act.detail
    );

    // The boundary, on both: everything above happened against the simulator.
    for platform in [&illiquid, &liquid] {
        assert!(!platform.orders().has_live_fills());
        assert!(!platform.is_live_capable());
    }
    Ok(())
}
