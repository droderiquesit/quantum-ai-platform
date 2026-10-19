//! Ledger operations: the economic facts the ledger books that are not fills
//! (R12-LEDGER-5005..5011, 5016..5021).
//!
//! A fill reaches the ledger as a journal outcome and is posted by
//! `qip_portfolio::ledger::post`. Everything else the ledger is authoritative
//! for — treasury funding, custody, reserved and physical inventory,
//! collateral, margin, transfers, tax lots, private commitments, tax and
//! mandate ownership — arrives as one [`LedgerOperation`], and the only way
//! any of it becomes a balance is [`LedgerOperationFields::to_event`]: a
//! balanced, simulated [`LedgerEvent`] whose postings land in the subledger
//! the operation names. There is no "set balance" operation and no field an
//! operation could carry a balance in, so every subledger's balance is a fold
//! of postings and nothing else (R12-LEDGER-5001).
//!
//! # Each kind posts to one subledger, by construction
//!
//! The kinds are typed rather than "debit account A, credit account B" so a
//! treasury allocation cannot be routed to the custody subledger by a
//! producer that filled in the wrong account: the kind decides the accounts.
//! [`Phase::Establish`] creates the state (allocate, deposit, reserve,
//! pledge, call, open, subscribe) and [`Phase::Unwind`] reverses it with the
//! same pair of accounts swapped.
//!
//! # Paper only
//!
//! [`LedgerOperationFields::settlement`] is the ledger's one-variant
//! [`Settlement`]; there is nothing an operation could say to describe a
//! live movement. A funded or recalled capital grant must carry
//! `GrantMode::Paper`, the only mode [`crate::capital::CapitalGrant`] has.
//!
//! # Attribution
//!
//! The event cites the operation: the source's `cell` is the operation's
//! `origin`, its `journal_sequence` is the origin's own `sequence`, its
//! `journal_digest` is the SHA-256 of the operation's canonical bytes, and
//! its `order_id` is the `operation_id`. `session` is zero — an operation is
//! not on a cell's journal chain, and the writer deduplicates it by its id
//! instead. Two different operations claiming one id therefore disagree on
//! the digest, which the writer refuses rather than choosing between.

use crate::capital::{CapitalGrant, GrantMode};
use crate::ledger::{Account, Direction, FeeReport, LedgerEvent, Posting, Settlement, SourceEvent};
use crate::validated::{Contract, Validated, named, positive, refuse};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};

/// Whether an operation creates its state or reverses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Allocate, deposit, reserve, pledge, call, open, subscribe.
    Establish,
    /// The same accounts, swapped: recall, withdraw, release, close, redeem.
    Unwind,
}

/// A trading book: one cell and one strategy, the owner of a
/// `trading:<cell>/<strategy>` account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Book {
    pub cell: String,
    pub strategy: String,
}

impl Book {
    fn trading(&self) -> Account {
        Account::Trading {
            cell: self.cell.clone(),
            strategy: self.strategy.clone(),
        }
    }

    fn reserved(&self) -> Account {
        Account::Reserved {
            cell: self.cell.clone(),
            strategy: self.strategy.clone(),
        }
    }

    fn lot_basis(&self) -> Account {
        Account::LotBasis {
            cell: self.cell.clone(),
            strategy: self.strategy.clone(),
        }
    }

    fn lot(&self, lot: &str) -> Account {
        Account::TaxLot {
            cell: self.cell.clone(),
            strategy: self.strategy.clone(),
            lot: lot.to_string(),
        }
    }
}

/// One end of a [`OperationKind::Transfer`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TransferEnd {
    /// A trading book, in any cell — a transfer between regions is a
    /// transfer between two books in two cells.
    Book { cell: String, strategy: String },
    /// A venue account.
    Venue { venue: String },
    /// A custodian.
    Custody { custodian: String },
}

impl TransferEnd {
    fn account(&self) -> Account {
        match self {
            Self::Book { cell, strategy } => Account::Trading {
                cell: cell.clone(),
                strategy: strategy.clone(),
            },
            Self::Venue { venue } => Account::Venue {
                venue: venue.clone(),
            },
            Self::Custody { custodian } => Account::Custody {
                custodian: custodian.clone(),
            },
        }
    }
}

/// One end of a [`OperationKind::PhysicalInventory`] movement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InventoryEnd {
    /// A place the platform's own units sit.
    Location { location: String },
    /// A supplier or buyer outside the platform, booked against its venue
    /// account.
    Counterparty { counterparty: String },
}

