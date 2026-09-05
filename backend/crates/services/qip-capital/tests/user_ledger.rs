//! The per-user, per-strategy ledger (blueprint §43.3, §43.4).
//!
//! Each test here is a refusal the ledger makes or a boundary it holds. The
//! properties are the ones that would fail quietly: a share that sums to
//! almost the fill, a deposit that was declared and counted before it
//! arrived, a mandate whose bad terms came back in through a stored record,
//! a mandate that promised more than the desk has, a request admitted past
//! a limit nobody named, a rounding unit that landed in nobody's book, and
//! a withdrawal that this platform must never grant.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::ledger::{
    AttributedFill, Capability, DESK_MANDATE_ID, Entitlement, InvestmentOutcome, InvestmentRequest,
    Jurisdiction, MAX_USER_ID_LENGTH, Mandate, MandateId, MandateRegistry, MandateTerms,
    PermittedFamilies, ProductEligibility, RefusedLimit, Role, UserId, UserLedger, UserShare,
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

fn mandate_id(name: &str) -> MandateId {
    MandateId::new(format!("m-{name}")).expect("a fixture mandate id is valid")
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

/// The desk this suite's users are admitted under: ten thousand, every
/// family, a full risk tolerance and half set aside for exploration, so the
/// user fixture (a fifth tolerated, a tenth explored) sits inside it and
/// every refusal below is about the term the test moved.
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
    ledger.enrol(user(name), mandate_id(name), mandate(capital), now())
}

fn attributed(amount: &str) -> AttributedFill {
    AttributedFill {
        strategy: strategy(),
        source: "cell-lon-1/momentum-v3/obj-AAA".to_string(),
        currency: Currency::USD,
        amount: Decimal::parse(amount).expect("a fixture amount parses"),
    }
}

fn request(name: &str, amount: &str) -> InvestmentRequest {
    InvestmentRequest {
        user: user(name),
        strategy: strategy(),
        family: "momentum".to_string(),
        currency: Currency::USD,
        amount: Decimal::parse(amount).expect("a fixture amount parses"),
        requested_at: now(),
    }
}

