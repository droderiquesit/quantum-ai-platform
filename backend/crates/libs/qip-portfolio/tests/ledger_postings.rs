//! A paper fill becomes one balanced ledger event, and nothing else does.
//!
//! ADR 0100 §1 puts the posting logic here, pure; §9 makes the refusal of a
//! fill not marked simulated the fourth paper fence. Each test below names
//! the failure it prevents.

use qip_contracts::ledger::{Account, Direction, FeeReport, Settlement};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry, OutcomeRecord};
use qip_core::error::Error;
use qip_core::rng::Rng;
use qip_core::testing::Property;
use qip_core::{Decimal, Timestamp};
use qip_portfolio::ledger::{PaperFill, post};
use std::collections::{BTreeMap, BTreeSet};

const OBJECT: &str = "OBJ-ACME";
const UNIT: &str = "USD";
const VENUE: &str = "sim-xnys";
const CELL: &str = "cell-eu-1";

/// Everything a `Decision::Filled` carries, as text, so each test can break
/// exactly one field.
#[derive(Clone, Debug)]
struct Spec {
    quantity: String,
    price: String,
    simulated: bool,
    shares: Vec<(String, String)>,
    side: Option<BookSide>,
    quote_unit: Option<String>,
    fee: Option<String>,
}

fn complete() -> Spec {
    Spec {
        quantity: "30".to_string(),
        price: "101.25".to_string(),
        simulated: true,
        shares: vec![
            ("alpha".to_string(), "10".to_string()),
            ("beta".to_string(), "20".to_string()),
        ],
        side: Some(BookSide::Ask),
        quote_unit: Some(UNIT.to_string()),
        fee: Some("0.3".to_string()),
    }
}

fn outcome(spec: &Spec) -> OutcomeRecord {
    let entry = JournalEntry {
        sequence: 41,
        at: Timestamp::from_secs(1_790_000_000),
        decision: Decision::Filled {
            order_id: "ord-7".to_string(),
            venue: VENUE.to_string(),
            object: OBJECT.to_string(),
            quantity: spec.quantity.clone(),
            price: spec.price.clone(),
            simulated: spec.simulated,
            shares: spec.shares.clone(),
            side: spec.side,
            quote_unit: spec.quote_unit.clone(),
            fee: spec.fee.clone(),
        },
        digest: "digest-41".to_string(),
        version: ChainVersion::V2,
    };
    OutcomeRecord {
        cell: CELL.to_string(),
        session: 3,
        journal_sequence: entry.sequence,
        journal_digest: entry.digest.clone(),
        entry,
    }
}

fn dec(text: &str) -> Decimal {
    Decimal::parse(text).expect("test literal")
}

/// A decimal with exactly four fractional digits, `1..=max` ten-thousandths.
fn four_dp(rng: &mut qip_core::Xoshiro256, max: u64) -> Decimal {
    let steps = i128::from(1 + rng.below(max));
    Decimal::from_raw(steps * 100_000)
}

