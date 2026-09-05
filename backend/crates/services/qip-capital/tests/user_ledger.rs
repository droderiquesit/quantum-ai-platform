//! The per-user, per-strategy ledger (blueprint §43.3, §43.4).
//!
//! Each test here is a refusal the ledger makes or a boundary it holds. The
//! properties are the ones that would fail quietly: a share that sums to
//! almost the fill, a deposit that was declared and counted before it
//! arrived, a mandate whose bad terms came back in through a stored record,
//! and a withdrawal that this platform must never grant.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::ledger::{
    AttributedFill, Capability, Entitlement, Jurisdiction, MAX_USER_ID_LENGTH, Mandate,
    MandateTerms, PermittedFamilies, ProductEligibility, Role, UserId, UserLedger, UserShare,
    WithdrawalEntitlement,
};
use qip_contracts::signal::StrategyId;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Timestamp, dec};
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

fn terms(capital: &str) -> MandateTerms {
    MandateTerms {
        capital: Decimal::parse(capital).expect("a fixture capital parses"),
        currency: Currency::USD,
        risk_tolerance: dec!("0.2"),
        permitted_families: PermittedFamilies::Only(BTreeSet::from(["momentum".to_string()])),
        liquidity_floor: dec!("0"),
        exploration_share: dec!("0.1"),
        jurisdiction: Jurisdiction::new("GB").expect("GB is a jurisdiction"),
    }
}

fn mandate(capital: &str) -> Mandate {
    Mandate::new(terms(capital)).expect("the fixture mandate is valid")
}

fn attributed(amount: &str) -> AttributedFill {
    AttributedFill {
        strategy: strategy(),
        source: "cell-lon-1/momentum-v3/obj-AAA".to_string(),
        currency: Currency::USD,
        amount: Decimal::parse(amount).expect("a fixture amount parses"),
    }
}

// --- identity ---------------------------------------------------------------

#[test]
fn a_user_id_that_is_padded_oversized_or_carries_a_foreign_character_is_refused_by_name() {
    // The failure: an id accepted with a trailing space is a second user the
    // moment one caller trims and another does not, and one person's capital
    // is split across two books neither can see whole. Premise first: the
    // clean id is accepted, so the refusals below are about the flaw.
    assert!(UserId::new("alice.chen-01").is_ok());

    let padded = UserId::new("alice ").expect_err("a padded id is refused");
    assert!(
        padded.message().contains("whitespace"),
        "the refusal names the padding: {}",
        padded.message()
    );
    let foreign = UserId::new("alice/chen").expect_err("a slash is refused");
    assert!(
        foreign.message().contains("'/'"),
        "the refusal names the character: {}",
        foreign.message()
    );
    let empty = UserId::new("").expect_err("an empty id is refused");
    assert!(empty.message().contains("empty"));
    let long = "a".repeat(MAX_USER_ID_LENGTH + 1);
    let oversized = UserId::new(long).expect_err("an oversized id is refused");
    assert!(
        oversized
            .message()
            .contains(&MAX_USER_ID_LENGTH.to_string()),
        "the refusal names the bound: {}",
        oversized.message()
    );
}

// --- mandate ----------------------------------------------------------------

#[test]
fn a_mandate_whose_shares_leave_the_unit_interval_or_whose_floor_exceeds_its_capital_is_refused_by_field()
 {
    // The failure: a share of 1.2 is a caller that confused percent with
    // fraction. Clamped to 1, it would explore with all of a user's capital
    // while the caller believed it was exploring with a fifth. Premise: the
    // fixture terms are valid as stated.
    assert!(Mandate::new(terms("1000")).is_ok());

    let mut over = terms("1000");
    over.exploration_share = dec!("1.2");
    let error = Mandate::new(over).expect_err("a share above one is refused");
    assert!(
        error.message().starts_with("exploration_share"),
        "the refusal names the field: {}",
        error.message()
    );

    let mut negative = terms("1000");
    negative.risk_tolerance = dec!("-0.1");
    let error = Mandate::new(negative).expect_err("a negative tolerance is refused");
    assert!(
        error.message().starts_with("risk_tolerance"),
        "the refusal names the field: {}",
        error.message()
    );

    let mut floored = terms("1000");
    floored.liquidity_floor = dec!("1001");
    let error = Mandate::new(floored).expect_err("a floor above the capital is refused");
    assert!(
        error.message().starts_with("liquidity_floor"),
        "the refusal names the field: {}",
        error.message()
    );

    let mut owed = terms("1000");
    owed.capital = dec!("-1");
    let error = Mandate::new(owed).expect_err("negative capital is refused");
    assert!(error.message().contains("capital cannot be negative"));
}

