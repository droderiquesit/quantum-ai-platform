//! R12-GOV-4077: regulated financing functions run only through a recorded
//! (entity, counterparty, jurisdiction) permission.
//!
//! No regulated function runs today. `qip_capital::funding::plan_funding` is
//! the one place a financing source is drawn on, and it asks
//! `FinancingPermissions::require` before it counts a single unit of one. The
//! risk this suite guards is the next seam: a second place that draws on a
//! financing source — a treasury sweep in the kernel, a repo leg in an app —
//! written without the permission check because the obvious code does not
//! need one. So the property is structural: outside the two files that own
//! the gate, no production source names a financing source or a financing
//! function, and the one seam that does takes the permissions and asks them.
//! A new seam fails here and makes its author route through the gate.

#![allow(clippy::panic_in_result_fn)]

use qip_capital::funding::{
    FundingRequirement, FundingSource, RequirementKind, SourceBalance, plan_funding,
};
use qip_capital::treasury::{FinancingFunction, FinancingPermissions};
use qip_core::Decimal;
use qip_core::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Spellings that construct, match or name a financing source or function.
/// Matched as whole path segments, so `FinancingFunctionality` would not
/// count, and on code rather than comments.
const FINANCING_PATHS: [&str; 2] = ["FundingSource::Financing", "FinancingFunction::"];

/// The two files that own the gate: the enum and registry, and the planner
/// that consults them.
const GATE_FILES: [&str; 2] = [
    "crates/services/qip-capital/src/treasury.rs",
    "crates/services/qip-capital/src/funding.rs",
];

fn backend() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Production source only: every crate's `src/`, minus `#[cfg(test)]`
/// modules, which are cut at their attribute because a test fixture naming a
/// financing source is how the gate's own tests prove it refuses.
fn production_code(text: &str) -> String {
    let code = match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => text,
    };
    code.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn names_financing(code: &str) -> Option<&'static str> {
    FINANCING_PATHS.into_iter().find(|path| {
        code.match_indices(path).any(|(at, _)| {
            let before = code[..at].chars().next_back();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    })
}

#[test]
fn no_production_source_outside_the_gate_names_a_financing_source_or_function() {
    let root = backend();
    let mut files = Vec::new();
    for group in ["libs", "services", "runtime", "apps", "edge"] {
        let dir = root.join("crates").join(group);
        let Ok(crates) = fs::read_dir(&dir) else {
            continue;
        };
        for krate in crates.flatten() {
            rust_files(&krate.path().join("src"), &mut files);
        }
    }
    // Premise: the walk found the workspace, and the planner — which draws
    // on financing sources — is visible to the same matcher. A matcher that
    // sees nothing passes everything. (`treasury.rs` spells its variants as
    // `Self::`, so it is exempt from the walk without being a witness.)
    assert!(files.len() > 100, "only {} source files found", files.len());
    let planner = fs::read_to_string(root.join(GATE_FILES[1])).expect("funding.rs exists");
    assert!(
        names_financing(&production_code(&planner)).is_some(),
        "funding.rs no longer names a financing source; the matcher is blind"
    );

    let mut seams = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .expect("under the workspace")
            .to_string_lossy()
            .replace('\\', "/");
        if GATE_FILES.contains(&relative.as_str()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        if let Some(path) = names_financing(&production_code(&text)) {
            seams.push(format!("{relative} names {path}"));
        }
    }
    assert!(
        seams.is_empty(),
        "a financing seam outside qip_capital::funding::plan_funding: {seams:?}. Route it \
         through plan_funding, which asks FinancingPermissions::require before drawing on a \
         financing source (R12-GOV-4077)"
    );
}

#[test]
fn the_one_financing_seam_takes_the_permissions_and_asks_them_before_drawing() {
    let text = fs::read_to_string(backend().join(GATE_FILES[1])).expect("funding.rs exists");
    let start = text
        .find("pub fn plan_funding(")
        .expect("plan_funding is the seam");
    let body = &text[start..];
    let signature = &body[..body.find("{").expect("a body")];
    assert!(
        signature.contains("permissions: &FinancingPermissions,"),
        "plan_funding no longer takes the permission registry: {signature}"
    );
    let end = body.find("\n}\n").expect("the function closes");
    assert!(
        body[..end].contains(".require("),
        "plan_funding no longer asks FinancingPermissions::require"
    );
}

#[test]
fn an_empty_permission_registry_refuses_every_regulated_financing_source() -> Result<()> {
    let requirement = FundingRequirement {
        id: "grant-1".into(),
        kind: RequirementKind::Grant,
        strategy: "s".into(),
        region: "us".into(),
        amount: Decimal::from_int(100),
    };
    for function in FinancingFunction::ALL {
        let balances = [SourceBalance {
            source: FundingSource::Financing {
                function,
                counterparty: "bank-a".into(),
            },
            available: Decimal::from_int(1_000_000),
        }];
        let empty = plan_funding(
            std::slice::from_ref(&requirement),
            &balances,
            &FinancingPermissions::new(),
            "entity-1",
            "US",
        )?;
        assert!(
            empty.funded.is_empty() && empty.refused.len() == 1,
            "{function:?} funded a requirement with no permission recorded: {empty:?}"
        );

        // The other half: the same source with its exact permission funds it,
        // so the refusal above is the registry and not a planner that refuses
        // everything.
        let mut permitted = FinancingPermissions::new();
        permitted.permit(function, "entity-1", "bank-a", "US");
        let plan = plan_funding(
            std::slice::from_ref(&requirement),
            &balances,
            &permitted,
            "entity-1",
            "US",
        )?;
        assert_eq!(plan.funded.len(), 1, "{function:?} refused when permitted");
    }
    Ok(())
}
