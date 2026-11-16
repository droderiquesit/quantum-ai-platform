//! One reader of the peer status keeps the rebalancing tasks (R12-CAPITAL-5067).
//!
//! `PeerStatus::rebalance_new` is consumed by a read. The scheduler once read
//! `port.status()` twice a turn, for the telemetry and for the health body,
//! and each read drained what the other left, so tasks reached a caller that
//! looked at none. The property is that exactly one call in the scheduler
//! reads the status, and that call hands the tasks to the keeper.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

fn scheduler() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/scheduler.rs"))
        .expect("scheduler.rs is readable")
}

#[test]
fn the_scheduler_reads_the_peer_status_once_and_keeps_its_tasks() {
    let source = scheduler();
    let reads = source.matches("port.status()").count();
    assert_eq!(
        reads, 1,
        "the scheduler reads the peer status {reads} times; a second read drains the rebalancing \
         tasks the first one delivered"
    );
    assert!(
        source.contains(".keep(std::mem::take(&mut status.rebalance_new))"),
        "the one read does not hand rebalance_new to the keeper"
    );
}
