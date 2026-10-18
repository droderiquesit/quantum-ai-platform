# The tenant foundation, planned (ADR 0118).
#
# Mocked provider, `command = plan`: no credential, no organisation, nothing
# created. The committed `terraform.tfvars` beside the root is loaded, so these
# runs plan the values the owner will apply — dev on, test/stage/prod off —
# and only the three runtime inputs below are invented.
#
# What each run guards, and the failure it is there for:
#   * a project without a budget is spend nobody is told about;
#   * a project without its labels is a line in the billing export nobody can
#     attribute;
#   * a folder without the key-creation policy lets the one credential this
#     repository forbids be minted in the seed, which no platform-root policy
#     covers;
#   * prod switched on by default would give the one environment every
#     workflow refuses a billing project of its own.

mock_provider "google" {
  mock_resource "google_project" {
    defaults = {
      number = "123456789012"
    }
  }
  mock_resource "google_folder" {
    defaults = {
      folder_id = "987654321098"
      name      = "folders/987654321098"
    }
  }
}

variables {
  org_id             = "111122223333"
  billing_account_id = "0A1B2C-3D4E5F-6A7B8C"
  prefix             = "qip-harness"
}

run "dev_is_the_only_environment_with_a_project_and_prod_is_off" {
  command = plan

  assert {
    condition     = keys(google_project.environment) == ["dev"]
    error_message = "The committed tfvars should create exactly one environment project, dev; got ${jsonencode(keys(google_project.environment))}."
  }

  assert {
    condition     = google_project.environment["dev"].project_id == "qip-harness-dev"
    error_message = "dev's project id is not <prefix>-dev."
  }

  assert {
    condition     = !contains(keys(google_project.environment), "prod") && var.environments["prod"].enabled == false
    error_message = "prod has a project by default. It is declared and off until a person turns it on."
  }

  assert {
    condition     = google_project.environment["dev"].deletion_policy == "PREVENT" && google_project.seed.deletion_policy == "PREVENT"
    error_message = "A project this root holds can be deleted by a plan; its id could never be reused."
  }

  assert {
    condition     = google_project.environment["dev"].auto_create_network == false
    error_message = "dev would be created with a default VPC."
  }

  # The folder's id is unknown until it exists, so membership is asserted
  # through what is known at plan time: one folder, under the organisation,
  # and no project left at the organisation's top level.
  assert {
    condition     = length(google_folder.platform) == 1 && google_folder.platform[0].parent == "organizations/111122223333"
    error_message = "In org mode exactly one platform folder is planned, under the organisation."
  }

  assert {
    condition     = google_project.environment["dev"].org_id == null && google_project.seed.org_id == null
    error_message = "A project is planned at the organisation's top level instead of in the platform folder."
  }
}

run "every_project_carries_the_four_labels" {
  command = plan

  assert {
    condition = alltrue([
      for project in concat(values(google_project.environment), [google_project.seed]) :
      alltrue([for key in ["owner", "environment", "cost-center", "managed-by"] : contains(keys(project.labels), key)])
    ])
    error_message = "A project is missing one of owner, environment, cost-center, managed-by."
  }

  assert {
    condition     = google_project.environment["dev"].labels["environment"] == "dev" && google_project.seed.labels["environment"] == "seed"
    error_message = "The environment label does not name the project's environment."
  }

  assert {
    condition     = google_project.environment["dev"].labels["managed-by"] == "terraform"
    error_message = "managed-by is not terraform."
  }

  assert {
    condition     = google_project.environment["dev"].labels["owner"] == "research-desk" && google_project.environment["dev"].labels["cost-center"] == "qip-platform"
    error_message = "owner/cost-center do not carry the committed values."
  }
}

