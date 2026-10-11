# The spend budget: absent by default, present with 4 thresholds when enabled,
# refused when enabled without an account. And the outcome-backlog (RES-9173)
# and belief-calibration (OBS-3118) alert policies: absent until
# `workload_metrics_exist` says a scrape is a fact, and when present, querying
# the series the kernel records. Mocked provider, plan only.

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

run "gated_off_by_default" {
  command = plan
  assert {
    condition     = length(google_monitoring_alert_policy.outcome_backlog) == 0 && length(google_monitoring_alert_policy.belief_calibration) == 0
    error_message = "a policy naming a descriptor nothing has ingested must not exist until workload_metrics_exist is true"
  }
}

run "enabled_watches_the_recorded_series" {
  command = plan
  variables {
    workload_metrics_exist = true
  }
  assert {
    condition     = google_monitoring_alert_policy.outcome_backlog[0].conditions[0].condition_prometheus_query_language[0].query == "min_over_time(qip_central_outcome_backlog_orders[1h]) > 0"
    error_message = "the outcome backlog policy must page on a backlog that never drained for an hour"
  }
  assert {
    condition     = google_monitoring_alert_policy.belief_calibration[0].conditions[0].condition_prometheus_query_language[0].query == "min_over_time(qip_belief_brier_score[1h]) > 0.25"
    error_message = "the calibration policy must page on a Brier score worse than a coin flip for an hour"
  }
}
