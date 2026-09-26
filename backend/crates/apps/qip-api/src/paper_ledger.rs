//! ADR 0100 §2: relays `qip-ledgerd`'s paper-ledger view as decimal text.
//! `qip-ledgerd` is the sole writer of the ledger chain; this crate holds no
//! ledger state (`api_boundary.rs`). SLICE-40.
