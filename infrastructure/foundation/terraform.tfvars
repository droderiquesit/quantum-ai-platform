# The foundation's committed inputs (ADR 0118).
#
# `org_id`, `billing_account_id` and `prefix` are NOT here and must never be:
# they are account identifiers, passed at run time by
# scripts/bootstrap-foundation.sh. Project ids derived from the prefix are
# committed where the platform needs them, in
# infrastructure/environments/<env>/terraform.tfvars, as they always were.

folder_display_name = "algorik-platform"

# dev on; the rest declared and off. `enabled = false` creates nothing and
# bills nothing. prod is also refused by infra.yml and deploy.yml whatever
# this says, and turning it on here only gives it an empty project.
environments = {
  dev   = { enabled = true, budget_amount = 100 }
  test  = { enabled = false, budget_amount = 100 }
  stage = { enabled = false, budget_amount = 200 }
  prod  = { enabled = false, budget_amount = 500 }
}

seed_budget_amount = 5
budget_thresholds  = [0.5, 0.9, 1.0]

labels = {
  owner       = "research-desk"
  cost_center = "qip-platform"
}

org_policies_enabled = true

# Domain-restricted sharing stays off until the owner names their directory's
# customer id here (variables.tf says why it is not a default).
allowed_policy_member_customer_ids = []
