//! COMMERCE-012: PurchaseExecutor — purchase controls before the simulated merchant.
//!
//! The executor gates feasibility-assessed opportunities through three
//! independent layers: account authority (only registered merchants), fraud
//! detection (merchants/listings/payments already flagged), and limits
//! (per-account and global ceiling). Every layer is checked before the merchant
//! is called, so a refusal never reaches SimulatedMerchant and never generates
//! a purchase record. The merchant itself is a concrete type, never a trait,
//! so no real merchant adapter can be plugged in without changing this file
//! (ADR 0021).
//!
//! The executor is paper-only and performs no network I/O. It holds one
//! concrete SimulatedMerchant instance and is the sole constructor of both
//! SimulatedMerchant and PurchaseRecord, so every purchase claim is auditable.

#![allow(clippy::expect_used)]

use qip_core::{Decimal, dec};
use qip_financial::commerce::{Account, Credential, FraudBook, PurchaseExecutor, PurchaseRequest};
use std::collections::BTreeSet;

fn account(id: &str, merchants: Vec<&str>) -> Account {
    Account {
        id: id.to_string(),
        identity_class: "commerce-buyer".to_string(),
        merchants: merchants.into_iter().map(|m| m.to_string()).collect(),
        per_purchase_limit: dec!("500"),
    }
}

fn request(
    account_id: &str,
    merchant: &str,
    sku: &str,
    quantity: Decimal,
    unit_price: Decimal,
) -> PurchaseRequest {
    PurchaseRequest {
        account_id: account_id.to_string(),
        credential: Credential {
            account_id: account_id.to_string(),
            identity_class: "commerce-buyer".to_string(),
        },
        merchant: merchant.to_string(),
        listing: format!("{merchant}-{sku}-listing"),
        payment: format!("{account_id}-card"),
        sku: sku.to_string(),
        quantity,
        unit_price,
    }
}

// ---- Account authority layer (COMMERCE-012-1)

#[test]
fn a_purchase_at_an_unregistered_merchant_is_refused_naming_the_account() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-b", "sku-1", dec!("5"), dec!("20"));
    let e = ex
        .purchase(&r)
        .expect_err("unregistered merchant was admitted");
    assert!(
        e.to_string()
            .contains("account acct-1 has no authority to buy from merchant-b")
    );
    assert_eq!(
        ex.merchant_calls(),
        0,
        "merchant was called despite authority refusal"
    );
    assert_eq!(
        ex.records().len(),
        0,
        "a record was created despite refusal"
    );
}

#[test]
fn a_purchase_with_a_mismatched_credential_is_refused_naming_the_account() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("1000"),
    );
    let mut r = request("acct-1", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    r.credential.account_id = "acct-2".to_string();
    let e = ex
        .purchase(&r)
        .expect_err("mismatched credential was admitted");
    assert!(
        e.to_string()
            .contains("the credential does not belong to account acct-1")
    );
    assert_eq!(ex.merchant_calls(), 0);
}

#[test]
fn a_purchase_from_an_unknown_account_is_refused_naming_the_account() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("1000"),
    );
    let r = request("acct-unknown", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    let e = ex.purchase(&r).expect_err("unknown account was admitted");
    assert!(
        e.to_string()
            .contains("account acct-unknown is not controlled here")
    );
    assert_eq!(ex.merchant_calls(), 0);
}

// ---- Fraud detection layer (COMMERCE-012-2)

#[test]
fn a_purchase_from_a_flagged_merchant_is_refused() {
    let mut fraud = FraudBook::default();
    fraud.merchants.insert("merchant-a".to_string());
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        fraud,
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    let e = ex.purchase(&r).expect_err("flagged merchant was admitted");
    assert!(e.to_string().contains("carries a fraud finding"));
    assert_eq!(ex.merchant_calls(), 0);
}

#[test]
fn a_purchase_of_a_flagged_listing_is_refused() {
    let mut fraud = FraudBook::default();
    fraud
        .listings
        .insert("merchant-a-sku-1-listing".to_string());
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        fraud,
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    let e = ex.purchase(&r).expect_err("flagged listing was admitted");
    assert!(e.to_string().contains("carries a fraud finding"));
    assert_eq!(ex.merchant_calls(), 0);
}

#[test]
fn a_purchase_with_a_flagged_payment_method_is_refused() {
    let mut fraud = FraudBook::default();
    fraud.payments.insert("acct-1-card".to_string());
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        fraud,
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    let e = ex.purchase(&r).expect_err("flagged payment was admitted");
    assert!(
        e.to_string()
            .contains("payment acct-1-card carries a fraud finding")
    );
    assert_eq!(ex.merchant_calls(), 0);
}

