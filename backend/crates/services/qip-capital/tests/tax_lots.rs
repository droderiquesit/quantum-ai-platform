//! Contributions, holding-period state, and the ceiling a realised loss used
//! to open a hole in (blueprint §43.3, `TaxLot`).
//!
//! The property behind most of this file is one the ledger could not state
//! before the lots existed: a [`CashBalance`](qip_capital::ledger::CashBalance)
//! is moved by funding *and* by realised profit and loss, so what a user has
//! at work is not what a user put in. `UserLedger::fund` limited the first and
//! was read as limiting the second, which meant a user could place more of
//! their own money under management than their mandate allows simply by losing
//! some of it first — the loss made the room, and the check that should have
//! caught it was looking at the number the loss had already reduced.
//!
//! The rest of the file holds the other half: holding-period state is declared
//! by an operator per jurisdiction and never guessed, because a default
//! threshold here would be this repository taking a tax position on behalf of
//! a jurisdiction nobody consulted.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::ledger::{
    AttributedFill, DecidedBy, Eligibility, EligibilityDecision, EligibilityRecord, HoldingPeriod,
    Jurisdiction, Mandate, MandateId, MandateTerms, PermittedFamilies, UserId, UserLedger,
    UserShare,
};
use qip_contracts::signal::StrategyId;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use std::collections::BTreeSet;

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn user(name: &str) -> UserId {
    UserId::new(name).expect("a fixture user id is valid")
}

fn strategy() -> StrategyId {
    StrategyId::new("momentum-v3")
}

fn gb() -> Jurisdiction {
    Jurisdiction::new("GB").expect("GB is a jurisdiction")
}

fn operator() -> DecidedBy {
    DecidedBy::operator("ops-alice", "oidc").expect("a named operator is valid")
}

/// Admit a user in GB for a year, so no test below is refused at the
/// eligibility when it is about a contribution.
fn clear(ledger: &mut UserLedger, name: &str) -> Result<()> {
    let eligibility = Eligibility::new(qip_capital::ledger::EligibilityTerms {
        verified_at: now(),
        can_invest: true,
        jurisdiction: gb(),
        expires_at: now().saturating_add(Duration::from_days(3650)),
    })
    .expect("the fixture eligibility is valid");
    ledger.decide_eligibility(EligibilityRecord {
        user: user(name),
        decision: EligibilityDecision::Granted { eligibility },
        by: operator(),
        decided_at: now(),
    })
}

/// A user mandate with no liquidity floor, so `investable` equals `capital`
/// and the two ceilings differ only in what they are measured against.
fn mandate(capital: &str) -> Mandate {
    Mandate::new(MandateTerms {
        capital: Decimal::parse(capital).expect("a fixture capital parses"),
        currency: Currency::USD,
        risk_tolerance: dec!("0.2"),
        permitted_families: PermittedFamilies::Only(BTreeSet::from(["momentum".to_string()])),
        liquidity_floor: dec!("0"),
        exploration_share: dec!("0.1"),
        jurisdiction: gb(),
    })
    .expect("the fixture mandate is valid")
}

fn desk_mandate() -> Mandate {
    Mandate::new(MandateTerms {
        capital: dec!("10000"),
        currency: Currency::USD,
        risk_tolerance: Decimal::ONE,
        permitted_families: PermittedFamilies::Any,
        liquidity_floor: Decimal::ZERO,
        exploration_share: dec!("0.5"),
        jurisdiction: Jurisdiction::new("ZZ").expect("ZZ is a jurisdiction"),
    })
    .expect("the desk fixture is valid")
}

fn ledger() -> UserLedger {
    UserLedger::opened_by(user("desk"), desk_mandate(), now()).expect("the desk opens a ledger")
}

fn enrol(ledger: &mut UserLedger, name: &str, capital: &str) -> Result<()> {
    let id = MandateId::new(format!("m-{name}")).expect("a fixture mandate id is valid");
    ledger.enrol(user(name), id, mandate(capital), now())
}

