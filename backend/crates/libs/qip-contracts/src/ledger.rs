//! The ledger posting contract: one economic event, its double-entry
//! postings, and the settlement state it was booked under (ADR 0100 §8, §9).
//!
//! The posting *logic* lives in `qip-portfolio::ledger`, pure; the ledger
//! *writer* is `qip-ledgerd`. What lives here is the shape both of them and
//! every reader agree on, and the three invariants that shape refuses to
//! represent a violation of:
//!
//! * **Every event balances in every unit** (LEDGER-002, LEDGER-006). Debits
//!   equal credits per unit, exactly, in [`Decimal`]. An unbalanced set is
//!   refused by [`LedgerEvent::new`] — never plugged with a difference — and
//!   deserialisation goes through the same constructor, so an event read off
//!   the wire meets the same refusal as one built in code.
//! * **Every event is simulated** (ADR 0100 §9, the fourth paper fence).
//!   [`Settlement`] has exactly one variant. There is no field on which a
//!   live settlement could be written, so no ledger event can claim one.
//! * **Every event cites the observed fact that caused it** (LEDGER-019,
//!   LEDGER-021). [`SourceEvent`] names the cell, the session, the journal
//!   entry and its digest, and the order; a posting with no source is a guess
//!   the ledger would have no way to reproduce.
//!
//! **There is no update and no delete** (LEDGER-017). Fields are private and
//! the accessors hand out shared references; a correction is a new event that
//! references the original, which is the writer's business, not this type's.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The settlement state a ledger event was booked under.
///
/// **One variant, on purpose.** The platform is paper-trading only, and a
/// ledger whose settlement type could say `Settled` or `Live` is a ledger
/// that could book a live fill if anything upstream let one through. Adding a
/// variant here is widening the paper boundary, and the test
/// `settlement_has_no_variant_but_simulated` refuses it by the wire text a
/// second variant would admit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    /// Filled by the simulated venue. Nothing moved anywhere but here.
    Simulated,
}

/// Which side of an account a posting lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Debit,
    Credit,
}

/// An account in the ledger, by the subledger it belongs to.
///
/// The fill subledgers: the trading subledger holds positions and cash per
/// cell and strategy (LEDGER-007), the venue account is the counterparty a
/// fill's two legs are exchanged with, the fees subledger holds what a venue
/// charged (LEDGER-011), and the accruals subledger holds amounts earned or
/// owed over time before cash moves (LEDGER-012).
///
/// The operation subledgers (R12-LEDGER-5005..5011, 5016..5021), posted only
/// through [`crate::ledger_operation::LedgerOperation`]: treasury, custody,
/// reserved inventory, physical inventory, collateral, margin, obligations,
/// tax lots and their basis contra, private commitments, tax, and mandate
/// ownership. Each is a subledger of the one double-entry journal, not a side
/// table: its balance is a fold of postings and of nothing else, so the
/// global trial balance closes in every unit whichever subledgers an event
/// touched.
///
/// A structured enum rather than a string so a reader cannot mistake one
/// kind for another by a prefix; [`fmt::Display`] renders the `kind:name`
/// text form and [`std::str::FromStr`] reads exactly that form back, which is
/// unambiguous because [`LedgerEvent::new`] refuses a `:` or `/` inside any
/// name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Account {
    /// `trading:<cell>/<strategy>` — who owns the position and the cash.
    Trading { cell: String, strategy: String },
    /// `venue:<venue>` — the other side of every exchange of units.
    Venue { venue: String },
    /// `fees:<venue>` — what the venue reported charging, and nothing else.
    Fees { venue: String },
    /// `accrual:<accrual_type>` — amounts earned or owed over time (interest, funding, etc).
    Accrual { accrual_type: String },
    /// `treasury:<treasury>` — the paper capital pool books are funded from
    /// and mandates subscribe into (R12-LEDGER-5016).
    Treasury { treasury: String },
    /// `custody:<custodian>` — units held at a simulated custodian on the
    /// platform's behalf (R12-LEDGER-5017).
    Custody { custodian: String },
    /// `reserved:<cell>/<strategy>` — trading inventory reserved and not
    /// available to sell twice (R12-LEDGER-5005).
    Reserved { cell: String, strategy: String },
    /// `inventory:<location>` — physical or commerce units by where they sit
    /// (R12-LEDGER-5018).
    Inventory { location: String },
    /// `collateral:<counterparty>` — units pledged to a counterparty
    /// (R12-LEDGER-5009).
    Collateral { counterparty: String },
    /// `margin:<venue>` — the margin a venue requires, debited against the
    /// obligation that requirement creates (R12-LEDGER-5009).
    Margin { venue: String },
    /// `obligation:<counterparty>` — what is owed to a counterparty: a margin
    /// requirement, an undrawn commitment (R12-LEDGER-5006).
    Obligation { counterparty: String },
    /// `taxlot:<cell>/<strategy>/<lot>` — one tax lot's open quantity, in
    /// the asset's unit, and its remaining cost basis, in the cost unit
    /// (R12-LEDGER-5008).
    TaxLot {
        cell: String,
        strategy: String,
        lot: String,
    },
    /// `lotbasis:<cell>/<strategy>` — the contra every lot of a book is
    /// opened against, so a lot's legs balance without moving the book's
    /// own position.
    LotBasis { cell: String, strategy: String },
    /// `commitment:<commitment>` — the undrawn part of a private commitment
    /// (R12-LEDGER-5020).
    Commitment { commitment: String },
    /// `tax:<jurisdiction>` — tax charged, kept apart from venue fees so a
    /// fee report is never reconciled against a tax (R12-LEDGER-5019).
    Tax { jurisdiction: String },
    /// `mandate:<mandate>` — what the platform owes a mandate's owner for
    /// the capital it subscribed (R12-LEDGER-5021).
    Mandate { mandate: String },
}

