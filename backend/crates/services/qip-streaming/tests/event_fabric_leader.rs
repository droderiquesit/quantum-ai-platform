//! FABRIC-076: partition leaders are placed by rendezvous assignment, so a
//! broker joining or leaving moves only the leaderships that must move.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_streaming::event_fabric::partition::leader_for;

const PARTITIONS: u32 = 2_000;

fn broker_names(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("broker-{i}")).collect()
}

fn leaders(brokers: &[String]) -> Vec<String> {
    let names: Vec<&str> = brokers.iter().map(String::as_str).collect();
    (0..PARTITIONS)
        .map(|p| leader_for("orders", p, &names).unwrap().to_string())
        .collect()
}

/// The requirement's own check, over several cluster sizes: adding one broker
/// to an n-broker cluster moves roughly 1/(n+1) of leaderships, and every
/// move goes to the new broker — none between two brokers present both
/// before and after.
///
/// Mutation: in `leader_for`, include `brokers.len()` in the hashed material
/// (an approximation of `hash mod n`, where the count changes every score) —
/// fails, because leaderships then move between old brokers.
#[test]
fn adding_one_broker_moves_about_one_in_n_plus_one_leaderships_and_only_to_the_new_broker() {
    for n in [2usize, 3, 5, 8] {
        let before_brokers = broker_names(n);
        let after_brokers = broker_names(n + 1);
        let before = leaders(&before_brokers);
        let after = leaders(&after_brokers);
        let newcomer = format!("broker-{n}");

        // Premise: every old broker led something, so "no move between old
        // brokers" is a claim about brokers that had leaderships to lose.
        for name in &before_brokers {
            assert!(
                before.contains(name),
                "n={n}: {name} led no partition before"
            );
        }

        let mut moved = 0usize;
        for (p, (b, a)) in before.iter().zip(&after).enumerate() {
            if b != a {
                moved += 1;
                assert_eq!(
                    a, &newcomer,
                    "n={n}: partition {p} moved from {b} to {a}, which was already in the cluster"
                );
            }
        }
        let expected = f64::from(PARTITIONS) / (n as f64 + 1.0);
        let moved_f = moved as f64;
        assert!(
            moved_f > expected * 0.8 && moved_f < expected * 1.2,
            "n={n}: {moved} of {PARTITIONS} moved, expected about {expected:.0}"
        );
    }
}

/// Removing a broker is the same property read backwards: only the
/// partitions it led move.
///
/// Mutation: as above.
#[test]
fn removing_a_broker_moves_only_the_partitions_it_led() {
    let all = broker_names(5);
    let without: Vec<String> = all[..4].to_vec();
    let before = leaders(&all);
    let after = leaders(&without);
    let removed = &all[4];
    assert!(
        before.contains(removed),
        "premise: the removed broker led something"
    );
    for (p, (b, a)) in before.iter().zip(&after).enumerate() {
        if b != removed {
            assert_eq!(b, a, "partition {p} moved though {b} did not leave");
        }
    }
}

#[test]
fn an_empty_duplicate_or_nameless_broker_set_is_refused() {
    assert!(leader_for("orders", 0, &[]).is_err());
    assert!(leader_for("orders", 0, &["a", "a"]).is_err());
    assert!(leader_for("orders", 0, &["a", ""]).is_err());
    assert_eq!(leader_for("orders", 0, &["only"]).unwrap(), "only");
}

/// The answer must not depend on the order the brokers are listed in, or two
/// brokers handed the same set in different orders would disagree on who
/// leads.
#[test]
fn the_leader_does_not_depend_on_the_order_the_brokers_are_listed() {
    for p in 0..200 {
        let forward = leader_for("orders", p, &["a", "b", "c", "d"]).unwrap();
        let reversed = leader_for("orders", p, &["d", "c", "b", "a"]).unwrap();
        assert_eq!(forward, reversed, "partition {p}");
    }
}
