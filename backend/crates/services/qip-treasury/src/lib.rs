//! `qip-treasury` — the Treasury Bank's books.
//!
//! Extracted from `qip-capital` by ADR 0385, which amends ADR 0127 decision 2.
//! The capital brain decides how much each strategy and cell should get; this
//! crate answers the questions that come before a unit can be promised at
//! all, and it answers them without reading anything the capital brain owns.
//!
//! * [`treasury`] holds four refusals over one fact, that capital the
//!   platform does not have must not be promised: the reserve floor
//!   ([`treasury::CapitalBook`]), the leverage ceiling per strategy and
//!   region ([`treasury::LeverageBook`]), the receivable and payable of a
//!   funding between two of the platform's books
//!   ([`treasury::InternalFunding`]), and the permission a regulated
//!   financing function needs before it runs
//!   ([`treasury::FinancingPermissions`]).
//! * [`funding`] names the source of every unit a grant or placement
//!   requires, and refuses a requirement it cannot fund whole rather than
//!   funding the part that fits.
//!
//! # What is deliberately not here
//!
//! `qip_capital::bank`, the Capital Bank's paper record of collateral
//! posted, currency converted and idle cash routed to yield, stays in
//! `qip-capital`. It drives `ReservationLedger` and `DatedLadder` and reads
//! `Pledge` and `SurvivalReport`. Two of those four carry behaviour, and a
//! type that carries behaviour does not move to `qip-contracts`, so the bank
//! could only have come here by giving this crate an edge to the capital
//! brain. ADR 0385 refuses that edge.
//!
//! # The boundary
//!
//! This crate depends on `qip-core` and `serde` and on no other workspace
//! crate; `qip-capital` does not depend on it. The two meet in the kernel or
//! not at all. `qip-acceptance`'s `treasury_boundary` suite holds both
//! halves.
//!
//! Nothing here reads a wall clock, moves money or contacts a counterparty:
//! every entry point takes the [`qip_core::Timestamp`] it reasons about, and
//! what it produces is a record or a refusal.

pub mod funding;
pub mod treasury;