impl Account {
    fn names(&self) -> Vec<&str> {
        match self {
            Self::Trading { cell, strategy }
            | Self::Reserved { cell, strategy }
            | Self::LotBasis { cell, strategy } => vec![cell, strategy],
            Self::TaxLot {
                cell,
                strategy,
                lot,
            } => vec![cell, strategy, lot],
            Self::Venue { venue } | Self::Fees { venue } | Self::Margin { venue } => vec![venue],
            Self::Accrual { accrual_type } => vec![accrual_type],
            Self::Treasury { treasury } => vec![treasury],
            Self::Custody { custodian } => vec![custodian],
            Self::Inventory { location } => vec![location],
            Self::Collateral { counterparty } | Self::Obligation { counterparty } => {
                vec![counterparty]
            }
            Self::Commitment { commitment } => vec![commitment],
            Self::Tax { jurisdiction } => vec![jurisdiction],
            Self::Mandate { mandate } => vec![mandate],
        }
    }

    /// The subledger this account belongs to: the `kind` before the `:` in
    /// its text form.
    pub fn subledger(&self) -> &'static str {
        match self {
            Self::Trading { .. } => "trading",
            Self::Venue { .. } => "venue",
            Self::Fees { .. } => "fees",
            Self::Accrual { .. } => "accrual",
            Self::Treasury { .. } => "treasury",
            Self::Custody { .. } => "custody",
            Self::Reserved { .. } => "reserved",
            Self::Inventory { .. } => "inventory",
            Self::Collateral { .. } => "collateral",
            Self::Margin { .. } => "margin",
            Self::Obligation { .. } => "obligation",
            Self::TaxLot { .. } => "taxlot",
            Self::LotBasis { .. } => "lotbasis",
            Self::Commitment { .. } => "commitment",
            Self::Tax { .. } => "tax",
            Self::Mandate { .. } => "mandate",
        }
    }

    /// Whether the account holds a quantity that cannot go below zero: units
    /// in custody, reserved, in inventory, pledged, required as margin, left
    /// in a lot, or left undrawn on a commitment.
    ///
    /// The ledger writer refuses an event that would take one of these
    /// negative. The failure it prevents is releasing more than was reserved,
    /// or closing more of a lot than was opened, and booking the difference
    /// as a negative holding nobody can explain.
    pub fn holds_a_quantity(&self) -> bool {
        matches!(
            self,
            Self::Custody { .. }
                | Self::Reserved { .. }
                | Self::Inventory { .. }
                | Self::Collateral { .. }
                | Self::Margin { .. }
                | Self::TaxLot { .. }
                | Self::Commitment { .. }
        )
    }
}

impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.subledger(), self.names().join("/"))
    }
}

impl std::str::FromStr for Account {
    type Err = Error;

    /// Read the `kind:name[/name…]` form [`fmt::Display`] writes, refusing —
    /// never guessing at — anything else: an unknown kind, the wrong number
    /// of names for the kind, or an empty name.
    ///
    /// The failure this prevents is the one `qip-ledgerd`'s removed
    /// `import_state` had: a hand parser that split on every `:` and silently
    /// skipped every key it could not read, which was every key
    /// [`fmt::Display`] produces.
    fn from_str(text: &str) -> Result<Self> {
        let refuse = |why: &str| -> Result<Self> {
            Err(Error::invalid(format!(
                "`{text}` is not a ledger account: {why}; write `kind:name`, for example \
                 `trading:<cell>/<strategy>` or `venue:<venue>`"
            )))
        };
        let Some((kind, rest)) = text.split_once(':') else {
            return refuse("it has no `kind:` prefix");
        };
        let names: Vec<String> = rest.split('/').map(str::to_string).collect();
        if names
            .iter()
            .any(|name| name.is_empty() || name.contains(':'))
        {
            return refuse("a name is empty or contains `:`");
        }
        let account = match (kind, names.as_slice()) {
            ("trading", [cell, strategy]) => Self::Trading {
                cell: cell.clone(),
                strategy: strategy.clone(),
            },
            ("reserved", [cell, strategy]) => Self::Reserved {
                cell: cell.clone(),
                strategy: strategy.clone(),
            },
            ("lotbasis", [cell, strategy]) => Self::LotBasis {
                cell: cell.clone(),
                strategy: strategy.clone(),
            },
            ("taxlot", [cell, strategy, lot]) => Self::TaxLot {
                cell: cell.clone(),
                strategy: strategy.clone(),
                lot: lot.clone(),
            },
            ("venue", [name]) => Self::Venue {
                venue: name.clone(),
            },
            ("fees", [name]) => Self::Fees {
                venue: name.clone(),
            },
            ("margin", [name]) => Self::Margin {
                venue: name.clone(),
            },
            ("accrual", [name]) => Self::Accrual {
                accrual_type: name.clone(),
            },
            ("treasury", [name]) => Self::Treasury {
                treasury: name.clone(),
            },
            ("custody", [name]) => Self::Custody {
                custodian: name.clone(),
            },
            ("inventory", [name]) => Self::Inventory {
                location: name.clone(),
            },
            ("collateral", [name]) => Self::Collateral {
                counterparty: name.clone(),
            },
            ("obligation", [name]) => Self::Obligation {
                counterparty: name.clone(),
            },
            ("commitment", [name]) => Self::Commitment {
                commitment: name.clone(),
            },
            ("tax", [name]) => Self::Tax {
                jurisdiction: name.clone(),
            },
            ("mandate", [name]) => Self::Mandate {
                mandate: name.clone(),
            },
            (
                "trading" | "reserved" | "lotbasis" | "taxlot" | "venue" | "fees" | "margin"
                | "accrual" | "treasury" | "custody" | "inventory" | "collateral" | "obligation"
                | "commitment" | "tax" | "mandate",
                _,
            ) => return refuse("it has the wrong number of `/`-separated names for its kind"),
            _ => return refuse("its kind is not a subledger this ledger keeps"),
        };
        Ok(account)
    }
}

/// One leg of a ledger event: an amount of one unit, on one side of one
/// account. `amount` is strictly positive; the direction carries the sign,
/// so a negative amount cannot silently flip a debit into a credit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    pub account: Account,
    pub direction: Direction,
    /// The asset or currency the posting moves (LEDGER-002): the fill's
    /// object for a position leg, its quote unit for a cash or fee leg.
    pub unit: String,
    pub amount: Decimal,
}

/// What the venue said about the fee on the fill this event books.
///
/// Two states and no default. **`Unreported` is not zero** (LEDGER-019): a
/// venue that said nothing about a fee has not said it was free, and a
/// ledger that booked it as zero has invented a venue fact that no later
/// statement can be reconciled against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeReport {
    /// The venue reported this fee, in the fill's quote unit.
    Reported { amount: Decimal },
    /// The venue reported no fee. No fee posting exists, and none is implied.
    Unreported,
}