fn momentum_in_gb() -> ProductEligibility {
    ProductEligibility::new("momentum")
        .eligible_in(Jurisdiction::new("GB").expect("GB is a jurisdiction"))
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

#[test]
fn a_mandate_id_is_held_to_the_same_rule_as_a_user_id() {
    // The failure: the registry refuses a duplicate mandate id, and the
    // refusal guards nothing if "m-1" and "m-1 " are two ids. Premise: the
    // clean id is accepted and reads back as given.
    let clean = MandateId::new("m-2026.09-alice").expect("a clean mandate id is accepted");
    assert_eq!(clean.as_str(), "m-2026.09-alice");

    let padded = MandateId::new(" m-1").expect_err("a padded mandate id is refused");
    assert!(
        padded.message().contains("mandate id") && padded.message().contains("whitespace"),
        "the refusal names the thing and the flaw: {}",
        padded.message()
    );
    assert!(
        MandateId::new("").is_err(),
        "an empty mandate id is refused"
    );
    assert!(
        MandateId::new("m/1").is_err(),
        "a foreign character is refused"
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

// --- the registry -----------------------------------------------------------

#[test]
fn a_mandate_id_registered_twice_is_refused_naming_its_holder_and_nothing_is_recorded() -> Result<()>
{
    // The failure: a superseding mandate recorded under the id of the one it
    // replaces, so the terms a fill was booked under are no longer findable
    // by name. Premise: the first registration under the id is admitted and
    // reads back under it.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    assert_eq!(
        ledger.registry().holder(&mandate_id("alice")),
        Some(&user("alice")),
        "the premise: the id is held by alice"
    );

    let refused = ledger
        .enrol(user("bram"), mandate_id("alice"), mandate("500"), now())
        .expect_err("a second registration under alice's id is refused");
    assert!(
        refused.message().contains("m-alice")
            && refused.message().contains("already registered to alice"),
        "the refusal names the id and its holder: {}",
        refused.message()
    );
    assert!(
        ledger.mandate(&user("bram")).is_none(),
        "the refused registration recorded no mandate for bram"
    );
    assert_eq!(
        ledger.registry().capital_under_users(),
        dec!("1000"),
        "and promised no capital"
    );

    // A second mandate for a user who already holds one is the same class
    // of refusal — replaced in place is unrecoverable — even under a new id.
    let again = ledger
        .enrol(user("alice"), mandate_id("alice-2"), mandate("500"), now())
        .expect_err("alice already holds a mandate");
    assert!(
        again.message().contains("already holds a mandate"),
        "{}",
        again.message()
    );
    assert!(
        ledger.registry().holder(&mandate_id("alice-2")).is_none(),
        "the refused id was not reserved either"
    );
    Ok(())
}

#[test]
fn a_mandate_that_promises_more_than_the_desk_carries_is_refused_by_the_term_that_exceeds_it()
-> Result<()> {
    // The failure: the desk tolerates losing a fifth and a user is enrolled
    // tolerating all; the desk has ten thousand and a user is enrolled with
    // twenty. Each promise is one the desk cannot keep, and a registry
    // without a ceiling makes every one. Premise: the fixture user, whose
    // every term is inside the desk's, is admitted.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;

    let mut rich = terms("20000");
    rich.capital = dec!("20000");
    let refused = ledger
        .enrol(user("bram"), mandate_id("bram"), Mandate::new(rich)?, now())
        .expect_err("capital above the desk's is refused");
    assert!(
        refused.message().contains("20000 under management")
            && refused.message().contains("desk capital of 10000"),
        "the refusal names both capitals: {}",
        refused.message()
    );

    let mut brave = terms("1000");
    brave.risk_tolerance = Decimal::ONE;
    // The desk fixture tolerates everything, so a user tolerating everything
    // is inside it; prove that first, then narrow the desk and show the
    // same user refused by that one term.
    UserLedger::opened_by(user("desk"), desk_mandate(), now())?.enrol(
        user("bram"),
        mandate_id("bram"),
        Mandate::new(brave.clone())?,
        now(),
    )?;
    let mut cautious_desk = desk_mandate().terms().clone();
    cautious_desk.risk_tolerance = dec!("0.2");
    let mut cautious = UserLedger::opened_by(user("desk"), Mandate::new(cautious_desk)?, now())?;
    let refused = cautious
        .enrol(
            user("bram"),
            mandate_id("bram"),
            Mandate::new(brave)?,
            now(),
        )
        .expect_err("a tolerance above the desk's is refused");
    assert!(
        refused.message().contains("tolerates losing 1")
            && refused.message().contains("desk's tolerance of 0.2"),
        "the refusal names both tolerances: {}",
        refused.message()
    );

    let mut curious = terms("1000");
    curious.exploration_share = dec!("0.6");
    let refused = ledger
        .enrol(
            user("bram"),
            mandate_id("bram"),
            Mandate::new(curious)?,
            now(),
        )
        .expect_err("an exploration share above the desk's is refused");
    assert!(
        refused.message().contains("sets aside 0.6") && refused.message().contains("desk's 0.5"),
        "the refusal names both shares: {}",
        refused.message()
    );

    let mut narrow_desk = desk_mandate().terms().clone();
    narrow_desk.permitted_families =
        PermittedFamilies::Only(BTreeSet::from(["momentum".to_string()]));
    let mut narrow = UserLedger::opened_by(user("desk"), Mandate::new(narrow_desk)?, now())?;
    let mut wide = terms("1000");
    wide.permitted_families = PermittedFamilies::Only(BTreeSet::from([
        "momentum".to_string(),
        "carry".to_string(),
    ]));
    let refused = narrow
        .enrol(user("bram"), mandate_id("bram"), Mandate::new(wide)?, now())
        .expect_err("a family the desk does not permit is refused");
    assert!(
        refused.message().contains("permits the family carry"),
        "the refusal names the family: {}",
        refused.message()
    );

    let mut foreign = terms("1000");
    foreign.currency = Currency::EUR;
    let refused = ledger
        .enrol(
            user("bram"),
            mandate_id("bram"),
            Mandate::new(foreign)?,
            now(),
        )
        .expect_err("a currency other than the desk's is refused");
    assert!(
        refused.message().contains("in EUR") && refused.message().contains("desk's is in USD"),
        "the refusal names both currencies: {}",
        refused.message()
    );

    assert!(
        ledger.mandate(&user("bram")).is_none()
            && ledger.registry().holder(&mandate_id("bram")).is_none(),
        "none of the refusals recorded anything"
    );
    Ok(())
}

#[test]
fn user_mandates_cannot_together_promise_more_capital_than_the_desk_holds() -> Result<()> {
    // The failure: ten users each inside the desk's capital, whose mandates
    // together promise ten times what there is — every per-mandate check
    // passes and the desk is over-promised anyway. Premise: the first two
    // users, together under the ceiling, are admitted and the total is what
    // they were enrolled with.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "6000")?;
    enrol(&mut ledger, "bram", "3000")?;
    assert_eq!(ledger.registry().capital_under_users(), dec!("9000"));

    let refused = ledger
        .enrol(user("cara"), mandate_id("cara"), mandate("1001"), now())
        .expect_err("the third mandate takes the total past the desk");
    assert!(
        refused.message().contains("from 9000 to 10001")
            && refused.message().contains("desk capital of 10000"),
        "the refusal names the total and the ceiling: {}",
        refused.message()
    );
    assert!(ledger.mandate(&user("cara")).is_none());
    assert_eq!(ledger.registry().capital_under_users(), dec!("9000"));

    ledger.enrol(user("cara"), mandate_id("cara"), mandate("1000"), now())?;
    assert_eq!(
        ledger.registry().capital_under_users(),
        dec!("10000"),
        "exactly the desk's capital is admitted"
    );
    Ok(())
}

#[test]
fn the_registry_reads_mandates_in_user_id_order_however_they_were_enrolled() -> Result<()> {
    // The failure: a report of who holds what that comes out in enrolment
    // order on one machine and another order on the next, so two replays of
    // one log disagree. Premise: enrolment order is the reverse of id order.
    let mut ledger = ledger();
    let enrolled = ["cara", "bram", "alice"];
    for name in enrolled {
        enrol(&mut ledger, name, "1000")?;
    }
    let read: Vec<&str> = ledger.mandates().keys().map(UserId::as_str).collect();
    assert_eq!(read, ["alice", "bram", "cara", "desk"]);
    assert_ne!(
        read[..3],
        enrolled[..],
        "the premise: the read order is not the enrolment order"
    );
    Ok(())
}

#[test]
fn a_stored_registry_that_has_gone_bad_is_refused_on_the_way_back_in() -> Result<()> {
    // The failure: the ceiling is checked at registration and a stored
    // record is trusted because it was once ours, so a record edited to
    // carry a user with twice the desk's capital comes back as a registry.
    // Premise: a valid registry round-trips exactly, desk id and all.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    let json = serde_json::to_value(ledger.registry()).expect("a registry serialises");
    assert_eq!(
        json["desk_mandate"]["id"].as_str(),
        Some(DESK_MANDATE_ID),
        "the record names the desk's mandate id"
    );
    let restored: MandateRegistry =
        serde_json::from_value(json.clone()).expect("a valid record restores");
    assert_eq!(&restored, ledger.registry());

    let mut edited = json;
    edited["users"]["alice"]["mandate"]["capital"] = serde_json::Value::String("20000".into());
    let refused = serde_json::from_value::<MandateRegistry>(edited)
        .expect_err("a record over the desk's capital is refused");
    assert!(
        refused.to_string().contains("desk capital of 10000"),
        "the refusal is the registry's own: {refused}"
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
    let product = momentum_in_gb();
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
    let product = momentum_in_gb();
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

// --- investment requests ----------------------------------------------------

#[test]
fn an_investment_request_is_admitted_or_refused_by_the_named_limit_before_anything_is_funded()
-> Result<()> {
    // The failure `.claude/rules/domains/risk-and-execution.md` opens with:
    // a limit checked after the order exists. Here the answer is a record
    // naming the limit, and the books are untouched whichever way it went.
    // Premise: a request inside every limit is admitted, and admitting it
    // funds nothing.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    let product = momentum_in_gb();
    let admitted = ledger.admit(&request("alice", "150"), Role::Investor, &product, now());
    assert!(
        admitted.is_admitted(),
        "the premise: 150 of 1000 at a fifth tolerated is inside every limit: {:?}",
        admitted.outcome()
    );
    assert_eq!(admitted.refused_by(), None);
    assert!(
        ledger.book(&user("alice"), &strategy()).is_none(),
        "an admitted request funded nothing"
    );

    // The risk tolerance: a fifth of a thousand is two hundred at one
    // strategy, and 150 already there leaves room for 50, not 51.
    ledger.fund(&user("alice"), &strategy(), dec!("150"), now())?;
    let over = ledger.admit(&request("alice", "51"), Role::Investor, &product, now());
    assert_eq!(over.refused_by(), Some(RefusedLimit::RiskTolerance));
    match over.outcome() {
        InvestmentOutcome::Refused { reason, .. } => assert!(
            reason.contains("would put 201 at one strategy against the 200"),
            "the refusal names the amounts: {reason}"
        ),
        InvestmentOutcome::Admitted { basis } => panic!("admitted past the tolerance: {basis}"),
    }
    assert!(
        ledger
            .admit(&request("alice", "50"), Role::Investor, &product, now())
            .is_admitted(),
        "exactly the tolerated amount is admitted"
    );

    // The investable capital, net of what is already at work across every
    // strategy: 150 at momentum-v3 leaves 850 of the mandate's 1000, and a
    // request at another strategy for 851 is refused by that limit even
    // though nothing is at that strategy yet — the tolerance check would
    // have admitted it. A generous tolerance so that limit is not the one
    // that fires.
    let mut generous = ledger.clone();
    let mut wide_open = terms("1000");
    wide_open.risk_tolerance = Decimal::ONE;
    generous.enrol(
        user("bram"),
        mandate_id("bram"),
        Mandate::new(wide_open)?,
        now(),
    )?;
    generous.fund(&user("bram"), &strategy(), dec!("150"), now())?;
    let mut elsewhere = request("bram", "851");
    elsewhere.strategy = StrategyId::new("carry-v1");
    let too_much = generous.admit(&elsewhere, Role::Investor, &product, now());
    assert_eq!(too_much.refused_by(), Some(RefusedLimit::InvestableCapital));
    match too_much.outcome() {
        InvestmentOutcome::Refused { reason, .. } => assert!(
            reason.contains("exceeds the 850 investable") && reason.contains("150 already at work"),
            "the refusal names the headroom and what is at work: {reason}"
        ),
        InvestmentOutcome::Admitted { basis } => panic!("admitted past the capital: {basis}"),
    }

    let mut nobody = request("nobody", "10");
    nobody.user = user("nobody");
    assert_eq!(
        ledger
            .admit(&nobody, Role::Investor, &product, now())
            .refused_by(),
        Some(RefusedLimit::NoMandate)
    );
    assert_eq!(
        ledger
            .admit(&request("alice", "10"), Role::Viewer, &product, now())
            .refused_by(),
        Some(RefusedLimit::Entitlement),
        "a viewer's request is refused at the entitlement"
    );
    let mut euros = request("alice", "10");
    euros.currency = Currency::EUR;
    assert_eq!(
        ledger
            .admit(&euros, Role::Investor, &product, now())
            .refused_by(),
        Some(RefusedLimit::Currency)
    );
    assert_eq!(
        ledger
            .admit(&request("alice", "0"), Role::Investor, &product, now())
            .refused_by(),
        Some(RefusedLimit::Amount)
    );
    let mut carry = request("alice", "10");
    carry.family = "carry".to_string();
    assert_eq!(
        ledger
            .admit(&carry, Role::Investor, &product, now())
            .refused_by(),
        Some(RefusedLimit::Entitlement),
        "a request naming a family other than the product's is refused at the entitlement"
    );

    assert_eq!(
        ledger
            .balance(&user("alice"), &strategy(), Currency::USD)
            .map(|cash| cash.settled()),
        Some(dec!("150")),
        "no decision, admitted or refused, moved a book"
    );
    Ok(())
}

#[test]
fn the_same_request_against_the_same_books_gets_the_same_decision() -> Result<()> {
    // The failure: a decision that depends on anything but the request and
    // the books cannot be reproduced from the log. Premise: the two decisions
    // are taken from the same books, and the second is not a clone of the
    // first — they were computed apart.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    let product = momentum_in_gb();
    let first = ledger.admit(&request("alice", "500"), Role::Investor, &product, now());
    let second = ledger.admit(&request("alice", "500"), Role::Investor, &product, now());
    assert_eq!(first.refused_by(), Some(RefusedLimit::RiskTolerance));
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).expect("a decision serialises"),
        serde_json::to_string(&second).expect("a decision serialises"),
        "and they journal identically"
    );
    Ok(())
}

// --- cash -------------------------------------------------------------------

#[test]
fn an_expected_inflow_cannot_be_spent_until_the_ledger_posts_it() -> Result<()> {
    // The failure §43.3 writes into the definition of `ExpectedInflow`: a
    // deposit the user says they sent is counted, a position is sized
    // against it, and the platform has lent to the user without anyone
    // deciding to. Premise: the book is empty before the declaration.
    let alice = user("alice");
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
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
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    enrol(&mut ledger, "bram", "1000")?;
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
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
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
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
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
    let mut ledger = ledger();
    ledger.enrol(
        alice.clone(),
        mandate_id("alice"),
        Mandate::new(floored)?,
        now(),
    )?;
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

// --- pro-rata splits --------------------------------------------------------

#[test]
fn a_pro_rata_split_reconciles_to_the_fill_exactly_and_the_remainder_is_recorded_not_dropped()
-> Result<()> {
    // ADR 0007 at the last link: three equal holders of a fill of a hundred
    // are each owed 33.333…, and nine decimals of that three times over is
    // 99.999999999 — one raw unit short of the fill. A split that rounded
    // each share on its own would either drop the unit into nobody's book or
    // round it up into three books that sum past the fill. Premise: the
    // three entitlements are equal and positive, so the shares are equal and
    // the remainder is the truncation's.
    let mut ledger = ledger();
    for name in ["cara", "alice", "bram"] {
        enrol(&mut ledger, name, "1000")?;
        ledger.fund(&user(name), &strategy(), dec!("100"), now())?;
    }
    let fill = attributed("100");
    let split = ledger.pro_rata_shares(&fill)?;
    assert_eq!(split.entitlement_total, dec!("300"));
    let users: Vec<&str> = split.shares.iter().map(|s| s.user.as_str()).collect();
    assert_eq!(
        users,
        ["alice", "bram", "cara"],
        "shares are in user id order"
    );
    assert_eq!(split.shares[1].amount, dec!("33.333333333"));
    assert_eq!(split.shares[2].amount, dec!("33.333333333"));
    assert_eq!(
        split.remainder,
        Decimal::from_raw(1),
        "the truncation left exactly one raw unit"
    );
    assert_eq!(
        split.remainder_to,
        user("alice"),
        "between equal entitlements the smaller id takes it"
    );
    assert_eq!(
        split.shares[0].amount,
        dec!("33.333333334"),
        "and the share it went to carries it"
    );
    assert_eq!(
        split.shares.iter().map(|s| s.amount).sum::<Decimal>(),
        fill.amount,
        "the shares sum to the fill to the last unit"
    );

    let booked = ledger.journal_pro_rata(&fill, now())?;
    assert_eq!(booked, split, "what was booked is what was computed");
    let settled: Decimal = ["alice", "bram", "cara"]
        .iter()
        .map(|name| {
            ledger
                .balance(&user(name), &strategy(), Currency::USD)
                .map_or(Decimal::ZERO, |cash| cash.settled())
        })
        .sum();
    assert_eq!(
        settled,
        dec!("300") + fill.amount,
        "the books hold the funding plus the whole fill"
    );

    // A loss splits the same way, with the remainder carrying the loss's
    // sign: truncation toward zero leaves each share a unit less negative,
    // and the largest holder takes the unit of loss. The largest holder is
    // now alice, who carries the extra unit from the gain above.
    let loss = attributed("-100");
    let split = ledger.pro_rata_shares(&loss)?;
    assert!(
        split.remainder.is_negative(),
        "the remainder of a loss is a loss: {}",
        split.remainder
    );
    assert_eq!(split.remainder_to, user("alice"));
    assert_eq!(
        split.shares.iter().map(|s| s.amount).sum::<Decimal>(),
        loss.amount
    );
    Ok(())
}

#[test]
fn a_pro_rata_split_follows_the_entitlements_and_the_largest_holder_takes_the_remainder()
-> Result<()> {
    // The failure: a split by head count rather than by capital, so a user
    // with a tenth of the strategy is booked a third of its gain. Premise:
    // the entitlements are unequal, and a user with a negative book — a
    // loss owed — holds no entitlement to the next gain.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    enrol(&mut ledger, "bram", "1000")?;
    enrol(&mut ledger, "cara", "1000")?;
    ledger.fund(&user("alice"), &strategy(), dec!("100"), now())?;
    ledger.fund(&user("bram"), &strategy(), dec!("900"), now())?;
    ledger.fund(&user("cara"), &strategy(), dec!("50"), now())?;
    ledger.journal_to(&user("cara"), &attributed("-80"), now())?;
    assert!(
        ledger
            .balance(&user("cara"), &strategy(), Currency::USD)
            .is_some_and(|cash| cash.settled().is_negative()),
        "the premise: cara's book at the strategy is negative"
    );

    let split = ledger.pro_rata_shares(&attributed("10"))?;
    assert_eq!(
        split.entitlement_total,
        dec!("1000"),
        "cara's book counts for nothing"
    );
    assert_eq!(split.shares.len(), 2, "and she takes no share");
    assert_eq!(split.shares[0].amount, dec!("1"));
    assert_eq!(split.shares[1].amount, dec!("9"));
    assert_eq!(
        split.remainder,
        Decimal::ZERO,
        "an exact split has no remainder"
    );
    assert_eq!(
        split.remainder_to,
        user("bram"),
        "and the remainder's destination is still the largest holder, on the record"
    );

    // Where the split is inexact, the unit goes to the largest holder, not
    // to the first: bram's id sorts after alice's.
    let split = ledger.pro_rata_shares(&attributed("1"))?;
    assert_eq!(split.shares[0].amount, dec!("0.1"));
    assert_eq!(split.shares[1].amount, dec!("0.9"));
    let split = ledger.pro_rata_shares(&attributed("0.000000007"))?;
    assert_eq!(
        split.remainder,
        Decimal::from_raw(1),
        "seven units over a tenth and nine tenths truncate to zero and six"
    );
    assert_eq!(split.remainder_to, user("bram"));
    assert_eq!(split.shares[1].amount, Decimal::from_raw(7));
    assert_eq!(split.shares[0].amount, Decimal::ZERO);
    Ok(())
}

#[test]
fn a_fill_with_no_capital_at_work_behind_it_is_not_split_and_nothing_is_booked() -> Result<()> {
    // The failure: a fill at a strategy nobody funded is split across an
    // empty list and "booked" to no one, and the attribution chain ends in
    // nothing without anyone noticing. Premise: the same fill books to the
    // desk explicitly.
    let mut ledger = ledger();
    enrol(&mut ledger, "alice", "1000")?;
    let fill = attributed("100");
    let mut explicit = ledger.clone();
    explicit.journal_to(ledger.desk(), &fill, now())?;
    assert_eq!(
        explicit.fills_journalled(),
        1,
        "the premise: the desk can be booked"
    );

    let refused = ledger
        .journal_pro_rata(&fill, now())
        .expect_err("no entitlement to split across");
    assert!(
        refused
            .message()
            .contains("no user has USD at work at momentum-v3"),
        "the refusal names the strategy and currency: {}",
        refused.message()
    );
    assert!(ledger.books().is_empty(), "nothing was booked");
    assert_eq!(ledger.fills_journalled(), 0);
    Ok(())
}
