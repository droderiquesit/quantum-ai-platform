//! Operation kinds the ledger can book and, by decision, no producer builds
//! (ADR 0153 decisions 3 and 7; R12-LEDGER-5009, R12-LEDGER-5019).
//!
//! The failure this guards is a model's figure entering the ledger as a fact.
//! `review_cross_margin` runs `MarginModel::default`, whose own comment says
//! the repository has never been shown a margin agreement, and the survival
//! governor reads the ledger back through `LedgerObligations` as truth. Tax
//! is the same shape: method, rate and jurisdiction are facts nobody here has
//! been shown. The kinds stay bookable and tested in `qip-contracts`; what
//! must not appear is a non-test construction of one anywhere else in the
//! workspace's source.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::fs;
use std::path::{Path, PathBuf};

/// The kinds no production code may build, spelled as a construction.
const DECLINED: [&str; 3] = [
    "OperationKind::TaxCharge",
    "OperationKind::MarginRequirement",
    "OperationKind::CollateralPledge",
];

fn crates_dir() -> PathBuf {
    qip_acceptance::repository_root().join("backend/crates")
}

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name != "tests" && name != "target" {
                source_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `line` names `token` as a delimited path, not as the prefix of a
/// longer identifier (`OperationKind::TaxChargeX`).
fn names(line: &str, token: &str) -> bool {
    line.match_indices(token).any(|(at, _)| {
        !line[at + token.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

#[test]
fn no_source_outside_the_contract_builds_an_assumed_margin_pledge_or_tax_operation() {
    let mut files = Vec::new();
    for group in fs::read_dir(crates_dir()).unwrap() {
        let group = group.unwrap().path();
        if group.is_dir() {
            source_files(&group, &mut files);
        }
    }

    // Premises first: the walk saw the kernel and the contract, and the
    // contract does define every declined kind, so an empty hit list below is
    // a finding and not a scan that read nothing.
    let platform = files
        .iter()
        .any(|f| f.ends_with("qip-kernel/src/platform.rs"));
    assert!(platform, "the scan must cover the kernel's platform.rs");
    let contract = crates_dir().join("libs/qip-contracts/src/ledger_operation.rs");
    let defined = fs::read_to_string(&contract).unwrap();
    for kind in ["TaxCharge", "MarginRequirement", "CollateralPledge"] {
        assert!(
            names(&defined, &format!("OperationKind::{kind}")),
            "premise: the contract still builds {kind}, so the kind exists to be declined"
        );
    }

    let mut offenders = Vec::new();
    for file in files.iter().filter(|f| **f != contract) {
        let text = fs::read_to_string(file).unwrap();
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for token in DECLINED {
                if names(line, token) {
                    offenders.push(format!("{}:{}: {token}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ADR 0153 declines a producer for margin, collateral and tax: a modelled figure is \
         advice, not an observed fact, and the ledger books only observed facts. Offenders:\n{}",
        offenders.join("\n")
    );
}
