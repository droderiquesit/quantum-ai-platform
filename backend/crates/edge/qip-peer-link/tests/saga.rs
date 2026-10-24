//! A cross-region cycle from origination to unwind, between two nodes that
//! each hold nothing but `boundary::ReflexPort` — the production path
//! (R12-MESH-5046, 5061–5065, 5070; MESH-007).
//!
//! Until the saga existed nothing in production built an epoch, reserved a
//! leg, declared FIRE or sent a fill, and the clock offset was never
//! measured, so even a token sent by hand was refused as `clock_unknown`.
//! Here each side publishes only what a decision loop publishes — its touch
//! and the legs its gate authorises — and everything else happens on the
//! two session threads over a real socket: the clocks are probed, the
//! crossing is found, the token, reservations, FIRE and fills cross, and the
//! node that filled more than its counterparty unwinds the excess.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::{Clock, Decimal, SystemClock, dec};
use qip_peer_link::boundary::{
    EdgeQuote, LegAuthority, Offer, PeerStatus, RebalanceTask, ReflexPort, Side,
};
use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
use qip_transport::tls::LinkSecurity;
use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

/// How long the cycle may take before the test calls it stuck. A cycle is
/// about two seconds of protocol on an idle machine (a first connect that
/// finds no listener backs off for a second, clocks are probed every 250 ms,
/// and the epoch takes a few round trips), and the wait polls, so a generous
/// bound costs nothing when the cycle completes. It was 15 s, which a
/// saturated full-workspace run, with hundreds of test threads sharing the
/// cores, can plausibly exceed without anything being wrong; a cycle that
/// really is stuck stays stuck for a minute as surely as for fifteen seconds.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

/// How many times to pick a fresh pair of ports before giving up.
const PORT_ATTEMPTS: usize = 10;

fn free_port() -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    probe.local_addr().expect("its address")
}

/// Both nodes, listening on ports no one else holds.
///
/// `free_port` binds port 0 and lets go, so the port is free only until
/// something else takes it: in a workspace run that is hundreds of test
/// processes binding and connecting on loopback at once, and a node that finds
/// its port taken stops with an error that has nothing to do with the saga. The
/// listener is bound inside `spawn`, so a collision is visible there as an
/// `Err`; retrying on a fresh pair turns a race into a delay. Anything that is
/// still failing after `PORT_ATTEMPTS` pairs is reported with its cause.
fn nodes() -> Result<(ReflexPort, ReflexPort)> {
    let mut last = None;
    for _ in 0..PORT_ATTEMPTS {
        let (americas_at, europe_at) = (free_port(), free_port());
        match (
            port("americas", americas_at, "europe", europe_at),
            port("europe", europe_at, "americas", americas_at),
        ) {
            (Ok(americas), Ok(europe)) => return Ok((americas, europe)),
            (americas, europe) => last = americas.err().or(europe.err()),
        }
    }
    Err(last.expect("an attempt failed, so it left an error"))
}

fn port(node: &str, at: SocketAddr, peer: &str, peer_at: SocketAddr) -> Result<ReflexPort> {
    let (port, _) = qip_peer_link::spawn(
        &PeerSettings {
            node: node.into(),
            listen: at,
            peers: BTreeMap::from([(peer.to_string(), peer_at)]),
            security: LinkSecurity::plaintext_for_tests(),
            clock_bound: DEFAULT_CLOCK_BOUND,
        },
        Arc::new(SystemClock),
    )?;
    Ok(port)
}

struct Book {
    venue: &'static str,
    bid: Decimal,
    ask: Decimal,
    displayed: Decimal,
    /// What the gate authorises per leg.
    authorised: Decimal,
    /// What the touch will actually fill when a leg is sent.
    fills: Decimal,
    /// What the cell's gate says crossing back out costs per unit, when it
    /// is not the spread the book shows.
    exit_cost: Option<Decimal>,
}

