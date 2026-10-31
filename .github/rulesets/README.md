# Rulesets for the default branch (R12-CICD-9095)

Branch protection was a repository setting no file in this tree held, so
nothing could say whether `main` was protected or check that it still was.
These two files are the rulesets, as GitHub's REST API accepts them, and
`.github/workflows/rules.yml` reads the live rules for `main` every day
and fails if any rule below is missing.

A repository admin applies them; nothing in this tree can, because
creating a ruleset needs admin on the repository:

```
gh api -X POST repos/<owner>/<repo>/rulesets --input .github/rulesets/main-integrity.json
gh api -X POST repos/<owner>/<repo>/rulesets --input .github/rulesets/main-review.json
```

## Why two rulesets

- **`main-integrity.json`** refuses deleting `main`, force-pushing to it,
  and merging anything whose required checks have not passed. It has **no
  bypass actor**, so the owner is held to it too. The required checks are
  job names from `ci.yml` and `agent-boundary.yml`, and
  `qip-acceptance`'s `branch_rules` suite fails if one of them stops being
  a job name, because a required check nobody runs blocks every merge.
- **`main-review.json`** requires a pull request with a code owner's
  approval. `.github/CODEOWNERS` names one owner, and that owner's account
  also opens the pull requests the lanes prepare. GitHub refuses an
  author's approval of their own pull request, so without a bypass this
  ruleset would block every merge. The repository-admin role may bypass it
  **only through a pull request** (`bypass_mode: pull_request`): changes
  still reach `main` through a pull request whose checks are green under
  the first ruleset, and GitHub records each bypass. Dropping the bypass
  once a second reviewer exists is the owner's call.

## What this stops

- A direct push to `main`. That includes Kargo's promotion commit, which
  pushes to `main` (runbook §4 step 10). Seeding Kargo therefore needs its
  App added as a bypass actor or its promotions routed through pull
  requests. That is the owner's decision; this file does not make it.