/// A realised loss of `amount`, booked entirely to one user.
fn loss(amount: &str) -> AttributedFill {
    AttributedFill {
        strategy: strategy(),
        source: "cell-lon-1/momentum-v3/obj-AAA".to_string(),
        currency: Currency::USD,
        amount: Decimal::parse(amount).expect("a fixture amount parses"),
    }
}

/// A ledger with `alice` enrolled at 1000 and cleared to invest.
fn with_alice(capital: &str) -> Result<UserLedger> {
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", capital)?;
    clear(&mut ledger, "alice")?;
    Ok(ledger)
}

#[test]
fn a_realised_loss_does_not_create_room_for_a_contribution_past_the_mandate() -> Result<()> {
    let mut ledger = with_alice("1000")?;
    ledger.fund(&user("alice"), &strategy(), dec!("1000"), now())?;

    // Premise, asserted before the conclusion: the whole mandate is funded,
    // the user has contributed all of it, and a further funding is refused
    // while nothing has gone wrong yet.
    assert_eq!(
        ledger.contributed_total(&user("alice"), Currency::USD)?,
        dec!("1000")
    );
    assert!(
        ledger
            .fund(&user("alice"), &strategy(), dec!("1"), now())
            .is_err(),
        "a fully funded mandate must refuse a further contribution"
    );

    // Now the loss. This is the step that used to make room: it reduces the
    // settled total the old ceiling was measured against, and it must not
    // reduce the contributed total the new one is measured against.
    ledger.journal(
        &loss("-400"),
        &[UserShare {
            user: user("alice"),
            amount: dec!("-400"),
        }],
        now(),
    )?;
    let settled = ledger
        .balance(&user("alice"), &strategy(), Currency::USD)
        .expect("alice holds a book")
        .settled();
    assert_eq!(
        settled,
        dec!("600"),
        "the loss must actually have moved the balance"
    );
    assert_eq!(
        ledger.contributed_total(&user("alice"), Currency::USD)?,
        dec!("1000"),
        "a realised loss is not an un-contribution; what went in still went in"
    );

    // The conclusion. The old settled ceiling would admit this: 600 + 400 is
    // not past 1000. The contribution ceiling refuses it, because 1000 + 400
    // is.
    let refused = ledger
        .fund(&user("alice"), &strategy(), dec!("400"), now())
        .expect_err("a contribution past the mandate's capital is refused");
    let message = refused.to_string();
    assert!(
        message.contains("contributed total from 1000 to 1400"),
        "the refusal must name the contributed total it is measured against, not the \
         settled one; got {message}"
    );
    assert!(
        message.contains("a realised loss does not create room"),
        "the refusal must say why the settled balance looks like it has room; got {message}"
    );

    // And the refusal moved nothing: the books and the lots are as they were.
    assert_eq!(
        ledger.contributed_total(&user("alice"), Currency::USD)?,
        dec!("1000"),
        "a refused funding records no lot"
    );
    assert_eq!(ledger.tax_lots(&user("alice"), &strategy()).len(), 1);
    Ok(())
}

#[test]
fn a_contribution_within_the_mandate_is_still_admitted_after_a_loss() -> Result<()> {
    // The other half of a working gate: one that refuses everything is not a
    // gate. A user who has not exhausted the mandate may still fund after a
    // loss, and the lot is recorded.
    let mut ledger = with_alice("1000")?;
    ledger.fund(&user("alice"), &strategy(), dec!("600"), now())?;
    ledger.journal(
        &loss("-500"),
        &[UserShare {
            user: user("alice"),
            amount: dec!("-500"),
        }],
        now(),
    )?;
    assert_eq!(
        ledger.contributed_total(&user("alice"), Currency::USD)?,
        dec!("600")
    );

    ledger.fund(&user("alice"), &strategy(), dec!("400"), now())?;
    assert_eq!(
        ledger.contributed_total(&user("alice"), Currency::USD)?,
        dec!("1000"),
        "a contribution that reaches the mandate exactly is admitted, not refused"
    );
    assert_eq!(
        ledger.tax_lots(&user("alice"), &strategy()).len(),
        2,
        "each admitted funding records its own lot"
    );
    Ok(())
}

