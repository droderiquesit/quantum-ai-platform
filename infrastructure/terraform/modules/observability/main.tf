# Alerting.
#
# Nine alerts, chosen because each one means somebody should look now. An
# alerting policy that fires on something nobody acts on trains people to
# ignore the ones that matter.
#
# All nine are gated on `workload_metrics_exist`, and the gate is the fix for
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
# to Cloud Monitoring. Two of those series are the ones a person must see: the
# halt gauge and the break counter below, and nothing else. This line said
# three for a while — the same miscount NOT-SCRAPED.md carried, which made the
# edge and central groups sum to one policy more than the file declares. The
# recount command is in NOT-SCRAPED.md; it is not repeated here, because a
# metric prefix written into this file as part of a grep pattern is read by
# the acceptance suite as a descriptor the policies query.

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

# --- a control that refused, which is not the same as a control that failed ----
#
# The two policies below are unlike the seven above them. Those fire because
# something went wrong. These fire because a safety control *worked*: the
# platform could not read a book, so it stopped trading it. An operator who
# reads either as a fault will spend the first ten minutes looking for the
# wrong problem, so both documentations say what they mean in their first
# line.
#
# Both series have a production caller, not only a registered constant.
# `Platform::stage_act` writes the gauge on both arms of the liquidity read
# and counts the withheld sign-off beside it, and `run_cycle` calls
# `stage_act` on every cycle of every central binary — checked by reading the
# caller, because a constant nothing calls satisfies the acceptance test that
# matches names and still pages nobody.

# The liquidity figure could not be computed, so nothing is being signed off.
resource "google_monitoring_alert_policy" "risk_figure_unevaluated" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: a risk figure has been uncomputable for fifteen minutes"
  combiner     = "OR"

  conditions {
    display_name = "risk figure unevaluated"

    condition_prometheus_query_language {
      # Zero or one, written on both arms of every cycle, so the series falls
      # back on its own and the alert clears without anyone acknowledging it.
      # A gauge only written when something is wrong would stay lit forever.
      #
      # Grouped by `figure` rather than maxed flat. `liquidity` is the only
      # value the kernel writes today; a second figure added later must get
      # its own alert stream rather than disappear under a series that is
      # already lit for a different reason.
      #
      # Fifteen minutes, argued against the slowest process that writes this
      # series. The gauge only moves at a cycle boundary, and the slowest
      # cycle is the deep brain's committed `cycle_interval_seconds` of 300 —
      # so even there the condition is three consecutive cycles that could not
      # price the book, and on the fast brain's 100-millisecond default it is
      # very many more. One cycle is a holding whose record had not landed yet
      # and the next cycle resolves it; that is a blip, and paging on it
      # teaches the desk to ignore this alert. Three is a read that is stuck.
      # It is also the same fifteen minutes as the persistent-breach policy
      # above, so an operator has one number to learn for "long enough to be
      # real" rather than two.
      query               = "max by (figure) (qip_risk_figures_unevaluated) > 0"
      duration            = "900s"
      evaluation_interval = "30s"
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      **This is a control working, not a control failing.** The platform could
      not compute the risk figure named by `figure`, so it has withheld
      sign-off and is sending no orders on that book. Nothing is trading
      badly. Nothing is trading at all. Do not go looking for a bad fill.

      For `figure="liquidity"`: the liquidation ladder could not be built, so
      the `MinLiquidity` and `MaxDaysToLiquidate` limits read nothing. Before
      this control existed they passed silently — a limit whose input is
      missing looks exactly like a limit the book satisfied — and the only
      trace was one sentence on a cycle report nobody read. Now ACT refuses to
      sign and each order is refused for the same reason.

      What to do. Read the ACT problems on the cycle report; they name the
      figure and the refusal in one sentence per cycle. The usual cause is a
      position with no liquidity record — a fill booked against an objective
      the catalogue does not carry. Repair the record and the gauge returns to
      zero on the next cycle without anyone clearing anything.

      Escalate if it does not return to zero within a few cycles: the desk is
      then flat, and nobody chose that.

      **Not evaluated today.** Nothing collects this series. The execution
      node's Ops Agent receiver is waiting on a node that no environment
      declares, and the Cloud Run collector is refused rather than pending —
      the only published sidecar image fails this platform's own image scan on
      an unfixed CRITICAL. `modules/observability/NOT-SCRAPED.md` is the
      record and has the commands. If you are reading this paragraph in the
      Cloud console then something changed, because the policy only exists
      once the gate is flipped: check that file before trusting the sentence
      above it.
    EOT
    mime_type = "text/markdown"
  }
}

