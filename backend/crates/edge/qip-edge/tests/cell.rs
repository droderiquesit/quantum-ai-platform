//! What an assembled cell must be true of.
//!
//! A cell decides without asking the central plane, so every safety property
//! it has is one it enforces locally. These tests are that enforcement checked
//! from outside: each asserts that the *unsafe* thing is refused, not merely
//! that the safe path works.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::{Decision, Journal, MemoryMirror};
use qip_edge::seam::{CellLiquidity, value_kind, value_type};
use qip_feature_dag::definition::ValueKind;
use qip_orderbook::venue::VenueState;
use qip_strategy::ir::Type;

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

/// An envelope signed the way the central allocator would sign it.
fn signed_envelope(cell: &str, gross: &str, order: &str, key: &[u8]) -> Result<CapitalEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        cell,
        Decimal::parse(gross).expect("a decimal literal"),
        Decimal::parse(order).expect("a decimal literal"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3600),
        "alice@example.com",
        "unsigned",
    )?;
    let signature = sign_payload(key, &unsigned.signing_payload());
    CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        cell,
        Decimal::parse(gross).expect("a decimal literal"),
        Decimal::parse(order).expect("a decimal literal"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3600),
        "alice@example.com",
        signature,
    )
}

// --- the envelope seam ------------------------------------------------------

#[test]
fn an_unsigned_envelope_cannot_become_a_verified_one() -> Result<()> {
    // `CapitalEnvelope::new` is public, so a well-typed grant nobody approved
    // can be built anywhere. This is the check that stops one being *used*:
    // construction is not the control, verification is.
    let unapproved = CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        CELL,
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3600),
        "attacker@example.com",
        "not-a-real-signature",
    )?;
    let refusal = VerifiedEnvelope::verify(unapproved, KEY, CELL, t(10)).unwrap_err();
    assert!(
        refusal.message().contains("does not verify"),
        "{}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn tampering_with_any_bound_invalidates_the_signature() -> Result<()> {
    // The signature covers every field that bounds what the cell may do. A
    // limit the signature did not cover is the limit that would be edited.
    let genuine = signed_envelope(CELL, "1000000", "100000", KEY)?;
    VerifiedEnvelope::verify(genuine.clone(), KEY, CELL, t(10))?;

    // Re-issue with a wider gross limit but the original signature.
    let widened = CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        CELL,
        dec!("9000000"),
        dec!("100000"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        t(0),
        t(3600),
        "alice@example.com",
        genuine.signature(),
    )?;
    assert!(VerifiedEnvelope::verify(widened, KEY, CELL, t(10)).is_err());
    Ok(())
}

