//! Ledger composition root's entry point.
//!
//! ADR 0100 assigns this binary the ledger's role: sole writer of the chain
//! of double-entry postings for fills, and its own read-side API (ADR 0100
//! §2). Every composition root here reads configuration and refuses anything
//! invalid, binds ports and proves storage writable *before* reporting
//! healthy, and only then serves
//! (`.claude/rules/architecture/00-boundaries.md`). This binary cannot do
//! any of that honestly yet: `config`, `consumer`, `read_api` and `store`
//! are doc-only stubs, each waiting on the packet named in its own module
//! comment (see `lib.rs`). Posting a fill against a store nobody proved
//! durable, or serving a read nobody validated, is exactly the failure the
//! composition-root order exists to close — so this process refuses outright
//! rather than starting broken. `docs/adr/0010-what-gets-deployed.md` records
//! why it is excluded from the image matrix and every workload catalogue
//! until that changes.

fn main() {
    eprintln!(
        "qip-ledgerd: refusing to start. ADR 0100 assigns this binary the \
         ledger's role, sole writer of the chain of double-entry postings for \
         fills; its configuration, consumer, read-side API and store modules \
         are doc-only stubs pending the packets that fill them (see \
         backend/crates/apps/qip-ledgerd/src/lib.rs), so there is no \
         validated configuration to read and no store proven writable. \
         Posting a fill against defaults nobody chose would violate the \
         composition-root order every binary in this workspace holds to."
    );
    std::process::exit(1);
}