#[test]
fn every_ledger_event_balances_in_every_unit_for_any_side_quantity_price_and_share_split() {
    // LEDGER-002 and LEDGER-006: debits equal credits in every unit
    // separately, exactly. The failure this prevents is a posting rule that
    // drops or misprices one leg — the set then closes only if something
    // plugs it, and a plugged ledger cannot tell a break from a rounding.
    // Every case is run on both sides of the book, so "any side" is every
    // side rather than whichever the generator happened to draw.
    let mut saw_multi_share = false;
    let mut saw_reported = false;
    let mut saw_unreported = false;
    Property::new("ledger events balance per unit")
        .cases(400)
        .for_all(
            |rng| {
                let count = 1 + rng.below(5) as usize;
                let shares: Vec<(String, Decimal)> = (0..count)
                    .map(|i| (format!("strategy-{i}"), four_dp(rng, 10_000_000)))
                    .collect();
                let price = four_dp(rng, 1_000_000_000);
                // A third unreported, a third reported as zero, a third a
                // rate on the quantity so each share's part is exact.
                let fee_rate = match rng.below(3) {
                    0 => None,
                    1 => Some(Decimal::ZERO),
                    _ => Some(Decimal::from_raw(
                        i128::from(1 + rng.below(1_000)) * 10_000_000,
                    )),
                };
                (shares, price, fee_rate)
            },
            |(shares, price, fee_rate)| {
                let quantity: Decimal = shares.iter().map(|(_, q)| *q).sum();
                let fee = fee_rate.map(|r| r * quantity);
                for side in [BookSide::Ask, BookSide::Bid] {
                    let spec = Spec {
                        quantity: quantity.to_string(),
                        price: price.to_string(),
                        simulated: true,
                        shares: shares
                            .iter()
                            .map(|(s, q)| (s.clone(), q.to_string()))
                            .collect(),
                        side: Some(side),
                        quote_unit: Some(UNIT.to_string()),
                        fee: fee.map(|f| f.to_string()),
                    };
                    let fill = PaperFill::try_from(&outcome(&spec))
                        .map_err(|e| format!("a valid paper fill was refused: {e}"))?;
                    let event = post(&fill).map_err(|e| format!("posting refused: {e}"))?;

                    // Premise: both units carry real amounts, so equality
                    // below is not zero equal to zero.
                    let mut sums: BTreeMap<&str, (Decimal, Decimal)> = BTreeMap::new();
                    for p in event.postings() {
                        let entry = sums.entry(p.unit.as_str()).or_default();
                        match p.direction {
                            Direction::Debit => entry.0 += p.amount,
                            Direction::Credit => entry.1 += p.amount,
                        }
                    }
                    let units: BTreeSet<&str> = sums.keys().copied().collect();
                    if units != BTreeSet::from([OBJECT, UNIT]) {
                        return Err(format!("{side:?}: units posted were {units:?}"));
                    }
                    for (unit, (debits, credits)) in &sums {
                        if !debits.is_positive() {
                            return Err(format!("{side:?}: nothing was debited in {unit}"));
                        }
                        if debits != credits {
                            return Err(format!(
                                "{side:?}: {unit} debits {debits} != credits {credits}"
                            ));
                        }
                    }

                    // Split exactly by the shares: each strategy's trading
                    // account moves by its own share of the object, on the
                    // side the fill took, and by nothing else.
                    for (strategy, share) in shares {
                        let trading = Account::Trading {
                            cell: CELL.to_string(),
                            strategy: strategy.clone(),
                        };
                        let mut net = Decimal::ZERO;
                        for p in event.postings() {
                            if p.account == trading && p.unit == OBJECT {
                                match p.direction {
                                    Direction::Debit => net += p.amount,
                                    Direction::Credit => net -= p.amount,
                                }
                            }
                        }
                        let expected = if side == BookSide::Ask {
                            *share
                        } else {
                            -*share
                        };
                        if net != expected {
                            return Err(format!(
                                "{side:?}: {strategy} moved {net} of {OBJECT}, share {share}"
                            ));
                        }
                    }

                    let fees_debited: Decimal = event
                        .postings()
                        .iter()
                        .filter(|p| matches!(p.account, Account::Fees { .. }))
                        .map(|p| p.amount)
                        .sum();
                    if fees_debited != fee.unwrap_or(Decimal::ZERO) {
                        return Err(format!("{side:?}: fees {fees_debited} != {fee:?}"));
                    }
                    if event.settlement() != Settlement::Simulated {
                        return Err("an event was booked under a non-simulated settlement".into());
                    }
                    // Pure: a replay books identical postings, in the same
                    // order. Compared as values and as rendered text, since
                    // this crate carries no JSON encoder to compare bytes.
                    let again = post(&fill).map_err(|e| format!("replay refused: {e}"))?;
                    if again != event || format!("{again:?}") != format!("{event:?}") {
                        return Err(format!("{side:?}: a replay booked different bytes"));
                    }
                }
                saw_multi_share |= shares.len() > 1;
                match fee_rate {
                    None => saw_unreported = true,
                    Some(r) if r.is_positive() => saw_reported = true,
                    Some(_) => {}
                }
                Ok(())
            },
        );
    assert!(
        saw_multi_share && saw_reported && saw_unreported,
        "premise: the generator reached split fills ({saw_multi_share}), reported fees \
         ({saw_reported}) and unreported fees ({saw_unreported})"
    );
}

