//! Event fabric composition root's entry point.
//!
//! ADR 0100 assigns this binary the event-fabric broker's role: sole writer
//! of every partition's batch chain, running its own clock for segment roll,
//! retention and archive (ADR 0100 §2). Every composition root here reads
//! configuration and refuses anything invalid, binds ports and proves
//! storage writable *before* reporting healthy, and only then serves
//! (`.claude/rules/architecture/00-boundaries.md`). This binary cannot do
//! any of that honestly yet: `config`, `health` and `archiver` are doc-only
//! stubs, each waiting on the packet named in its own module comment (see
//! `lib.rs`). Serving on a configuration nobody validated, or acknowledging
//! a write to a store nobody proved durable, is exactly the failure the
//! composition-root order exists to close — so this process refuses outright
//! rather than starting broken. `docs/adr/0010-what-gets-deployed.md` records
//! why it is excluded from the image matrix and every workload catalogue
//! until that changes.

fn main() {
    eprintln!(
        "qip-fabricd: refusing to start. ADR 0100 assigns this binary the \
         event-fabric broker's role, sole writer of every partition's batch \
         chain; its configuration, health and archiver modules are doc-only \
         stubs pending the packets that fill them (see \
         backend/crates/apps/qip-fabricd/src/lib.rs), so there is no \
         validated configuration to read and no store proven writable. \
         Serving on defaults nobody chose would violate the composition-root \
         order every binary in this workspace holds to."
    );
    std::process::exit(1);
}