/// The observed fact a ledger event was booked from (LEDGER-019, LEDGER-021).
///
/// The journal entry's sequence and digest place the fill on the chain it
/// came from; `order_id` is the next link back toward the intent. A cell and
/// a session with no entry would cite nothing a replay could find.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEvent {
    pub cell: String,
    /// The persisted, monotonic session counter (ADR 0100 §4).
    pub session: u64,
    pub journal_sequence: u64,
    pub journal_digest: String,
    pub order_id: String,
}

/// One immutable, balanced, simulated economic event (CONTRACT-016).
///
/// The only constructor is [`LedgerEvent::new`], and deserialisation is
/// routed through it. Nothing mutates an event once built.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LedgerEventWire")]
pub struct LedgerEvent {
    source: SourceEvent,
    settlement: Settlement,
    fee: FeeReport,
    postings: Vec<Posting>,
    /// Absent from the encoding when not stated, so an event that never had
    /// an owner re-encodes byte for byte as it was stored and the digests
    /// that cover it do not move.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attribution: Option<Attribution>,
}

/// The links from a booked fill back to the decision that caused it, beyond
/// the journal entry [`SourceEvent`] places it on (R12-CONTRACT-7069). Every
/// link is named: a chain with a blank link reads as complete and cannot be
/// followed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub fill_id: String,
    pub order_id: String,
    pub intent_id: String,
    pub decision_id: String,
}

/// The deserialised shape, so an event read from a store or a stream meets
/// the refusals [`LedgerEvent::new`] applies to one built in code. Without
/// it the balance invariant would hold for every event this process wrote
/// and for none it read.
#[derive(Deserialize)]
struct LedgerEventWire {
    source: SourceEvent,
    settlement: Settlement,
    fee: FeeReport,
    postings: Vec<Posting>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    attribution: Option<Attribution>,
}

impl TryFrom<LedgerEventWire> for LedgerEvent {
    type Error = Error;

    fn try_from(wire: LedgerEventWire) -> Result<Self> {
        let mut event = Self::new(wire.source, wire.settlement, wire.fee, wire.postings)?;
        // An event written before ownership existed has neither; one that
        // states either meets the same refusals as one built in code.
        if let Some(owner) = wire.owner {
            event = event.with_owner(owner)?;
        }
        if let Some(attribution) = wire.attribution {
            event = event.with_attribution(attribution)?;
        }
        Ok(event)
    }
}

fn refuse_unnamed(what: &str, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::invalid(format!(
            "a ledger event needs a non-empty {what}; an unattributed posting cannot be traced \
             to the fact that caused it"
        )));
    }
    Ok(())
}

impl LedgerEvent {
    /// The id of the decision chain this posting belongs to, derived from its
    /// source exactly as the cell and the spool writer derive it
    /// ([`crate::reflex::chain_correlation`]). Not stored, so an event read
    /// back from a store names the same chain it did when it was written.
    pub fn chain_correlation(&self) -> qip_core::CorrelationId {
        crate::reflex::chain_correlation(
            &self.source.cell,
            self.source.session,
            &self.source.order_id,
        )
    }

