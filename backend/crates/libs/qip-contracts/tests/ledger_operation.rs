//! Ledger operations and the subledger accounts they post to
//! (R12-LEDGER-5005..5011, 5016..5021, 5001).
//!
//! Each test names the failure it prevents and the mutation that should make
//! it fail.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::{CapitalGrantFields, CollateralClass, GrantMode};
use qip_contracts::ledger::{Account, Direction, LedgerEvent, Settlement};
use qip_contracts::ledger_operation::{
    Book, CommitmentStage, InventoryEnd, LedgerOperation, LedgerOperationFields, OperationKind,
    Phase, TransferEnd,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::validated::Validated;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp, dec};
use std::collections::{BTreeMap, BTreeSet};

fn book() -> Book {
    Book {
        cell: "newyork-1".to_string(),
        strategy: "alpha".to_string(),
    }
}

fn operation(id: &str, kind: OperationKind) -> LedgerOperationFields {
    LedgerOperationFields {
        schema_version: 1,
        operation_id: id.to_string(),
        origin: "capital-desk".to_string(),
        sequence: 1,
        recorded_at: Timestamp::from_secs(1_790_000_100),
        settlement: Settlement::Simulated,
        kind,
    }
}

fn grant() -> CapitalGrantFields {
    CapitalGrantFields {
        schema_version: 1,
        grant_id: "cg-1".to_string(),
        mode: GrantMode::Paper,
        owner: "research-desk".to_string(),
        mandate: "mandate-paper".to_string(),
        strategy: StrategyId::new("alpha"),
        region: "us-east4".to_string(),
        venue: VenueId::new("SIM-XNYS"),
        amount: dec!("100000"),
        collateral: CollateralClass::Cash,
        max_leverage: dec!("2"),
        risk_budget: dec!("5000"),
        max_drawdown_bp: 800,
        liquidity_reserve: dec!("10000"),
        max_adv_share_bp: 100,
        granted_at: Timestamp::from_secs(1_790_000_000),
        expires_at: Timestamp::from_secs(1_790_086_400),
    }
}

