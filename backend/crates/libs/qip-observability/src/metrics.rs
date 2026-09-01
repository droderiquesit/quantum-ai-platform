//! Metrics: counters, gauges and histograms with labels.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Monotonically increasing total.
    Counter,
    /// Instantaneous value that may rise or fall.
    Gauge,
    /// Distribution of observations.
    Histogram,
}

/// Label set identifying one time series.
pub type Labels = BTreeMap<String, String>;

/// Build a label set concisely: `labels([("venue", "XNYS")])`.
pub fn labels<const N: usize>(pairs: [(&str, &str); N]) -> Labels {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Fixed-bucket histogram.
///
/// Buckets are explicit rather than computed so the same boundaries appear in
/// dashboards and alerts across every deployment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Histogram {
    pub bounds: Vec<f64>,
    pub counts: Vec<u64>,
    pub sum: f64,
    pub count: u64,
    pub min: f64,
    pub max: f64,
}

impl Histogram {
    /// Latency buckets in milliseconds, spanning the platform's full range from
    /// order-book handling to a quantum job.
    pub fn latency_ms() -> Self {
        Self::with_bounds(vec![
            0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0,
            5_000.0, 30_000.0, 300_000.0,
        ])
    }

    /// Buckets for unit-interval quantities such as confidence or utilisation.
    pub fn unit_interval() -> Self {
        Self::with_bounds(vec![
            0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.99,
        ])
    }

