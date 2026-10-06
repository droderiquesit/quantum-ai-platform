//! Accruals: amounts earned or owed over time, accruing on a schedule and
//! cleared by a settlement event (LEDGER-012).
//!
//! An accrual grows as time passes (daily interest on a position, funding costs,
//! etc.) and is recorded as a ledger entry with two postings: one to the accrual
//! account (growing as time passes) and one to the corresponding offset account
//! (e.g., the trading account when interest accrues on a cash position).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::ledger::{
    Account, AccrualEntry, Direction, LedgerEvent, Posting, Settlement, SourceEvent,
};
use qip_core::{Decimal, Duration, Timestamp};

const ACCRUAL_TYPE: &str = "interest";
const UNIT: &str = "USD";
const CELL: &str = "cell-eu-1";
const STRATEGY: &str = "alpha";

fn now() -> Timestamp {
    Timestamp::from_secs(1_790_000_000)
}

fn source() -> SourceEvent {
    SourceEvent {
        cell: CELL.to_string(),
        session: 1,
        journal_sequence: 7,
        journal_digest: "d".to_string(),
        order_id: "o-1".to_string(),
    }
}

fn dec(text: &str) -> Decimal {
    Decimal::parse(text).expect("test literal")
}

#[test]
fn an_accrual_entry_can_be_created_with_a_type_account_unit_and_amount() {
    let accrual = AccrualEntry {
        accrual_type: ACCRUAL_TYPE.to_string(),
        account: Account::Accrual {
            accrual_type: ACCRUAL_TYPE.to_string(),
        },
        unit: UNIT.to_string(),
        amount: dec("10.50"),
        from: now(),
        to: now().saturating_add(Duration::from_secs(86_400)),
        source: source(),
    };
    assert_eq!(accrual.accrual_type, ACCRUAL_TYPE);
    assert_eq!(accrual.amount, dec("10.50"));
    assert_eq!(accrual.unit, UNIT);
}

#[test]
fn an_accrual_posting_to_the_accrual_account_and_trading_account_creates_a_balanced_ledger_event() {
    // An accrual of $10.50 on a USD position creates two postings:
    // Dr accrual:interest  USD $10.50  Cr trading:cell-eu-1/alpha  USD $10.50
    // This leaves the trading account unchanged (debits = credits in USD).
    let trading = Account::Trading {
        cell: CELL.to_string(),
        strategy: STRATEGY.to_string(),
    };
    let accrual_account = Account::Accrual {
        accrual_type: ACCRUAL_TYPE.to_string(),
    };

    let postings = vec![
        Posting {
            account: accrual_account.clone(),
            direction: Direction::Debit,
            unit: UNIT.to_string(),
            amount: dec("10.50"),
        },
        Posting {
            account: trading.clone(),
            direction: Direction::Credit,
            unit: UNIT.to_string(),
            amount: dec("10.50"),
        },
    ];

    let event = LedgerEvent::new(
        source(),
        Settlement::Simulated,
        qip_contracts::ledger::FeeReport::Unreported,
        postings,
    );

    assert!(event.is_ok(), "accrual postings should balance");
    let event = event.unwrap();
    assert_eq!(event.postings().len(), 2);

    // Verify the postings are correct
    let mut accrual_debited = false;
    let mut trading_credited = false;
    for posting in event.postings() {
        if posting.account == accrual_account && posting.direction == Direction::Debit {
            accrual_debited = true;
            assert_eq!(posting.amount, dec("10.50"));
            assert_eq!(posting.unit, UNIT);
        }
        if posting.account == trading && posting.direction == Direction::Credit {
            trading_credited = true;
            assert_eq!(posting.amount, dec("10.50"));
            assert_eq!(posting.unit, UNIT);
        }
    }
    assert!(accrual_debited, "accrual account must be debited");
    assert!(trading_credited, "trading account must be credited");
}

#[test]
fn an_accrual_clearing_event_has_postings_that_reverse_the_accrual_and_debit_cash() {
    // When an accrual is settled (e.g., interest is paid), two postings occur:
    // Dr trading:cell-eu-1/alpha  USD $10.50  (cash paid)
    // Cr accrual:interest  USD $10.50  (accrual cleared)
    let trading = Account::Trading {
        cell: CELL.to_string(),
        strategy: STRATEGY.to_string(),
    };
    let accrual_account = Account::Accrual {
        accrual_type: ACCRUAL_TYPE.to_string(),
    };

    let postings = vec![
        Posting {
            account: trading.clone(),
            direction: Direction::Debit,
            unit: UNIT.to_string(),
            amount: dec("10.50"),
        },
        Posting {
            account: accrual_account.clone(),
            direction: Direction::Credit,
            unit: UNIT.to_string(),
            amount: dec("10.50"),
        },
    ];

    let event = LedgerEvent::new(
        source(),
        Settlement::Simulated,
        qip_contracts::ledger::FeeReport::Unreported,
        postings,
    );

    assert!(event.is_ok(), "accrual clearing postings should balance");
    let event = event.unwrap();

    // Verify the clearing postings reverse the accrual
    let mut trading_debited = false;
    let mut accrual_credited = false;
    for posting in event.postings() {
        if posting.account == trading && posting.direction == Direction::Debit {
            trading_debited = true;
            assert_eq!(posting.amount, dec("10.50"));
        }
        if posting.account == accrual_account && posting.direction == Direction::Credit {
            accrual_credited = true;
            assert_eq!(posting.amount, dec("10.50"));
        }
    }
    assert!(
        trading_debited,
        "trading account must be debited for cash payment"
    );
    assert!(
        accrual_credited,
        "accrual account must be credited to clear it"
    );
}

#[test]
fn an_accrual_display_is_unambiguous() {
    let accrual_account = Account::Accrual {
        accrual_type: ACCRUAL_TYPE.to_string(),
    };
    let display = format!("{}", accrual_account);
    assert_eq!(display, "accrual:interest");

    // Two different accrual types should display differently
    let funding_account = Account::Accrual {
        accrual_type: "funding".to_string(),
    };
    let funding_display = format!("{}", funding_account);
    assert_eq!(funding_display, "accrual:funding");
    assert_ne!(display, funding_display);
}