#[test]
fn a_contribution_records_the_basis_the_instant_and_the_jurisdiction_it_was_made_under()
-> Result<()> {
    let mut ledger = with_alice("1000")?;
    assert!(
        ledger.tax_lots(&user("alice"), &strategy()).is_empty(),
        "premise: an unfunded book holds no lots, so the assertions below are about \
         what funding wrote"
    );
    let later = now().saturating_add(Duration::from_days(30));
    ledger.fund(&user("alice"), &strategy(), dec!("250"), later)?;

    let lots = ledger.tax_lots(&user("alice"), &strategy());
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].basis, dec!("250"));
    assert_eq!(
        lots[0].acquired_at, later,
        "the lot carries the instant the caller stated, not the ledger's opening time"
    );
    assert_eq!(
        lots[0].jurisdiction,
        gb(),
        "the lot carries the mandate's jurisdiction, copied at acquisition"
    );
    assert_eq!(lots[0].currency, Currency::USD);
    assert_eq!(lots[0].strategy, strategy());
    Ok(())
}

#[test]
fn an_undeclared_jurisdiction_reports_undetermined_rather_than_a_guessed_short_term() -> Result<()>
{
    // The failure this prevents: folding the undeclared case into `Short`
    // would make a distribution of 100% short-term indistinguishable from one
    // where nobody ever wrote the rules down. A number nobody computed must
    // not read as a measurement.
    let mut ledger = with_alice("1000")?;
    ledger.fund(&user("alice"), &strategy(), dec!("400"), now())?;
    assert!(
        ledger.holding_period_rules().threshold(&gb()).is_none(),
        "premise: no rule is declared for GB, so the lot below has nothing to be measured \
         against"
    );

    // Ten years after acquisition — long by any real rule, and still
    // undetermined because no rule exists.
    let far = now().saturating_add(Duration::from_days(3650));
    let distribution = ledger.holding_period_distribution(far)?;
    assert_eq!(distribution.basis(HoldingPeriod::Undetermined), dec!("400"));
    assert_eq!(distribution.basis(HoldingPeriod::Long), Decimal::ZERO);
    assert_eq!(distribution.basis(HoldingPeriod::Short), Decimal::ZERO);
    assert_eq!(distribution.total_lots(), 1);
    Ok(())
}

#[test]
fn a_declared_threshold_splits_contributions_into_short_and_long_at_the_boundary() -> Result<()> {
    let mut ledger = with_alice("1000")?;
    ledger.declare_holding_period(gb(), Duration::from_days(365))?;

    // One lot at the epoch of this suite, one a year and a day later.
    ledger.fund(&user("alice"), &strategy(), dec!("300"), now())?;
    let late = now().saturating_add(Duration::from_days(366));
    ledger.fund(&user("alice"), &strategy(), dec!("100"), late)?;
    assert_eq!(
        ledger.tax_lots(&user("alice"), &strategy()).len(),
        2,
        "premise: two lots exist, so a split between them is a split of something"
    );

    // Read a day after the second lot: the first is 367 days old and long,
    // the second is one day old and short.
    let at = late.saturating_add(Duration::from_days(1));
    let distribution = ledger.holding_period_distribution(at)?;
    assert_eq!(distribution.basis(HoldingPeriod::Long), dec!("300"));
    assert_eq!(distribution.basis(HoldingPeriod::Short), dec!("100"));
    assert_eq!(
        distribution.basis(HoldingPeriod::Undetermined),
        Decimal::ZERO
    );
    assert_eq!(distribution.total_basis(), dec!("400"));
    assert_eq!(distribution.lots(HoldingPeriod::Long), 1);
    assert_eq!(distribution.lots(HoldingPeriod::Short), 1);

    // The boundary itself: a lot exactly at the threshold is long, not short.
    // `>= threshold`, and an off-by-one here would silently reclassify every
    // holding that sits on its jurisdiction's boundary.
    let exactly = now().saturating_add(Duration::from_days(365));
    let boundary = ledger.holding_period_distribution(exactly)?;
    assert_eq!(
        boundary.basis(HoldingPeriod::Long),
        dec!("300"),
        "a lot held for exactly the declared threshold has reached it"
    );
    Ok(())
}