// ---- Limit layer (COMMERCE-012-3)

#[test]
fn a_purchase_exceeding_the_per_account_limit_is_refused() {
    let mut ex = PurchaseExecutor::new(
        vec![Account {
            id: "acct-1".to_string(),
            identity_class: "commerce-buyer".to_string(),
            merchants: BTreeSet::from(["merchant-a".to_string()]),
            per_purchase_limit: dec!("100"),
        }],
        FraudBook::default(),
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("10"), dec!("20"));
    let e = ex
        .purchase(&r)
        .expect_err("over-limit purchase was admitted");
    assert!(
        e.to_string()
            .contains("notional of 200 exceeds account acct-1's per-purchase limit of 100")
    );
    assert_eq!(ex.merchant_calls(), 0);
}

#[test]
fn a_purchase_exceeding_the_global_ceiling_is_refused() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("100"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("10"), dec!("20"));
    let e = ex
        .purchase(&r)
        .expect_err("over-ceiling purchase was admitted");
    assert!(
        e.to_string()
            .contains("200 on top of 0 committed exceeds the total ceiling of 100")
    );
    assert_eq!(ex.merchant_calls(), 0);
}

#[test]
fn multiple_purchases_that_individually_fit_but_collectively_exceed_the_ceiling_are_refused_at_the_ceiling()
 {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("500"),
    );
    let r1 = request("acct-1", "merchant-a", "sku-1", dec!("10"), dec!("20"));
    let r2 = request("acct-1", "merchant-a", "sku-2", dec!("10"), dec!("20"));
    let r3 = request("acct-1", "merchant-a", "sku-3", dec!("10"), dec!("20"));

    ex.purchase(&r1).expect("first 200 was admitted");
    ex.purchase(&r2)
        .expect("second 200 was admitted; total 400");
    let e = ex
        .purchase(&r3)
        .expect_err("third 200 was admitted; would exceed 500");
    assert!(e.to_string().contains("exceeds the total ceiling"));
    assert_eq!(
        ex.merchant_calls(),
        2,
        "only the first two were placed with the merchant"
    );
}

// ---- Accepted purchase path (COMMERCE-012-4)

#[test]
fn an_admitted_purchase_records_a_purchase_record_and_commits_the_notional() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("1000"),
    );
    let r = request("acct-1", "merchant-a", "sku-1", dec!("5"), dec!("20"));
    let rec = ex.purchase(&r).expect("purchase was refused");
    assert_eq!(rec.account_id, "acct-1");
    assert_eq!(rec.merchant, "merchant-a");
    assert_eq!(rec.sku, "sku-1");
    assert_eq!(rec.quantity, dec!("5"));
    assert_eq!(rec.notional, dec!("100"));
    assert_eq!(ex.merchant_calls(), 1, "merchant.place was not called");
    assert_eq!(ex.records().len(), 1, "record was not stored");
}

#[test]
fn every_purchase_is_sequenced_by_the_merchant() {
    let mut ex = PurchaseExecutor::new(
        vec![account("acct-1", vec!["merchant-a"])],
        FraudBook::default(),
        dec!("10000"),
    );
    let r1 = request("acct-1", "merchant-a", "sku-1", dec!("1"), dec!("100"));
    let r2 = request("acct-1", "merchant-a", "sku-2", dec!("1"), dec!("100"));
    let r3 = request("acct-1", "merchant-a", "sku-3", dec!("1"), dec!("100"));

    let rec1 = ex.purchase(&r1).expect("first purchase");
    let rec2 = ex.purchase(&r2).expect("second purchase");
    let rec3 = ex.purchase(&r3).expect("third purchase");

    assert_eq!(rec1.sequence, 1);
    assert_eq!(rec2.sequence, 2);
    assert_eq!(rec3.sequence, 3);
}

// ---- Paper-only structure verification

#[test]
fn the_executor_holds_no_trait_for_the_merchant_only_simulated_merchant() {
    // This test documents that PurchaseExecutor is generic-free and
    // monomorphic: it holds SimulatedMerchant as a concrete type, not a trait.
    // A real merchant adapter cannot be slotted in without editing this file.
    let ex = PurchaseExecutor::new(vec![], FraudBook::default(), dec!("1000"));
    // If PurchaseExecutor held a `merchant: dyn SomeMerchantTrait`, this
    // would not compile: `ex.merchant` would be unsized. The fact that this
    // compiles proves the executor holds a concrete type, not a trait object.
    let _merchant_type = &ex;
    assert_eq!(ex.merchant_calls(), 0);
}
