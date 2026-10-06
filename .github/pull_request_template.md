## Originating Goal

**Goal ID:** (incident #, backlog item #, or expansion gap ID)

**Goal Origin:** (incident / backlog item / expansion gap)

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