#[test]
fn a_fill_not_marked_simulated_has_no_representation_in_the_ledger() {
    // ADR 0100 §9's fourth fence. The three existing layers keep a live
    // order from being sent; this one keeps a live fill from being booked
    // if anything ever got past them. Premise: the identical fill, marked
    // simulated, is accepted and posts — so the refusal is the flag's.
    let paper = complete();
    let fill = PaperFill::try_from(&outcome(&paper)).expect("premise: a paper fill is accepted");
    assert!(post(&fill).is_ok(), "premise: the paper fill posts");

    let live = Spec {
        simulated: false,
        ..paper
    };
    match PaperFill::try_from(&outcome(&live)) {
        Err(Error::Denied(reason)) => assert!(
            reason.contains("not marked simulated"),
            "refused for the wrong reason: {reason}"
        ),
        other => panic!("a live fill became something the ledger can book: {other:?}"),
    }
}

#[test]
fn a_fill_missing_its_side_or_quote_unit_is_refused_and_never_guessed() {
    // Red-team B2: without a side a fill cannot be a debit or a credit, and
    // without a quote unit its price is a number in no currency. Entries
    // sealed before those fields existed carry neither, and a reader that
    // defaulted the side to "bought" would book every such sale backwards.
    let whole = complete();
    assert!(
        PaperFill::try_from(&outcome(&whole)).is_ok(),
        "premise: the complete fill is accepted"
    );

    let cases = [
        (
            "no side",
            Spec {
                side: None,
                ..whole.clone()
            },
        ),
        (
            "no quote unit",
            Spec {
                quote_unit: None,
                ..whole.clone()
            },
        ),
        (
            "no quote unit",
            Spec {
                quote_unit: Some(String::new()),
                ..whole.clone()
            },
        ),
    ];
    for (reason, spec) in cases {
        match PaperFill::try_from(&outcome(&spec)) {
            Err(Error::Invalid(message)) => assert!(
                message.contains(reason),
                "refused, but not for `{reason}`: {message}"
            ),
            other => panic!("a fill with {reason} was not refused: {other:?}"),
        }
    }
}

#[test]
fn shares_that_do_not_sum_to_the_fill_are_refused() {
    // The cell ships its own pro-rata split. A split that is off by a single
    // nano-unit books a position no fill created, or loses one a fill did;
    // the ledger refuses it rather than re-splitting on its own arithmetic.
    let whole = complete();
    let fill = PaperFill::try_from(&outcome(&whole)).expect("premise: an exact split is accepted");
    assert_eq!(
        fill.shares().values().copied().sum::<Decimal>(),
        fill.quantity(),
        "premise: the accepted split sums exactly"
    );

    for beta in ["19.999999999", "20.000000001"] {
        let spec = Spec {
            shares: vec![
                ("alpha".to_string(), "10".to_string()),
                ("beta".to_string(), beta.to_string()),
            ],
            ..whole.clone()
        };
        match PaperFill::try_from(&outcome(&spec)) {
            Err(Error::Invalid(message)) => assert!(
                message.contains("sum to"),
                "refused for the wrong reason: {message}"
            ),
            other => panic!("shares summing to 10 + {beta} against 30 were accepted: {other:?}"),
        }
    }
}

#[test]
fn an_unreported_fee_is_marked_unreported_and_never_estimated() {
    // LEDGER-019: an entry is never a guess. A venue that said nothing
    // about a fee has not said it was zero, and booking zero invents a fact
    // no statement will ever reconcile. Premise: a reported fee does post,
    // to the fees subledger, for exactly what was reported — so the absence
    // below is the absence of a report, not a rule that never posts fees.
    let reported = complete();
    let event = post(&PaperFill::try_from(&outcome(&reported)).expect("reported fill"))
        .expect("reported fill posts");
    assert_eq!(event.fee(), FeeReport::Reported { amount: dec("0.3") });
    let fees: Decimal = event
        .postings()
        .iter()
        .filter(|p| {
            p.account
                == Account::Fees {
                    venue: VENUE.to_string(),
                }
        })
        .map(|p| p.amount)
        .sum();
    assert_eq!(fees, dec("0.3"), "premise: a reported fee is booked");

    let unreported = Spec {
        fee: None,
        ..reported
    };
    let fill = PaperFill::try_from(&outcome(&unreported)).expect("an unreported fee is bookable");
    assert_eq!(fill.fee(), FeeReport::Unreported);
    let event = post(&fill).expect("posts");
    assert_eq!(
        event.fee(),
        FeeReport::Unreported,
        "the event does not say the fee was unreported"
    );
    assert!(
        !event.postings().is_empty(),
        "premise: the event has postings to search"
    );
    for p in event.postings() {
        assert!(
            !matches!(p.account, Account::Fees { .. }),
            "an unreported fee was posted: {p:?}"
        );
    }
}