#[test]
fn a_holding_period_rule_is_not_replaced_in_place_and_a_bad_span_is_refused() -> Result<()> {
    let mut ledger = ledger();
    // A threshold of zero makes every lot long-term at the instant it is
    // acquired, which distinguishes nothing. Refused rather than clamped.
    let refused = ledger
        .declare_holding_period(gb(), Duration::ZERO)
        .expect_err("a zero threshold is refused");
    assert!(
        refused.to_string().contains("distinguishes nothing"),
        "the refusal must say why zero is not a threshold; got {refused}"
    );
    assert!(
        ledger.holding_period_rules().threshold(&gb()).is_none(),
        "a refused declaration must leave the table empty"
    );

    ledger.declare_holding_period(gb(), Duration::from_days(365))?;
    let again = ledger
        .declare_holding_period(gb(), Duration::from_days(30))
        .expect_err("a second declaration under one jurisdiction is refused");
    assert!(
        again.to_string().contains("not replaced in place"),
        "the refusal must name what to do instead; got {again}"
    );
    assert_eq!(
        ledger.holding_period_rules().threshold(&gb()),
        Some(Duration::from_days(365)),
        "the refused overwrite must not have taken effect — the louder claim must not win"
    );

    // Withdrawal is the named way to change one, and a code nobody declared
    // is refused so a mistyped jurisdiction is reported rather than ignored.
    let missing = Jurisdiction::new("FR").expect("FR is a jurisdiction");
    assert!(ledger.withdraw_holding_period(&missing).is_err());
    ledger.withdraw_holding_period(&gb())?;
    assert!(ledger.holding_period_rules().threshold(&gb()).is_none());
    Ok(())
}

#[test]
fn the_contributed_total_and_the_settled_balance_are_different_facts_about_a_book() -> Result<()> {
    // A book holding 150 is a 100 contribution that earned 50 and a 200 that
    // lost 50. Before the lots, nothing in the tree could tell those apart,
    // and the difference is the realised profit and loss.
    let mut ledger = with_alice("1000")?;
    ledger.fund(&user("alice"), &strategy(), dec!("100"), now())?;
    ledger.journal(
        &loss("50"),
        &[UserShare {
            user: user("alice"),
            amount: dec!("50"),
        }],
        now(),
    )?;

    let settled = ledger
        .balance(&user("alice"), &strategy(), Currency::USD)
        .expect("alice holds a book")
        .settled();
    let contributed = ledger.contributed_total(&user("alice"), Currency::USD)?;
    assert_eq!(settled, dec!("150"));
    assert_eq!(contributed, dec!("100"));
    assert_eq!(
        settled - contributed,
        dec!("50"),
        "the difference between the two is what the book earned, which neither could \
         state alone"
    );
    Ok(())
}

