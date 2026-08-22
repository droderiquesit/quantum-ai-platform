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

/// Thread-safe metric registry.
#[derive(Debug)]
pub struct Metrics {
    service: String,
    series: Mutex<BTreeMap<String, Series>>,
}

impl Metrics {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            series: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Increment a counter.
    pub fn increment(&self, name: &str, labels: Labels, by: u64) {
        self.upsert(name, labels, "", |value| match value {
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
        self.upsert(name, labels, "", |slot| *slot = MetricValue::Gauge(value));
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
        self.upsert(name, labels, "", |slot| match slot {
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

    /// Attach documentation to a metric name.
    pub fn describe(&self, name: &str, help: impl Into<String>) {
        let help = help.into();
        let mut guard = self.series.lock().unwrap_or_else(|e| e.into_inner());
        for series in guard.values_mut() {
            if series.name == name {
                series.help = help.clone();
            }
        }
    }

    fn upsert<F: FnOnce(&mut MetricValue)>(
        &self,
        name: &str,
        labels: Labels,
        help: &str,
        update: F,
    ) {
        let key = series_key(name, &labels);
        let mut guard = self.series.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.entry(key).or_insert_with(|| Series {
            name: name.to_string(),
            labels,
            value: MetricValue::Counter(0),
            help: help.to_string(),
        });
        update(&mut entry.value);
    }

    pub fn snapshot(&self) -> Snapshot {
        let guard = self.series.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            service: self.service.clone(),
            series: guard
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

    pub fn reset(&self) {
        self.series
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
}
