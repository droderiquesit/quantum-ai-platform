//! The spool writer on a core of its own (R12-FABRIC-7058), proven against
//! the kernel's own account rather than the pin's exit status.
//!
//! Needs Linux's `/proc/thread-self` and util-linux's `taskset`, which the
//! execution node's image carries; on a host without them the pin is refused
//! by name, which is the behaviour `main.rs` stops the process on.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_edge_node::event_fabric::placement::{current_placement, pin_current_thread};

/// A thread pinned to one of the CPUs it was allowed is, by the kernel's
/// account, allowed that CPU alone and runs there; the thread that asked is
/// unaffected, so pinning the writer cannot move the decision thread.
///
/// Mutation: drop the `after.allowed != [cpu]` check and point `taskset` at
/// a different tid (the process's, say) — the spawned thread's own
/// placement still allows every CPU, and the assertion below fails.
#[test]
fn a_pinned_thread_is_allowed_one_core_and_runs_on_it_and_its_parent_is_not_moved() {
    let parent = current_placement().expect("the test thread's placement is readable");
    assert!(
        !parent.allowed.is_empty(),
        "premise: the process may run somewhere"
    );
    let cpu = *parent.allowed.last().expect("non-empty");
    let pinned = std::thread::spawn(move || {
        let placed = pin_current_thread(cpu)?;
        // Yield so the scheduler has moved the thread, then read again.
        for _ in 0..50 {
            std::thread::yield_now();
        }
        Ok::<_, qip_core::error::Error>((placed, current_placement()?))
    })
    .join()
    .expect("the pinning thread did not panic")
    .expect("the pin is applied where taskset and /proc exist");
    let (placed, after) = pinned;
    assert_eq!(placed.allowed, vec![cpu]);
    assert_eq!(after.allowed, vec![cpu]);
    assert_eq!(after.last_cpu, cpu, "the pinned thread ran elsewhere");
    assert_ne!(
        after.tid, parent.tid,
        "premise: a different thread was pinned"
    );
    assert_eq!(
        current_placement().expect("readable").allowed,
        parent.allowed,
        "pinning the writer moved the thread that asked"
    );
}

#[test]
fn a_core_the_process_may_not_run_on_is_refused_before_anything_is_spawned() {
    let error = pin_current_thread(100_000).expect_err("no machine here has that core");
    assert!(
        error.message().starts_with("configuration:"),
        "{}",
        error.message()
    );
}
