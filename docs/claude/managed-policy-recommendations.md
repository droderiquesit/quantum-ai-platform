# Managed-policy recommendations

Controls that belong in enterprise-managed policy rather than in this
repository. **Nothing here has been applied** — a repository cannot configure
its own organisation, and a control an agent can edit is a control an agent can
remove.

Managed settings are read from a system location that repository configuration
cannot override. `claude doctor` reports `Managed settings (remote): none
configured for this organization` for this account today.

## Recommended, in priority order

1. **Pin the permission deny list centrally.** `.claude/settings.json` denies
   reads of key material, `*.tfstate`, `~/.ssh`, `~/.aws` and
   `~/.config/gcloud`. That file is writable by anything with repository
   access, so today the control protects against accident, not intent. The
   same deny list in managed policy protects against both.

2. **Require the dangerous-command hook.** `guard-dangerous-command.sh` blocks
   force pushes, cloud deletions and unapproved Terraform. A managed hook
   entry survives a branch that deletes the local one.

3. **Forbid `--allow-dangerously-skip-permissions` and
   `defaultMode: bypassPermissions`** for any session touching this
   repository. Both defeat every permission control above.

4. **Restrict which repositories a session may attach.** This platform holds
   trading logic and cloud credentials chains; sessions should not be able to
   pull in arbitrary repositories alongside it.

5. **Require `prod` deployment approval through GitHub environment reviewers.**
   `deploy.yml` refuses an automatic prod path, but the required-reviewer
   setting on the `prod` environment is a repository setting no workflow file
   can create. Until it is set, prod's protection is one dispatch away from
   nothing.

6. **Centralise the model and effort routing** if cost control matters more
   than per-agent tuning. This repository leaves agent `model` to inheritance
   precisely so an organisation-level choice governs.

## What stays here

Agent definitions, skills, domain rules, and the Definition of Done are
repository concerns: they describe *this* codebase and should version with it.
Only the controls that must survive a hostile or careless branch belong in
managed policy.
