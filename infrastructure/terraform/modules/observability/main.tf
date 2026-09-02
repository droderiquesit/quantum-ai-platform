# Alerting.
#
# Seven alerts, chosen because each one means somebody should look now. An
# alerting policy that fires on something nobody acts on trains people to
# ignore the ones that matter.
#
# All seven are gated on `workload_metrics_exist`, and the gate is the fix for
# a failure two applies hit in two different shapes: Cloud Monitoring refuses
# an alert policy naming a metric it has never ingested — a filter condition
# fails on the missing descriptor, and a PromQL condition fails validation on
# the unknown metric name. For application metrics that is every apply that
# precedes the first scrape, so the policies simply cannot exist first. The
# tfvars flips the gate after the first scrape is proven, and NOT-SCRAPED.md
# says what scrapes what on this runtime and what does not yet.
#
# Every descriptor named below is one `backend/crates/libs/qip-observability/src/metrics.rs`
# registers, and `every_metric_an_alert_policy_queries_is_one_the_platform_emits`
# in the acceptance suite refuses a policy naming one it does not.

# The kill switch tripping. No threshold and no duration: any trip is worth
# waking someone for, and one that resolves itself before an alert would fire
# is exactly the one worth knowing about.
resource "google_monitoring_alert_policy" "kill_switch" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: kill switch tripped"
  combiner     = "OR"

  conditions {
    display_name = "kill switch tripped"

    condition_prometheus_query_language {
      query               = "max(qip_kill_switch_tripped) > 0"
      duration            = "0s"
      evaluation_interval = "30s"
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
  count = var.workload_metrics_exist && var.environment != "prod" ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: a live fill occurred in a non-production environment"
  combiner     = "OR"

  conditions {
    display_name = "live fill"

    condition_prometheus_query_language {
      query               = "increase(qip_live_fills_total[5m]) > 0"
      duration            = "0s"
      evaluation_interval = "30s"
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
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: a risk limit has been breached for fifteen minutes"
  combiner     = "OR"

  conditions {
    display_name = "persistent limit breach"

    condition_prometheus_query_language {
      query               = "max(qip_limit_breaches) > 0"
      duration            = "900s"
      evaluation_interval = "30s"
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
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: an agent attempted an ungranted capability"
  combiner     = "OR"

  conditions {
    display_name = "permission violation"

    condition_prometheus_query_language {
      query               = "increase(qip_permission_denials_total[5m]) > 0"
      duration            = "300s"
      evaluation_interval = "30s"
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

# --- the edge plane -----------------------------------------------------------
#
# The execution node records into the edge crate's `CellMetrics` and serves the
# series on its health port; the Ops Agent's Prometheus receiver on the node
# (`modules/execution-node/templates/startup.sh.tftpl`) is what carries them
# to Cloud Monitoring. Three of those series are the ones a person must see.

# A cell halted, by any of its three disciplines. `qip_edge_halted{source}`
# is a gauge written wherever a halt can change and at wiring time, so a node
# halted before its first pass still reports halted. Any value above zero means the
# node is refusing to trade, and a node refusing to trade is either right —
# in which case the reason is the incident — or wrong, in which case the halt
# is. Both are worth a person now.
resource "google_monitoring_alert_policy" "edge_halted" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: an execution node is halted"
  combiner     = "OR"

  conditions {
    display_name = "edge node halted"

    condition_prometheus_query_language {
      query               = "max by (cell, source) (qip_edge_halted) > 0"
      duration            = "0s"
      evaluation_interval = "30s"
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      An execution node reports itself halted. The `source` label says which
      discipline stopped it: `kill_switch` is an operator or the platform
      tripping the switch, `policy` is the cell refusing on its own envelope,
      and `polled` is the halt flag the node polls on its own filesystem —
      the second wire of §46.2, so a node cut off from the centre can still
      be stopped by hand on the machine.

      A halt is the correct response to whatever caused it. Find the cause in
      the node's journal before clearing anything; clearing a halt whose
      reason still holds re-halts it on the next pass and loses the first
      record of why.
    EOT
    mime_type = "text/markdown"
  }
}

# A reconciliation break the cell found: its own book and the venue's
# disagree. Counted where the cell finds it, so a break the centre never
# hears about still charts.
resource "google_monitoring_alert_policy" "edge_reconciliation_break" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: an execution node found a reconciliation break"
  combiner     = "OR"

  conditions {
    display_name = "edge reconciliation break"

    condition_prometheus_query_language {
      query               = "increase(qip_edge_reconciliation_breaks_total[5m]) > 0"
      duration            = "0s"
      evaluation_interval = "30s"
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      A cell's book and its venue's record disagree. The cell halts on a
      break — that is the policy discipline — so this fires beside
      `an execution node is halted` and names the cause.

      Reconcile from the venue, never from the journal: the journal records
      what the cell decided, and the venue records what actually filled.
      `docs/operations/reconciliation-break.md` is the runbook.
    EOT
    mime_type = "text/markdown"
  }
}

# --- the central plane's view of the same fact ---------------------------------
#
# `Platform::ingest_cell_report` counts a break by the direction of the gap,
# on the outcome rather than the report, so a refused report charts no break.
# The central prefix keeps it distinct from the edge's own counter:
# that one records what the cell found, this one what the centre acted on,
# and the two disagreeing is itself a finding.
resource "google_monitoring_alert_policy" "central_reconciliation_break" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: the central plane acted on a reconciliation break"
  combiner     = "OR"

  conditions {
    display_name = "central reconciliation break"

    condition_prometheus_query_language {
      query               = "increase(qip_central_reconciliation_breaks_total[5m]) > 0"
      duration            = "0s"
      evaluation_interval = "30s"
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      The central plane received a cell report whose exposure disagrees with
      the envelope it granted, and acted on it. The `direction` label says
      which way the gap runs.

      If `an execution node found a reconciliation break` did not fire for
      the same cell in the same window, the two planes disagree about whether
      there was a break at all, and that disagreement is the first thing to
      resolve.
    EOT
    mime_type = "text/markdown"
  }
}
