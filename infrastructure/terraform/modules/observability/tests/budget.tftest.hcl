# The spend budget: absent by default, present with 4 thresholds when enabled,
# refused when enabled without an account. Mocked provider, plan only.

mock_provider "google" {}

variables {
  project_id            = "budget-plan-harness"
  environment           = "dev"
  labels                = {}
  notification_channels = []
}

run "off_by_default_creates_no_budget" {
  command = plan
  assert {
    condition     = length(google_billing_budget.monthly) == 0
    error_message = "the budget must not exist unless enabled"
  }
}

run "enabled_creates_the_budget_with_four_thresholds" {
  command = plan
  variables {
    billing_budget_enabled = true
    billing_account_id     = "000000-000000-000000"
  }
  assert {
    condition     = length(google_billing_budget.monthly) == 1 && length(google_billing_budget.monthly[0].threshold_rules) == 4
    error_message = "enabled must plan one budget with 50/70/90/100 thresholds"
  }
}

run "enabled_without_an_account_is_refused" {
  command = plan
  variables {
    billing_budget_enabled = true
  }
  expect_failures = [google_billing_budget.monthly]
}

run "a_malformed_account_is_refused" {
  command = plan
  variables {
    billing_account_id = "not-an-account"
  }
  expect_failures = [var.billing_account_id]
}
