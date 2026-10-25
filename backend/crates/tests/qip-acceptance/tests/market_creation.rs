//! Market creation's permission gate, across the seam no single crate holds
//! (EXEC-007).
//!
//! `qip-execution-engine` owns the register of recorded permissions and
//! `qip-brokers` owns the only venue a market could be listed at, the
//! in-process simulator. Each is internally consistent on its own; the
//! property that matters is between them: **a request the register refuses
//! leaves no trace at the venue**, and the same request reaches the venue
//! once the venue, the product rules and the jurisdiction have all been
//! recorded as permitting it.
//!
//! Since 2026-10-24 the venue demands the admission itself:
//! `SimulatedExchange::list` takes a `CreationAdmission` and refuses one for
//! another action, venue or product before anything is written
//! (R12-COVERAGE-5107). Until then this suite wrapped an ungated `list` in a
//! stand-in that asked the register first, so the gate sat beside the venue
//! and any other caller could list without it. `list_where_permitted` below
//! now only asks the register and hands the venue what it answered.
//!
//! Nothing here weakens the paper-trading boundary: the venue is
//! `AdapterClass::Simulated`, and the suite asserts it.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::read;
use qip_brokers::adapter::VenueAdapter;
use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_execution_engine::creation::{
    CreationAction, CreationAdmission, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;

const VENUE: &str = "XSIM";
const PRODUCT: &str = "OBJ0000000000000000000NEW1";
const JURISDICTION: &str = "paper";
const GATE: &str = "backend/crates/services/qip-execution-engine/src/creation.rs";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn product() -> ObjectId {
    ObjectId::from_string(PRODUCT)
}

/// The instrument somebody proposes to list. The liquidity is stated because
/// `LiquidityProfile` has no default: a fixture states its own premise.
fn proposed_instrument() -> FinancialObject {
    FinancialObject::builder(
        product(),
        "NEW1",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .name("A proposed listing")
    .venue(VENUE)
    .price(dec!("100"))
    .lot_size(Decimal::ONE)
    .tick_size(dec!("0.01"))
    .provenance(Provenance::synthetic(
        "qip-acceptance market_creation",
        start(),
    ))
    .build(start())
    .expect("a structurally valid instrument")
}

/// Ask the register, then hand the venue whatever it admitted. A refused
/// request returns before the venue is named at all.
fn list_where_permitted(
    permissions: &CreationPermissions,
    request: &CreationRequest,
    exchange: &mut SimulatedExchange,
) -> Result<()> {
    let admission = permissions.admit(request)?;
    exchange.list(proposed_instrument(), &admission)
}

/// A register holding all three permissions for `request`.
fn fully_permitted(request: &CreationRequest) -> Result<CreationPermissions> {
    let mut permissions = CreationPermissions::new();
    for authority in CreationAuthority::ALL {
        permissions.record(authority, request, "qip-acceptance market_creation")?;
    }
    Ok(permissions)
}

/// A genuine admission to `action` this product at this venue in this
/// jurisdiction.
fn admitted(action: CreationAction) -> Result<CreationAdmission> {
    let request = CreationRequest::new(action, VENUE, PRODUCT, JURISDICTION);
    fully_permitted(&request)?.admit(&request)
}

/// R12-COVERAGE-5107: the simulator's synthetic listing invents the
/// instrument — it writes the record a risk engine reads as its
/// definition — so it needs a `create` admission as well as a `list` one.
/// Until 2026-10-24 it took `list` alone, and the edge node's gateway
/// brought instruments into existence on demand under no permission to
/// create them. Every admission offered below is genuine; each case offers
/// something other than an admission to create exactly this instrument
/// here as the creation one, and the venue lists nothing.
///
/// Mutation: drop the `self.admits(CreationAction::Create, ..)` line from
/// `SimulatedExchange::list_synthetic` and the listing admission offered in
/// the creation slot invents the instrument.
#[test]
fn the_venue_invents_an_instrument_only_under_an_admission_to_create_it() -> Result<()> {
    let mut exchange =
        SimulatedExchange::new(VenueId::new(VENUE), ExchangeSettings::orderly(), 7, start());
    let listing = admitted(CreationAction::List)?;
    let other = CreationRequest::new(
        CreationAction::Create,
        VENUE,
        "OBJ0000000000000000000NEW2",
        JURISDICTION,
    );
    for (case, creation) in [
        (
            "an admission to list in the creation slot",
            admitted(CreationAction::List)?,
        ),
        (
            "an admission to seed in the creation slot",
            admitted(CreationAction::Seed)?,
        ),
        (
            "an admission to create another product",
            fully_permitted(&other)?.admit(&other)?,
        ),
    ] {
        let refusal = exchange
            .list_synthetic(
                product(),
                "NEW1",
                dec!("100"),
                Decimal::ONE,
                dec!("0.01"),
                LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
                start(),
                &creation,
                &listing,
            )
            .expect_err(&format!("{case}: {PRODUCT} was invented at {VENUE}"));
        assert!(
            refusal
                .message()
                .contains("needs an admission to create exactly this instrument"),
            "{case} was refused for another reason: {}",
            refusal.message()
        );
        assert!(
            !exchange.is_listed(&product()),
            "{case}: a listing was left behind"
        );
    }
    // Contrast: the admission to create it, beside the one to list it,
    // invents and lists it.
    exchange.list_synthetic(
        product(),
        "NEW1",
        dec!("100"),
        Decimal::ONE,
        dec!("0.01"),
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        start(),
        &admitted(CreationAction::Create)?,
        &listing,
    )?;
    assert!(exchange.is_listed(&product()));
    Ok(())
}

#[test]
fn the_venue_refuses_an_admission_for_another_action_venue_or_product_and_lists_nothing()
-> Result<()> {
    // R12-COVERAGE-5107: until 2026-10-24 `SimulatedExchange::list` took any
    // instrument from any caller, and the register's admission was checked
    // only by this suite's wrapper, beside the venue rather than on it. Each
    // admission below is genuine, minted by a register holding all three
    // permissions, and each is for something other than listing this
    // instrument here, so only the venue's own check can refuse it.
    //
    // Mutation: delete the action, venue or product comparison in
    // `SimulatedExchange::list`, and the matching case below lists the
    // instrument.
    let mut exchange =
        SimulatedExchange::new(VenueId::new(VENUE), ExchangeSettings::orderly(), 7, start());
    for (case, request) in [
        (
            "an admission to seed",
            CreationRequest::new(CreationAction::Seed, VENUE, PRODUCT, JURISDICTION),
        ),
        (
            "an admission for another venue",
            CreationRequest::new(CreationAction::List, "XOTHER", PRODUCT, JURISDICTION),
        ),
        (
            "an admission for another product",
            CreationRequest::new(
                CreationAction::List,
                VENUE,
                "OBJ0000000000000000000NEW2",
                JURISDICTION,
            ),
        ),
    ] {
        // Premise: the admission is real, so a refusal below is the venue's.
        let admission = fully_permitted(&request)?.admit(&request)?;
        let refusal = exchange
            .list(proposed_instrument(), &admission)
            .expect_err(&format!("{case} listed {PRODUCT} at {VENUE}"));
        assert!(
            refusal
                .message()
                .contains("needs an admission to list exactly this instrument"),
            "{case} was refused for another reason: {}",
            refusal.message()
        );
        assert!(
            !exchange.is_listed(&product()),
            "{case} was refused and still left a listing at the venue"
        );
        // The simulator's own listing path, which the edge node's gateway
        // uses to list on demand, makes the same check — handed a genuine
        // admission to create this instrument here beside the one under
        // test, so the refusal is the listing check and not the creation one.
        let creation = admitted(CreationAction::Create)?;
        let refusal = exchange
            .list_synthetic(
                product(),
                "NEW1",
                dec!("100"),
                Decimal::ONE,
                dec!("0.01"),
                LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
                start(),
                &creation,
                &admission,
            )
            .expect_err(&format!("{case} listed {PRODUCT} synthetically at {VENUE}"));
        assert!(
            refusal
                .message()
                .contains("needs an admission to list exactly this instrument"),
            "{case} was refused synthetically for another reason: {}",
            refusal.message()
        );
        assert!(
            !exchange.is_listed(&product()),
            "{case} was refused synthetically and still left a listing at the venue"
        );
    }

    // And the admission it does need lists the instrument, so the refusals
    // above are not a venue that refuses everything.
    let request = CreationRequest::new(CreationAction::List, VENUE, PRODUCT, JURISDICTION);
    exchange.list(
        proposed_instrument(),
        &fully_permitted(&request)?.admit(&request)?,
    )?;
    assert!(
        exchange.is_listed(&product()),
        "an admission to list this instrument here listed nothing"
    );
    Ok(())
}

#[test]
fn a_market_is_listed_at_a_simulated_venue_only_once_the_venue_the_product_rules_and_the_jurisdiction_all_permit_it()
-> Result<()> {
    // The failure this prevents: a market brought into existence because it
    // was technically possible. Each of the three permissions is withheld in
    // turn below, so none of them is the one the gate forgot to ask for.
    let mut exchange =
        SimulatedExchange::new(VenueId::new(VENUE), ExchangeSettings::orderly(), 7, start());
    assert!(
        exchange.class().is_paper(),
        "the premise failed: the venue under test is not a paper venue"
    );
    let request = CreationRequest::new(CreationAction::List, VENUE, PRODUCT, JURISDICTION);
    let mut permissions = CreationPermissions::new();

    // With nothing recorded: refused, and the venue holds no record of it.
    let mut still_missing = CreationAuthority::ALL.to_vec();
    for (authority, reference) in [
        (CreationAuthority::Venue, "XSIM rulebook 4.2"),
        (CreationAuthority::Product, "product approval PA-9"),
        (CreationAuthority::Jurisdiction, "legal opinion 2026-03"),
    ] {
        let refusal = list_where_permitted(&permissions, &request, &mut exchange)
            .expect_err("a market was listed without all three recorded permissions");
        let names: Vec<&str> = still_missing.iter().map(|a| a.as_str()).collect();
        // The delimited list of what is missing: the sentence around it names
        // all three authorities whatever is outstanding, so a bare
        // `contains("venue")` would pass on any refusal at all.
        assert!(
            refusal
                .message()
                .contains(&format!("missing ({})", names.join(", "))),
            "with {names:?} outstanding the refusal lists something else: {}",
            refusal.message()
        );
        assert!(
            !exchange.is_listed(&product()),
            "a refused creation left a market record at the venue"
        );
        assert_eq!(
            exchange.submitted_count(),
            0,
            "a refused creation reached the venue's order entry"
        );
        permissions.record(authority, &request, reference)?;
        still_missing.retain(|outstanding| *outstanding != authority);
    }

    // All three recorded: the same request, admitted against the venue.
    assert!(
        permissions.missing(&request).is_empty(),
        "the premise failed: a permission is still outstanding"
    );
    list_where_permitted(&permissions, &request, &mut exchange)?;
    assert!(
        exchange.is_listed(&product()),
        "the fully permitted request was admitted and nothing was listed"
    );

    // And the permission is for listing this product here, not for seeding
    // it: the neighbouring action is still refused.
    let seed = CreationRequest::new(CreationAction::Seed, VENUE, PRODUCT, JURISDICTION);
    assert!(
        permissions.admit(&seed).is_err(),
        "permission to list was read as permission to seed"
    );
    Ok(())
}

/// The text of `impl CreationAdmission { .. }`, from its opening line to the
/// first line that closes an item at column zero.
fn admission_impl(source: &str) -> &str {
    let at = source
        .find("\nimpl CreationAdmission {")
        .expect("the admission still has an inherent impl in this file");
    let rest = &source[at + 1..];
    let end = rest.find("\n}\n").expect("the impl block closes");
    &rest[..end]
}

#[test]
fn a_creation_admission_has_one_mint_and_cannot_be_built_or_decoded_anywhere_else() {
    // The gate is worth nothing if the token it mints can be had another way:
    // decoded out of a payload, defaulted into existence, built field by
    // field, or returned by a second constructor.
    let source = read(GATE);
    let at = source
        .find("\npub struct CreationAdmission {")
        .expect("the admission type is still declared in this file");
    let derive_line = source[..at]
        .lines()
        .next_back()
        .expect("the declaration has a line above it");
    // The premise: the line found really is the derive attribute. Without
    // this the assertions below pass on any line lacking the two words.
    assert!(
        derive_line.starts_with("#[derive(") && derive_line.contains("Debug"),
        "the line above the admission declaration is not its derive attribute: {derive_line}"
    );
    for forbidden in ["Deserialize", "Default"] {
        assert!(
            !derive_line.contains(forbidden),
            "`CreationAdmission` derives `{forbidden}`, so one exists without the register \
             admitting anything: {derive_line}"
        );
    }

    // Private fields: nothing outside the module can write the struct out.
    let body = source[at + 1..]
        .split("\n}\n")
        .next()
        .expect("the declaration has a body");
    assert!(
        body.lines().count() > 2,
        "the premise failed: the admission's declaration was not found whole: {body}"
    );
    let public_fields: Vec<&str> = body
        .lines()
        .skip(1)
        .filter(|line| line.trim_start().starts_with("pub "))
        .collect();
    assert!(
        public_fields.is_empty(),
        "`CreationAdmission` has public fields, so any caller can build one: {public_fields:?}"
    );

    // One mint inside the module: the literal in `admit`, and no constructor
    // on the type itself.
    let block = admission_impl(&source);
    assert!(
        block.contains("pub fn request(&self)"),
        "the premise failed: the admission's impl block was not found: {block}"
    );
    assert!(
        !block.contains("Self {") && !block.contains("-> Self"),
        "`CreationAdmission` has a constructor of its own beside `CreationPermissions::admit`"
    );
    let production = source
        .split("\n#[cfg(test)]")
        .next()
        .expect("the file has production text");
    let literals = production
        .lines()
        .filter(|line| line.trim_start().starts_with("Ok(CreationAdmission {"))
        .count();
    assert_eq!(
        literals, 1,
        "the premise failed: `admit` no longer mints the admission where this test looks"
    );
    // The declaration, the impl block and the one literal in `admit`. The
    // fields are private, so no other module can write the struct out at
    // all; this is the count inside the one module that can.
    assert_eq!(
        production.matches("CreationAdmission {").count(),
        3,
        "the admission is written out somewhere in its module beside `admit`"
    );
}
