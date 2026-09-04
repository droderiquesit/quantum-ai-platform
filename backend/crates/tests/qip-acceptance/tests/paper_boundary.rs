//! Layer two of the paper-trading boundary, which nothing tested.
//!
//! `.claude/rules/01-security-and-safety.md` names three independent layers
//! and says none of them may be weakened, bypassed or temporarily disabled.
//! Layer one — Terraform refusing a live ceiling at plan time — is proved by
//! `infrastructure.rs`. Layer three — a runtime raise refused by the autonomy
//! controller — is proved by `acceptance.rs`. Layer two is
//! `AutonomyLevel::deployable`, and
//! `grep -rn "deployable" backend/crates/services/qip-risk-engine/` returns
//! one line: its own definition. Nothing asserted that it refuses a live rung,
//! and nothing asserted that the composition roots reading
//! `QIP_AUTONOMY_CEILING` route the configured value through that refusal
//! rather than through `AutonomyLevel::parse`, which admits every rung on the
//! ladder.
//!
//! That distinction is a one-word edit. Changing `deployable(` to `parse(` in
//! a composition root deletes a documented layer of the boundary: the process
//! starts at `autonomous_live`, and layer one never sees a ConfigMap edited
//! past review. Before this file, every test in the workspace still passed.
//!
//! The unit-level half of this belongs beside the code, in
//! `qip-risk-engine/src/autonomy.rs`, and is named as a follow-up. It is
//! asserted here because the layer is a claim about a type and three binaries
//! at once, and this crate is the first place that can see all four.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read};
use qip_risk_engine::autonomy::AutonomyLevel;

// --- the refusal itself ------------------------------------------------------

#[test]
fn a_configured_live_ceiling_stops_the_deployment_instead_of_being_lowered_to_paper() {
    // Silently lowering is the failure this exists to prevent, and it is worse
    // than refusing: an operator who configured live trading and got a running
    // paper process believes something false about their deployment, and the
    // next thing they do is act on that belief.
    let ladder = AutonomyLevel::all();
    // The premise, both halves. A ladder with no live rung would make the
    // refusal loop vacuous; a ladder with no admissible rung would make the
    // admission loop vacuous, and a `deployable` that refused everything would
    // then pass — a layer nobody can deploy behind is not a layer.
    assert!(
        ladder.iter().any(AutonomyLevel::is_live),
        "no rung on the ladder is live; this test's premise needs rewriting"
    );
    assert!(
        ladder.iter().any(|level| !level.is_live()),
        "every rung on the ladder is live; this test's premise needs rewriting"
    );

    for level in ladder {
        let configured = level.as_str();
        if level.is_live() {
            let refusal = AutonomyLevel::deployable(Some(configured))
                .expect_err("a live ceiling must stop the process, not start it");
            assert!(
                refusal.message().contains("paper-trading only"),
                "{configured} was refused, but for some other reason: {}",
                refusal.message()
            );
            // The quoted token, not the bare name. `limited_autonomous_live`
            // contains `autonomous_live`, so a bare `contains` would accept a
            // refusal that named the wrong rung — the exact class of defect
            // this repository has already been caught by once.
            assert!(
                refusal.message().contains(&format!("'{configured}'")),
                "the refusal does not name the level it refused: {}",
                refusal.message()
            );
        } else {
            assert_eq!(
                AutonomyLevel::deployable(Some(configured))
                    .expect("a level below live is admitted"),
                level,
                "{configured} is below live and must be admitted unchanged"
            );
        }
    }

    // Absent configuration is the shipped default, not an error: a deployment
    // that says nothing about its ceiling gets paper trading.
    assert_eq!(
        AutonomyLevel::deployable(None).expect("absent configuration is the default"),
        AutonomyLevel::PaperTrading
    );
}

#[test]
fn the_delimited_check_on_the_refusal_would_reject_a_message_naming_the_neighbouring_rung() {
    // Written out rather than left to a manual mutation, because the mutation
    // that matters here is one a bare `contains` survives. `autonomous_live`
    // is a substring of `limited_autonomous_live`; a refusal that named the
    // wrong rung would satisfy `contains("autonomous_live")` and tell an
    // operator their `autonomous_live` ceiling was the thing refused when it
    // was not.
    let names_the_neighbour = "configured at autonomy level 'limited_autonomous_live', at which";
    let refused = AutonomyLevel::AutonomousLive.as_str();
    assert!(
        names_the_neighbour.contains(refused),
        "the premise of this test is that the bare substring matches; if it no longer does, the \
         two level names have changed and the discipline below needs re-deriving"
    );
    assert!(
        !names_the_neighbour.contains(&format!("'{refused}'")),
        "the quoted form matched a message naming the neighbouring rung, so the check in the \
         test above discriminates nothing"
    );
}

// --- the wiring seam, which is where the layer is actually deleted -----------