#[test]
fn a_mandate_permitting_no_family_is_refused_rather_than_enrolled() {
    // The failure: a mandate under which nothing can invest is enrolled, and
    // every investment request against it is refused for a reason that
    // reads like the product's fault. Premise: the named-family form and
    // the desk's `Any` are both accepted.
    assert!(Mandate::new(terms("1000")).is_ok());
    assert!(Mandate::desk(dec!("1000"), Currency::USD).is_ok());

    let mut empty = terms("1000");
    empty.permitted_families = PermittedFamilies::Only(BTreeSet::new());
    let error = Mandate::new(empty).expect_err("no families is refused");
    assert!(
        error
            .message()
            .contains("permitted_families names no family"),
        "the refusal names the field: {}",
        error.message()
    );

    let mut blank = terms("1000");
    blank.permitted_families = PermittedFamilies::Only(BTreeSet::from([" ".to_string()]));
    let error = Mandate::new(blank).expect_err("a blank family is refused");
    assert!(error.message().contains("blank family"));
}

#[test]
fn a_stored_mandate_whose_terms_have_gone_bad_is_refused_on_the_way_back_in() -> Result<()> {
    // The failure: validation at construction and none at deserialisation,
    // so a record edited on disk — or written by an older version with a
    // different rule — comes back as a trusted mandate with a share of two.
    // Premise: the round trip of a valid mandate is exact.
    let valid = mandate("1000");
    let json = serde_json::to_value(&valid).expect("a mandate serialises");
    let restored: Mandate = serde_json::from_value(json.clone()).expect("a valid record restores");
    assert_eq!(restored, valid);

    let mut edited = json;
    edited["exploration_share"] = serde_json::Value::String("2".to_string());
    let refused = serde_json::from_value::<Mandate>(edited)
        .expect_err("a record with a share of two is refused");
    assert!(
        refused.to_string().contains("exploration_share"),
        "the refusal names the field: {refused}"
    );
    Ok(())
}

// --- entitlement ------------------------------------------------------------

#[test]
fn a_withdrawal_is_refused_for_every_role_and_the_refusal_names_the_adr() {
    // The failure ADR 0021 names: the permitted half of the treasury is
    // built, then a granted withdrawal "to make it useful", each step
    // defensible. The withdrawal arm has one variant, and the match below
    // is exhaustive without a wildcard — if a second arm is ever added,
    // this test stops compiling, which is the point. Premise: the investor
    // is otherwise fully entitled, so the refusal is not incidental.
    let alice = user("alice");
    let mandate = mandate("1000");
    let product = ProductEligibility::new("momentum")
        .eligible_in(Jurisdiction::new("GB").expect("GB is a jurisdiction"));
    let investor = Entitlement::evaluate(&alice, &mandate, Role::Investor, &product, now());
    assert!(
        investor.can_invest().is_granted(),
        "the premise is a fully entitled investor: {:?}",
        investor.can_invest()
    );
    assert!(investor.can_view().is_granted());

    for role in [Role::Viewer, Role::Investor, Role::Operator] {
        let entitlement = Entitlement::evaluate(&alice, &mandate, role, &product, now());
        let WithdrawalEntitlement::Refused { reason } = entitlement.can_withdraw();
        assert!(
            reason.contains("ADR 0021"),
            "the {role:?} refusal names the decision: {reason}"
        );
    }
}

#[test]
fn an_investment_is_refused_by_the_input_that_refused_it() {
    // The failure: one refusal reason for every cause, so "why can't I" is
    // a support ticket. Premise: with every input right the grant is made.
    let alice = user("alice");
    let full = mandate("1000");
    let gb = Jurisdiction::new("GB").expect("GB is a jurisdiction");
    let product = ProductEligibility::new("momentum").eligible_in(gb);
    assert!(
        Entitlement::evaluate(&alice, &full, Role::Investor, &product, now())
            .can_invest()
            .is_granted()
    );

    let viewer = Entitlement::evaluate(&alice, &full, Role::Viewer, &product, now());
    match viewer.can_invest() {
        Capability::Refused { reason } => assert!(reason.contains("viewer role"), "{reason}"),
        Capability::Granted { basis } => panic!("a viewer was granted investment: {basis}"),
    }

    let elsewhere = ProductEligibility::new("momentum")
        .eligible_in(Jurisdiction::new("US").expect("US is a jurisdiction"));
    let abroad = Entitlement::evaluate(&alice, &full, Role::Investor, &elsewhere, now());
    match abroad.can_invest() {
        Capability::Refused { reason } => {
            assert!(reason.contains("not eligible in GB"), "{reason}");
        }
        Capability::Granted { basis } => panic!("an ineligible jurisdiction was granted: {basis}"),
    }

    let other_family = ProductEligibility::new("carry").eligible_in(gb);
    let unpermitted = Entitlement::evaluate(&alice, &full, Role::Investor, &other_family, now());
    match unpermitted.can_invest() {
        Capability::Refused { reason } => {
            assert!(
                reason.contains("does not permit the family carry"),
                "{reason}"
            );
        }
        Capability::Granted { basis } => panic!("an unpermitted family was granted: {basis}"),
    }

    let broke = Entitlement::evaluate(&alice, &mandate("0"), Role::Investor, &product, now());
    match broke.can_invest() {
        Capability::Refused { reason } => {
            assert!(reason.contains("no investable capital"), "{reason}");
        }
        Capability::Granted { basis } => panic!("a zero mandate was granted: {basis}"),
    }
}

