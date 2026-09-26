//! Shared support for the slice suites named in ADR 0100 §8: real processes,
//! a fault-injecting proxy, and a scrape of what they publish.
//!
//! This is a module (`mod support;`), not its own integration-test target —
//! `cargo test` never auto-discovers a file under `tests/support/`, so no
//! sibling crate's binary becomes a dev-dependency of `qip-acceptance` just
//! because this file exists, and the workspace's two-dependency rule is
//! untouched. Each test file that needs it declares `mod support;` and gets
//! its own copy, compiled into that one test binary.
//!
//! Three concerns, three files, because each fails for a different reason and
//! a reader debugging one should not have to read the others:
//!
//! * [`processes`] resolves and spawns the five slice binaries — refusing a
//!   missing or stale one by naming the build command rather than skipping —
//!   and guarantees a spawned child is SIGKILLed when its handle drops, panic
//!   or not.
//! * [`proxy`] sits between two processes and, per connection, injects one of
//!   four faults from a schedule seeded once so a failing run replays rather
//!   than merely repeats.
//! * [`scrape`] is a bounded `GET /metrics`, just enough of HTTP/1.1 to prove
//!   a line came back — never a general client, and never shipped.

pub(crate) mod processes;
pub(crate) mod proxy;
pub(crate) mod scrape;