    /// Build an event, refusing anything the ledger must not hold.
    ///
    /// Refused, never adjusted:
    ///
    /// * fewer than two postings, or a posting of zero or negative amount —
    ///   a one-legged or empty entry is not double-entry, and a signed amount
    ///   would let a sign error flip a side silently;
    /// * any unit whose debits and credits differ by any amount at all, or
    ///   whose sums overflow — the difference is reported, never posted to a
    ///   suspense account to make the set close;
    /// * postings to a fees account that do not equal the reported fee, or
    ///   any fees posting when the fee is unreported — a fee the venue did
    ///   not report is not a fee the ledger may book;
    /// * an empty name, unit or source field, or a `:` or `/` inside an
    ///   account name, which would make two different accounts render to the
    ///   same text.
    pub fn new(
        source: SourceEvent,
        settlement: Settlement,
        fee: FeeReport,
        postings: Vec<Posting>,
    ) -> Result<Self> {
        refuse_unnamed("source cell", &source.cell)?;
        refuse_unnamed("source journal digest", &source.journal_digest)?;
        refuse_unnamed("source order id", &source.order_id)?;
        if postings.len() < 2 {
            return Err(Error::invalid(format!(
                "a ledger event needs at least two postings to be double-entry; got {}",
                postings.len()
            )));
        }

        // Per unit: (debits, credits). BTreeMap so the first imbalance named
        // is the same on every run, and a replayed refusal reads identically.
        let mut sums: BTreeMap<&str, (Decimal, Decimal)> = BTreeMap::new();
        let mut fees_debited = Decimal::ZERO;
        let mut fees_credited = Decimal::ZERO;
        let mut touches_fees = false;
        for posting in &postings {
            refuse_unnamed("posting unit", &posting.unit)?;
            for name in posting.account.names() {
                refuse_unnamed("account name", name)?;
                if name.contains(':') || name.contains('/') {
                    return Err(Error::invalid(format!(
                        "account name `{name}` contains `:` or `/`, which would make `{}` \
                         ambiguous as text; rename the cell, strategy or venue",
                        posting.account
                    )));
                }
            }
            if !posting.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "posting to {} of {} {} is not positive; the direction carries the \
                     sign, and a zero leg records nothing that happened",
                    posting.account, posting.amount, posting.unit
                )));
            }
            let overflow = || {
                Error::numeric(format!(
                    "the {} postings overflow when summed; the event is refused rather \
                     than booked with a wrapped total",
                    posting.unit
                ))
            };
            let entry = sums
                .entry(posting.unit.as_str())
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            match posting.direction {
                Direction::Debit => {
                    entry.0 = entry.0.checked_add(posting.amount).ok_or_else(overflow)?
                }
                Direction::Credit => {
                    entry.1 = entry.1.checked_add(posting.amount).ok_or_else(overflow)?
                }
            }
            if let Account::Fees { .. } = posting.account {
                touches_fees = true;
                match posting.direction {
                    Direction::Debit => {
                        fees_debited = fees_debited
                            .checked_add(posting.amount)
                            .ok_or_else(overflow)?;
                    }
                    Direction::Credit => {
                        fees_credited = fees_credited
                            .checked_add(posting.amount)
                            .ok_or_else(overflow)?;
                    }
                }
            }
        }
        for (unit, (debits, credits)) in &sums {
            if debits != credits {
                return Err(Error::invalid(format!(
                    "unbalanced in {unit}: debits {debits}, credits {credits}; the ledger \
                     refuses the set rather than plugging the difference"
                )));
            }
        }

        match fee {
            FeeReport::Unreported if touches_fees => {
                return Err(Error::invalid(
                    "the fee is unreported but the event posts to a fees account; a fee the \
                     venue did not report is not a fee the ledger may book",
                ));
            }
            FeeReport::Unreported => {}
            FeeReport::Reported { amount } => {
                if amount.is_negative() {
                    return Err(Error::invalid(format!(
                        "reported fee {amount} is negative; a rebate needs its own posting \
                         rule, not a negative fee"
                    )));
                }
                let net = fees_debited
                    .checked_sub(fees_credited)
                    .ok_or_else(|| Error::numeric("the fees postings overflow when netted"))?;
                if net != amount {
                    return Err(Error::invalid(format!(
                        "the fees postings net to {net} but the venue reported {amount}; \
                         the ledger books the reported fee, not another figure"
                    )));
                }
            }
        }

        Ok(Self {
            source,
            settlement,
            fee,
            postings,
            owner: None,
            attribution: None,
        })
    }

    /// State whose capital this event moved: a desk or mandate id. Refused
    /// when blank — an owner that reads as stated and names nobody is worse
    /// than none — or when it holds a `:` or `/`, which would make an
    /// owner-qualified account render ambiguously like any account name.
    pub fn with_owner(mut self, owner: impl Into<String>) -> Result<Self> {
        let owner = owner.into();
        refuse_unnamed("owner", &owner)?;
        if owner.contains(':') || owner.contains('/') {
            return Err(Error::invalid(format!(
                "owner `{owner}` contains `:` or `/`, which would make it ambiguous as text;                  rename the desk or mandate"
            )));
        }
        self.owner = Some(owner);
        Ok(self)
    }

    /// State the chain from the booked fill back to the decision. Refused when
    /// any link is blank, and when its order is not the order the source event
    /// names: two claims about the same order will disagree, and the louder
    /// one will be wrong.
    pub fn with_attribution(mut self, attribution: Attribution) -> Result<Self> {
        refuse_unnamed("attribution fill id", &attribution.fill_id)?;
        refuse_unnamed("attribution order id", &attribution.order_id)?;
        refuse_unnamed("attribution intent id", &attribution.intent_id)?;
        refuse_unnamed("attribution decision id", &attribution.decision_id)?;
        if attribution.order_id != self.source.order_id {
            return Err(Error::invalid(format!(
                "the attribution chain names order `{}` but the source event names `{}`; \
                 one fill has one order",
                attribution.order_id, self.source.order_id
            )));
        }
        self.attribution = Some(attribution);
        Ok(self)
    }

    /// Whose capital this event moved, when it was stated.
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// The chain back to the deciding intent, when it was stated.
    pub fn attribution(&self) -> Option<&Attribution> {
        self.attribution.as_ref()
    }

    pub fn source(&self) -> &SourceEvent {
        &self.source
    }

    pub fn settlement(&self) -> Settlement {
        self.settlement
    }

    pub fn fee(&self) -> FeeReport {
        self.fee
    }

    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }
}

