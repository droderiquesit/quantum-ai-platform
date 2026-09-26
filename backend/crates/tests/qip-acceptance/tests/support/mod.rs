//! Shared support for the slice suites named in ADR 0100 §8: real processes,
//! a fault-injecting relay between them, and a scrape of what they publish.
//!
//! This is a module (`mod support;`), not its own integration-test target —
//! `cargo test` never auto-discovers a file under `tests/support/`, so no
//! sibling crate's binary becomes a dev-dependency of `qip-acceptance` just
//! because this file exists, and the workspace's two-dependency rule is
//! untouched. Each test file that needs it declares `mod support;` and gets
//! its own copy, compiled into that one test binary.
//!
//! Because each suite compiles its own copy, each uses a different subset —
//! the degradation suite drives the proxy, the happy path never does — and
//! under CI's `-D warnings` an item one suite leaves unused would be a
//! `dead_code` error in that suite alone. The allowance is therefore declared
//! once, below, so a consumer needs nothing but `mod support;`. The price,
//! stated: an item no suite uses is not reported here either.
//!
//! Three concerns, three files, because each fails for a different reason and
//! a reader debugging one should not have to read the others:
//!
//! * [`processes`] resolves and spawns the five slice binaries — refusing a
//!   missing or stale one by naming the build command rather than skipping —
//!   parses the address each announces, and guarantees a spawned child is
//!   SIGKILLed when its handle drops, panic or not, printing what it said.
//! * [`proxy`] relays between two processes and, on command, stalls,
//!   blackholes, drops the replies of, or cuts every connection through it,
//!   stepping through a schedule seeded once so a failing run replays rather
//!   than merely repeats.
//! * [`scrape`] is a bounded `GET` of `/metrics` and an exact reading of one
//!   series from it — never a general client, and never shipped.
//!
//! [`poll_until`] is the one way a suite waits: on an observable condition,
//! with a deadline its failure names. A sleep is not a wait here.

#![allow(
    dead_code,
    reason = "each suite compiles its own copy of this module and uses a different subset of it"
)]

pub(crate) mod processes;
pub(crate) mod proxy;
pub(crate) mod scrape;

use std::time::{Duration, Instant};

/// How often [`poll_until`] looks again. The deadline bounds the wait; this
/// only bounds how late the wait notices.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Call `probe` until it yields a value and return it, or panic naming `what`
/// and `deadline` once the deadline passes.
///
/// A fixed sleep is either too short on a loaded runner — ADR 0098 measured
/// the reference desktop running 169 worktrees — or wastes the difference on
/// every run; a poll on the condition itself is neither.
pub(crate) fn poll_until<T>(
    what: &str,
    deadline: Duration,
    mut probe: impl FnMut() -> Option<T>,
) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            started.elapsed() < deadline,
            "{what} did not happen within {deadline:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}
