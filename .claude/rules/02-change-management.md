# Change management

## Branch and commit

- All work lands on the designated feature branch. Never push to another
  branch without explicit permission.
- Never force-push. A merge commit keeps other checkouts valid; a rewrite does
  not, and the guard hook blocks it.
- Commit messages explain **why**, name the failure prevented, and state what
  was verified with the evidence. Match `git log` in this repository — the
  register is argumentative prose, not a changelog line.
- Never put a model identifier in a commit message, PR body, or code comment.

## Pull requests

Open one only when asked. When you do, mirror any template under
`.github/`. Every GitHub comment ends with the Claude Code attribution footer.

## Definition of Done

A change is done when every applicable gate below has **run** and its output
has been read. Gates that do not apply are named and explained; gates that
apply are not skipped.

| Gate | Command |
|---|---|
| Format | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets` (zero warnings; CI uses `-D warnings`) |
| Tests | `cargo test --workspace --no-fail-fast` |
| Dependency policy | `./scripts/check-dependencies.sh` |
| Secret scan | `./scripts/check-secrets.sh` |
| Terraform | `terraform fmt -check` and `terraform validate` |
| Frontend | `npm run lint` and `npm run build` in `frontend/portal/` |
| Everything | `make check` |

**Use `--no-fail-fast`.** Without it `cargo test` stops at the first failing
binary, and a run that reports 64 passing tests out of 3,075 looks like a small
suite rather than an aborted one.

## Tests

- Never weaken, skip, `#[ignore]`, or delete a test to obtain a passing run.
  Fix the implementation, or document a genuine external limitation.
- A test that fails after your change is evidence about your change until you
  have proven otherwise.
- **Mutation-verify every new test**: break the implementation, confirm the
  test fails for the right reason, restore byte-for-byte, confirm it passes.
  An unverified test is not finished — a test asserting `contains("x")` where
  the surrounding text always contains `x` passes forever and guards nothing.

## Production

- Never deploy to production, delete resources, merge a branch, or perform any
  irreversible operation without explicit approval in the conversation.
- `prod` is refused by both `infra.yml` and the deploy gate, and requires a
  human dispatch. Do not attempt to work around either.
- Approval for one action is not approval for the next.
