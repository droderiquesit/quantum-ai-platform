//! Pool pricing, unit conversion, gas, the mempool and bridges.
//!
//! The pricing properties are checked against the curve's own invariant rather
//! than against recorded outputs: a golden number proves the code still does
//! what it did, while the invariant proves it does what the pool does.

use qip_chain::amm::{constant_product_holds, FeeBps, FeeSide, Pool, PoolCurve, PoolId, PoolInvariant};
use qip_chain::block::{Address, Block, BlockNumber, ChainId, Hash32, BlockHash, TxHash};
use qip_chain::bridge::{
    BridgeFailure, BridgeLedger, BridgeRoute, BridgeTransfer, TransferId, TransferStatus,
};
use qip_chain::finality::{Confirmations, Finality};
use qip_chain::gas::{effective_gas_price, GasCost, GasProfile};
use qip_chain::mempool::{Mempool, PendingTransaction};
use qip_chain::units::TokenAmount;
use qip_contracts::{BookSide, DeductionKind, VenueClass, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Currency, Decimal, Duration, ObjectId, Timestamp};

fn pool(curve: PoolCurve, base: Decimal, quote: Decimal, fee_bps: u32) -> Pool {
    Pool::new(
        PoolId::new("pool"),
        VenueId::new("TEST-DEX"),
        ObjectId::from_string("BASE"),
        ObjectId::from_string("QUOTE"),
        curve,
        FeeBps::new(fee_bps).expect("a valid fee"),
        base,
        quote,
        BlockNumber::new(1),
    )
    .expect("a pool with positive reserves")
}

fn constant_product(base: i64, quote: i64) -> Pool {
    pool(
        PoolCurve::ConstantProduct,
        Decimal::from_int(base),
        Decimal::from_int(quote),
        30,
    )
}

// --- constant product -------------------------------------------------------

