//! R12-RES-10133: when GitHub or the GitOps control plane is lost, the running
//! system is unchanged, and an emergency change goes through a pre-approved
//! break-glass procedure that is reconciled to Git afterwards.
//!
//! The first half is held by `gitops.rs` (every `RunService` carries
//! `deletion-policy: abandon`). This suite holds the second: the procedure
//! exists as a committed runbook, says who may act, what is recorded, and how
//! the change gets back into `infrastructure/gitops` — and refuses the one
//! shortcut the procedure must never offer, a Binary Authorization
//! break-glass that admits an unattested image.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::read;

const RUNBOOK: &str = "docs/ops/runbooks/break-glass.md";

/// Markdown with its fenced code removed, so a command shown as an example
/// of what *not* to run is not read as the runbook recommending it.
fn prose_only(markdown: &str) -> String {
    let mut inside = false;
    markdown
        .lines()
        .filter(|line| {
            if line.trim_start().starts_with("```") {
                inside = !inside;
                return false;
            }
            !inside
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Mutation: delete the "## Reconciliation to Git" section — the assertion
/// naming `infrastructure/gitops/envs/` fires. Deleting the sentence that
/// refuses `--breakglass` makes the last assertion fire.
#[test]
fn the_break_glass_runbook_names_who_acts_what_is_recorded_and_how_the_change_is_reconciled_to_git()
{
    let runbook = read(RUNBOOK);
    // Premise: this is the runbook and not a stub left to satisfy the test.
    assert!(
        runbook.lines().count() > 60,
        "{RUNBOOK} is {} lines; a break-glass procedure that fits in fewer is missing a step",
        runbook.lines().count()
    );
    assert!(
        runbook.contains("R12-RES-10133"),
        "{RUNBOOK} does not say which requirement it serves"
    );

    // The three parts the requirement asks for, each as a section a reader
    // can turn to, not a phrase that happens to occur.
    for heading in [
        "## Who may act",
        "## What is recorded, at the time",
        "## Reconciliation to Git",
    ] {
        assert!(
            runbook.lines().any(|line| line.starts_with(heading)),
            "{RUNBOOK} has no `{heading}` section"
        );
    }

    // Reconciliation is concrete: it names the GitOps path the change must
    // land in and the mechanism (a pull request) that gets it there.
    let reconciliation = runbook
        .split("## Reconciliation to Git")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the reconciliation section has a body");
    assert!(
        reconciliation.contains("infrastructure/gitops/envs/"),
        "the reconciliation step does not say where in infrastructure/gitops the change lands"
    );
    assert!(
        reconciliation.contains("pull request"),
        "the reconciliation step does not go through a pull request"
    );
    assert!(
        reconciliation.contains("selfHeal"),
        "the reconciliation step does not warn that dev's selfHeal reverts a manual change"
    );

    // The procedure is pre-approved in the sense the requirement means —
    // bounded to operations that introduce no image — and never offers the
    // Binary Authorization bypass. Read the prose, not the table of refused
    // shapes, so the refusal itself is not mistaken for a recommendation.
    let prose = prose_only(&runbook);
    assert!(
        prose.contains("never** `--breakglass`") || prose.contains("never `--breakglass`"),
        "{RUNBOOK} no longer refuses `--breakglass`, the one flag that admits an unattested image"
    );
    assert!(
        prose.contains("breakglassJustification"),
        "{RUNBOOK} does not name the RunService field gitops.rs refuses"
    );
    assert!(
        prose.contains("Not run"),
        "{RUNBOOK} claims to have been exercised; nothing is deployed to exercise it on"
    );
}

/// The runbook is reachable from the pages an operator starts at, so it is
/// found during an incident rather than by someone who already knows it
/// exists.
///
/// Mutation: remove the link from `incidents.md` — the assertion fires.
#[test]
fn the_break_glass_runbook_is_linked_from_the_incident_playbooks() {
    let incidents = read("docs/ops/runbooks/incidents.md");
    assert!(
        incidents.contains("](break-glass.md"),
        "docs/ops/runbooks/incidents.md does not link break-glass.md; the procedure exists \
         where nobody in an incident will look"
    );
}