impl InventoryEnd {
    fn account(&self) -> Account {
        match self {
            Self::Location { location } => Account::Inventory {
                location: location.clone(),
            },
            Self::Counterparty { counterparty } => Account::Venue {
                venue: counterparty.clone(),
            },
        }
    }
}

/// Where a private commitment is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStage {
    /// The commitment is made: undrawn commitment against an obligation.
    Commit,
    /// Part of it is called and paid from the book.
    Draw,
    /// Part of it lapses undrawn.
    Release,
}

/// Why the deployable capital of a funded grant is recalled from its book
/// ([`OperationKind::CapitalGrantRecalled`]). Three causes and no fourth,
/// because each is a fact a reader can check against the grant the
/// operation carries: an expiry is at or after `expires_at`, and the other
/// two are inside the window and name the act — the centre's recall order,
/// or the renewal that funded the book under a later grant. A cause the
/// grant's own window contradicts is refused rather than recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapitalRecallCause {
    /// The grant's window ended; recorded at or after `expires_at`.
    Expired,
    /// The centre recalled the grant inside its window (a
    /// `qip_capital::recall::RecallOrder`).
    Revoked,
    /// A later grant funded the same book inside this one's window, so this
    /// one is no longer the grant in force.
    Superseded,
}

impl CapitalRecallCause {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }
}