#[test]
fn the_distribution_walks_the_lots_in_ledger_key_order_so_a_replay_does_not_reorder() -> Result<()>
{
    // Two users and two strategies, funded in an order that is deliberately
    // not key order, so insertion order and key order genuinely differ.
    //
    // The assertion is on the *sequence*, not on an aggregate. An earlier
    // version of this test summed the distribution eight times and compared
    // the totals, which guarded nothing whatever: the distribution adds, and
    // addition is commutative, so every iteration order produces the same
    // sums. It would have passed over a hash map, which is the exact failure
    // it was named for.
    let mut ledger = ledger();
    for name in ["zoe", "alice"] {
        enrol(&mut ledger, name, "1000")?;
        clear(&mut ledger, name)?;
    }
    let other = StrategyId::new("carry-v1");
    ledger.fund(&user("zoe"), &other, dec!("7"), now())?;
    ledger.fund(&user("alice"), &strategy(), dec!("11"), now())?;
    ledger.fund(&user("zoe"), &strategy(), dec!("13"), now())?;

    // Premise: the three keys were inserted in an order that is not their
    // sorted order, so the assertion below is about sorting and not about
    // insertion.
    let inserted = [
        (user("zoe"), other.clone()),
        (user("alice"), strategy()),
        (user("zoe"), strategy()),
    ];
    let mut sorted = inserted.clone();
    sorted.sort();
    assert_ne!(
        inserted.as_slice(),
        sorted.as_slice(),
        "premise: the fixture must fund in an order other than key order"
    );

    let walked: Vec<_> = ledger.lots().keys().cloned().collect();
    assert_eq!(
        walked.as_slice(),
        sorted.as_slice(),
        "the lots must be walked in LedgerKey order — alice before zoe, and carry-v1 \
         before momentum-v3 within zoe — whatever order they were funded in"
    );

    // And the basis under each key, so the sequence assertion above is about
    // the lots and not only about the keys.
    let bases: Vec<Decimal> = ledger
        .lots()
        .values()
        .map(|lots| lots.iter().map(|lot| lot.basis).sum())
        .collect();
    assert_eq!(bases, vec![dec!("11"), dec!("7"), dec!("13")]);
    Ok(())
}

#[test]
fn a_lot_read_before_it_was_acquired_is_short_rather_than_long() -> Result<()> {
    // A negative age has not reached any positive threshold. Reporting such a
    // lot as long-term — which a duration compared without sign would do —
    // would put capital that did not yet exist in the long-term share.
    let mut ledger = with_alice("1000")?;
    ledger.declare_holding_period(gb(), Duration::from_days(365))?;
    let acquired = now().saturating_add(Duration::from_days(100));
    ledger.fund(&user("alice"), &strategy(), dec!("500"), acquired)?;
    assert_eq!(
        ledger.tax_lots(&user("alice"), &strategy()).len(),
        1,
        "premise: the lot exists and was acquired after the instant read below"
    );

    let distribution = ledger.holding_period_distribution(now())?;
    assert_eq!(distribution.basis(HoldingPeriod::Short), dec!("500"));
    assert_eq!(distribution.basis(HoldingPeriod::Long), Decimal::ZERO);
    Ok(())
}

#[test]
fn the_share_of_each_holding_period_is_reported_against_the_contributed_total() -> Result<()> {
    // The one place money becomes a statistic, and the only consumer of it is
    // a person reading ADR 0008's reversal condition. The amounts stay
    // `Decimal`; only the ratio is `f64`.
    let mut ledger = with_alice("1000")?;
    ledger.declare_holding_period(gb(), Duration::from_days(365))?;
    ledger.fund(&user("alice"), &strategy(), dec!("750"), now())?;
    let late = now().saturating_add(Duration::from_days(400));
    ledger.fund(&user("alice"), &strategy(), dec!("250"), late)?;

    let at = late.saturating_add(Duration::from_days(1));
    let distribution = ledger.holding_period_distribution(at)?;
    assert_eq!(
        distribution.total_basis(),
        dec!("1000"),
        "premise: the whole mandate is in"
    );
    assert!(
        (distribution.share(HoldingPeriod::Long) - 0.75).abs() < 1e-9,
        "three quarters of the contributed basis is long, got {}",
        distribution.share(HoldingPeriod::Long)
    );
    assert!(
        (distribution.share(HoldingPeriod::Short) - 0.25).abs() < 1e-9,
        "one quarter is short, got {}",
        distribution.share(HoldingPeriod::Short)
    );

    // An empty ledger reports zero rather than dividing by nothing.
    let empty = UserLedger::opened_by(user("desk"), desk_mandate(), now())?
        .holding_period_distribution(now())?;
    assert_eq!(empty.total_basis(), Decimal::ZERO);
    let share = empty.share(HoldingPeriod::Long);
    assert!(
        share.is_finite() && share.abs() < f64::EPSILON,
        "an empty ledger has nothing to divide by, and the share must be zero rather than \
         the NaN that zero over zero produces; got {share}"
    );
    Ok(())
}