/// Every place a composition root reads the configured ceiling out of the
/// environment without routing it through `AutonomyLevel::deployable`, by line.
///
/// Matched on `env::var("QIP_AUTONOMY_CEILING")` — the read itself — rather
/// than on the bare variable name, because two of the three roots carry a
/// comment naming the variable and a scan that could not tell the two apart
/// would report the comment.
///
/// The window searched for the call is everything back to the previous
/// statement or block boundary. The known limit, stated rather than
/// discovered: a comment containing one of those delimiters between the
/// previous statement and the read would truncate the window and could report
/// a compliant call. That failure is loud and investigable, not a silent pass.
fn ceiling_reads_that_skip_the_refusal(source: &str) -> Vec<usize> {
    const READ: &str = "env::var(\"QIP_AUTONOMY_CEILING\")";
    let mut findings = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = source[offset..].find(READ) {
        let at = offset + relative;
        let boundary = source[..at]
            .rfind([';', '{', '}'])
            .map_or(0, |index| index + 1);
        if !source[boundary..at].contains("AutonomyLevel::deployable(") {
            findings.push(source[..at].matches('\n').count() + 1);
        }
        offset = at + READ.len();
    }
    findings
}

#[test]
fn every_binary_that_reads_the_configured_ceiling_routes_it_through_the_refusal() {
    // The one-word change that deletes a documented layer of the boundary:
    // `AutonomyLevel::parse` in place of `AutonomyLevel::deployable`. `parse`
    // admits every rung on the ladder, so the process starts at
    // `autonomous_live` and Terraform's plan-time refusal — layer one — never
    // sees an environment edited past review. Nothing else in this workspace
    // fails when that edit is made.
    let mut read_sites = 0usize;
    let mut findings = Vec::new();
    for path in files_with_extension("backend/crates/apps", "rs") {
        let source = std::fs::read_to_string(&path).expect("a readable source file");
        read_sites += source.matches("env::var(\"QIP_AUTONOMY_CEILING\")").count();
        for line in ceiling_reads_that_skip_the_refusal(&source) {
            findings.push(format!("{}:{line}", path.display()));
        }
    }
    // The premise. Three binaries read the ceiling — qip-api, qip-fastbrain
    // and qip-deepbrain. A scan that found none would pass while the layer had
    // been deleted outright, which is the stronger version of the very defect
    // this test is about.
    assert!(
        read_sites >= 3,
        "only {read_sites} composition roots read QIP_AUTONOMY_CEILING; three did when this test \
         was written, so either a binary stopped reading its configured ceiling or this scan \
         stopped seeing the read"
    );
    assert!(
        findings.is_empty(),
        "a composition root reads the configured ceiling without AutonomyLevel::deployable, so a \
         live value would start the process instead of stopping it: {findings:#?}"
    );
}

/// The read, written the way that deletes the layer.
const A_ROOT_THAT_PARSES_INSTEAD_OF_REFUSING: &str = r#"
fn main() -> Result<()> {
    let ceiling = AutonomyLevel::parse(std::env::var("QIP_AUTONOMY_CEILING").ok().as_deref())?;
    Ok(())
}
"#;

/// The read, written the way all three roots write it today.
const A_ROOT_THAT_REFUSES: &str = r#"
fn main() -> Result<()> {
    let ceiling =
        AutonomyLevel::deployable(std::env::var("QIP_AUTONOMY_CEILING").ok().as_deref())?;
    Ok(())
}
"#;

#[test]
fn a_composition_root_that_parsed_the_ceiling_without_refusing_is_found_by_this_scan() {
    // Both directions, because a scan that flagged everything would satisfy
    // the first assertion and refuse the tree that is correct — and would then
    // be edited away rather than believed.
    assert_eq!(
        ceiling_reads_that_skip_the_refusal(A_ROOT_THAT_PARSES_INSTEAD_OF_REFUSING).len(),
        1,
        "the scan does not find a root that parses the ceiling instead of refusing a live one, \
         so it would not have found the deletion of layer two"
    );
    assert!(
        ceiling_reads_that_skip_the_refusal(A_ROOT_THAT_REFUSES).is_empty(),
        "the scan reports the compliant form, so its finding of nothing in the tree would mean \
         nothing"
    );
}

// --- the other two layers are still proved somewhere -------------------------

#[test]
fn the_three_layers_the_safety_rules_name_each_still_have_a_test() {
    // A suite that proves one third of a property asserts the other two thirds
    // still exist rather than duplicating them. The rules file names three
    // layers; this file proves the second, and these are the two it does not.
    // Deleting either of those tests would otherwise leave a boundary
    // documented as triply defended and singly proved.
    //
    // Each name carries its opening parenthesis. Without it the match is a
    // bare prefix, and a mutation that truncated the name by one character
    // survived this assertion when it was first written — the substring trap
    // the testing rules name, caught here by mutation and not by review.
    let plan_time = read("backend/crates/tests/qip-acceptance/tests/infrastructure.rs");
    assert!(
        plan_time
            .contains("fn no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue("),
        "layer one's test has been renamed or deleted; Terraform's plan-time refusal of a live \
         ceiling is now unproved"
    );
    let runtime = read("backend/crates/tests/qip-acceptance/tests/acceptance.rs");
    assert!(
        runtime.contains("fn the_assembled_platform_cannot_be_talked_into_live_trading("),
        "layer three's test has been renamed or deleted; the runtime refusal of an escalation is \
         now unproved"
    );
}
