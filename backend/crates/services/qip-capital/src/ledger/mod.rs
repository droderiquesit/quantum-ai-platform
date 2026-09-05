//! The per-user, per-strategy ledger (blueprint §43.3, §43.4).
//!
//! The failure this module removes: the platform's books were per strategy
//! and not per user. The centre settles every fill into a strategy's lot and
//! closes the attribution to the last unit (ADR 0007), and then the chain
//! stopped — `Fill → contributor vector → Strategy` — with nothing on the
//! far side to say *whose* capital the strategy was trading. Blueprint §43.4
//! writes the rest of the chain, `→ StrategyFamily → Mandate → User`, and
//! until this module existed nothing in the tree could terminate it. A
//! platform that cannot say which user a fill was for cannot say why it did
//! what it did for that user, which is the one thing it exists to say.
//!
//! What lives here is typed, deterministic state and nothing else:
//!
//! * [`UserId`] and [`Jurisdiction`] — validated newtypes. An identifier
//!   that is empty, padded or unbounded is refused at construction rather
//!   than discovered as two books for one person.
//! * [`Mandate`] — the per-user terms §43.3 names: capital, risk tolerance,
//!   permitted families, liquidity floor, exploration share, jurisdiction.
//!   Every field is validated by name; nothing is clamped.
//! * [`Entitlement`] — a capability evaluated on every request from
//!   jurisdiction, product eligibility, role and mandate. Its withdrawal arm
//!   is [`WithdrawalEntitlement`], a type with one variant, `Refused`, because
//!   ADR 0021 permits the deterministic half of the treasury and refuses the
//!   path by which capital leaves; a granted withdrawal cannot be constructed
//!   here, deserialised here, or reached by any function in this crate.
//! * [`CashBalance`] — currency at a strategy for a user, with
//!   [`ExpectedInflow`]s the user says are on their way and the ledger has
//!   not yet seen. [`CashBalance::available`] excludes them and every
//!   reservation, so a deposit that was announced and never arrived cannot be
//!   spent.
//! * [`UserLedger`] — the books, keyed `(UserId, StrategyId)` in a
//!   [`BTreeMap`](std::collections::BTreeMap) so a report of them is the same
//!   on every machine. Fills reach it as [`AttributedFill`]s — what the
//!   centre's exact attribution said a strategy realised — and are split
//!   across users by [`UserShare`]s that must sum to the fill exactly, or
//!   the whole fill is refused and no book moves.
//!
//! Nothing here reads a clock; every entry point takes the
//! [`qip_core::Timestamp`] it is reasoning about, like the rest of the crate.
//!
//! # Where the chain now ends
//!
//! ```
//! use qip_capital::ledger::{AttributedFill, Mandate, UserId, UserLedger, UserShare};
//! use qip_contracts::signal::StrategyId;
//! use qip_core::{Currency, Timestamp, dec};
//!
//! # fn main() -> qip_core::error::Result<()> {
//! let now = Timestamp::from_secs(1_700_000_000);
//! let alice = UserId::new("alice")?;
//! let mut ledger = UserLedger::new();
//! ledger.enrol(alice.clone(), Mandate::desk(dec!("1000000"), Currency::USD)?)?;
//!
//! // The centre's attribution said `momentum-v3` realised 250 on a fill.
//! let fill = AttributedFill {
//!     strategy: StrategyId::new("momentum-v3"),
//!     source: "cell-lon-1/momentum-v3/obj-AAA".to_string(),
//!     currency: Currency::USD,
//!     amount: dec!("250"),
//! };
//! ledger.journal(&fill, &[UserShare { user: alice.clone(), amount: dec!("250") }], now)?;
//!
//! let balance = ledger
//!     .balance(&alice, &StrategyId::new("momentum-v3"), Currency::USD)
//!     .expect("the fill opened a book");
//! assert_eq!(balance.available(), dec!("250"));
//! # Ok(())
//! # }
//! ```

mod book;
mod cash;
mod entitlement;
mod identity;
mod mandate;

pub use book::{AttributedFill, LedgerKey, StrategyBook, UserLedger, UserShare};
pub use cash::{CashBalance, ExpectedInflow};
pub use entitlement::{Capability, Entitlement, ProductEligibility, Role, WithdrawalEntitlement};
pub use identity::{Jurisdiction, MAX_USER_ID_LENGTH, UserId};
pub use mandate::{Mandate, MandateTerms, PermittedFamilies};