// --- cash -------------------------------------------------------------------

#[test]
fn an_expected_inflow_cannot_be_spent_until_the_ledger_posts_it() -> Result<()> {
    // The failure §43.3 writes into the definition of `ExpectedInflow`: a
    // deposit the user says they sent is counted, a position is sized
    // against it, and the platform has lent to the user without anyone
    // deciding to. Premise: the book is empty before the declaration.
    let alice = user("alice");
    let mut ledger = UserLedger::new();
    ledger.enrol(alice.clone(), mandate("1000"))?;
    assert!(ledger.balance(&alice, &strategy(), Currency::USD).is_none());

    ledger.expect_inflow(&alice, &strategy(), "wire-0001", dec!("500"), now())?;
    let balance = ledger
        .balance(&alice, &strategy(), Currency::USD)
        .expect("the declaration opened a book")
        .clone();
    assert_eq!(
        balance.expected_total(),
        dec!("500"),
        "the claim is visible"
    );
    assert_eq!(balance.available(), Decimal::ZERO, "and not available");

    let mut spend = balance.clone();
    let refused = spend
        .debit(dec!("100"))
        .expect_err("spending against an expected inflow is refused");
    assert!(
        refused
            .message()
            .contains("500 is expected and not available"),
        "the refusal names the money the caller was told about: {}",
        refused.message()
    );
    let mut hold = balance.clone();
    assert!(hold.reserve(dec!("100")).is_err(), "nor can it be reserved");
    assert_eq!(spend, balance, "a refused debit moves nothing");

    let posted = ledger.post_inflow(&alice, &strategy(), "wire-0001", now())?;
    assert_eq!(posted, dec!("500"));
    let mut balance = ledger
        .balance(&alice, &strategy(), Currency::USD)
        .expect("the book persists")
        .clone();
    assert_eq!(balance.available(), dec!("500"));
    assert_eq!(balance.expected_total(), Decimal::ZERO);
    balance.debit(dec!("100"))?;
    assert_eq!(balance.available(), dec!("400"));

    // A second declaration under the same reference is refused: it is a
    // retry or a reused reference, and reconciliation cannot tell which.
    let again = ledger
        .expect_inflow(&alice, &strategy(), "wire-0001", dec!("500"), now())
        .is_err();
    assert!(
        !again,
        "the posted reference is free again; what follows tests an unposted duplicate"
    );
    assert!(
        ledger
            .expect_inflow(&alice, &strategy(), "wire-0001", dec!("500"), now())
            .is_err(),
        "a duplicate of an unposted reference is refused"
    );
    Ok(())
}

// --- the books --------------------------------------------------------------

#[test]
fn a_fill_split_across_users_that_does_not_sum_to_the_fill_is_refused_and_no_book_moves()
-> Result<()> {
    // ADR 0007 one link further down the chain: a split that sums to almost
    // the fill leaves an amount nobody is attributed, and a ledger that
    // booked the shares it had would hide it in the gap between two users'
    // statements. Premise: both users hold mandates and no book exists.
    let alice = user("alice");
    let bram = user("bram");
    let mut ledger = UserLedger::new();
    ledger.enrol(alice.clone(), mandate("1000"))?;
    ledger.enrol(bram.clone(), mandate("1000"))?;
    assert!(ledger.books().is_empty());
    let fill = attributed("100");

    let short = [
        UserShare {
            user: alice.clone(),
            amount: dec!("60"),
        },
        UserShare {
            user: bram.clone(),
            amount: dec!("30"),
        },
    ];
    assert_ne!(
        short.iter().map(|s| s.amount).sum::<Decimal>(),
        fill.amount,
        "the premise is a split that does not sum"
    );
    let refused = ledger
        .journal(&fill, &short, now())
        .expect_err("a short split is refused");
    assert!(
        refused.message().contains("difference of 10"),
        "the refusal names the residual: {}",
        refused.message()
    );
    assert!(ledger.books().is_empty(), "no book moved");
    assert_eq!(ledger.fills_journalled(), 0);

    let twice = [
        UserShare {
            user: alice.clone(),
            amount: dec!("50"),
        },
        UserShare {
            user: alice.clone(),
            amount: dec!("50"),
        },
    ];
    let refused = ledger
        .journal(&fill, &twice, now())
        .expect_err("a user named twice is refused even though the sum closes");
    assert!(refused.message().contains("twice"), "{}", refused.message());
    assert!(ledger.books().is_empty());

    let exact = [
        UserShare {
            user: alice.clone(),
            amount: dec!("60"),
        },
        UserShare {
            user: bram.clone(),
            amount: dec!("40"),
        },
    ];
    ledger.journal(&fill, &exact, now())?;
    assert_eq!(ledger.fills_journalled(), 1);
    let alice_cash = ledger
        .balance(&alice, &strategy(), Currency::USD)
        .expect("alice's book opened");
    let bram_cash = ledger
        .balance(&bram, &strategy(), Currency::USD)
        .expect("bram's book opened");
    assert_eq!(alice_cash.settled(), dec!("60"));
    assert_eq!(bram_cash.settled(), dec!("40"));
    assert_eq!(
        alice_cash.settled() + bram_cash.settled(),
        fill.amount,
        "the books sum to the fill"
    );
    assert_eq!(
        ledger.book(&alice, &strategy()).map(|book| book.entries()),
        Some(1)
    );
    Ok(())
}