/// What an operation is. Each kind decides its own accounts; see the module
/// documentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationKind {
    /// Treasury funds a book (Establish: Dr trading, Cr treasury) or recalls
    /// from it (Unwind). R12-LEDGER-5016.
    TreasuryAllocation {
        treasury: String,
        book: Book,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
    /// Treasury funds the book a [`CapitalGrant`] names, inside the grant's
    /// window, for the grant's *deployable* capital — its amount less its
    /// liquidity reserve, which stays in treasury: a reserve moved into the
    /// book is a reserve the book can spend. The book is the grant's strategy
    /// in `cell`. R12-LEDGER-5016.
    CapitalGrantFunded {
        treasury: String,
        cell: String,
        unit: String,
        grant: CapitalGrant,
    },
    /// Treasury recalls the deployable capital of a [`CapitalGrant`] it
    /// funded: the same accounts as [`OperationKind::CapitalGrantFunded`],
    /// swapped (Dr treasury, Cr the grant's book in `cell`), for the same
    /// `grant.deployable()`, so a funding and its recall net the book to
    /// zero by construction rather than by a producer restating the amount.
    /// `cause` says why — see [`CapitalRecallCause`] — and `recorded_at`
    /// must agree with it. Not a `Phase` on the funding, because a funding
    /// is only admissible inside the grant's window and a recall at expiry
    /// is recorded at or after it. R12-LEDGER-5016.
    CapitalGrantRecalled {
        treasury: String,
        cell: String,
        unit: String,
        grant: CapitalGrant,
        cause: CapitalRecallCause,
    },
    /// A book deposits units with a custodian (Establish: Dr custody, Cr
    /// trading) or withdraws them (Unwind). R12-LEDGER-5017.
    CustodyMovement {
        custodian: String,
        book: Book,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
    /// A book reserves inventory (Establish: Dr reserved, Cr trading) or
    /// releases it (Unwind). R12-LEDGER-5005.
    InventoryReservation {
        book: Book,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
    /// Physical units move between locations, or arrive from or leave to a
    /// counterparty (Dr to, Cr from). R12-LEDGER-5018.
    PhysicalInventory {
        from: InventoryEnd,
        to: InventoryEnd,
        unit: String,
        amount: Decimal,
    },
    /// A book pledges units to a counterparty (Establish: Dr collateral, Cr
    /// trading) or has them returned (Unwind). R12-LEDGER-5009.
    CollateralPledge {
        book: Book,
        counterparty: String,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
    /// A venue requires margin (Establish: Dr margin, Cr obligation) or
    /// releases the requirement (Unwind). R12-LEDGER-5009, 5006.
    MarginRequirement {
        venue: String,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
    /// Units move between two books, venues or custodians (Dr to, Cr from).
    /// R12-LEDGER-5010.
    Transfer {
        from: TransferEnd,
        to: TransferEnd,
        unit: String,
        amount: Decimal,
    },
    /// A tax lot is opened or closed: its quantity in `unit` and its cost in
    /// `cost_unit`, both against the book's lot-basis contra. The lot's
    /// holding period starts at `acquired_at`. R12-LEDGER-5008.
    TaxLot {
        book: Book,
        lot: String,
        unit: String,
        quantity: Decimal,
        cost_unit: String,
        cost: Decimal,
        acquired_at: Timestamp,
        phase: Phase,
    },
    /// A private commitment is made, drawn or released. R12-LEDGER-5020,
    /// 5006.
    Commitment {
        commitment: String,
        counterparty: String,
        book: Book,
        unit: String,
        amount: Decimal,
        stage: CommitmentStage,
    },
    /// Tax charged to a book (Dr tax, Cr trading). R12-LEDGER-5019.
    TaxCharge {
        book: Book,
        jurisdiction: String,
        unit: String,
        amount: Decimal,
    },
    /// A mandate subscribes capital into treasury (Establish: Dr treasury, Cr
    /// mandate) or redeems it (Unwind). R12-LEDGER-5021.
    MandateSubscription {
        mandate: String,
        treasury: String,
        unit: String,
        amount: Decimal,
        phase: Phase,
    },
}

/// The fields of a [`LedgerOperation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerOperationFields {
    pub schema_version: u32,
    /// Unique across every origin; the writer's deduplication key.
    pub operation_id: String,
    /// Who recorded the operation: a desk, a region, the capital service.
    pub origin: String,
    /// The origin's own monotonic counter for this operation.
    pub sequence: u64,
    pub recorded_at: Timestamp,
    pub settlement: Settlement,
    pub kind: OperationKind,
}

/// A validated ledger operation. Built only through [`Validated::new`];
/// deserialisation goes through the same refusals, which include building
/// the event it posts.
pub type LedgerOperation = Validated<LedgerOperationFields>;

fn leg(account: Account, direction: Direction, unit: &str, amount: Decimal) -> Posting {
    Posting {
        account,
        direction,
        unit: unit.to_string(),
        amount,
    }
}

/// `amount` of `unit` debited to `debit` and credited to `credit`, swapped on
/// [`Phase::Unwind`].
fn pair(
    debit: Account,
    credit: Account,
    unit: &str,
    amount: Decimal,
    phase: Phase,
) -> [Posting; 2] {
    let (debit, credit) = match phase {
        Phase::Establish => (debit, credit),
        Phase::Unwind => (credit, debit),
    };
    [
        leg(debit, Direction::Debit, unit, amount),
        leg(credit, Direction::Credit, unit, amount),
    ]
}

impl LedgerOperationFields {
    /// The SHA-256 of the operation's canonical `serde_json` bytes: what the
    /// event cites as its source digest.
    pub fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| Error::invalid(format!("a LedgerOperation does not encode: {e}")))?;
        Ok(sha256_hex(&bytes))
    }

    /// The postings this operation books. Every pair is the same unit and
    /// amount on both sides, so the event balances by construction, and
    /// [`LedgerEvent::new`] checks it anyway.
    pub fn postings(&self) -> Vec<Posting> {
        let est = Phase::Establish;
        match &self.kind {
            OperationKind::TreasuryAllocation {
                treasury,
                book,
                unit,
                amount,
                phase,
            } => pair(
                book.trading(),
                Account::Treasury {
                    treasury: treasury.clone(),
                },
                unit,
                *amount,
                *phase,
            )
            .to_vec(),
            OperationKind::CapitalGrantFunded {
                treasury,
                cell,
                unit,
                grant,
            } => pair(
                Account::Trading {
                    cell: cell.clone(),
                    strategy: grant.strategy.as_str().to_string(),
                },
                Account::Treasury {
                    treasury: treasury.clone(),
                },
                unit,
                grant.deployable(),
                est,
            )
            .to_vec(),
            OperationKind::CapitalGrantRecalled {
                treasury,
                cell,
                unit,
                grant,
                ..
            } => pair(
                Account::Trading {
                    cell: cell.clone(),
                    strategy: grant.strategy.as_str().to_string(),
                },
                Account::Treasury {
                    treasury: treasury.clone(),
                },
                unit,
                grant.deployable(),
                Phase::Unwind,
            )
            .to_vec(),
            OperationKind::CustodyMovement {
                custodian,
                book,
                unit,
                amount,
                phase,
            } => pair(
                Account::Custody {
                    custodian: custodian.clone(),
                },
                book.trading(),
                unit,
                *amount,
                *phase,
            )
            .to_vec(),
            OperationKind::InventoryReservation {
                book,
                unit,
                amount,
                phase,
            } => pair(book.reserved(), book.trading(), unit, *amount, *phase).to_vec(),
            OperationKind::PhysicalInventory {
                from,
                to,
                unit,
                amount,
            } => pair(to.account(), from.account(), unit, *amount, est).to_vec(),
            OperationKind::CollateralPledge {
                book,
                counterparty,
                unit,
                amount,
                phase,
            } => pair(
                Account::Collateral {
                    counterparty: counterparty.clone(),
                },
                book.trading(),
                unit,
                *amount,
                *phase,
            )
            .to_vec(),
            OperationKind::MarginRequirement {
                venue,
                unit,
                amount,
                phase,
            } => pair(
                Account::Margin {
                    venue: venue.clone(),
                },
                Account::Obligation {
                    counterparty: venue.clone(),
                },
                unit,
                *amount,
                *phase,
            )
            .to_vec(),
            OperationKind::Transfer {
                from,
                to,
                unit,
                amount,
            } => pair(to.account(), from.account(), unit, *amount, est).to_vec(),
            OperationKind::TaxLot {
                book,
                lot,
                unit,
                quantity,
                cost_unit,
                cost,
                phase,
                ..
            } => {
                let mut postings =
                    pair(book.lot(lot), book.lot_basis(), unit, *quantity, *phase).to_vec();
                postings.extend(pair(
                    book.lot(lot),
                    book.lot_basis(),
                    cost_unit,
                    *cost,
                    *phase,
                ));
                postings
            }
            OperationKind::Commitment {
                commitment,
                counterparty,
                book,
                unit,
                amount,
                stage,
            } => {
                let undrawn = Account::Commitment {
                    commitment: commitment.clone(),
                };
                let owed = Account::Obligation {
                    counterparty: counterparty.clone(),
                };
                match stage {
                    CommitmentStage::Commit => pair(undrawn, owed, unit, *amount, est).to_vec(),
                    CommitmentStage::Release => pair(owed, undrawn, unit, *amount, est).to_vec(),
                    CommitmentStage::Draw => {
                        // The undrawn commitment and the obligation both fall
                        // by the drawn amount, and the book pays it to the
                        // counterparty: four legs, two pairs, one unit.
                        let mut postings = pair(owed, undrawn, unit, *amount, est).to_vec();
                        postings.extend(pair(
                            Account::Venue {
                                venue: counterparty.clone(),
                            },
                            book.trading(),
                            unit,
                            *amount,
                            est,
                        ));
                        postings
                    }
                }
            }
            OperationKind::TaxCharge {
                book,
                jurisdiction,
                unit,
                amount,
            } => pair(
                Account::Tax {
                    jurisdiction: jurisdiction.clone(),
                },
                book.trading(),
                unit,
                *amount,
                est,
            )
            .to_vec(),
            OperationKind::MandateSubscription {
                mandate,
                treasury,
                unit,
                amount,
                phase,
            } => pair(
                Account::Treasury {
                    treasury: treasury.clone(),
                },
                Account::Mandate {
                    mandate: mandate.clone(),
                },
                unit,
                *amount,
                *phase,
            )
            .to_vec(),
        }
    }

    /// The balanced, simulated event this operation books, citing the
    /// operation as its source. Refused, never adjusted, by
    /// [`LedgerEvent::new`]'s rules: an empty name or unit, a `:` or `/`
    /// inside a name, a non-positive amount.
    pub fn to_event(&self) -> Result<LedgerEvent> {
        let event = LedgerEvent::new(
            SourceEvent {
                cell: self.origin.clone(),
                session: 0,
                journal_sequence: self.sequence,
                journal_digest: self.digest()?,
                order_id: self.operation_id.clone(),
            },
            self.settlement,
            FeeReport::Unreported,
            self.postings(),
        )?;
        match self.owner() {
            Some(owner) => event.with_owner(owner),
            None => Ok(event),
        }
    }

    /// Whose capital the operation moves, for the kinds that name exactly one
    /// owner: the treasury that allocates or grants, the mandate that
    /// subscribes. The rest move capital between books or ends and are left
    /// unowned rather than attributed to whichever party happens to be named
    /// first, which would read as a statement and be a guess.
    pub fn owner(&self) -> Option<&str> {
        match &self.kind {
            OperationKind::TreasuryAllocation { treasury, .. }
            | OperationKind::CapitalGrantFunded { treasury, .. }
            | OperationKind::CapitalGrantRecalled { treasury, .. } => Some(treasury),
            OperationKind::MandateSubscription { mandate, .. } => Some(mandate),
            _ => None,
        }
    }
}

impl Contract for LedgerOperationFields {
    const NAME: &'static str = "LedgerOperation";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id or origin; a non-positive amount, quantity or
    /// cost; a transfer or inventory movement from an end to itself, or
    /// between two counterparties (a movement the platform is not party to);
    /// a tax lot whose quantity and cost share a unit (the lot's two balances
    /// would merge into one number); a capital grant funded outside its
    /// window; a capital grant recalled before it was granted, recalled as
    /// expired before its expiry, or recalled as revoked or superseded at or
    /// after it; and then anything the event itself refuses.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "operation_id", &self.operation_id)?;
        named(c, "origin", &self.origin)?;
        match &self.kind {
            OperationKind::TreasuryAllocation { amount, .. }
            | OperationKind::CustodyMovement { amount, .. }
            | OperationKind::InventoryReservation { amount, .. }
            | OperationKind::CollateralPledge { amount, .. }
            | OperationKind::MarginRequirement { amount, .. }
            | OperationKind::Commitment { amount, .. }
            | OperationKind::TaxCharge { amount, .. }
            | OperationKind::MandateSubscription { amount, .. } => {
                positive(c, "amount", *amount)?;
            }
            OperationKind::CapitalGrantFunded { grant, .. } => {
                // `GrantMode` has one variant; matching it by name means a
                // second, live, mode would stop this compiling rather than
                // be funded.
                match grant.mode {
                    GrantMode::Paper => {}
                }
                if !grant.is_live(self.recorded_at) {
                    return refuse(
                        c,
                        format!(
                            "funds grant {} outside its window; a grant is funded between its \
                             grant and its expiry, or not at all",
                            grant.grant_id
                        ),
                    );
                }
            }
            OperationKind::CapitalGrantRecalled { grant, cause, .. } => {
                match grant.mode {
                    GrantMode::Paper => {}
                }
                if self.recorded_at < grant.granted_at {
                    return refuse(
                        c,
                        format!(
                            "recalls grant {} before it was granted; nothing was funded to \
                             recall",
                            grant.grant_id
                        ),
                    );
                }
                // The cause and the instant must tell one story: a reader
                // checking the record against the grant's window must not
                // find an "expiry" recorded while the grant was live, or a
                // "revocation" of a grant that had already lapsed on its own.
                let live = grant.is_live(self.recorded_at);
                match cause {
                    CapitalRecallCause::Expired if live => {
                        return refuse(
                            c,
                            format!(
                                "recalls grant {} as expired at {} while its window runs to {}; a \
                                 grant recalled inside its window is revoked or superseded, not \
                                 expired",
                                grant.grant_id,
                                self.recorded_at.to_rfc3339(),
                                grant.expires_at.to_rfc3339()
                            ),
                        );
                    }
                    CapitalRecallCause::Revoked | CapitalRecallCause::Superseded if !live => {
                        return refuse(
                            c,
                            format!(
                                "recalls grant {} as {} at {} after its window ended at {}; a \
                                 grant past its expiry is recalled as expired",
                                grant.grant_id,
                                cause.as_str(),
                                self.recorded_at.to_rfc3339(),
                                grant.expires_at.to_rfc3339()
                            ),
                        );
                    }
                    _ => {}
                }
            }
            OperationKind::PhysicalInventory {
                from, to, amount, ..
            } => {
                positive(c, "amount", *amount)?;
                if from == to {
                    return refuse(c, "moves inventory from a location to itself");
                }
                if matches!(
                    (from, to),
                    (
                        InventoryEnd::Counterparty { .. },
                        InventoryEnd::Counterparty { .. }
                    )
                ) {
                    return refuse(
                        c,
                        "moves inventory between two counterparties; the platform is not party \
                         to that movement and its ledger does not book it",
                    );
                }
            }
            OperationKind::Transfer {
                from, to, amount, ..
            } => {
                positive(c, "amount", *amount)?;
                if from == to {
                    return refuse(c, "transfers from an account to itself");
                }
            }
            OperationKind::TaxLot {
                unit,
                quantity,
                cost_unit,
                cost,
                ..
            } => {
                positive(c, "quantity", *quantity)?;
                positive(c, "cost", *cost)?;
                if unit == cost_unit {
                    return refuse(
                        c,
                        format!(
                            "states a tax lot whose quantity and cost are both in {unit}; the \
                             lot's quantity and its basis would fold into one balance"
                        ),
                    );
                }
            }
        }
        self.to_event().map(|_| ())
    }
}