#[test]
fn a_constant_product_quote_satisfies_the_curve_invariant_exactly() {
    Property::new("constant product output is maximal against the invariant")
        .cases(400)
        .for_all(
            |rng: &mut Xoshiro256| {
                let base = Decimal::from_int(1 + rng.below(500_000) as i64);
                let quote = Decimal::from_int(1 + rng.below(500_000) as i64);
                let side = if rng.bernoulli(0.5) {
                    BookSide::Ask
                } else {
                    BookSide::Bid
                };
                let fraction = rng.uniform(0.0001, 0.5);
                (base, quote, side, fraction)
            },
            |(base, quote, side, fraction)| {
                let pool = pool(PoolCurve::ConstantProduct, *base, *quote, 30);
                let reserve_in = match side {
                    BookSide::Ask => *quote,
                    BookSide::Bid => *base,
                };
                let reserve_out = match side {
                    BookSide::Ask => *base,
                    BookSide::Bid => *quote,
                };
                let factor = Decimal::from_f64(*fraction).ok_or("unrepresentable fraction")?;
                let amount_in = reserve_in * factor;
                if !amount_in.is_positive() {
                    return Ok(());
                }
                let quoted = match pool.quote_exact_in(*side, amount_in) {
                    Ok(quoted) => quoted,
                    // A size that buys nothing at these reserves is refused
                    // rather than rounded to zero, which is the correct answer.
                    Err(_) => return Ok(()),
                };
                if quoted.fee_side != FeeSide::Input {
                    return Err("constant product must charge the fee on the input".to_string());
                }
                let reached_curve = quoted.amount_in - quoted.fee;
                let holds = constant_product_holds(
                    reserve_in,
                    reserve_out,
                    reached_curve,
                    quoted.amount_out,
                )
                .map_err(|e| e.to_string())?;
                if !holds {
                    return Err(format!(
                        "the invariant fell: in {} out {}",
                        quoted.amount_in, quoted.amount_out
                    ));
                }
                // One more unit out than the pool can pay must break it, or
                // the quote was leaving value on the table.
                let greedier = constant_product_holds(
                    reserve_in,
                    reserve_out,
                    reached_curve,
                    quoted.amount_out + Decimal::from_raw(1),
                )
                .map_err(|e| e.to_string())?;
                if greedier {
                    return Err(format!(
                        "the pool could have paid more than {} out",
                        quoted.amount_out
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn the_input_required_for_an_exact_output_round_trips_without_underpaying() {
    Property::new("exact-out inverts exact-in")
        .cases(300)
        .for_all(
            |rng: &mut Xoshiro256| {
                let base = Decimal::from_int(100 + rng.below(200_000) as i64);
                let quote = Decimal::from_int(100 + rng.below(200_000) as i64);
                let side = if rng.bernoulli(0.5) {
                    BookSide::Ask
                } else {
                    BookSide::Bid
                };
                let fraction = rng.uniform(0.0001, 0.2);
                (base, quote, side, fraction)
            },
            |(base, quote, side, fraction)| {
                for curve in [
                    PoolCurve::ConstantProduct,
                    PoolCurve::StableSwap { amplification: 85 },
                ] {
                    let pool = pool(curve, *base, *quote, 30);
                    let reserve_out = match side {
                        BookSide::Ask => *base,
                        BookSide::Bid => *quote,
                    };
                    let factor = Decimal::from_f64(*fraction).ok_or("unrepresentable fraction")?;
                    let wanted = reserve_out * factor;
                    if !wanted.is_positive() {
                        continue;
                    }
                    let Ok(inverse) = pool.quote_exact_out(*side, wanted) else {
                        continue;
                    };
                    let Ok(forward) = pool.quote_exact_in(*side, inverse.amount_in) else {
                        continue;
                    };
                    if forward.amount_out < wanted {
                        return Err(format!(
                            "{}: an input of {} bought {} against a target of {wanted}",
                            curve.as_str(),
                            inverse.amount_in,
                            forward.amount_out
                        ));
                    }
                    // The overshoot is rounding, not a different trade.
                    let slack = forward.amount_out - wanted;
                    let tolerance = wanted / Decimal::from_int(1_000) + Decimal::from_raw(1_000_000);
                    if slack > tolerance {
                        return Err(format!(
                            "{}: the inverse overshot by {slack}, beyond the rounding tolerance {tolerance}",
                            curve.as_str()
                        ));
                    }
                }
                Ok(())
            },
        );
}

#[test]
fn price_impact_grows_with_size_and_a_larger_trade_never_prices_better() {
    for curve in [
        PoolCurve::ConstantProduct,
        PoolCurve::StableSwap { amplification: 85 },
    ] {
        let pool = pool(
            curve,
            Decimal::from_int(50_000),
            Decimal::from_int(50_000),
            30,
        );
        let mut previous_price = Decimal::ZERO;
        let mut previous_impact = Decimal::ZERO;
        for step in 1..40 {
            let size = Decimal::from_int(step * 25);
            let quoted = pool
                .quote_exact_in(BookSide::Ask, size)
                .expect("a size well inside the reserves is quotable");
            assert!(
                quoted.effective_price >= previous_price,
                "{}: buying {size} priced better than a smaller trade ({} < {previous_price})",
                curve.as_str(),
                quoted.effective_price
            );
            assert!(
                quoted.price_impact >= previous_impact,
                "{}: impact fell as size rose at {size}",
                curve.as_str()
            );
            assert!(
                !quoted.price_impact.is_negative(),
                "a pool cannot fill better than its own marginal price"
            );
            previous_price = quoted.effective_price;
            previous_impact = quoted.price_impact;
        }
    }
}

#[test]
fn a_stable_swap_pool_absorbs_size_more_flatly_than_a_constant_product_pool() {
    let balanced_base = Decimal::from_int(1_000_000);
    let balanced_quote = Decimal::from_int(1_000_000);
    let size = Decimal::from_int(50_000);

    let stable = pool(
        PoolCurve::StableSwap { amplification: 100 },
        balanced_base,
        balanced_quote,
        30,
    );
    let product = pool(
        PoolCurve::ConstantProduct,
        balanced_base,
        balanced_quote,
        30,
    );
    let stable_quote = stable
        .quote_exact_in(BookSide::Ask, size)
        .expect("stable quote");
    let product_quote = product
        .quote_exact_in(BookSide::Ask, size)
        .expect("constant product quote");

    assert!(
        stable_quote.amount_out > product_quote.amount_out,
        "a stable curve should deliver more base for the same quote near the peg: {} vs {}",
        stable_quote.amount_out,
        product_quote.amount_out
    );
    assert!(
        stable_quote.price_impact < product_quote.price_impact,
        "a stable curve should show less impact near the peg"
    );
    assert_eq!(
        stable_quote.fee_side,
        FeeSide::Output,
        "the stable curve charges its fee on the output"
    );
}

#[test]
fn a_stable_swap_never_lets_its_invariant_fall_across_a_swap() {
    let mut pool = pool(
        PoolCurve::StableSwap { amplification: 60 },
        Decimal::from_int(400_000),
        Decimal::from_int(600_000),
        30,
    );
    let before = pool.invariant().expect("the invariant is computable");
    for step in 1..12 {
        let side = if step % 2 == 0 {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        let quoted = pool
            .quote_exact_in(side, Decimal::from_int(step * 1_000))
            .expect("a modest swap is quotable");
        pool.apply(&quoted).expect("the swap applies");
    }
    let after = pool.invariant().expect("the invariant is computable");
    let (PoolInvariant::StableSwap { d: before_d }, PoolInvariant::StableSwap { d: after_d }) =
        (before, after)
    else {
        panic!("a stable pool must report a stable invariant");
    };
    assert!(
        after_d >= before_d,
        "the invariant fell from {before_d} to {after_d}; the pool paid out more than it took in"
    );
}

#[test]
fn a_swap_larger_than_the_pool_can_deliver_is_refused_rather_than_clamped() {
    let pool = constant_product(1_000, 2_000_000);
    let error = pool
        .quote_exact_out(BookSide::Ask, Decimal::from_int(1_000))
        .expect_err("a pool cannot sell its entire base reserve");
    assert_eq!(error.code(), "invalid");
}

#[test]
fn applying_a_quote_moves_the_reserves_the_way_the_quote_said_it_would() {
    let mut pool = constant_product(1_000, 2_000_000);
    let quoted = pool
        .quote_exact_in(BookSide::Ask, Decimal::from_int(20_000))
        .expect("quote");
    pool.apply(&quoted).expect("apply");
    assert_eq!(pool.reserve_base(), quoted.reserve_base_after);
    assert_eq!(pool.reserve_quote(), quoted.reserve_quote_after);
    assert!(
        pool.reserve_quote() > Decimal::from_int(2_000_000),
        "the whole input including the fee stays in the pool"
    );
}

// --- chain integers ---------------------------------------------------------

#[test]
fn an_eighteen_decimal_amount_reports_the_precision_it_cannot_represent() {
    // 1.234567891234567891 ETH: nine digits fit, nine do not.
    let amount = TokenAmount::wei(1_234_567_891_234_567_891);
    let converted = amount.to_decimal().expect("within range");
    assert!(!converted.is_exact(), "nine digits were dropped");
    assert_eq!(converted.residual(), 234_567_891);
    assert_eq!(converted.truncated(), Decimal::parse("1.234567891").expect("parses"));
    assert_eq!(converted.rounded(), Decimal::parse("1.234567891").expect("parses"));

    let error = converted
        .require_exact()
        .expect_err("an inexact conversion must refuse to answer");
    assert_eq!(error.code(), "numeric");
    assert!(
        error.message().contains("residual"),
        "the error should quantify the loss, got {error}"
    );
    assert!(converted.residual_fraction() > 0.0);
}

#[test]
fn an_eighteen_decimal_amount_that_fits_converts_exactly() {
    let amount = TokenAmount::wei(2_500_000_000_000_000_000);
    let converted = amount.to_decimal().expect("within range");
    assert!(converted.is_exact());
    assert_eq!(
        converted.require_exact().expect("exact"),
        Decimal::parse("2.5").expect("parses")
    );
}

#[test]
fn rounding_half_away_from_zero_is_visible_rather_than_silent() {
    // The tenth decimal is a five, so rounding and truncation disagree.
    let amount = TokenAmount::new(1_500_000_005, 10).expect("ten decimals");
    let converted = amount.to_decimal().expect("within range");
    assert!(!converted.is_exact());
    assert_eq!(converted.truncated(), Decimal::parse("0.15").expect("parses"));
    assert_eq!(
        converted.rounded(),
        Decimal::parse("0.150000001").expect("parses")
    );
}

#[test]
fn a_token_with_fewer_decimals_cannot_carry_more_precision_than_it_has() {
    let value = Decimal::parse("12.3456785").expect("parses");
    let quantised = TokenAmount::quantise(value, 6).expect("six decimals");
    assert!(!quantised.is_exact());
    assert_eq!(quantised.truncated().raw(), 12_345_678);
    assert!(
        quantised.require_exact().is_err(),
        "a six-decimal token cannot pay nine decimals"
    );

    let payable = Decimal::parse("12.345678").expect("parses");
    assert!(
        TokenAmount::quantise(payable, 6)
            .expect("six decimals")
            .require_exact()
            .is_ok(),
        "an amount the token can carry converts exactly"
    );
}

#[test]
fn every_exact_multiple_survives_a_conversion_round_trip() {
    Property::new("exact chain amounts round trip")
        .cases(500)
        .for_all(
            |rng: &mut Xoshiro256| {
                let units = rng.below(1_000_000_000) as i128;
                let decimals = 9 + rng.below(10) as u8;
                (units, decimals)
            },
            |(units, decimals)| {
                // A whole number of nano-units is exactly representable at any
                // decimal exponent of nine or more.
                let scale = 10i128.pow(u32::from(*decimals) - 9);
                let amount = TokenAmount::new(units * scale, *decimals).map_err(|e| e.to_string())?;
                let converted = amount.to_decimal().map_err(|e| e.to_string())?;
                let value = converted.require_exact().map_err(|e| e.to_string())?;
                let back = TokenAmount::quantise(value, *decimals)
                    .map_err(|e| e.to_string())?
                    .require_exact()
                    .map_err(|e| e.to_string())?;
                if back.raw() != units * scale {
                    return Err(format!("{} became {}", units * scale, back.raw()));
                }
                Ok(())
            },
        );
}

// --- gas --------------------------------------------------------------------

#[test]
fn gas_becomes_a_funding_deduction_in_the_quote_currency() {
    let profile = GasProfile::new("dex-swap", 140_000, 320_000).expect("a valid profile");
    // 25 gwei, with the native token at 2,000 quote units.
    let price_per_gas = Decimal::from_raw(25);
    let native_price = Decimal::from_int(2_000);
    let cost = GasCost::estimate(&profile, price_per_gas, native_price, Currency::USD)
        .expect("priceable");

    assert_eq!(cost.native_cost, Decimal::parse("0.0035").expect("parses"));
    assert_eq!(cost.cost.amount, Decimal::from_int(7));
    assert_eq!(cost.cost.currency, Currency::USD);

    let deduction = cost.deduction().expect("a deduction");
    assert_eq!(deduction.kind, DeductionKind::Funding);
    assert_eq!(deduction.amount, Decimal::from_int(7));
    assert!(
        deduction.basis.contains("140000"),
        "the basis should record the gas it priced, got {}",
        deduction.basis
    );

    let worst = GasCost::worst_case(&profile, price_per_gas, native_price, Currency::USD)
        .expect("priceable");
    assert!(
        worst.cost.amount > cost.cost.amount,
        "the worst case must cost more than the expectation"
    );
    assert_eq!(
        cost.per_unit(Decimal::from_int(2)).expect("amortisable"),
        Decimal::parse("3.5").expect("parses")
    );
}

#[test]
fn a_fee_ceiling_below_the_base_fee_is_not_a_price_but_an_exclusion() {
    let base = Decimal::from_raw(30);
    assert_eq!(
        effective_gas_price(base, Decimal::from_raw(50), Decimal::from_raw(2))
            .expect("includable"),
        Decimal::from_raw(32),
        "the transaction pays base plus tip when its ceiling allows"
    );
    assert_eq!(
        effective_gas_price(base, Decimal::from_raw(31), Decimal::from_raw(5))
            .expect("includable"),
        Decimal::from_raw(31),
        "the ceiling caps the price"
    );
    let error = effective_gas_price(base, Decimal::from_raw(29), Decimal::from_raw(5))
        .expect_err("a ceiling below the base fee is not includable");
    assert_eq!(error.code(), "invalid");
}

// --- mempool ----------------------------------------------------------------

fn pending(label: &str, sender: &str, nonce: u64, tip: i128, at: Timestamp) -> PendingTransaction {
    PendingTransaction {
        hash: TxHash::new(Hash32::of(&[label.as_bytes()])),
        from: Address::new(sender),
        nonce,
        gas_limit: 200_000,
        max_fee_per_gas: Decimal::from_raw(100),
        max_priority_fee_per_gas: Decimal::from_raw(tip),
        first_seen: at,
        intent: None,
    }
}

fn mempool() -> Mempool {
    Mempool::new(
        VenueId::new("TEST-DEX"),
        VenueClass::DecentralisedExchange,
    )
    .expect("a chain venue has a mempool")
}

#[test]
fn a_venue_whose_quotes_are_firm_is_refused_a_mempool() {
    let error = Mempool::new(VenueId::new("XNYS"), VenueClass::Exchange)
        .expect_err("an exchange has no mempool");
    assert_eq!(error.code(), "invalid");
}

#[test]
fn the_predicted_ordering_cannot_be_read_without_acknowledging_it_can_change() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut mempool = mempool();
    mempool.insert(pending("low", "0xa", 0, 1, at));
    mempool.insert(pending("high", "0xb", 0, 9, at));
    mempool.insert(pending("mid", "0xc", 0, 4, at));

    let base_fee = Decimal::from_raw(20);
    let ordering = mempool
        .likely_ordering(base_fee, 30_000_000, at)
        .expect("an ordering is predictable");
    let risk = ordering.assess();
    assert!(
        !risk.quotes_are_firm && !risk.settles_atomically,
        "the venue class already says neither holds"
    );
    assert!(risk.can_be_front_run());
    assert_eq!(risk.worst_case_position(), 2, "three transactions compete");

    let sequence = ordering.sequence(&risk).expect("the matching risk unlocks it");
    let tips: Vec<Decimal> = sequence.iter().map(|entry| entry.tip).collect();
    assert_eq!(
        tips,
        vec![
            Decimal::from_raw(9),
            Decimal::from_raw(4),
            Decimal::from_raw(1)
        ],
        "a builder maximising fees takes the highest tip first"
    );

    // A risk assessed against a different mempool state does not unlock it.
    let mut moved = mempool.clone();
    moved.insert(pending("newcomer", "0xd", 0, 12, at));
    let stale = moved
        .likely_ordering(base_fee, 30_000_000, at)
        .expect("ordering")
        .assess();
    let error = ordering
        .sequence(&stale)
        .expect_err("a risk from another state must not unlock this one");
    assert_eq!(error.code(), "invalid");
}

#[test]
fn a_transaction_priced_below_the_base_fee_is_left_out_of_the_prediction() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut mempool = mempool();
    let mut poor = pending("poor", "0xa", 0, 1, at);
    poor.max_fee_per_gas = Decimal::from_raw(5);
    let poor_hash = poor.hash;
    mempool.insert(poor);
    mempool.insert(pending("rich", "0xb", 0, 3, at));

    let ordering = mempool
        .likely_ordering(Decimal::from_raw(20), 30_000_000, at)
        .expect("ordering");
    assert_eq!(ordering.len(), 1);
    assert!(ordering.excluded().contains(&poor_hash));
    assert!(ordering.predicted_position(&poor_hash).is_none());
}

#[test]
fn a_nonce_gap_leaves_a_senders_later_transactions_unorderable() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut mempool = mempool();
    mempool.insert(pending("first", "0xa", 4, 9, at));
    let gapped = pending("gapped", "0xa", 6, 9, at);
    let gapped_hash = gapped.hash;
    mempool.insert(gapped);

    let ordering = mempool
        .likely_ordering(Decimal::from_raw(20), 30_000_000, at)
        .expect("ordering");
    assert_eq!(ordering.len(), 1, "the gap makes nonce 6 unreachable");
    assert!(ordering.excluded().contains(&gapped_hash));
}

#[test]
fn an_included_transaction_leaves_the_mempool() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut mempool = mempool();
    let transaction = pending("included", "0xa", 0, 3, at);
    let hash = transaction.hash;
    mempool.insert(transaction);

    let block = Block {
        chain: ChainId::new("test-chain"),
        number: BlockNumber::new(1),
        hash: BlockHash::new(Hash32::of(&[b"block"])),
        parent_hash: BlockHash::new(Hash32::of(&[b"parent"])),
        timestamp: at,
        base_fee: Decimal::from_raw(20),
        gas_used: 21_000,
        gas_limit: 30_000_000,
        transactions: vec![qip_chain::block::Transaction {
            hash,
            index: 0,
            from: Address::new("0xa"),
            to: None,
            status: qip_chain::block::TxStatus::Succeeded,
            gas_used: 21_000,
            effective_gas_price: Decimal::from_raw(22),
            traces: Vec::new(),
        }],
    };
    assert_eq!(mempool.absorb(&block), 1);
    assert!(mempool.is_empty());
}

// --- bridges ----------------------------------------------------------------

fn route() -> BridgeRoute {
    BridgeRoute::new(
        "test-bridge",
        ChainId::new("chain-a"),
        ChainId::new("chain-b"),
        Duration::from_mins(15),
        Duration::from_hours(2),
        Confirmations::exactly(12),
        FeeBps::new(5).expect("a valid fee"),
        vec![BridgeFailure::SourceReorg, BridgeFailure::RelayerHalt],
    )
    .expect("a valid route")
}

fn transfer(at: Timestamp) -> BridgeTransfer {
    BridgeTransfer::open(
        TransferId::new("transfer-1"),
        route(),
        ObjectId::from_string("BASE"),
        Decimal::from_int(100),
        at,
        BlockNumber::new(10),
        BlockHash::new(Hash32::of(&[b"source-block"])),
    )
    .expect("a valid transfer")
}

#[test]
fn a_bridged_position_is_visible_as_exposure_until_it_is_credited() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut ledger = BridgeLedger::new();
    ledger.open(transfer(at)).expect("opens");

    let exposures = ledger.exposures(at.saturating_add(Duration::from_mins(5)));
    assert_eq!(exposures.len(), 1, "an in-flight transfer is a position");
    assert_eq!(exposures[0].amount, Decimal::from_int(100));
    assert!(exposures[0].failure_modes.contains(&BridgeFailure::SourceReorg));
    assert_eq!(
        ledger
            .exposure_by_object(at)
            .get(&ObjectId::from_string("BASE")),
        Some(&Decimal::from_int(100)),
        "the exposure must be attributable to the asset"
    );

    let id = TransferId::new("transfer-1");
    let held = ledger.get_mut(&id).expect("the transfer is open");
    held.observe_source(
        Finality::Confirmed {
            confirmations: 12,
            required: 12,
        },
        at.saturating_add(Duration::from_mins(3)),
    )
    .expect("observing finality");
    let delivered = held
        .credit(
            at.saturating_add(Duration::from_mins(10)),
            BlockNumber::new(400),
        )
        .expect("crediting a confirmed transfer");
    assert_eq!(
        delivered,
        Decimal::parse("99.95").expect("parses"),
        "the route's fee comes out of the delivered amount"
    );
    assert!(
        ledger.exposures(at.saturating_add(Duration::from_mins(11))).is_empty(),
        "a credited transfer is no longer an exposure"
    );
}

#[test]
fn a_transfer_cannot_be_credited_before_its_source_side_is_confirmed() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut held = transfer(at);
    let error = held
        .credit(at.saturating_add(Duration::from_mins(1)), BlockNumber::new(400))
        .expect_err("crediting an unconfirmed transfer must be refused");
    assert_eq!(error.code(), "denied");
    assert!(error.message().contains("12 confirmations"));

    held.observe_source(
        Finality::Included {
            confirmations: 3,
            required: 12,
        },
        at,
    )
    .expect("observing");
    assert!(
        matches!(held.status(), TransferStatus::AwaitingSourceFinality),
        "three confirmations of twelve is not confirmed"
    );
}

#[test]
fn a_reorganised_source_block_fails_the_transfer_it_was_funding() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut held = transfer(at);
    held.observe_source(
        Finality::Orphaned {
            was_at: BlockNumber::new(10),
        },
        at.saturating_add(Duration::from_mins(1)),
    )
    .expect("observing");
    assert!(
        matches!(
            held.status(),
            TransferStatus::Failed {
                failure: BridgeFailure::SourceReorg,
                ..
            }
        ),
        "a transfer whose deposit was withdrawn cannot complete"
    );
    assert!(
        held.exposure(at.saturating_add(Duration::from_mins(2)))
            .is_none(),
        "a failed transfer is no longer in flight"
    );
}

#[test]
fn an_overdue_transfer_is_reported_before_anyone_asks() {
    let at = Timestamp::from_secs(1_700_000_000);
    let mut ledger = BridgeLedger::new();
    ledger.open(transfer(at)).expect("opens");
    let late = at.saturating_add(Duration::from_hours(3));
    assert_eq!(ledger.overdue(late).len(), 1);
    let exposure = &ledger.exposures(late)[0];
    assert!(exposure.overdue);
    assert!(
        exposure.expected_remaining.as_nanos() < 0,
        "a transfer past its expected latency has negative time remaining"
    );
}