    pub fn with_bounds(bounds: Vec<f64>) -> Self {
        let counts = vec![0; bounds.len() + 1];
        Self {
            bounds,
            counts,
            sum: 0.0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    pub fn observe(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        let index = self
            .bounds
            .iter()
            .position(|b| value <= *b)
            .unwrap_or(self.bounds.len());
        self.counts[index] += 1;
        self.sum += value;
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    /// Approximate quantile, interpolated within the containing bucket.
    ///
    /// Bucketed quantiles are approximations; the interpolation keeps them
    /// usable for SLO evaluation without storing every observation.
    pub fn quantile(&self, q: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = (q.clamp(0.0, 1.0) * self.count as f64).ceil().max(1.0) as u64;
        let mut cumulative = 0u64;
        for (i, count) in self.counts.iter().enumerate() {
            let previous = cumulative;
            cumulative += count;
            if cumulative >= target {
                let lower = if i == 0 {
                    self.min.min(0.0)
                } else {
                    self.bounds[i - 1]
                };
                let upper = if i < self.bounds.len() {
                    self.bounds[i]
                } else {
                    self.max
                };
                if *count == 0 || upper <= lower {
                    return upper;
                }
                let within = (target - previous) as f64 / *count as f64;
                return lower + (upper - lower) * within;
            }
        }
        self.max
    }
}

/// A recorded metric value.
///
/// Adjacently tagged: an internally-tagged enum cannot carry a primitive
/// payload, and a counter is a bare integer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MetricValue {
    Counter(u64),
    Gauge(f64),
    Histogram(Box<Histogram>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Series {
    name: String,
    labels: Labels,
    value: MetricValue,
    /// Documentation string, exported alongside the metric.
    help: String,
}

/// A point-in-time export of every series.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub service: String,
    pub series: Vec<SeriesSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeriesSnapshot {
    pub name: String,
    pub labels: Labels,
    pub value: MetricValue,
    pub help: String,
}

impl Snapshot {
    /// Look up one series by name and labels.
    pub fn get(&self, name: &str, labels: &Labels) -> Option<&MetricValue> {
        self.series
            .iter()
            .find(|s| s.name == name && &s.labels == labels)
            .map(|s| &s.value)
    }

    /// Counter value, or zero if absent.
    pub fn counter(&self, name: &str, labels: &Labels) -> u64 {
        match self.get(name, labels) {
            Some(MetricValue::Counter(v)) => *v,
            _ => 0,
        }
    }

    /// Sum of a counter across every label combination.
    pub fn counter_total(&self, name: &str) -> u64 {
        self.series
            .iter()
            .filter(|s| s.name == name)
            .filter_map(|s| match &s.value {
                MetricValue::Counter(v) => Some(*v),
                _ => None,
            })
            .sum()
    }

    pub fn gauge(&self, name: &str, labels: &Labels) -> Option<f64> {
        match self.get(name, labels) {
            Some(MetricValue::Gauge(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn histogram(&self, name: &str, labels: &Labels) -> Option<&Histogram> {
        match self.get(name, labels) {
            Some(MetricValue::Histogram(h)) => Some(h),
            _ => None,
        }
    }

    /// Prometheus text exposition, for scraping.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::new();
        let mut documented: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for series in &self.series {
            if documented.insert(&series.name) {
                out.push_str(&format!("# HELP {} {}\n", series.name, series.help));
                let kind = match series.value {
                    MetricValue::Counter(_) => "counter",
                    MetricValue::Gauge(_) => "gauge",
                    MetricValue::Histogram(_) => "histogram",
                };
                out.push_str(&format!("# TYPE {} {kind}\n", series.name));
            }
            let label_text = if series.labels.is_empty() {
                String::new()
            } else {
                let parts: Vec<String> = series
                    .labels
                    .iter()
                    .map(|(k, v)| format!("{k}=\"{}\"", v.replace('"', "\\\"")))
                    .collect();
                format!("{{{}}}", parts.join(","))
            };
            match &series.value {
                MetricValue::Counter(v) => {
                    out.push_str(&format!("{}{label_text} {v}\n", series.name));
                }
                MetricValue::Gauge(v) => {
                    out.push_str(&format!("{}{label_text} {v}\n", series.name));
                }
                MetricValue::Histogram(h) => {
                    let mut cumulative = 0u64;
                    for (i, bound) in h.bounds.iter().enumerate() {
                        cumulative += h.counts[i];
                        let mut bucket_labels = series.labels.clone();
                        bucket_labels.insert("le".into(), bound.to_string());
                        let parts: Vec<String> = bucket_labels
                            .iter()
                            .map(|(k, v)| format!("{k}=\"{v}\""))
                            .collect();
                        out.push_str(&format!(
                            "{}_bucket{{{}}} {cumulative}\n",
                            series.name,
                            parts.join(",")
                        ));
                    }
                    out.push_str(&format!("{}_sum{label_text} {}\n", series.name, h.sum));
                    out.push_str(&format!("{}_count{label_text} {}\n", series.name, h.count));
                }
            }
        }
        out
    }
}

/// Everything one registry holds, behind one lock.
///
/// The documentation lives beside the series rather than only on them, and in
/// the same mutex rather than a second one. Two locks would need an ordering
/// rule between them, and an ordering rule that exists only in a comment is a
/// deadlock waiting for the first person who does not read it.
#[derive(Debug, Default)]
struct Registry {
    series: BTreeMap<String, Series>,
    /// Help text by metric name, whether or not that metric has been recorded
    /// yet. See [`Metrics::describe`] for why it is kept apart from the series.
    help: BTreeMap<String, String>,
}

/// Thread-safe metric registry.
#[derive(Debug)]
pub struct Metrics {
    service: String,
    registry: Mutex<Registry>,
}

impl Metrics {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            registry: Mutex::new(Registry::default()),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Increment a counter.
    pub fn increment(&self, name: &str, labels: Labels, by: u64) {
        self.upsert(name, labels, |value| match value {
            MetricValue::Counter(v) => *v += by,
            other => *other = MetricValue::Counter(by),
        });
    }

    /// Increment a counter by one.
    pub fn count(&self, name: &str, labels: Labels) {
        self.increment(name, labels, 1);
    }

    /// Set a gauge.
    pub fn gauge(&self, name: &str, labels: Labels, value: f64) {
        self.upsert(name, labels, |slot| *slot = MetricValue::Gauge(value));
    }

    /// Record an observation into a histogram, creating it with `bounds` on
    /// first use.
    pub fn observe_with(
        &self,
        name: &str,
        labels: Labels,
        value: f64,
        template: fn() -> Histogram,
    ) {
        self.upsert(name, labels, |slot| match slot {
            MetricValue::Histogram(h) => h.observe(value),
            other => {
                let mut h = template();
                h.observe(value);
                *other = MetricValue::Histogram(Box::new(h));
            }
        });
    }

    /// Record a latency observation in milliseconds.
    pub fn observe_latency_ms(&self, name: &str, labels: Labels, millis: f64) {
        self.observe_with(name, labels, millis, Histogram::latency_ms);
    }

    /// Record a value in `[0, 1]`.
    pub fn observe_unit(&self, name: &str, labels: Labels, value: f64) {
        self.observe_with(name, labels, value, Histogram::unit_interval);
    }

    /// Attach documentation to a metric name, before or after it is recorded.
    ///
    /// The order used to matter and silently should not have. This walked the
    /// series that existed and set their help, so a component that described
    /// its metrics where it was assembled — the one place that knows what they
    /// mean, and the one place guaranteed to run once — described nothing,
    /// because no series existed yet. The help text was dropped without a word
    /// and every later `# HELP` line in the export read as a bare metric name.
    ///
    /// So the text is kept by name and applied to a series when it is created,
    /// as well as back-filled onto the series already recorded. Describing a
    /// metric still does not create it: a documented name that nothing ever
    /// records must stay absent from the export, because a series that appears
    /// merely by being mentioned is a metric nobody emits reading as one
    /// somebody does.
    pub fn describe(&self, name: &str, help: impl Into<String>) {
        let help = help.into();
        let mut guard = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        for series in guard.series.values_mut() {
            if series.name == name {
                series.help.clone_from(&help);
            }
        }
        guard.help.insert(name.to_string(), help);
    }

    fn upsert<F: FnOnce(&mut MetricValue)>(&self, name: &str, labels: Labels, update: F) {
        let key = series_key(name, &labels);
        let mut guard = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        // Split borrow: the help map is read only when a series is created, so
        // the common path — a counter that already exists being incremented —
        // does not clone the documentation on every increment.
        let Registry { series, help } = &mut *guard;
        let entry = series.entry(key).or_insert_with(|| Series {
            name: name.to_string(),
            labels,
            value: MetricValue::Counter(0),
            help: help.get(name).cloned().unwrap_or_default(),
        });
        update(&mut entry.value);
    }

    pub fn snapshot(&self) -> Snapshot {
        let guard = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            service: self.service.clone(),
            series: guard
                .series
                .values()
                .map(|s| SeriesSnapshot {
                    name: s.name.clone(),
                    labels: s.labels.clone(),
                    value: s.value.clone(),
                    help: s.help.clone(),
                })
                .collect(),
        }
    }

    /// Discard every recorded series.
    ///
    /// The documentation survives. A description is registered once where a
    /// component is assembled, and a reset that forgot it would leave every
    /// series recorded afterwards undocumented for the life of the process —
    /// with no second call to put it back.
    pub fn reset(&self) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .series
            .clear();
    }
}

fn series_key(name: &str, labels: &Labels) -> String {
    let parts: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{name}|{}", parts.join(","))
}

/// The metric names the platform publishes. Centralised so dashboards, alerts
/// and the documentation-drift test all read from one list.
pub mod names {
    // Streaming
    pub const EVENTS_PUBLISHED: &str = "qip_events_published_total";
    pub const EVENTS_DISPATCHED: &str = "qip_events_dispatched_total";
    pub const EVENT_HANDLER_FAILURES: &str = "qip_event_handler_failures_total";
    pub const EVENT_DUPLICATES: &str = "qip_event_duplicates_suppressed_total";
    pub const EVENT_LAG_MS: &str = "qip_event_lag_milliseconds";

    // Data
    pub const DATA_QUALITY_SCORE: &str = "qip_data_quality_score";
    pub const DATA_FRESHNESS_SECONDS: &str = "qip_data_freshness_seconds";
    pub const DATA_VALIDATION_FAILURES: &str = "qip_data_validation_failures_total";

    // Discovery and reasoning
    pub const SIGNALS_GENERATED: &str = "qip_signals_generated_total";
    pub const OPPORTUNITIES_DETECTED: &str = "qip_opportunities_detected_total";
    pub const HYPOTHESES_CREATED: &str = "qip_hypotheses_created_total";
    pub const HYPOTHESES_APPROVED: &str = "qip_hypotheses_approved_total";
    pub const HYPOTHESES_REJECTED: &str = "qip_hypotheses_rejected_total";
    pub const HYPOTHESIS_CONFIDENCE: &str = "qip_hypothesis_confidence";

    // Agents
    pub const AGENT_RUNS: &str = "qip_agent_runs_total";
    pub const AGENT_FAILURES: &str = "qip_agent_failures_total";
    pub const AGENT_DURATION_MS: &str = "qip_agent_duration_milliseconds";
    pub const AGENT_TOKENS: &str = "qip_agent_tokens_total";
    pub const AGENT_TOOL_CALLS: &str = "qip_agent_tool_calls_total";
    pub const AGENT_PERMISSION_DENIALS: &str = "qip_agent_permission_denials_total";

    // Optimisation
    pub const OPTIMIZATION_RUNS: &str = "qip_optimization_runs_total";
    pub const OPTIMIZATION_DURATION_MS: &str = "qip_optimization_duration_milliseconds";
    pub const SOLVER_SELECTED: &str = "qip_solver_selected_total";
    pub const QUANTUM_FALLBACKS: &str = "qip_quantum_fallbacks_total";

    // Risk and execution
    pub const RISK_EVALUATIONS: &str = "qip_risk_evaluations_total";
    pub const RISK_REJECTIONS: &str = "qip_risk_rejections_total";
    pub const KILL_SWITCH_ENGAGED: &str = "qip_kill_switch_engaged_total";
    pub const ORDERS_SUBMITTED: &str = "qip_orders_submitted_total";
    pub const ORDERS_FILLED: &str = "qip_orders_filled_total";
    pub const SLIPPAGE_BPS: &str = "qip_slippage_basis_points";
    pub const EXECUTION_LATENCY_MS: &str = "qip_execution_latency_milliseconds";

    // Portfolio
    pub const PORTFOLIO_VALUE: &str = "qip_portfolio_value";
    pub const PORTFOLIO_LEVERAGE: &str = "qip_portfolio_leverage";
    pub const PORTFOLIO_VAR: &str = "qip_portfolio_value_at_risk";
    pub const REALISED_PNL: &str = "qip_realised_pnl";
    pub const UNREALISED_PNL: &str = "qip_unrealised_pnl";

    // Cost
    pub const COMPUTE_COST: &str = "qip_compute_cost_units_total";

    // The cycle. One turn of SENSE → … → LEARN, as the kernel runs it.
    pub const CYCLES_RUN: &str = "qip_cycles_total";
    pub const CYCLE_DURATION_MS: &str = "qip_cycle_duration_milliseconds";
    pub const STAGE_RUNS: &str = "qip_stage_runs_total";
    pub const STAGE_DURATION_MS: &str = "qip_stage_duration_milliseconds";
    pub const STAGE_PROBLEMS: &str = "qip_stage_problems_total";
    /// Entries in the platform's own hash-chained event log. A gauge rather
    /// than a counter: it is the length of a log, not a rate of appends, and
    /// an operator asking whether the chain is growing wants the length.
    pub const EVENT_LOG_ENTRIES: &str = "qip_event_log_entries";
    pub const JOURNAL_FAILURES: &str = "qip_journal_write_failures_total";

    // Release. What the ACT stage signed, offered and got back.
    pub const PROPOSALS_SIGNED: &str = "qip_proposals_signed_total";
    pub const PROPOSALS_UNSIGNED: &str = "qip_proposals_unsigned_total";
    pub const ORDERS_REFUSED: &str = "qip_orders_refused_total";

    // Reasoning. Where the cost router put the decision, and whether the panel
    // convened as a result.
    pub const REASON_ROUTINGS: &str = "qip_reason_routings_total";

    /// What one cycle's [`qip_capital::ComputeLedger`] charged, and the running
    /// total since the process started.
    ///
    /// Both gauges, and deliberately not the `_total` counter above them: a
    /// compute charge is a `Decimal`, and a `u64` counter would truncate every
    /// fractional charge to zero — a bill that reads as free because each line
    /// on it rounded down. The crossing from `Decimal` to `f64` happens at the
    /// recording site and is commented there.
    pub const CYCLE_COMPUTE_COST: &str = "qip_cycle_compute_cost_units";
    pub const COMPUTE_SPEND: &str = "qip_compute_spend_units";

    /// Resyncs of the capital reservation ledger that found holds exceeding
    /// equity — the book claiming more capital is reserved than exists.
    ///
    /// Spelled here rather than at the recording site because it was spelled
    /// at the recording site: `qip-kernel` records this name as a bare string
    /// literal, so it is the one series the platform emits that this list does
    /// not know about. A name only the call site knows is a name a dashboard
    /// query, an alert policy and this module can each spell differently
    /// without anything noticing — and a metric nobody can find is a metric
    /// nobody reads. Recorded with a `reason` label naming which invariant
    /// failed.
    ///
    /// Registering the help text is the kernel's to do, in
    /// `Platform::describe_metrics`, which is where every other description in
    /// this list is registered:
    ///
    /// ```text
    /// metrics.describe(
    ///     names::RESERVATION_SHORTFALL,
    ///     "resyncs that found capital holds exceeding equity, by reason",
    /// );
    /// ```
    ///
    /// Until that lands the series exports with an empty `# HELP`, which is
    /// valid exposition and unreadable documentation.
    pub const RESERVATION_SHORTFALL: &str = "qip_reservation_shortfall";

    /// The four names the Cloud Monitoring alert policies in
    /// `infrastructure/terraform/modules/observability/main.tf` query.
    ///
    /// Spelled to match those queries exactly, and that is the whole point of
    /// them being here. The policies are gated behind `workload_metrics_exist`
    /// because Cloud Monitoring refuses a policy naming a descriptor it has
    /// never ingested — and until these were emitted no descriptor by these
    /// names could ever exist, so the gate could never be opened. Nothing else
    /// in the tree spelled them; the alerting layer and the platform had no
    /// name in common. Renaming any one of these breaks an alert policy that
    /// cannot say why it broke, so change these and the Terraform together or
    /// not at all.
    /// The edge plane. Every name below is recorded by `qip-edge`'s `Cell`
    /// into a registry the cell is *given*, and served by `qip-edge-node` at
    /// `/metrics`.
    ///
    /// They are spelled here rather than at the recording sites for the reason
    /// [`RESERVATION_SHORTFALL`] sets out: a name only the call site knows is
    /// a name a dashboard query and this module can each spell differently
    /// without anything noticing. Before these existed the cell knew its own
    /// halt state, its policy freshness and its netting ratio, and no operator
    /// could chart any of them — the facts were computed and then formatted
    /// into a journal string.
    ///
    /// **Every label on these series is bounded by something fixed at
    /// deployment or by an enum.** `cell` and `region` are one value each per
    /// process; `venue` is bounded by the cell's configured venue set; `gate`
    /// is bounded by the string literals `Cell::refuse` is called with; and
    /// `capability`, `source`, `kind` and `outcome` are each bounded by an
    /// enum. There is deliberately no label for an instrument, a strategy or
    /// an order id: those are unbounded, and a series per order id is how a
    /// metric registry becomes a memory leak.
    pub const EDGE_WORK_PASSES: &str = "qip_edge_work_passes_total";
    /// Whether the cell is stopped, by `source`. Two halts with two different
    /// release disciplines, so two series: a single boolean would tell an
    /// operator that the cell is stopped without saying which door to knock
    /// on.
    pub const EDGE_HALTED: &str = "qip_edge_halted";
    pub const EDGE_REFUSALS: &str = "qip_edge_refusals_total";
    pub const EDGE_SIGNALS_RAISED: &str = "qip_edge_signals_raised_total";
    /// How current each §6.2 capability is: `0` fresh, `1` stale, `2`
    /// unavailable. A numeric severity rather than one series per state so the
    /// chart an operator wants — "is anything degraded" — is a `max`.
    pub const EDGE_CAPABILITY_FRESHNESS: &str = "qip_edge_capability_freshness";
    /// The degradation table's sizing multiplier currently in force. A cell
    /// sizing at the 0.375 floor is an operator-visible fact.
    pub const EDGE_SIZING_MULTIPLIER: &str = "qip_edge_sizing_multiplier";
    /// The sequence of the policy payload this cell has applied, for
    /// correlation against what the central plane believes it published.
    pub const EDGE_POLICY_SEQUENCE: &str = "qip_edge_policy_sequence";
    /// Gross intent over net order volume (blueprint §27) — the best single
    /// summary of whether the strategy set has genuine diversity. A histogram
    /// rather than a gauge because it is a per-pass quantity and a gauge would
    /// report whichever pass happened to be last before the scrape.
    pub const EDGE_NETTING_RATIO: &str = "qip_edge_netting_ratio";
    pub const EDGE_ORDERS_PLACED: &str = "qip_edge_orders_placed_total";
    pub const EDGE_INTENTS_CANCELLED: &str = "qip_edge_intents_cancelled_total";
    pub const EDGE_INTERNAL_CROSSES: &str = "qip_edge_internal_crosses_total";
    pub const EDGE_RECONCILIATION_BREAKS: &str = "qip_edge_reconciliation_breaks_total";
    /// The cell's link to the central plane, recorded by the node from the
    /// same counters its health body publishes. Charting them is what turns
    /// "this cell stopped talking to the centre" from a number that stopped
    /// increasing in a JSON blob nothing collects into a series.
    pub const EDGE_MESH_DELTAS: &str = "qip_edge_mesh_deltas_total";
    pub const EDGE_MESH_GRANTS: &str = "qip_edge_mesh_grants_total";
    pub const EDGE_MESH_POLICY_FRAMES: &str = "qip_edge_mesh_policy_frames_total";
    /// The uplink circuit to the centre, by `state`: `1` on the state it is
    /// in and `0` on the others.
    pub const EDGE_MESH_CIRCUIT: &str = "qip_edge_mesh_circuit";

    pub const KILL_SWITCH_TRIPPED: &str = "qip_kill_switch_tripped";
    pub const LIVE_FILLS: &str = "qip_live_fills_total";
    pub const LIMIT_BREACHES: &str = "qip_limit_breaches";
    pub const PERMISSION_DENIALS: &str = "qip_permission_denials_total";

    /// Reconciliation breaks the central plane absorbed from a cell report,
    /// by `direction`: `cell_over_venue`, `venue_over_cell`, or `detail_only`
    /// where the quantities agree and the break is in the detail. The
    /// instrument and the cell are deliberately not labels — one is free
    /// text and the other is a fleet an operator can grow — so the series
    /// stays bounded by construction. Distinct from
    /// [`EDGE_RECONCILIATION_BREAKS`], which is what the cell itself found;
    /// this is what the centre acted on, and the two differ whenever a report
    /// was lost in transit.
    pub const CENTRAL_RECONCILIATION_BREAKS: &str = "qip_central_reconciliation_breaks_total";
    /// Scoped halts the central plane placed on a cell, by `cause`. The
    /// highest-consequence thing the plane does: a cell that stops trading
    /// because its book was wrong used to trip the kill switch and raise an
    /// incident without writing a series, so the one event an operator most
    /// needed to see was the one no chart could show.
    pub const CENTRAL_CELL_HALTS: &str = "qip_central_cell_halts_total";
}