/// An accrual entry: an amount earned or owed over time, accruing on a schedule
/// and cleared by a settlement event (LEDGER-012).
///
/// An accrual grows as time passes (daily interest on a position, funding costs,
/// etc.) and is posted to the ledger both as it accrues (increasing the accrual
/// account balance) and as it is cleared (when a cash settlement removes the
/// accrued amount).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccrualEntry {
    /// The type of accrual (e.g., "interest", "funding").
    pub accrual_type: String,
    /// The account that holds the accrued amount.
    pub account: Account,
    /// The unit (currency) the accrual is in.
    pub unit: String,
    /// The amount accrued so far.
    pub amount: Decimal,
    /// When the accrual started.
    pub from: Timestamp,
    /// When the accrual ends or is expected to clear.
    pub to: Timestamp,
    /// The observed fact that caused this accrual (not updated once set).
    pub source: SourceEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceEvent {
        SourceEvent {
            cell: "cell-a".to_string(),
            session: 1,
            journal_sequence: 7,
            journal_digest: "d".to_string(),
            order_id: "o-1".to_string(),
        }
    }

    fn leg(account: Account, direction: Direction, unit: &str, amount: &str) -> Posting {
        Posting {
            account,
            direction,
            unit: unit.to_string(),
            amount: Decimal::parse(amount).expect("test literal"),
        }
    }

    fn trading() -> Account {
        Account::Trading {
            cell: "cell-a".to_string(),
            strategy: "alpha".to_string(),
        }
    }

    fn venue() -> Account {
        Account::Venue {
            venue: "sim".to_string(),
        }
    }

    fn balanced() -> Vec<Posting> {
        vec![
            leg(trading(), Direction::Debit, "USD", "10.5"),
            leg(venue(), Direction::Credit, "USD", "10.5"),
        ]
    }

    fn build(source: SourceEvent, postings: Vec<Posting>) -> Result<LedgerEvent> {
        LedgerEvent::new(
            source,
            Settlement::Simulated,
            FeeReport::Unreported,
            postings,
        )
    }

    /// LEDGER-5002's other two refusals, which had no test: an empty or
    /// one-legged entry is not double-entry, and a zero or negative amount
    /// would let a sign error flip a side silently. Premise first: the
    /// balanced pair is accepted.
    ///
    /// Mutation: lower the posting-count floor to one — the one-legged
    /// assertion passes the constructor and fails; drop the positivity check —
    /// the zero and negative assertions fail.
    #[test]
    fn fewer_than_two_postings_or_a_non_positive_amount_is_refused() {
        assert!(build(source(), balanced()).is_ok(), "premise");

        let empty = build(source(), Vec::new()).expect_err("an empty event must be refused");
        assert!(
            empty.to_string().contains("at least two postings"),
            "refused for the wrong reason: {empty}"
        );
        let one_legged = build(
            source(),
            vec![leg(trading(), Direction::Debit, "USD", "10.5")],
        )
        .expect_err("a one-legged event must be refused");
        assert!(
            one_legged.to_string().contains("at least two postings"),
            "refused for the wrong reason: {one_legged}"
        );

        for amount in ["0", "-10.5"] {
            let signed = build(
                source(),
                vec![
                    leg(trading(), Direction::Debit, "USD", amount),
                    leg(venue(), Direction::Credit, "USD", amount),
                ],
            )
            .expect_err("a non-positive posting must be refused");
            assert!(
                signed.to_string().contains("is not positive"),
                "{amount}: refused for the wrong reason: {signed}"
            );
        }
    }

    /// LEDGER-5012: an event without a source is refused, field by field —
    /// the cell, the journal digest and the order id (session and sequence
    /// are numbers and cannot be absent). An unattributed posting cannot be
    /// reproduced from the journal that is supposed to have caused it.
    ///
    /// Mutation: remove the `refuse_unnamed` call for the journal digest — the
    /// digest case is accepted and its assertion fails.
    #[test]
    fn an_event_whose_source_names_no_cell_digest_or_order_is_refused() {
        assert!(build(source(), balanced()).is_ok(), "premise");
        let blank = |field: &str| {
            let mut source = source();
            match field {
                "cell" => source.cell = String::new(),
                "digest" => source.journal_digest = String::new(),
                "order" => source.order_id = String::new(),
                _ => unreachable!("test bug"),
            }
            source
        };
        for (field, named) in [
            ("cell", "source cell"),
            ("digest", "source journal digest"),
            ("order", "source order id"),
        ] {
            let refused = build(blank(field), balanced())
                .expect_err("an event without a source must be refused");
            assert!(
                refused.to_string().contains(named),
                "{field}: refused for the wrong reason: {refused}"
            );
        }
    }

    #[test]
    fn a_set_unbalanced_by_one_nano_unit_is_refused_and_not_plugged() {
        // The failure this prevents: a ledger that closes a rounding
        // difference by posting it somewhere, so a real break later reads as
        // accumulated noise. Premise first: the balanced twin is accepted.
        let balanced = vec![
            leg(trading(), Direction::Debit, "USD", "10.5"),
            leg(venue(), Direction::Credit, "USD", "10.5"),
        ];
        assert!(
            LedgerEvent::new(
                source(),
                Settlement::Simulated,
                FeeReport::Unreported,
                balanced
            )
            .is_ok(),
            "premise: a balanced pair is accepted"
        );
        let off_by_one = vec![
            leg(trading(), Direction::Debit, "USD", "10.5"),
            leg(venue(), Direction::Credit, "USD", "10.500000001"),
        ];
        let refused = LedgerEvent::new(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            off_by_one,
        )
        .expect_err("an unbalanced set must be refused");
        assert!(
            refused.to_string().contains("unbalanced in USD"),
            "refused for the wrong reason: {refused}"
        );
    }

    #[test]
    fn an_event_read_off_the_wire_meets_the_same_balance_refusal() {
        // Deserialisation is routed through `new`; a derived `Deserialize`
        // would have admitted this bytes-for-bytes.
        let good = LedgerEvent::new(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            vec![
                leg(trading(), Direction::Debit, "USD", "3"),
                leg(venue(), Direction::Credit, "USD", "3"),
            ],
        )
        .expect("balanced");
        let text = serde_json::to_string(&good).expect("serialises");
        let back: LedgerEvent = serde_json::from_str(&text).expect("premise: round-trips");
        assert_eq!(back, good);
        let tampered = text.replacen("\"amount\":\"3\"", "\"amount\":\"4\"", 1);
        assert_ne!(tampered, text, "premise: the tamper changed the bytes");
        let refused = serde_json::from_str::<LedgerEvent>(&tampered)
            .expect_err("an unbalanced event on the wire must not deserialise");
        assert!(
            refused.to_string().contains("unbalanced in USD"),
            "refused for the wrong reason: {refused}"
        );
    }
}