fn offer(book: &Book, authorise: bool) -> Offer {
    let now = SystemClock.now().as_nanos();
    let spread = book.exit_cost.unwrap_or(book.ask - book.bid);
    let authority = |side: Side, limit: Decimal| LegAuthority {
        venue: book.venue.into(),
        instrument: "obj-ACME".into(),
        side,
        limit_price: limit,
        max_size: book.authorised,
        touch_size: book.fills,
        unwind_per_unit: spread,
        valid_until_ns: now + 1_000_000_000,
    };
    Offer {
        quotes: vec![EdgeQuote {
            venue: book.venue.into(),
            instrument: "obj-ACME".into(),
            bid: book.bid,
            bid_size: book.displayed,
            ask: book.ask,
            ask_size: book.displayed,
            as_of_ns: now,
        }],
        legs: if authorise {
            vec![
                authority(Side::Buy, book.ask),
                authority(Side::Sell, book.bid),
            ]
        } else {
            Vec::new()
        },
        withheld: 0,
    }
}

/// Publish both books every few milliseconds, as two decision loops would,
/// until `done` holds of the two statuses or patience runs out.
fn run_until(
    americas: &ReflexPort,
    europe: &ReflexPort,
    books: (&Book, &Book),
    authorise: (bool, bool),
    done: impl Fn(&PeerStatus, &PeerStatus) -> bool,
) -> (PeerStatus, PeerStatus) {
    let began = std::time::Instant::now();
    loop {
        americas.try_publish(offer(books.0, authorise.0));
        europe.try_publish(offer(books.1, authorise.1));
        let (a, e) = (americas.status(), europe.status());
        if done(&a, &e) {
            return (a, e);
        }
        assert!(
            began.elapsed() < PATIENCE,
            "the cycle did not complete within {PATIENCE:?}:\namericas {a:?}\neurope {e:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// americas buys at 100 what europe bids 101 for: a hundred basis points.
/// americas sorts first, so it coordinates. europe's touch fills six of the
/// ten the cycle was priced at, so americas, filled at ten, unwinds four.
///
/// Mutations (each run, each failed the wait, each restored):
/// * drop `PeerSession::probe` from the session thread — no clock is ever
///   measured, `clocks_measured` stays zero and no cycle is originated;
/// * drop `Saga::originate` from `Saga::step` — `cycles_originated` stays 0;
/// * make `Saga::fire` skip `declare_fire` — `fires` stays 0 and no leg is
///   sent;
/// * make `Saga::unwind` ignore the counterparty's smaller fill — `unwinds`
///   stays 0.
#[test]
fn two_nodes_publishing_only_through_their_ports_run_a_cross_region_cycle_to_its_unwind()
-> Result<()> {
    let (americas, europe) = nodes()?;
    let usa = Book {
        venue: "XNYS",
        bid: dec!("99"),
        ask: dec!("100"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let europe_book = Book {
        venue: "XLON",
        bid: dec!("101"),
        ask: dec!("102"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("6"),
        exit_cost: None,
    };
    let (a, e) = run_until(
        &americas,
        &europe,
        (&usa, &europe_book),
        (true, true),
        |a, e| a.unwinds >= 1 && e.fills >= 1,
    );
    assert!(
        a.clocks_measured >= 1 && e.clocks_measured >= 1,
        "{a:?} {e:?}"
    );
    assert!(a.cycles_originated >= 1, "americas coordinates: {a:?}");
    assert_eq!(e.cycles_originated, 0, "europe sorts second: {e:?}");
    assert!(a.reservations >= 1 && e.reservations >= 1, "{a:?} {e:?}");
    assert!(
        a.fires >= 1 && a.legs_sent >= 1 && e.legs_sent >= 1,
        "{a:?} {e:?}"
    );
    assert!(a.fills >= 1, "{a:?}");
    assert_eq!(e.declines, 0, "{e:?}");
    Ok(())
}

/// A leg whose declared unwind loss does not cover what the cell's own gate
/// says crossing back out costs is declined, though the cell authorises the
/// size and its book could hold it: the gate refuses before the book is
/// asked. The coordinator never declares FIRE, no leg is sent on either
/// side, and the cycle is rejected at the end of its TTL by name.
///
/// The rejection is also counted against the cycle americas *originated*
/// (`cycles_expired`, R12-RES-7095), which `cycles_rejected` alone cannot
/// say: it counts every unfired epoch the endpoint expires, joined or not.
///
/// Mutation (run): make `Saga::gate` return `Ok(())` for any positive leg —
/// europe reserves, FIRE is declared and legs are sent, and the wait for a
/// decline times out.
#[test]
fn a_leg_whose_unwind_bound_does_not_cover_the_cells_exit_is_declined_and_no_leg_is_sent()
-> Result<()> {
    let (americas, europe) = nodes()?;
    let usa = Book {
        venue: "XNYS",
        bid: dec!("99"),
        ask: dec!("100"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    // europe's book shows a one-point spread, which is what americas prices
    // the leg's unwind bound from; europe's gate knows its exit costs five.
    let europe_book = Book {
        venue: "XLON",
        bid: dec!("101"),
        ask: dec!("102"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: Some(dec!("5")),
    };
    let (a, e) = run_until(
        &americas,
        &europe,
        (&usa, &europe_book),
        (true, true),
        |a, e| e.declines >= 1 && a.cycles_rejected >= 1 && a.cycles_expired >= 1,
    );
    assert!(
        a.cycles_originated >= 1,
        "premise: a cycle was offered: {a:?}"
    );
    assert!(
        a.cycles_expired <= a.cycles_originated,
        "more cycles lapsed than were opened: {a:?}"
    );
    assert_eq!(e.cycles_expired, 0, "europe opened nothing: {e:?}");
    assert_eq!(e.reservations, 0, "{e:?}");
    assert_eq!(a.fires, 0, "{a:?}");
    assert_eq!(a.legs_sent + e.legs_sent, 0, "{a:?} {e:?}");
    Ok(())
}

/// Every cycle a coordinator opens leaves the endpoint exactly one way:
/// fired, or lapsed at its TTL unfired. Once both nodes stop authorising and
/// every epoch has outlived its TTL, `fires + cycles_expired` accounts for
/// `cycles_originated` exactly (R12-RES-7095). A fired cycle also reaches
/// its TTL and is dropped by the endpoint the same way an unfired one is;
/// counting it as lapsed would report a cycle that traded as refused.
///
/// Mutation (run): drop the `!fired.contains(opportunity)` test from
/// `Saga::forget_gone` — the fired cycle is counted as lapsed too and the
/// identity is off by the number of fires.
#[test]
fn a_fired_cycle_that_outlives_its_ttl_is_not_counted_as_lapsed() -> Result<()> {
    let (americas, europe) = nodes()?;
    let usa = Book {
        venue: "XNYS",
        bid: dec!("99"),
        ask: dec!("100"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let europe_book = Book {
        venue: "XLON",
        bid: dec!("101"),
        ask: dec!("102"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let (a, _) = run_until(
        &americas,
        &europe,
        (&usa, &europe_book),
        (true, true),
        |a, e| a.fires >= 1 && a.fills >= 1 && e.fills >= 1,
    );
    assert!(a.cycles_originated >= 1, "premise: a cycle opened: {a:?}");
    // Stop authorising, so nothing new opens, and keep publishing the touch
    // past every open epoch's TTL so the endpoint has expired them all.
    let quiet_until = std::time::Instant::now()
        + std::time::Duration::from_nanos(
            u64::try_from(qip_peer_link::saga::EPOCH_TTL.as_nanos()).expect("a positive TTL"),
        )
        + std::time::Duration::from_secs(1);
    while std::time::Instant::now() < quiet_until {
        americas.try_publish(offer(&usa, false));
        europe.try_publish(offer(&europe_book, false));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let a = americas.status();
    assert!(a.fires >= 1, "premise: a cycle fired: {a:?}");
    assert_eq!(
        a.fires + a.cycles_expired,
        a.cycles_originated,
        "every opened cycle either fired or lapsed, once: {a:?}"
    );
    Ok(())
}

/// Poll both ports for the tasks their saga raised, publishing as a decision
/// loop would, until `done` holds of what each has collected so far.
fn collect_tasks(
    americas: &ReflexPort,
    europe: &ReflexPort,
    books: (&Book, &Book),
    done: impl Fn(&[RebalanceTask], &[RebalanceTask]) -> bool,
) -> (Vec<RebalanceTask>, Vec<RebalanceTask>) {
    let (mut a, mut e) = (Vec::new(), Vec::new());
    let began = std::time::Instant::now();
    loop {
        americas.try_publish(offer(books.0, true));
        europe.try_publish(offer(books.1, true));
        a.extend(americas.status().rebalance_new);
        e.extend(europe.status().rebalance_new);
        if done(&a, &e) {
            return (a, e);
        }
        assert!(
            began.elapsed() < PATIENCE,
            "no task arrived within {PATIENCE:?}:\namericas {:?}\neurope {:?}",
            americas.status(),
            europe.status()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// europe's touch fills six of the ten the cycle was priced at, so americas
/// fills ten and unwinds four and both are left four short of the ten they
/// planned. Each node closes the cycle once it knows every fill and reports
/// one task per place that is short, through the port and nothing else
/// (ADR 0154 decision 6; R12-CAPITAL-5067).
///
/// Mutation (run): drop `self.close(session, &mut step)` from `Saga::step` —
/// no cycle closes, no task is raised, and the wait for tasks times out.
#[test]
fn a_closed_epoch_with_drift_raises_one_rebalance_task_per_drifting_place_through_the_port()
-> Result<()> {
    let (americas, europe) = nodes()?;
    let usa = Book {
        venue: "XNYS",
        bid: dec!("99"),
        ask: dec!("100"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let europe_book = Book {
        venue: "XLON",
        bid: dec!("101"),
        ask: dec!("102"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("6"),
        exit_cost: None,
    };
    let (a, e) = collect_tasks(&americas, &europe, (&usa, &europe_book), |a, e| {
        !a.is_empty() && !e.is_empty()
    });
    // Premise: the cycle really ran to fills on both sides before it closed.
    let (sa, se) = (americas.status(), europe.status());
    assert!(sa.fills >= 1 && se.fills >= 1, "{sa:?} {se:?}");
    // The cycle is held for a while, and a later crossing may open another;
    // what must hold is per epoch: each opportunity's tasks are one per
    // place, once, which a close that repeated every turn would break.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!a.is_empty() && !e.is_empty(), "premise: tasks were raised");
    for tasks in [
        [a, americas.status().rebalance_new].concat(),
        [e, europe.status().rebalance_new].concat(),
    ] {
        let mut by_epoch: BTreeMap<(String, u64), Vec<&str>> = BTreeMap::new();
        for task in &tasks {
            by_epoch
                .entry((task.opportunity.clone(), task.epoch_sequence))
                .or_default()
                .push(task.place.as_str());
        }
        assert!(!by_epoch.is_empty(), "premise: tasks were raised");
        for (epoch, places) in &by_epoch {
            assert_eq!(
                places,
                &vec!["americas/XNYS/obj-ACME", "europe/XLON/obj-ACME"],
                "one task for each place epoch {epoch:?} left short, ordered by place, raised once"
            );
        }
        assert!(
            tasks.iter().all(|t| t.amount == dec!("-4")),
            "ten planned, six held after the unwind: {tasks:?}"
        );
    }
    Ok(())
}

/// A cycle both sides filled in full leaves nothing short, so it closes and
/// raises no task: the report is silent when there is nothing to move, and a
/// test that only counted tasks could not tell that from a cycle that never
/// closed, which is why it also asserts the close.
#[test]
fn a_closed_epoch_that_filled_in_full_raises_no_rebalance_task() -> Result<()> {
    let (americas, europe) = nodes()?;
    let usa = Book {
        venue: "XNYS",
        bid: dec!("99"),
        ask: dec!("100"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let europe_book = Book {
        venue: "XLON",
        bid: dec!("101"),
        ask: dec!("102"),
        displayed: dec!("50"),
        authorised: dec!("10"),
        fills: dec!("10"),
        exit_cost: None,
    };
    let (a, e) = run_until(
        &americas,
        &europe,
        (&usa, &europe_book),
        (true, true),
        |a, e| a.cycles_closed >= 1 && e.cycles_closed >= 1,
    );
    assert!(a.fills >= 1 && e.fills >= 1, "{a:?} {e:?}");
    assert_eq!(a.unwinds + e.unwinds, 0, "premise: nothing was unwound");
    assert!(americas.status().rebalance_new.is_empty());
    assert!(europe.status().rebalance_new.is_empty());
    assert_eq!(a.rebalance_tasks + e.rebalance_tasks, 0, "{a:?} {e:?}");
    Ok(())
}