#[test]
fn a_correctly_signed_envelope_for_another_cell_is_refused() -> Result<()> {
    // A signature alone does not stop replay: the grant is genuine, it is
    // simply not this cell's. Without the cell check, one compromised cell
    // could spend every other cell's capital.
    let elsewhere = signed_envelope("tokyo-1", "1000000", "100000", KEY)?;
    let refusal = VerifiedEnvelope::verify(elsewhere, KEY, CELL, t(10)).unwrap_err();
    assert!(
        refusal.message().contains("tokyo-1"),
        "{}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_cell_with_no_key_refuses_to_verify_anything() -> Result<()> {
    // The failure mode this guards: a missing secret mount silently becoming
    // an empty key, and an empty key verifying whatever it is handed.
    let genuine = signed_envelope(CELL, "1000000", "100000", KEY)?;
    assert!(VerifiedEnvelope::verify(genuine, b"", CELL, t(10)).is_err());
    Ok(())
}

#[test]
fn an_expired_envelope_stops_the_cell_rather_than_letting_it_continue() -> Result<()> {
    // Expiry is the backstop that bounds a cell which has lost contact with
    // the centre. A cell that ignored it would trade indefinitely on a grant
    // nobody could withdraw.
    let genuine = signed_envelope(CELL, "1000000", "100000", KEY)?;
    assert!(VerifiedEnvelope::verify(genuine.clone(), KEY, CELL, t(4000)).is_err());

    let verified = VerifiedEnvelope::verify(genuine, KEY, CELL, t(10))?;
    assert!(verified.is_live(t(10)));
    assert!(!verified.is_live(t(3600)), "the grant outlived its window");
    Ok(())
}

#[test]
fn a_verified_envelope_admits_then_reduces_then_refuses_as_capital_is_used() -> Result<()> {
    // The three answers in the order a cell meets them, so a `Reduced` cannot
    // be mistaken for approval of what was asked for.
    let verified =
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400", KEY)?, KEY, CELL, t(10))?;
    let venue = VenueId::new("XLON");
    let fresh = Utilisation::default();
    assert!(matches!(
        verified.admit(&venue, dec!("300"), &fresh, t(10)),
        CapitalGrant::Full
    ));

    let mostly_used = Utilisation {
        gross_committed: dec!("800"),
        realised_loss: Decimal::ZERO,
        orders_sent: 2,
    };
    match verified.admit(&venue, dec!("400"), &mostly_used, t(10)) {
        CapitalGrant::Reduced(size) => assert_eq!(size, dec!("200")),
        other => panic!("expected a reduction, got {other:?}"),
    }

    let exhausted = Utilisation {
        gross_committed: dec!("1000"),
        realised_loss: Decimal::ZERO,
        orders_sent: 3,
    };
    assert!(
        verified
            .admit(&venue, dec!("10"), &exhausted, t(10))
            .is_refused()
    );

    // And a venue outside the grant is refused however much headroom remains.
    assert!(
        verified
            .admit(&VenueId::new("XNYS"), dec!("10"), &fresh, t(10))
            .is_refused()
    );
    Ok(())
}

// --- the type seam ----------------------------------------------------------

#[test]
fn the_value_kind_mapping_is_total_and_round_trips() {
    // `qip-strategy` cannot see `qip-feature-dag`, so each has its own tag and
    // something must map them. Exhaustive matches mean a new variant on either
    // side breaks the build here rather than defaulting silently to a type the
    // compiler will then happily check against the wrong thing.
    for kind in [
        ValueKind::Exact,
        ValueKind::Statistic,
        ValueKind::Count,
        ValueKind::Flag,
    ] {
        assert_eq!(value_kind(value_type(kind)), kind);
    }
    for declared in [Type::Exact, Type::Statistic, Type::Count, Type::Flag] {
        assert_eq!(value_type(value_kind(declared)), declared);
    }
    assert_eq!(value_type(ValueKind::Exact), Type::Exact);
    assert_eq!(value_type(ValueKind::Statistic), Type::Statistic);
}

// --- the liquidity seam -----------------------------------------------------

fn book_with_depth(venue: &str, symbol: &str) -> VenueState {
    use qip_contracts::message::{MarketMessage, MessageBody};
    use qip_contracts::venue::Origin;

    let venue_id = VenueId::new(venue);
    let mut state = VenueState::aggregated(object(symbol), venue_id.clone(), VenueStatus::Open);
    let levels = [
        (BookSide::Bid, "99", "500"),
        (BookSide::Bid, "98", "800"),
        (BookSide::Ask, "101", "400"),
        (BookSide::Ask, "102", "900"),
    ];
    for (index, (side, price, size)) in levels.iter().enumerate() {
        let message = MarketMessage::new(
            object(symbol),
            Origin::new(venue_id.clone(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side: *side,
                price: Decimal::parse(price).expect("a price"),
                quantity: Decimal::parse(size).expect("a size"),
                order_count: None,
            },
            t(index as i64),
            t(index as i64),
        );
        state.apply(&message).expect("a well-formed level");
    }
    state
}

#[test]
fn the_real_book_never_offers_more_liquidity_than_it_holds() -> Result<()> {
    // The seam `qip-arbitrage` left open on purpose. Its slippage deduction is
    // computed from this call, so an overstatement here is a trade that looked
    // profitable and was not.
    use qip_arbitrage::liquidity::LiquiditySource;

    let mut liquidity = CellLiquidity::new();
    liquidity.insert(book_with_depth("XLON", "ACME"));
    let venue = VenueId::new("XLON");
    let acme = object("ACME");

    let (_, available) = liquidity
        .sweep_cost(&venue, &acme, BookSide::Ask, dec!("10000"))
        .expect("the book has some depth");
    assert_eq!(
        available,
        dec!("1300"),
        "the sweep claimed more than the two ask levels hold"
    );

    let (price, filled) = liquidity
        .sweep_cost(&venue, &acme, BookSide::Ask, dec!("400"))
        .expect("the touch alone covers this");
    assert_eq!(filled, dec!("400"));
    assert_eq!(
        price,
        dec!("101"),
        "a fill inside the touch paid more than the touch"
    );
    Ok(())
}

#[test]
fn a_stale_book_supplies_no_depth_at_all() -> Result<()> {
    // After a gap the book is wrong, and depth from before the gap is the most
    // dangerous kind: it looks like a fact. `None` here means "this source is
    // ignorant", which the path pricer already distinguishes from a thin book.
    use qip_arbitrage::liquidity::LiquiditySource;

    let mut liquidity = CellLiquidity::new();
    liquidity.insert(book_with_depth("XLON", "ACME"));
    let venue = VenueId::new("XLON");
    let acme = object("ACME");
    assert!(
        liquidity
            .sweep_cost(&venue, &acme, BookSide::Ask, dec!("100"))
            .is_some()
    );

    liquidity
        .get_mut(&venue, &acme)
        .expect("the book is tracked")
        .reset("a sequence gap was abandoned");

    assert!(
        liquidity
            .sweep_cost(&venue, &acme, BookSide::Ask, dec!("100"))
            .is_none()
    );
    assert!(liquidity.touch(&venue, &acme, BookSide::Ask).is_none());
    assert!(liquidity.mid(&venue, &acme).is_none());
    assert!(liquidity.as_of(&venue, &acme).is_none());
    assert_eq!(liquidity.observations(&venue, &acme), 0);
    Ok(())
}

#[test]
fn an_unreachable_venue_supplies_no_depth_even_with_a_full_book() -> Result<()> {
    // Distinct from halted: the venue may well be trading and this cell simply
    // cannot see it, which is the more dangerous of the two.
    use qip_arbitrage::liquidity::LiquiditySource;

    use qip_contracts::message::{MarketMessage, MessageBody};
    use qip_contracts::venue::Origin;

    let mut state = book_with_depth("XLON", "ACME");
    // Status arrives as a message like everything else the book learns; there
    // is deliberately no setter that bypasses the feed.
    state.apply(&MarketMessage::new(
        object("ACME"),
        Origin::new(VenueId::new("XLON"), "feed-a", 0, 99),
        MessageBody::StatusChange {
            status: VenueStatus::Unreachable,
        },
        t(50),
        t(50),
    ))?;
    let mut liquidity = CellLiquidity::new();
    liquidity.insert(state);

    assert!(
        liquidity
            .sweep_cost(
                &VenueId::new("XLON"),
                &object("ACME"),
                BookSide::Ask,
                dec!("100")
            )
            .is_none()
    );
    Ok(())
}

// --- the journal and the mirror ---------------------------------------------

#[test]
fn the_journal_chain_catches_an_edited_decision() -> Result<()> {
    // The chain is what lets the centre detect a cell that dropped or edited
    // entries, whatever the sequence numbers claim.
    let mut journal = Journal::new();
    journal.record(
        Decision::Ingested {
            feed: "XLON/feed-a".to_string(),
            decoded: 12,
            skipped: 0,
        },
        t(1),
    );
    journal.record(
        Decision::OrderSent {
            order_id: "london-1-1".to_string(),
            venue: "XLON".to_string(),
            quantity: "100".to_string(),
            simulated: true,
        },
        t(2),
    );
    journal.verify().expect("an untouched chain verifies");

    let serialized = serde_json::to_string(&journal).expect("a journal serializes");
    let edited = serialized.replace("\"quantity\":\"100\"", "\"quantity\":\"9000\"");
    let tampered: Journal = serde_json::from_str(&edited).expect("still valid json");
    assert_eq!(
        tampered.verify(),
        Err(1),
        "an edited decision went unnoticed"
    );
    Ok(())
}

#[test]
fn mirror_batches_chain_onto_each_other_across_flushes() -> Result<()> {
    // A batch whose first entry does not chain onto the last one received is a
    // gap in the cell's record — the centre's only way to notice that a cell
    // stopped telling it things.
    let mut journal = Journal::new();
    let mut mirror = MemoryMirror::new();

    for round in 0..3 {
        for index in 0..4 {
            journal.record(
                Decision::Refused {
                    gate: "capital".to_string(),
                    reason: format!("round {round} item {index}"),
                },
                t(round * 10 + index),
            );
        }
        let shipped = qip_edge::journal::ship(
            &mut journal,
            &mut mirror,
            CELL,
            vec![("XLON/feed-a/0".to_string(), (round * 4 + 4) as u64)],
            t(round * 10 + 9),
        )?;
        assert_eq!(shipped, 4);
    }

    assert_eq!(mirror.batches().len(), 3);
    mirror.verify_continuity()?;
    assert_eq!(mirror.batches()[0].chains_onto, Journal::GENESIS);
    assert_eq!(
        mirror.batches()[1].chains_onto,
        mirror.batches()[0].tail_digest(),
        "the second batch did not chain onto the first"
    );

    // A batch presented out of order is refused.
    assert!(
        mirror.batches()[2]
            .verify_against(&mirror.batches()[0].tail_digest())
            .is_err()
    );
    Ok(())
}

#[test]
fn a_flush_with_nothing_pending_ships_no_batch() -> Result<()> {
    // An empty batch would look identical to a cell that had genuinely
    // decided nothing, and would make the chain harder to read for no gain.
    let mut journal = Journal::new();
    let mut mirror = MemoryMirror::new();
    assert_eq!(
        qip_edge::journal::ship(&mut journal, &mut mirror, CELL, Vec::new(), t(1))?,
        0
    );
    assert!(mirror.batches().is_empty());
    Ok(())
}

// --- drop copy --------------------------------------------------------------

#[test]
fn reconciliation_looks_in_both_directions() -> Result<()> {
    // A cell that only checks its own fills against the venue misses the case
    // that matters most: a fill the cell never knew about, which is an
    // unhedged position nobody is watching.
    let mut reconciler = DropCopyReconciler::new();
    let venue = VenueId::new("XLON");

    reconciler.observe(DropCopyFill {
        order_id: "known-to-both".to_string(),
        venue: venue.clone(),
        quantity: dec!("100"),
        price: dec!("50"),
        at: t(1),
    });
    reconciler.observe(DropCopyFill {
        order_id: "venue-only".to_string(),
        venue: venue.clone(),
        quantity: dec!("300"),
        price: dec!("50"),
        at: t(2),
    });
    reconciler.observe(DropCopyFill {
        order_id: "size-differs".to_string(),
        venue: venue.clone(),
        quantity: dec!("500"),
        price: dec!("50"),
        at: t(3),
    });

    let cell_fills = vec![
        CellFill {
            order_id: "known-to-both".to_string(),
            venue: venue.clone(),
            quantity: dec!("100"),
            price: dec!("50"),
        },
        CellFill {
            order_id: "size-differs".to_string(),
            venue: venue.clone(),
            quantity: dec!("400"),
            price: dec!("50"),
        },
        CellFill {
            order_id: "cell-only".to_string(),
            venue,
            quantity: dec!("200"),
            price: dec!("50"),
        },
    ];

    let breaks = reconciler.reconcile(&cell_fills);
    assert_eq!(breaks.len(), 3, "{breaks:?}");
    assert!(
        breaks
            .iter()
            .any(|b| matches!(b, Discrepancy::UnknownToCell { .. }))
    );
    assert!(
        breaks
            .iter()
            .any(|b| matches!(b, Discrepancy::UnknownToVenue { .. }))
    );
    assert!(
        breaks
            .iter()
            .any(|b| matches!(b, Discrepancy::QuantityDiffers { .. }))
    );
    for discrepancy in &breaks {
        assert!(!discrepancy.describe().is_empty());
    }
    Ok(())
}

#[test]
fn a_redelivered_drop_copy_fill_is_not_a_doubled_position() -> Result<()> {
    // Repeated delivery is the norm on a drop copy. Accumulating instead of
    // replacing would make one fill look exactly like two.
    let mut reconciler = DropCopyReconciler::new();
    let venue = VenueId::new("XLON");
    let fill = DropCopyFill {
        order_id: "repeated".to_string(),
        venue: venue.clone(),
        quantity: dec!("100"),
        price: dec!("50"),
        at: t(1),
    };
    reconciler.observe(fill.clone());
    reconciler.observe(fill);
    assert_eq!(reconciler.observed(), 1);

    let breaks = reconciler.reconcile(&[CellFill {
        order_id: "repeated".to_string(),
        venue,
        quantity: dec!("100"),
        price: dec!("50"),
    }]);
    assert!(breaks.is_empty(), "{breaks:?}");
    Ok(())
}

#[test]
fn agreement_produces_no_discrepancy_at_all() -> Result<()> {
    // A signal that fires on a normal day is one nobody acts on when it
    // matters, so the negative case is tested as deliberately as the positive.
    let mut reconciler = DropCopyReconciler::new();
    let venue = VenueId::new("XLON");
    for index in 0..5 {
        reconciler.observe(DropCopyFill {
            order_id: format!("order-{index}"),
            venue: venue.clone(),
            quantity: dec!("100"),
            price: dec!("50"),
            at: t(index),
        });
    }
    let cell_fills: Vec<CellFill> = (0..5)
        .map(|index| CellFill {
            order_id: format!("order-{index}"),
            venue: venue.clone(),
            quantity: dec!("100"),
            price: dec!("50"),
        })
        .collect();
    assert!(reconciler.reconcile(&cell_fills).is_empty());
    assert_eq!(reconciler.checked(), 1);
    Ok(())
}

// --- the cell's own configuration -------------------------------------------

#[test]
fn a_cell_is_assembled_with_a_paper_ceiling_and_no_way_to_raise_it() -> Result<()> {
    // There is no constructor taking another ceiling. A live-capable cell is a
    // differently-assembled deployment the central plane signs off, and the
    // absence of the constructor is what makes that true rather than intended.
    use qip_edge::cell::{Cell, CellConfig};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;

    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let cell = Cell::new(config, engine)?;

    assert!(
        !cell.autonomy().ceiling().is_live(),
        "a cell was assembled live-capable"
    );
    assert!(!cell.is_halted());
    assert_eq!(cell.config().cell_id, CELL);
    assert!(cell.journal().is_empty());
    Ok(())
}

#[test]
fn a_strategy_cannot_be_deployed_under_another_cells_grant() -> Result<()> {
    // Belt and braces with the verification check: even a verified envelope
    // must match the cell it is being deployed into.
    use qip_edge::cell::{Cell, CellConfig};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;

    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, engine)?;

    let elsewhere = VerifiedEnvelope::verify(
        signed_envelope("tokyo-1", "1000", "400", KEY)?,
        KEY,
        "tokyo-1",
        t(10),
    )?;
    let (strategy, program) = trivial_strategy()?;
    assert!(
        cell.deploy(strategy, program, elsewhere).is_err(),
        "a grant for another cell deployed here"
    );
    Ok(())
}

#[test]
fn a_plan_and_a_program_that_do_not_belong_together_are_refused_at_deployment() -> Result<()> {
    // The failure this closes is not a crash. `NodeRef` is an index, so a plan
    // evaluated against a foreign arena of the same size reads real nodes
    // belonging to a different strategy and emits a signal computed from
    // arithmetic nobody wrote for it. Deployment is where the mismatch is
    // detectable without the market, so deployment is where it is refused.
    use qip_edge::cell::{Cell, CellConfig};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;
    use qip_strategy::program::Program;

    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, engine)?;

    let grant =
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400", KEY)?, KEY, CELL, t(10))?;
    let (strategy, program) = trivial_strategy()?;
    assert!(
        !strategy.plan().is_empty(),
        "the fixture must plan at least one node, or the check has nothing to catch"
    );

    // The empty arena a cell used to be constructed with.
    let error = cell
        .deploy(strategy.clone(), Program::default(), grant)
        .expect_err("a strategy deployed against an arena that does not hold its plan");
    assert!(
        error.message().contains("do not belong together"),
        "the refusal did not name the mismatch: {error}"
    );
    assert!(
        cell.deployed_strategies().is_empty(),
        "a refused deployment was still recorded"
    );

    // The same strategy with its own program is accepted, so the test is
    // about the mismatch rather than about the strategy.
    let grant =
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400", KEY)?, KEY, CELL, t(10))?;
    cell.deploy(strategy, program, grant)?;
    assert_eq!(cell.deployed_strategies(), vec!["mean-reversion-1"]);
    Ok(())
}

