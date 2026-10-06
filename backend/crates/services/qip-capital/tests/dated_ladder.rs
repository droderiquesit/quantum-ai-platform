//! The dated liquidity ladder (CAPITAL-002): the rungs sum to total
//! liquidity, and nothing sits in a rung that closes before it is available.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_capital::dated_ladder::{DatedLadder, EntryKind, LadderEntry};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, Timestamp, dec};

const KINDS: [EntryKind; 4] = [
    EntryKind::Balance,
    EntryKind::Inflow,
    EntryKind::Outflow,
    EntryKind::Encumbered,
];

fn as_of() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn horizons() -> [Duration; 4] {
    [
        Duration::from_days(1),
        Duration::from_days(7),
        Duration::from_days(30),
        Duration::from_days(90),
    ]
}

#[test]
fn for_generated_books_the_rungs_sum_to_total_liquidity_and_nothing_sits_in_a_rung_that_closes_before_it_is_available()
-> Result<()> {
    let mut rng = Xoshiro256::seeded(0x00CA_0002);
    // What the sweep saw, so the premise below can prove it saw everything.
    let (mut landed_now, mut landed_dated, mut landed_open, mut outflows) = (0, 0, 0, 0);
    for book in 0..200u64 {
        let mut ladder = DatedLadder::new(as_of(), &horizons())?;
        let mut expected = Decimal::ZERO;
        let entries = 1 + rng.below(30);
        for n in 0..entries {
            let kind = KINDS[rng.below(4) as usize];
            let amount = Decimal::from_int(1 + rng.below(5_000) as i64);
            // From two days before the as-of instant to 120 days after it, so
            // some entries are already settled and some fall past the last rung.
            let offset = Duration::from_hours(rng.below(122 * 24) as i64 - 48);
            let at = as_of().saturating_add(offset);
            let entry = LadderEntry {
                id: format!("b{book}-e{n}"),
                kind,
                amount,
                at,
            };
            expected += entry.signed();
            outflows += usize::from(kind == EntryKind::Outflow);
            ladder.place(entry)?;
        }

        let buckets = ladder.buckets();
        let summed = buckets.iter().fold(Decimal::ZERO, |sum, b| sum + b.net);
        assert_eq!(
            summed, expected,
            "book {book}: the rungs do not sum to total liquidity"
        );
        assert_eq!(ladder.total(), expected);

        let placed: usize = buckets.iter().map(|b| b.entries.len()).sum();
        assert_eq!(
            placed as u64, entries,
            "book {book}: an entry is missing or doubled"
        );

        for (index, bucket) in buckets.iter().enumerate() {
            for id in &bucket.entries {
                let entry = ladder.entry(id).expect("a bucketed entry is on the ladder");
                match bucket.closes {
                    // Never earlier than it is available: the rung closes at
                    // or after the entry's own instant.
                    Some(close) => {
                        assert!(
                            close >= entry.at,
                            "{id} is available at {} and sits in a rung closing {close}",
                            entry.at
                        );
                        if index == 0 {
                            landed_now += 1;
                        } else {
                            landed_dated += 1;
                        }
                    }
                    None => landed_open += 1,
                }
                // And no later than it has to be: the rung before closes
                // before the entry is available. Without this the property
                // above is satisfied by putting everything in the last rung.
                if index > 0 {
                    let before = buckets[index - 1]
                        .closes
                        .expect("every rung but the last has a close");
                    assert!(
                        before < entry.at,
                        "{id} is available at {} and the rung closing {before} would have held it",
                        entry.at
                    );
                }
            }
        }
    }
    // Premise: the sweep reached the as-of rung, a dated rung, the open rung
    // and the one kind that subtracts.
    assert!(
        landed_now > 20 && landed_dated > 20 && landed_open > 20 && outflows > 20,
        "{landed_now} {landed_dated} {landed_open} {outflows}"
    );
    Ok(())
}

#[test]
fn a_balance_that_settles_tomorrow_is_not_available_today() -> Result<()> {
    let mut ladder = DatedLadder::new(as_of(), &horizons())?;
    let settles = as_of().saturating_add(Duration::from_hours(20));
    ladder.place(LadderEntry {
        id: "cash".into(),
        kind: EntryKind::Balance,
        amount: dec!("100"),
        at: as_of(),
    })?;
    ladder.place(LadderEntry {
        id: "t-plus-one".into(),
        kind: EntryKind::Balance,
        amount: dec!("900"),
        at: settles,
    })?;
    ladder.place(LadderEntry {
        id: "pledged".into(),
        kind: EntryKind::Encumbered,
        amount: dec!("500"),
        at: as_of().saturating_add(Duration::from_days(5)),
    })?;
    // Premise: all three are on the ladder and counted in the total.
    assert_eq!(ladder.total(), dec!("1500"));

    assert_eq!(ladder.available_by(as_of()), dec!("100"));
    // Even at the instant it settles, its rung has not closed.
    assert_eq!(ladder.available_by(settles), dec!("100"));
    assert_eq!(
        ladder.available_by(as_of().saturating_add(Duration::from_days(1))),
        dec!("1000")
    );
    // The pledged balance appears only once its unlock rung has closed.
    assert_eq!(
        ladder.available_by(as_of().saturating_add(Duration::from_days(6))),
        dec!("1000")
    );
    assert_eq!(
        ladder.available_by(as_of().saturating_add(Duration::from_days(7))),
        dec!("1500")
    );
    Ok(())
}

#[test]
fn horizons_out_of_order_are_refused_rather_than_sorted() {
    let refusal = DatedLadder::new(as_of(), &[Duration::from_days(7), Duration::from_days(1)])
        .expect_err("an unordered ladder was accepted");
    assert!(
        refusal.message().contains("strictly ascending"),
        "{}",
        refusal.message()
    );
}
