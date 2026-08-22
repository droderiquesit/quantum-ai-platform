# Alerting.
#
# Four alerts, chosen because each one means somebody should look now. An
# alerting policy that fires on something nobody acts on trains people to
# ignore the ones that matter.

# The kill switch tripping. No threshold and no duration: any trip is worth
# waking someone for, and one that resolves itself before an alert would fire
# is exactly the one worth knowing about.
resource "google_monitoring_alert_policy" "kill_switch" {
  project      = var.project_id
  display_name = "qip ${var.environment}: kill switch tripped"
  combiner     = "OR"

  conditions {
    display_name = "kill switch tripped"

    condition_threshold {
      filter = join(" AND ", [
        "metric.type=\"prometheus.googleapis.com/qip_kill_switch_tripped/gauge\"",
        "resource.type=\"prometheus_target\"",
      ])
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "0s"

      aggregations {
        alignment_period   = "60s"
        per_series_aligner = "ALIGN_MAX"
      }
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      The platform has halted. No orders will be sent until an operator clears
      the halt.

      Find the reason: `qip status`, or GET /api/v1/system/status. The first
      reason recorded is the trigger; later ones are consequences.

      Clearing the halt is deliberate and requires an operator credential. Do
      not clear it before understanding why it tripped.
    EOT
    mime_type = "text/markdown"
  }
}

# A live order reaching a venue. In a paper environment this should never fire
# at all, which is why the threshold is zero rather than a rate.
resource "google_monitoring_alert_policy" "live_fill" {
  count = var.environment == "production" ? 0 : 1

  project      = var.project_id
  display_name = "qip ${var.environment}: a live fill occurred in a non-production environment"
  combiner     = "OR"

  conditions {
    display_name = "live fill"

    condition_threshold {
      filter = join(" AND ", [
        "metric.type=\"prometheus.googleapis.com/qip_live_fills_total/counter\"",
        "resource.type=\"prometheus_target\"",
      ])
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "0s"

      aggregations {
        alignment_period   = "60s"
        per_series_aligner = "ALIGN_DELTA"
      }
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      An order reached a real venue in an environment that should only ever
      trade on paper.

      This should be impossible: the application refuses a live venue below a
      live autonomy level, and the venue credential is unreadable where the
      ceiling is paper trading. If this fires, one of those two controls has
      failed and the other did not catch it.

      Halt the platform first, then investigate.
    EOT
    mime_type = "text/markdown"
  }
}

# A risk limit breached and not resolved. The monitor goes reduce-only on the
# first breach and halts on the third, so a breach persisting for fifteen
# minutes means the book is not coming back inside on its own.
resource "google_monitoring_alert_policy" "persistent_breach" {
  project      = var.project_id
  display_name = "qip ${var.environment}: a risk limit has been breached for fifteen minutes"
  combiner     = "OR"

  conditions {
    display_name = "persistent limit breach"

    condition_threshold {
      filter = join(" AND ", [
        "metric.type=\"prometheus.googleapis.com/qip_limit_breaches/gauge\"",
        "resource.type=\"prometheus_target\"",
      ])
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "900s"

      aggregations {
        alignment_period   = "60s"
        per_series_aligner = "ALIGN_MAX"
      }
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      A risk limit has been breached continuously for fifteen minutes.

      The monitor will already have gone reduce-only and may have halted the
      scope. What this alert adds is that the book is not coming back inside
      the limit on its own.
    EOT
    mime_type = "text/markdown"
  }
}

# An agent attempting something its manifest does not grant. Blocked, and
# still worth knowing about: it is either a bug or an agent behaving in a way
# nobody anticipated.
resource "google_monitoring_alert_policy" "permission_violation" {
  project      = var.project_id
  display_name = "qip ${var.environment}: an agent attempted an ungranted capability"
  combiner     = "OR"

  conditions {
    display_name = "permission violation"

    condition_threshold {
      filter = join(" AND ", [
        "metric.type=\"prometheus.googleapis.com/qip_permission_denials_total/counter\"",
        "resource.type=\"prometheus_target\"",
      ])
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "300s"

      aggregations {
        alignment_period   = "300s"
        per_series_aligner = "ALIGN_DELTA"
      }
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      An agent reached for a capability its manifest does not grant. The
      attempt was refused and recorded in the run's audit trail.

      This is not an incident on its own — the control worked — but it is
      either a bug or an agent doing something nobody anticipated, and both are
      worth understanding before they recur.
    EOT
    mime_type = "text/markdown"
  }
}