/// A strategy that compiles and never fires.
///
/// The deployment gate under test is about *whose* capital it runs on, not
/// about what it computes, so the cheapest well-typed program is the honest
/// fixture here.
fn trivial_strategy() -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    use qip_contracts::signal::SignalKind;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new("mean-reversion-1"),
        object("ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "never",
        SignalKind::Enter,
        Expr::Flag(false),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

// --- verified policy: the only route into a cell's policy state --------------

use qip_contracts::policy::PolicyPayload;
use qip_edge::VerifiedPolicy;

#[test]
fn a_policy_payload_verifies_only_with_the_right_key_cell_and_bytes() -> Result<()> {
    // The same three refusals `VerifiedEnvelope` earns, for the other thing
    // the centre ships. Arriving well-typed proves nothing: the constructor
    // recomputes the MAC, matches the address, and there is no other way in.
    let key = b"cell-policy-test-key";
    let signed = PolicyPayload::unproduced(1, "cell-a", t(0)).signed(key)?;

    // Premise: the genuine article verifies.
    assert!(VerifiedPolicy::verify(signed.clone(), key, "cell-a", t(1)).is_ok());

    // A different key refuses.
    assert!(
        VerifiedPolicy::verify(signed.clone(), b"another-key", "cell-a", t(1)).is_err(),
        "a payload verified against a key that did not sign it"
    );
    // An empty key refuses rather than verifying nothing.
    assert!(VerifiedPolicy::verify(signed.clone(), b"", "cell-a", t(1)).is_err());

    // The right key, the wrong cell: a genuine signature is not an address.
    assert!(
        VerifiedPolicy::verify(signed.clone(), key, "cell-b", t(1)).is_err(),
        "a payload for cell-a was accepted by cell-b"
    );

    // Tampered content: flip the halt flag after signing. The signature covers
    // it, so the edited payload must refuse — this is the un-halt forgery.
    let mut tampered = signed;
    tampered.halted = true;
    assert!(
        VerifiedPolicy::verify(tampered, key, "cell-a", t(1)).is_err(),
        "a payload edited after signing still verified, so anyone on the \
         path can halt or un-halt a cell"
    );
    Ok(())
}
