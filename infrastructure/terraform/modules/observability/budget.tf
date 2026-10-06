# Monthly spend budget: 750 USD (25 USD/day) with 50/70/90/100% current-spend
# thresholds. Gated on `billing_budget_enabled`, false by default. It alerts
# through the given notification channels; it cannot stop spend, and nothing
# here claims otherwise.

data "google_project" "this" {
  count      = var.billing_budget_enabled ? 1 : 0
  project_id = var.project_id
}

resource "google_billing_budget" "monthly" {
  count = var.billing_budget_enabled ? 1 : 0

  billing_account = var.billing_account_id
  display_name    = "qip ${var.environment}: monthly budget"

  budget_filter {
    projects = ["projects/${data.google_project.this[0].number}"]
  }

  amount {
    specified_amount {
      currency_code = "USD"
      units         = tostring(var.billing_budget_monthly_usd)
    }
  }

  dynamic "threshold_rules" {
    for_each = [0.5, 0.7, 0.9, 1.0]
    content {
      threshold_percent = threshold_rules.value
      spend_basis       = "CURRENT_SPEND"
    }
  }

  dynamic "all_updates_rule" {
    for_each = length(var.notification_channels) > 0 ? [1] : []
    content {
      monitoring_notification_channels = var.notification_channels
      disable_default_iam_recipients   = false
    }
  }

  lifecycle {
    precondition {
      condition     = var.billing_account_id != null
      error_message = "billing_budget_enabled needs billing_account_id; a budget with no account cannot exist."
    }
  }
}
