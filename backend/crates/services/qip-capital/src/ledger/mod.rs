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
//! * [`EligibilityRegistry`] — an operator's decision per user that the
//!   user was verified, where, whether they may invest and until when, as an
//!   [`Eligibility`] created or revoked only by an [`EligibilityDecision`]
//!   carrying the [`DecidedBy`] who took it. Consulted by [`UserLedger::fund`]
//!   at the one place capital enters a book and by [`UserLedger::admit`]
//!   before it, refusing by an [`Ineligible`] reason named. There is no
//!   `can_withdraw` on the record — see the module's own comment and ADR
//!   0021.
//! * [`ProductCatalogue`] — which strategy family may be sold in which
//!   jurisdiction, as a [`ProductEligibility`] per family. The product half
//!   of the gate [`Entitlement`] evaluates: the eligibility registry says an
//!   operator verified *this user*, and the catalogue says whether the
//!   *family* their capital is going into was ever cleared where they are.
//!   Empty by default, because that determination is compliance's and an
//!   empty catalogue is the honest record of nobody having taken one. No
//!   `can_withdraw` here either, and see the module's own comment for why a
//!   family cleared in no jurisdiction is refused rather than stored.
//! * [`MandateRegistry`] — the mandates, keyed by [`UserId`] and each under
//!   a [`MandateId`] of its own, admitted against the desk's mandate as a
//!   ceiling term by term and in aggregate. An id seen twice, or a mandate
//!   that promises more than the desk has, is refused with the term named.
//! * [`InvestmentRequest`] and [`InvestmentDecision`] — a user asking for
//!   capital to be put to work, admitted or refused by
//!   [`UserLedger::admit`] before anything downstream exists, the refusal
//!   naming the [`RefusedLimit`] that fired.
//! * [`UserLedger`] — the books, keyed `(UserId, StrategyId)` in a
//!   [`BTreeMap`](std::collections::BTreeMap) so a report of them is the same
//!   on every machine. Fills reach it as [`AttributedFill`]s — what the
//!   centre's exact attribution said a strategy realised — and are split
//!   across users by [`UserShare`]s that must sum to the fill exactly, or
//!   the whole fill is refused and no book moves.
//!   [`UserLedger::pro_rata_shares`] produces such a split from what each
//!   user has at work, as a [`ProRataSplit`] that names where the rounding
//!   remainder went.
//!
//! Nothing here reads a clock; every entry point takes the
//! [`qip_core::Timestamp`] it is reasoning about, like the rest of the crate.
//! Nothing here moves capital off the platform: the withdrawal arm is a
//! type with one variant, and no function here submits, transfers or signs
//! anything (ADR 0021, ADR 0023).
//!
//! # Custody, and which half of it this module can enforce
//!
//! Custody answers two questions: **who holds this**, and **by what authority
//! can it change hands**. Blueprint §37.4 answers them for assets — a table
//! of asset class against custodian and the corridor kinds through which each
//! may ever leave, closing with the rule that three independent enforcement
//! points must agree and that trading authority and transfer authority never
//! share an identity, a credential or a code path. That half lives in
//! `qip_capital_fabric::custody` — named rather than linked, because this
//! crate does not depend on that one and must not — and is enforced where it
//! can be: `CustodyPolicy::permits` is the first of
//! the seven vetoes in the transfer gate. Its remaining half — the policy
//! engine holding a share of a signing key and releasing it on gate approval —
//! is in ADR 0021's **refused** column, beside withdrawal APIs and live venue
//! submission, and the refusal has no phase attached to it. It is not work
//! deferred to a later wave; reaching it would require an owner decision
//! superseding ADR 0003 and rewriting all three paper-trading layers, which
//! is a change of purpose rather than a change of scope.
//!
//! This platform holds no capital, so the custody it *can* enforce is custody
//! of the record: **the set of ways a number can appear in or leave a user's
//! book is closed, named, and every one of them is gated.** That is a
//! property of the books rather than of the money, so a paper-trading
//! platform can hold it in full, and commingling — the custody failure that
//! needs no capital to move in order to happen — is exactly what it prevents.
//!
//! The set, in full:
//!
//! * **In:** [`UserLedger::fund`], which asks the mandate registry, the
//!   [`EligibilityRegistry`] and the investable ceiling first;
//!   [`UserLedger::journal`], [`UserLedger::journal_to`] and
//!   [`UserLedger::journal_pro_rata`], which refuse any split that does not
//!   sum to the attributed fill exactly; and [`UserLedger::post_inflow`],
//!   which refuses a reference nobody declared.
//! * **Out:** nothing. A book is reduced only by a negative
//!   [`AttributedFill`], which is a realised loss and a fact about what
//!   happened. There is no redemption, no transfer and no withdrawal, and
//!   [`WithdrawalEntitlement`] has one variant so that a granted one cannot be
//!   named.
//! * **Neither:** `&mut CashBalance` never leaves this crate — the ledger
//!   hands out shared references only — so no caller outside it can move a
//!   book at all.
//!
//! **`serde` used to be an ungated sixth way in.** [`CashBalance`],
//! [`StrategyBook`] and [`UserLedger`] derived `Deserialize`, so a document
//! naming a settled figure produced one, past every gate above. The rule that
//! closes it is already written one file away, in `entitlement.rs`: *a record
//! is evidence of what was decided, never an input that decides.* It was
//! applied to the entitlement and not to the money. The money types now
//! serialise and do not deserialise, proven by a `compile_fail` doctest
//! beside [`CashBalance`] rather than by a check, because a trait that is not
//! implemented cannot be worked around. ADR 0021 refuses the path by which
//! capital leaves this platform; nothing in it permits one by which capital
//! arrives from a file.
//!
//! `UserLedger`'s `Serialize` went with it, and that is a correction rather
//! than a decision: [`LedgerKey`] is a tuple, `serde_json` refuses a map key
//! that is not a string, and serialising a ledger holding a single book
//! returned `Error("key must be a string")` — while an empty one succeeded,
//! which is why no test caught it. A report of the books is built from
//! [`UserLedger::books`] in key order, which is what `qip-api`'s ledger views
//! already do and what
//! `the_books_report_in_ledger_key_order_however_they_were_funded_and_each_book_reports_its_own_balance`
//! holds.
//!
//! # Where the chain now ends
//!
//! ```
//! use qip_capital::ledger::{AttributedFill, UserId, UserLedger, UserShare};
//! use qip_contracts::signal::StrategyId;
//! use qip_core::{Currency, Timestamp, dec};
//!
//! # fn main() -> qip_core::error::Result<()> {
//! let now = Timestamp::from_secs(1_700_000_000);
//! let desk = UserId::new("desk")?;
//! let mut ledger = UserLedger::with_desk(desk.clone(), dec!("1000000"), Currency::USD)?;
//!
//! // The centre's attribution said `momentum-v3` realised 250 on a fill.
//! let fill = AttributedFill {
//!     strategy: StrategyId::new("momentum-v3"),
//!     source: "cell-lon-1/momentum-v3/obj-AAA".to_string(),
//!     currency: Currency::USD,
//!     amount: dec!("250"),
//! };
//! ledger.journal(&fill, &[UserShare { user: desk.clone(), amount: dec!("250") }], now)?;
//!
//! let balance = ledger
//!     .balance(&desk, &StrategyId::new("momentum-v3"), Currency::USD)
//!     .expect("the fill opened a book");
//! assert_eq!(balance.available(), dec!("250"));
//! # Ok(())
//! # }
//! ```

mod book;
mod cash;
mod eligibility;
mod entitlement;
mod identity;
mod mandate;
mod product;
mod registry;
mod request;

pub use book::{AttributedFill, LedgerKey, ProRataSplit, StrategyBook, UserLedger, UserShare};
pub use cash::{CashBalance, ExpectedInflow};
pub use eligibility::{
    DecidedBy, DecidedByRecord, Eligibility, EligibilityDecision, EligibilityRecord,
    EligibilityRegistry, EligibilityTerms, Ineligible,
};
pub use entitlement::{Capability, Entitlement, ProductEligibility, Role, WithdrawalEntitlement};
pub use identity::{Jurisdiction, MAX_MANDATE_ID_LENGTH, MAX_USER_ID_LENGTH, MandateId, UserId};
pub use mandate::{Mandate, MandateTerms, PermittedFamilies};
pub use product::ProductCatalogue;
pub use registry::{DESK_MANDATE_ID, MandateRegistry, RegisteredMandate, RegistryRecord};
pub use request::{InvestmentDecision, InvestmentOutcome, InvestmentRequest, RefusedLimit};