run "every_project_has_its_own_budget_at_fifty_ninety_and_one_hundred_percent" {
  command = plan

  # Premise: there is more than one project, so "one budget each" is not
  # satisfied by a single budget across the account.
  assert {
    condition     = length(google_project.environment) + 1 == 2
    error_message = "Premise: the seed and dev are the two projects planned."
  }

  assert {
    condition     = toset(keys(google_billing_budget.project)) == toset(["seed", "dev"])
    error_message = "Budgets planned for ${jsonencode(keys(google_billing_budget.project))}; every project needs exactly one."
  }

  assert {
    condition = alltrue([
      for budget in values(google_billing_budget.project) :
      [for rule in budget.threshold_rules : rule.threshold_percent] == [0.5, 0.9, 1.0]
    ])
    error_message = "A budget does not alert at 50/90/100%."
  }

  assert {
    condition = alltrue([
      for budget in values(google_billing_budget.project) :
      length(budget.budget_filter[0].projects) == 1
    ])
    error_message = "A budget is filtered to more or fewer than one project, so its alert cannot name who spent."
  }

  assert {
    condition     = google_billing_budget.project["dev"].amount[0].specified_amount[0].units == "100"
    error_message = "dev's budget is not the committed 100."
  }
}

run "the_folder_refuses_service_account_keys_and_the_other_baseline_policies" {
  command = plan

  assert {
    condition     = google_org_policy_policy.boolean["iam.disableServiceAccountKeyCreation"].spec[0].rules[0].enforce == "TRUE"
    error_message = "The folder does not enforce iam.disableServiceAccountKeyCreation."
  }

  assert {
    condition     = google_org_policy_policy.boolean["storage.uniformBucketLevelAccess"].spec[0].rules[0].enforce == "TRUE"
    error_message = "The folder does not enforce uniform bucket-level access."
  }

  assert {
    condition     = google_org_policy_policy.vm_external_ip[0].spec[0].rules[0].deny_all == "TRUE"
    error_message = "The folder does not deny external IPs on VMs."
  }

  assert {
    condition     = length(google_org_policy_policy.allowed_member_domains) == 0
    error_message = "Domain-restricted sharing is set without any customer id; it is optional and off by default."
  }

  assert {
    condition     = length(output.controls_not_applied) == 1
    error_message = "With an organisation and the policies on, only the optional domain restriction should be listed as not applied: ${jsonencode(output.controls_not_applied)}."
  }
}

run "domain_restricted_sharing_is_set_when_a_customer_id_is_named" {
  command = plan

  variables {
    allowed_policy_member_customer_ids = ["C0abc1234"]
  }

  assert {
    condition     = google_org_policy_policy.allowed_member_domains[0].spec[0].rules[0].values[0].allowed_values == tolist(["C0abc1234"])
    error_message = "The named customer id is not the policy's allowed value."
  }
}

run "no_org_mode_creates_no_folder_and_says_which_controls_are_missing" {
  command = plan

  variables {
    org_id = null
  }

  assert {
    condition     = length(google_folder.platform) == 0 && length(google_org_policy_policy.boolean) == 0 && length(google_org_policy_policy.vm_external_ip) == 0
    error_message = "No-org mode planned a folder or an organisation policy, neither of which has a parent to live on."
  }

  assert {
    condition     = google_project.environment["dev"].folder_id == null
    error_message = "No-org mode gave dev a folder."
  }

  # Admission half: the projects and budgets are still planned.
  assert {
    condition     = length(google_project.environment) == 1 && length(google_billing_budget.project) == 2
    error_message = "No-org mode stopped planning the project or its budgets."
  }

  assert {
    condition     = contains(output.controls_not_applied, "iam.disableServiceAccountKeyCreation (the platform root still sets it per project)")
    error_message = "No-org mode does not say the folder's key-creation policy is absent."
  }
}

run "an_owner_without_policy_admin_can_turn_the_policies_off_and_is_told" {
  command = plan

  variables {
    org_policies_enabled = false
  }

  assert {
    condition     = length(google_org_policy_policy.boolean) == 0 && length(google_folder.platform) == 1
    error_message = "org_policies_enabled = false should keep the folder and drop only the policies."
  }

  assert {
    condition     = contains(output.controls_not_applied, "compute.vmExternalIpAccess")
    error_message = "The missing external-IP policy is not reported."
  }
}

run "a_prefix_that_would_overflow_a_project_id_is_refused" {
  command = plan

  variables {
    # 25 characters: `<prefix>-stage` would be 31, one over Google's limit.
    prefix = "abcdefghijklmnopqrstuvwxy"
  }

  expect_failures = [var.prefix]
}

run "a_prefix_ending_in_a_hyphen_is_refused" {
  command = plan

  variables {
    prefix = "qip-"
  }

  expect_failures = [var.prefix]
}