# Sign-off withheld with the unreadable book as the sole reason. The gauge
# above is the state; this is the attribution, and they are not the same fact.
# `control` is chosen in priority order — the risk monitor first, then the
# liquidity read, then compliance — so `control="liquidity-read"` says the
# monitor permitted new risk and compliance was enforced, and the only thing
# between the desk and a signed proposal was a figure nobody could compute.
resource "google_monitoring_alert_policy" "sign_off_withheld_on_liquidity" {
  count = var.workload_metrics_exist ? 1 : 0

  project      = var.project_id
  display_name = "qip ${var.environment}: sign-off is being withheld because the book cannot be read"
  combiner     = "OR"

  conditions {
    display_name = "proposals unsigned on the liquidity read"

    condition_prometheus_query_language {
      # The failure this catches is the one the gauge above structurally
      # cannot see: a read that fails on a minority of cycles never holds the
      # gauge high for fifteen continuous minutes, and the platform has still
      # declined to trade on every one of those cycles.
      #
      # Threshold `> 0` and the discrimination in the duration, deliberately,
      # because a count threshold would not mean the same thing on the two
      # processes that write this series. `run_cycle` calls the recording site,
      # and the binaries that run cycles do so at very different rates: the
      # deep brain on the committed `cycle_interval_seconds`, 300, and the fast
      # brain on its own clock, defaulting to 100 milliseconds. "More than two
      # in an hour" would therefore be a quarter of the deep brain's decisions
      # and a rounding error of the fast brain's — one number meaning two
      # things, which is how a threshold nobody can argue for gets written.
      #
      # Time is the same on both. The condition is "some sign-off was withheld
      # in the last half hour", held continuously for half an hour: at least
      # one withholding in each of two consecutive half-hour lookbacks with no
      # clean gap between them. Half an hour twice over, rather than fifteen
      # minutes once, so that on a stuck read this arrives after the gauge
      # policy and reads as confirmation rather than as a second incident.
      #
      # `increase` extrapolates to the window edges, which shifts the boundary
      # by about one scrape. Against a `> 0` threshold that changes nothing:
      # the window is either empty or it is not.
      #
      # Scoped to one label value deliberately. `control="risk-monitor"` is
      # already the kill-switch and persistent-breach policies above, and
      # folding it in here would send two pages for one incident.
      # `control="compliance"` has no policy at all; that gap is written down
      # in NOT-SCRAPED.md rather than papered over by widening this query into
      # something no single runbook could answer.
      query               = "sum(increase(qip_proposals_unsigned_total{control=\"liquidity-read\"}[30m])) > 0"
      duration            = "1800s"
      evaluation_interval = "30s"
    }
  }

  notification_channels = var.notification_channels

  documentation {
    content   = <<-EOT
      **This is a control working, not a control failing.** The ACT stage
      declined to sign any proposal, and the liquidity read was the whole
      reason: the risk monitor permitted new risk, compliance was enforced,
      and the book still could not be priced. No orders were sent.

      It fires on repetition rather than on one cycle, so by the time it
      reaches you the platform has refused to trade on more than two cycles in
      the last hour.

      If `a risk figure has been uncomputable for fifteen minutes` fired for
      the same window, this is one stuck read and that alert carries the
      runbook. If it did not, the read is intermittent — which is the case
      this policy exists for, because a gauge with a fifteen-minute duration
      cannot see a failure that clears between cycles.

      What to do. The first step is the same: the ACT problems on the cycle
      report. An intermittent failure usually means one instrument's record is
      arriving late rather than being absent, so compare the cycles that
      refused against the ones that did not and look at what differs.

      **Not evaluated today.** Nothing collects this series either; see the
      note on the policy above and `modules/observability/NOT-SCRAPED.md`.
    EOT
    mime_type = "text/markdown"
  }
}
