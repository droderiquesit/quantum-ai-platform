## Originating Goal

**Goal ID:** (incident #, backlog item #, or expansion gap ID)

**Goal Origin:** (incident / backlog item / expansion gap)

## Rule Exceptions

**Rule changed:** none

<!-- If this change relaxes, waives or edits a rule, replace "none" with the path of every rule file it changes: `CLAUDE.md`, a file under `.claude/rules/` or `.claude/hooks/`, `.claude/settings.json`, an ADR under `docs/adr/`, a policy under `scripts/`, or `.github/CODEOWNERS`. ci's rule-citation job refuses a pull request that changes one without naming it here (R12 GOV-9080). -->

## Summary

<!-- Describe the change and why it was made -->

## Test Plan

<!-- Describe how this change was tested -->

## Verification

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets` passes (zero warnings)
- [ ] `cargo test --workspace --no-fail-fast` passes
- [ ] `./scripts/check-dependencies.sh` passes
- [ ] `./scripts/check-secrets.sh` passes