#[test]
fn a_fill_naming_a_user_without_a_mandate_is_refused_whole() -> Result<()> {
    // The failure: the first share is booked before the second is found to
    // name nobody, and the fill is half in the books. Premise: alice's
    // share alone is valid and would book if journalled by itself.
    let alice = user("alice");
    let nobody = user("nobody");
    let mut ledger = UserLedger::new();
    ledger.enrol(alice.clone(), mandate("1000"))?;
    let fill = attributed("100");
    let mut alone = ledger.clone();
    alone.journal_to(&alice, &fill, now())?;
    assert_eq!(
        alone.fills_journalled(),
        1,
        "the premise: alice alone books"
    );

    let split = [
        UserShare {
            user: alice.clone(),
            amount: dec!("50"),
        },
        UserShare {
            user: nobody.clone(),
            amount: dec!("50"),
        },
    ];
    let refused = ledger
        .journal(&fill, &split, now())
        .expect_err("a user without a mandate is refused");
    assert!(
        refused.message().contains("nobody, who holds no mandate"),
        "{}",
        refused.message()
    );
    assert!(
        ledger.books().is_empty(),
        "alice's half was not booked either"
    );
    assert_eq!(ledger.fills_journalled(), 0);
    Ok(())
}

#[test]
fn a_realised_loss_is_booked_as_a_loss_and_never_floored() -> Result<()> {
    // The failure: a balance floored at zero hides what a user owes, which
    // is the last thing a per-user ledger may do. Premise: the book holds a
    // hundred before the loss.
    let alice = user("alice");
    let mut ledger = UserLedger::new();
    ledger.enrol(alice.clone(), mandate("1000"))?;
    ledger.fund(&alice, &strategy(), dec!("100"), now())?;
    assert_eq!(
        ledger
            .balance(&alice, &strategy(), Currency::USD)
            .map(|cash| cash.settled()),
        Some(dec!("100"))
    );
    ledger.journal_to(&alice, &attributed("-250"), now())?;
    assert_eq!(
        ledger
            .balance(&alice, &strategy(), Currency::USD)
            .map(|cash| cash.settled()),
        Some(dec!("-150")),
        "the loss stands as booked"
    );
    Ok(())
}

#[test]
fn funding_past_the_investable_capital_is_refused_and_the_liquidity_floor_is_honoured() -> Result<()>
{
    // The failure: the floor is a number on the mandate that nothing reads,
    // so a user with a stated floor of two hundred has all thousand at
    // work. Premise: the first funding inside the floor is accepted.
    let alice = user("alice");
    let mut floored = terms("1000");
    floored.liquidity_floor = dec!("200");
    let mut ledger = UserLedger::new();
    ledger.enrol(alice.clone(), Mandate::new(floored)?)?;
    ledger.fund(&alice, &strategy(), dec!("500"), now())?;

    let refused = ledger
        .fund(&alice, &StrategyId::new("carry-v1"), dec!("400"), now())
        .expect_err("nine hundred at work leaves only a hundred liquid");
    assert!(
        refused.message().contains("past the 800 investable"),
        "the refusal names the investable capital: {}",
        refused.message()
    );
    assert!(
        ledger.book(&alice, &StrategyId::new("carry-v1")).is_none(),
        "a refused funding opens no book"
    );
    ledger.fund(&alice, &StrategyId::new("carry-v1"), dec!("300"), now())?;
    assert_eq!(
        ledger
            .balance(&alice, &StrategyId::new("carry-v1"), Currency::USD)
            .map(|cash| cash.available()),
        Some(dec!("300"))
    );
    Ok(())
}