/// One of every kind, with the subledgers its postings must touch and no
/// others.
fn every_kind() -> Vec<(OperationKind, BTreeSet<&'static str>)> {
    let set = |kinds: &[&'static str]| kinds.iter().copied().collect::<BTreeSet<_>>();
    vec![
        (
            OperationKind::TreasuryAllocation {
                treasury: "paper-pool".to_string(),
                book: book(),
                unit: "USD".to_string(),
                amount: dec!("1000"),
                phase: Phase::Establish,
            },
            set(&["trading", "treasury"]),
        ),
        (
            OperationKind::CapitalGrantFunded {
                treasury: "paper-pool".to_string(),
                cell: "newyork-1".to_string(),
                unit: "USD".to_string(),
                grant: Validated::new(grant()).expect("a valid grant"),
            },
            set(&["trading", "treasury"]),
        ),
        (
            OperationKind::CustodyMovement {
                custodian: "sim-custodian".to_string(),
                book: book(),
                unit: "OBJ-ACME".to_string(),
                amount: dec!("10"),
                phase: Phase::Establish,
            },
            set(&["custody", "trading"]),
        ),
        (
            OperationKind::InventoryReservation {
                book: book(),
                unit: "OBJ-ACME".to_string(),
                amount: dec!("5"),
                phase: Phase::Establish,
            },
            set(&["reserved", "trading"]),
        ),
        (
            OperationKind::PhysicalInventory {
                from: InventoryEnd::Counterparty {
                    counterparty: "supplier-a".to_string(),
                },
                to: InventoryEnd::Location {
                    location: "warehouse-1".to_string(),
                },
                unit: "SKU-1".to_string(),
                amount: dec!("3"),
            },
            set(&["inventory", "venue"]),
        ),
        (
            OperationKind::CollateralPledge {
                book: book(),
                counterparty: "sim-prime".to_string(),
                unit: "USD".to_string(),
                amount: dec!("250"),
                phase: Phase::Establish,
            },
            set(&["collateral", "trading"]),
        ),
        (
            OperationKind::MarginRequirement {
                venue: "sim-xnys".to_string(),
                unit: "USD".to_string(),
                amount: dec!("120"),
                phase: Phase::Establish,
            },
            set(&["margin", "obligation"]),
        ),
        (
            OperationKind::Transfer {
                from: TransferEnd::Book {
                    cell: "newyork-1".to_string(),
                    strategy: "alpha".to_string(),
                },
                to: TransferEnd::Book {
                    cell: "london-1".to_string(),
                    strategy: "alpha".to_string(),
                },
                unit: "USD".to_string(),
                amount: dec!("75"),
            },
            set(&["trading"]),
        ),
        (
            OperationKind::TaxLot {
                book: book(),
                lot: "lot-1".to_string(),
                unit: "OBJ-ACME".to_string(),
                quantity: dec!("10"),
                cost_unit: "USD".to_string(),
                cost: dec!("105"),
                acquired_at: Timestamp::from_secs(1_790_000_050),
                phase: Phase::Establish,
            },
            set(&["taxlot", "lotbasis"]),
        ),
        (
            OperationKind::Commitment {
                commitment: "fund-iv".to_string(),
                counterparty: "fund-iv-gp".to_string(),
                book: book(),
                unit: "USD".to_string(),
                amount: dec!("5000"),
                stage: CommitmentStage::Commit,
            },
            set(&["commitment", "obligation"]),
        ),
        (
            OperationKind::TaxCharge {
                book: book(),
                jurisdiction: "us-federal".to_string(),
                unit: "USD".to_string(),
                amount: dec!("12.5"),
            },
            set(&["tax", "trading"]),
        ),
        (
            OperationKind::MandateSubscription {
                mandate: "mandate-paper".to_string(),
                treasury: "paper-pool".to_string(),
                unit: "USD".to_string(),
                amount: dec!("2000"),
                phase: Phase::Establish,
            },
            set(&["mandate", "treasury"]),
        ),
    ]
}

fn net_per_unit(event: &LedgerEvent) -> BTreeMap<String, Decimal> {
    let mut net: BTreeMap<String, Decimal> = BTreeMap::new();
    for posting in event.postings() {
        let entry = net.entry(posting.unit.clone()).or_insert(Decimal::ZERO);
        *entry = match posting.direction {
            Direction::Debit => *entry + posting.amount,
            Direction::Credit => *entry - posting.amount,
        };
    }
    net
}

/// R12-LEDGER-5016..5021's check: an operation posts to its own subledger,
/// and the trial balance still closes. The failure this prevents is a
/// treasury event booked to custody (or anywhere else) by a producer that
/// chose the wrong account: the kind decides the accounts, so the set of
/// subledgers each kind touches is fixed and asserted here.
///
/// Mutation: in `LedgerOperationFields::postings`, make
/// `TreasuryAllocation` credit `Account::Custody` instead of
/// `Account::Treasury` — its row fails with `{custody, trading}`.
#[test]
fn every_operation_kind_posts_to_its_own_subledgers_and_closes_in_every_unit() {
    let kinds = every_kind();
    assert_eq!(kinds.len(), 12, "premise: one of every kind");
    for (i, (kind, expected)) in kinds.into_iter().enumerate() {
        let fields = operation(&format!("op-{i}"), kind);
        let admitted = Validated::new(fields.clone())
            .unwrap_or_else(|e| panic!("premise: operation {i} is admissible: {e}"));
        let event = admitted
            .to_event()
            .expect("an admitted operation builds its event");
        assert!(event.postings().len() >= 2, "operation {i} is double-entry");
        let touched: BTreeSet<&str> = event
            .postings()
            .iter()
            .map(|p| p.account.subledger())
            .collect();
        assert_eq!(touched, expected, "operation {i} touched {touched:?}");
        for (unit, net) in net_per_unit(&event) {
            assert_eq!(net, Decimal::ZERO, "operation {i} does not close in {unit}");
        }
        assert_eq!(event.settlement(), Settlement::Simulated);
        assert_eq!(event.source().order_id, format!("op-{i}"));
        assert_eq!(
            event.source().journal_digest,
            fields.digest().expect("digest"),
            "the event cites the operation it was booked from"
        );
    }
}

/// An unwind is the same accounts swapped, so establishing and then
/// unwinding the same amount nets every account to zero. The failure this
/// prevents is a release that posts to a different account than the
/// reservation it releases, leaving both non-zero.
///
/// Mutation: make `pair` ignore `phase` — the unwind posts the establish's
/// sides again and every account nets to twice the amount.
#[test]
fn an_unwind_reverses_exactly_the_accounts_its_establish_posted() {
    let establish = operation(
        "reserve-1",
        OperationKind::InventoryReservation {
            book: book(),
            unit: "OBJ-ACME".to_string(),
            amount: dec!("5"),
            phase: Phase::Establish,
        },
    );
    let mut unwind = establish.clone();
    unwind.operation_id = "release-1".to_string();
    if let OperationKind::InventoryReservation { phase, .. } = &mut unwind.kind {
        *phase = Phase::Unwind;
    }
    let mut balances: BTreeMap<Account, Decimal> = BTreeMap::new();
    for fields in [establish, unwind] {
        for posting in fields.to_event().expect("builds").postings() {
            let entry = balances.entry(posting.account.clone()).or_default();
            *entry = match posting.direction {
                Direction::Debit => *entry + posting.amount,
                Direction::Credit => *entry - posting.amount,
            };
        }
    }
    assert_eq!(balances.len(), 2, "premise: one pair of accounts");
    assert!(
        balances.values().all(|v| *v == Decimal::ZERO),
        "an establish and its unwind net to zero: {balances:?}"
    );
}

/// Every account renders to text and reads back to itself, and text that
/// is not an account is refused rather than guessed at. The failure this
/// prevents is the one the removed `import_state` had: a parser that split
/// on every `:` and skipped what it could not read, which was every trading
/// account.
///
/// Mutation: in `Account::from_str`, accept `("trading", [cell])` as a
/// trading account with an empty strategy — `trading:cell-only` parses and
/// the refusal loop fails.
#[test]
fn an_account_reads_back_from_its_text_form_and_anything_else_is_refused() {
    let mut accounts = Vec::new();
    for (kind, _) in every_kind() {
        for posting in operation("op", kind).postings() {
            accounts.push(posting.account);
        }
    }
    accounts.push(Account::Fees {
        venue: "sim-xnys".to_string(),
    });
    accounts.push(Account::Accrual {
        accrual_type: "interest".to_string(),
    });
    let kinds: BTreeSet<&str> = accounts.iter().map(Account::subledger).collect();
    assert_eq!(
        kinds.len(),
        16,
        "premise: every subledger is exercised: {kinds:?}"
    );
    for account in &accounts {
        let text = account.to_string();
        let back: Account = text.parse().unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(&back, account, "{text} did not read back to itself");
    }
    for bad in [
        "trading:cell-only",
        "trading:a/b/c",
        "taxlot:a/b",
        "venue:",
        "venue",
        ":x",
        "ledger:x",
        "trading:a:b/c",
        "venue:a/b",
    ] {
        assert!(bad.parse::<Account>().is_err(), "{bad} was accepted");
    }
}

/// An operation read off the wire meets the same refusals as one built in
/// code: a non-positive amount, an unknown field, a foreign schema version,
/// and a name that would make an account ambiguous. The failure this
/// prevents is a derived `Deserialize` that admits what the constructor
/// refuses.
///
/// Mutation: remove the `positive(c, "amount", …)` arm in
/// `LedgerOperationFields::check` *and* `LedgerEvent::new`'s positive-leg
/// rule — the zero-amount wire record decodes. (Either alone still refuses,
/// which is why both layers are named.)
#[test]
fn an_operation_read_off_the_wire_meets_the_same_refusals_as_one_built_in_code() {
    let fields = operation(
        "op-wire",
        OperationKind::TaxCharge {
            book: book(),
            jurisdiction: "us-federal".to_string(),
            unit: "USD".to_string(),
            amount: dec!("12.5"),
        },
    );
    let text = serde_json::to_string(&fields).expect("encodes");
    let back: LedgerOperation = serde_json::from_str(&text).expect("premise: round-trips");
    assert_eq!(back.fields(), &fields);

    let zero = text.replacen("\"12.5\"", "\"0\"", 1);
    assert_ne!(zero, text, "premise: the tamper changed the bytes");
    assert!(serde_json::from_str::<LedgerOperation>(&zero).is_err());

    let unknown = text.replacen("\"origin\"", "\"balance\":\"5\",\"origin\"", 1);
    assert_ne!(unknown, text);
    assert!(serde_json::from_str::<LedgerOperation>(&unknown).is_err());

    let foreign = text.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
    assert_ne!(foreign, text);
    assert!(serde_json::from_str::<LedgerOperation>(&foreign).is_err());

    let ambiguous = text.replacen("us-federal", "us:federal", 1);
    assert_ne!(ambiguous, text);
    assert!(serde_json::from_str::<LedgerOperation>(&ambiguous).is_err());

    let live = text.replacen("\"simulated\"", "\"live\"", 1);
    assert_ne!(live, text, "premise: the settlement is on the wire");
    assert!(
        serde_json::from_str::<LedgerOperation>(&live).is_err(),
        "an operation claiming a live settlement decodes"
    );
}

/// A capital grant is funded for its deployable capital — the reserve
/// stays in treasury — and only while the grant is live. The failure this
/// prevents is a reserve moved into the book it was held back from, or an
/// expired grant funded after the fact.
///
/// Mutation: post `grant.amount` instead of `grant.deployable()` — the
/// funded amount reads 100000 and the first assertion fails; or drop the
/// `is_live` check — the expired funding is admitted.
#[test]
fn a_capital_grant_is_funded_for_its_deployable_capital_and_only_while_it_is_live() {
    let funded = operation(
        "fund-cg-1",
        OperationKind::CapitalGrantFunded {
            treasury: "paper-pool".to_string(),
            cell: "newyork-1".to_string(),
            unit: "USD".to_string(),
            grant: Validated::new(grant()).expect("valid grant"),
        },
    );
    let event = Validated::new(funded.clone())
        .expect("premise: a live grant is funded")
        .to_event()
        .expect("builds");
    let debit = event
        .postings()
        .iter()
        .find(|p| p.direction == Direction::Debit)
        .expect("a debit leg");
    assert_eq!(debit.amount, dec!("90000"));
    assert_eq!(
        debit.account,
        Account::Trading {
            cell: "newyork-1".to_string(),
            strategy: "alpha".to_string()
        }
    );
    let mut late = funded;
    late.recorded_at = Timestamp::from_secs(1_790_086_400);
    assert!(Validated::new(late).is_err(), "an expired grant is funded");
}

/// Movements the platform is not party to, and movements to oneself, are
/// refused rather than booked as a zero-sum no-op.
#[test]
fn a_transfer_to_itself_and_an_inventory_move_between_counterparties_are_refused() {
    let to_self = operation(
        "t-self",
        OperationKind::Transfer {
            from: TransferEnd::Venue {
                venue: "sim-xnys".to_string(),
            },
            to: TransferEnd::Venue {
                venue: "sim-xnys".to_string(),
            },
            unit: "USD".to_string(),
            amount: dec!("1"),
        },
    );
    assert!(Validated::new(to_self).is_err());
    let foreign = operation(
        "i-foreign",
        OperationKind::PhysicalInventory {
            from: InventoryEnd::Counterparty {
                counterparty: "a".to_string(),
            },
            to: InventoryEnd::Counterparty {
                counterparty: "b".to_string(),
            },
            unit: "SKU-1".to_string(),
            amount: dec!("1"),
        },
    );
    assert!(Validated::new(foreign).is_err());
    let merged_lot = operation(
        "lot-merged",
        OperationKind::TaxLot {
            book: book(),
            lot: "lot-2".to_string(),
            unit: "USD".to_string(),
            quantity: dec!("1"),
            cost_unit: "USD".to_string(),
            cost: dec!("1"),
            acquired_at: Timestamp::from_secs(1),
            phase: Phase::Establish,
        },
    );
    assert!(Validated::new(merged_lot).is_err());
}