run "an_environment_the_platform_does_not_know_is_refused" {
  command = plan

  variables {
    environments = {
      dev        = { enabled = true, budget_amount = 100 }
      test       = { enabled = false, budget_amount = 100 }
      stage      = { enabled = false, budget_amount = 100 }
      production = { enabled = false, budget_amount = 100 }
    }
  }

  expect_failures = [var.environments]
}

run "a_zero_budget_is_refused" {
  command = plan

  variables {
    environments = {
      dev   = { enabled = true, budget_amount = 0 }
      test  = { enabled = false, budget_amount = 100 }
      stage = { enabled = false, budget_amount = 100 }
      prod  = { enabled = false, budget_amount = 100 }
    }
  }

  expect_failures = [var.environments]
}

run "a_billing_account_in_the_wrong_shape_is_refused" {
  command = plan

  variables {
    billing_account_id = "billingAccounts/0A1B2C-3D4E5F-6A7B8C"
  }

  expect_failures = [var.billing_account_id]
}

run "a_label_carrying_an_address_is_refused" {
  command = plan

  variables {
    labels = {
      owner       = "someone@example.com"
      cost_center = "qip-platform"
    }
  }

  expect_failures = [var.labels]
}

# R12-SEC-9131: a hierarchical firewall policy on the folder, evaluated before
# any project's VPC rules, denying internet ingress and handing private and
# Google front-end ranges on to the project.
run "the_folder_carries_a_hierarchical_firewall_policy_that_denies_internet_ingress" {
  command = plan

  # The folder's id is computed, so under a mocked plan the policy's parent
  # and the association's target are unknown; their references to
  # google_folder.platform are held by qip-acceptance's infrastructure suite,
  # which reads the file. Here: both are planned, once each.
  assert {
    condition     = length(google_compute_firewall_policy.platform) == 1 && length(google_compute_firewall_policy_association.platform) == 1
    error_message = "The platform folder carries no hierarchical firewall policy and association (R12-SEC-9131)."
  }

  assert {
    condition = (
      google_compute_firewall_policy_rule.deny_internet_ingress_ipv4[0].action == "deny" &&
      contains(flatten([for m in google_compute_firewall_policy_rule.deny_internet_ingress_ipv4[0].match : m.src_ip_ranges]), "0.0.0.0/0") &&
      google_compute_firewall_policy_rule.deny_internet_ingress_ipv6[0].action == "deny" &&
      contains(flatten([for m in google_compute_firewall_policy_rule.deny_internet_ingress_ipv6[0].match : m.src_ip_ranges]), "::/0")
    )
    error_message = "The folder policy does not deny ingress from every IPv4 and IPv6 source."
  }

  # The delegation is evaluated first and never allows: it hands traffic to
  # the project's own rules, and covers none of the public internet.
  assert {
    condition = (
      google_compute_firewall_policy_rule.delegate_private_and_google[0].action == "goto_next" &&
      google_compute_firewall_policy_rule.delegate_private_and_google[0].priority < google_compute_firewall_policy_rule.deny_internet_ingress_ipv4[0].priority &&
      !contains(flatten([for m in google_compute_firewall_policy_rule.delegate_private_and_google[0].match : m.src_ip_ranges]), "0.0.0.0/0")
    )
    error_message = "The delegation rule must be goto_next, ahead of the deny, and must not cover 0.0.0.0/0."
  }
}

run "an_owner_without_firewall_policy_admin_can_turn_it_off_and_is_told" {
  command = plan

  variables {
    hierarchical_firewall_enabled = false
  }

  assert {
    condition     = length(google_compute_firewall_policy.platform) == 0 && length(google_compute_firewall_policy_association.platform) == 0 && length(google_folder.platform) == 1
    error_message = "hierarchical_firewall_enabled = false should keep the folder and drop only the firewall policy."
  }

  assert {
    condition     = contains(output.controls_not_applied, "hierarchical firewall policy (the platform root's network firewall policy still denies ingress per network)")
    error_message = "The missing hierarchical firewall policy is not reported."
  }
}

run "no_org_mode_plans_no_hierarchical_firewall_policy" {
  command = plan

  variables {
    org_id = null
  }

  assert {
    condition     = length(google_compute_firewall_policy.platform) == 0 && length(google_project_service.seed_compute) == 0
    error_message = "No-org mode planned a hierarchical firewall policy with no folder to hold it."
  }
}
