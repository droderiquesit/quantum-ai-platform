//! The node's own metric seam: the mesh link, as a series.
//!
//! The cell records its own facts through `qip_edge::CellMetrics`. The link to
//! the central plane is not the cell's — it belongs to this node — and until
//! now its counters existed only inside the JSON health body, which nothing
//! collects. "This cell has stopped talking to the centre" was a number that
//! stopped increasing in a blob nobody scrapes.
//!
//! [`MeshSeries`] converts that link's cumulative totals into counter
//! increments. It has to hold the previous reading to do it: `MeshHealth`
//! reports totals since the process started, and a metric registry only knows
//! how to add. Publishing a total into a counter would double-count it on
//! every probe — a delta delivered once would read as delivered as many times
//! as anything asked whether the node was alive.

use crate::mesh::MeshHealth;
use qip_observability::metrics::{Labels, Metrics, names};
use qip_transport::breaker::BreakerState;
use std::sync::Arc;

/// The path of an HTTP request line, or `None` if this is not one.
///
/// It exists because the node's health server answered every request
/// identically until it had exposition to serve, and a `/metrics` path
/// returning a health blob is silently unparseable to every collector that
/// asks. The path is the whole second token and callers match it in full: a
/// prefix match would route `/metricsanything` to the exposition, and this
/// node's health surface is the one thing an orchestrator trusts to be what it
/// says it is.
pub fn requested_path(request: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(request).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split(' ');
    let method = parts.next()?;
    if method != "GET" && method != "HEAD" {
        return None;
    }
    parts.next()
}

/// What the node answers a request with, and the media type it answers under.
///
/// The whole of the node's routing, in the library rather than in `main.rs`,
/// because it is the seam this scrape surface is worth nothing without: a
/// `/metrics` path answering with the health JSON is a surface that looks
/// present in a manifest and is unparseable to every collector that asks. A
/// four-line match inside a binary is a four-line match nothing can test.
///
/// A scraper reads the media type to decide how to parse what follows, so the
/// two answers carry different ones. Everything that is not the exposition is
/// the health body: this node has one probe answer and always did, and adding
/// a scrape route must not turn an unrecognised path into a 404 an
/// orchestrator has never had to handle before.
pub fn respond(request: &[u8], metrics: &Metrics, health_body: &str) -> (&'static str, String) {
    if requested_path(request) == Some("/metrics") {
        return (
            "text/plain; version=0.0.4; charset=utf-8",
            metrics.snapshot().to_prometheus(),
        );
    }
    ("application/json", health_body.to_string())
}

/// The mesh link's counters, rendered as deltas.
///
/// Every label is bounded: `cell` and `region` are fixed for the process,
/// `outcome` and `state` are enum-shaped sets of string literals. There is no
/// per-peer label because a cell has one peer.
#[derive(Debug)]
pub struct MeshSeries {
    metrics: Arc<Metrics>,
    base: Labels,
    previous: Totals,
}

/// The last reading, so the next one can be published as a difference.
#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    delivered: u64,
    dead_lettered: u64,
    circuit_refusals: u64,
    grants_verified: u64,
    grants_refused: u64,
    grants_duplicate: u64,
    policy_verified: u64,
    policy_refused: u64,
}

impl MeshSeries {
    pub fn new(metrics: Arc<Metrics>, cell: &str, region: &str) -> Self {
        let mut base = Labels::new();
        base.insert("cell".to_string(), cell.to_string());
        base.insert("region".to_string(), region.to_string());
        metrics.describe(
            names::EDGE_MESH_DELTAS,
            "state deltas this cell offered the central plane, by outcome",
        );
        metrics.describe(
            names::EDGE_MESH_GRANTS,
            "capital grants this cell received from the central plane, by outcome",
        );
        metrics.describe(
            names::EDGE_MESH_POLICY_FRAMES,
            "policy payloads this cell received from the central plane, by outcome",
        );
        metrics.describe(
            names::EDGE_MESH_CIRCUIT,
            "the uplink circuit to the central plane: 1 on the state it is in",
        );
        Self {
            metrics,
            base,
            previous: Totals::default(),
        }
    }

    fn labelled(&self, key: &str, value: &str) -> Labels {
        let mut labels = self.base.clone();
        labels.insert(key.to_string(), value.to_string());
        labels
    }

    /// Publish what has happened on the link since the last call.
    ///
    /// `saturating_sub` rather than a subtraction that could wrap: the totals
    /// are monotone today, and a future reset that made one go backwards would
    /// otherwise publish an increment of about eighteen quintillion. A counter
    /// that stalls for one probe is a reporting gap; one that jumps by `u64`
    /// range is an incident nobody can distinguish from a real one.
    pub fn observe(&mut self, health: &MeshHealth) {
        let current = Totals {
            delivered: health.uplink.delivered,
            dead_lettered: health.uplink.dead_lettered,
            circuit_refusals: health.uplink.circuit_refusals,
            grants_verified: health.downlink.verified,
            grants_refused: health.downlink.refused,
            grants_duplicate: health.downlink.duplicates,
            policy_verified: health.policy.verified,
            policy_refused: health.policy.refused,
        };
        let previous = self.previous;
        self.previous = current;

        for (name, outcome, now, before) in [
            (
                names::EDGE_MESH_DELTAS,
                "delivered",
                current.delivered,
                previous.delivered,
            ),
            (
                names::EDGE_MESH_DELTAS,
                "dead_lettered",
                current.dead_lettered,
                previous.dead_lettered,
            ),
            (
                names::EDGE_MESH_DELTAS,
                "circuit_refused",
                current.circuit_refusals,
                previous.circuit_refusals,
            ),
            (
                names::EDGE_MESH_GRANTS,
                "verified",
                current.grants_verified,
                previous.grants_verified,
            ),
            (
                names::EDGE_MESH_GRANTS,
                "refused",
                current.grants_refused,
                previous.grants_refused,
            ),
            (
                names::EDGE_MESH_GRANTS,
                "duplicate",
                current.grants_duplicate,
                previous.grants_duplicate,
            ),
            (
                names::EDGE_MESH_POLICY_FRAMES,
                "verified",
                current.policy_verified,
                previous.policy_verified,
            ),
            (
                names::EDGE_MESH_POLICY_FRAMES,
                "refused",
                current.policy_refused,
                previous.policy_refused,
            ),
        ] {
            let by = now.saturating_sub(before);
            if by > 0 {
                // `outcome` is one of the eight literals in the table above;
                // the series count is fixed by this source file.
                self.metrics
                    .increment(name, self.labelled("outcome", outcome), by);
            }
        }

        // Every state is written, not only the one in force. A gauge set to 1
        // on `open` and never cleared would still say the circuit is open long
        // after it closed, which is the only fact on this link an operator is
        // ever paged about. `state` is the three-variant `BreakerState`, so
        // this is three series per cell.
        for state in [
            BreakerState::Closed,
            BreakerState::Open,
            BreakerState::HalfOpen,
        ] {
            self.metrics.gauge(
                names::EDGE_MESH_CIRCUIT,
                self.labelled("state", state.as_str()),
                f64::from(u8::from(state == health.circuit)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_edge::mesh::{DownlinkStats, PolicyDownlinkStats, UplinkStats};
    use qip_observability::metrics::labels;

    fn health(delivered: u64, circuit: BreakerState) -> MeshHealth {
        MeshHealth {
            uplink: UplinkStats {
                published: delivered,
                delivered,
                circuit_refusals: 0,
                dead_lettered: 0,
            },
            downlink: DownlinkStats::default(),
            policy: PolicyDownlinkStats::default(),
            circuit,
        }
    }

    #[test]
    fn a_cumulative_total_is_published_once_rather_than_on_every_probe() {
        // The failure this prevents: publishing the total instead of the
        // delta. This node observes the link on every liveness probe, so a
        // single delivered delta would read as delivered once per probe —
        // hundreds of times an hour for one event.
        let metrics = Arc::new(Metrics::new("qip-edge-node"));
        let mut series = MeshSeries::new(Arc::clone(&metrics), "cell-a", "eu-west");
        let health = health(3, BreakerState::Closed);

        series.observe(&health);
        assert_eq!(
            metrics.snapshot().counter(
                names::EDGE_MESH_DELTAS,
                &labels([
                    ("cell", "cell-a"),
                    ("outcome", "delivered"),
                    ("region", "eu-west")
                ])
            ),
            3,
            "the premise failed: three delivered deltas were not counted at all"
        );

        // The same reading again. Nothing new happened on the link.
        series.observe(&health);
        assert_eq!(
            metrics.snapshot().counter(
                names::EDGE_MESH_DELTAS,
                &labels([
                    ("cell", "cell-a"),
                    ("outcome", "delivered"),
                    ("region", "eu-west")
                ])
            ),
            3,
            "re-observing an unchanged total counted the same deltas twice"
        );
    }

    #[test]
    fn a_circuit_that_closes_stops_reporting_itself_as_open() {
        let metrics = Arc::new(Metrics::new("qip-edge-node"));
        let mut series = MeshSeries::new(Arc::clone(&metrics), "cell-a", "eu-west");

        series.observe(&health(0, BreakerState::Open));
        let open = labels([("cell", "cell-a"), ("region", "eu-west"), ("state", "open")]);
        assert_eq!(
            metrics.snapshot().gauge(names::EDGE_MESH_CIRCUIT, &open),
            Some(1.0),
            "the premise failed: an open circuit never reported as open"
        );

        series.observe(&health(0, BreakerState::Closed));
        assert_eq!(
            metrics.snapshot().gauge(names::EDGE_MESH_CIRCUIT, &open),
            Some(0.0),
            "a circuit that closed still reports as open, which is the one fact here anybody pages on"
        );
    }

    #[test]
    fn a_path_that_merely_starts_with_metrics_is_not_the_scrape_surface() {
        // Substring matching is a trap this repository has already been
        // caught by: `/metrics` is a prefix of a great many paths, and a probe
        // routed to Prometheus exposition is a probe an orchestrator cannot
        // parse.
        assert_eq!(
            requested_path(b"GET /metrics HTTP/1.1\r\n\r\n"),
            Some("/metrics"),
            "the premise failed: a plain scrape was not recognised"
        );
        assert_eq!(
            requested_path(b"GET /metricsomething HTTP/1.1\r\n\r\n"),
            Some("/metricsomething"),
            "a longer path was truncated to the scrape route"
        );
        assert_eq!(
            requested_path(b"POST /metrics HTTP/1.1\r\n\r\n"),
            None,
            "a write method was treated as a read of the scrape surface"
        );
    }

    #[test]
    fn a_scrape_is_answered_with_what_the_cell_recorded_and_not_with_the_probe_body() {
        // The failure this prevents: a `/metrics` route that returns the
        // health blob. It answers 200 under `application/json`, an
        // orchestrator sees a healthy endpoint, and the collector silently
        // ingests nothing — the exact shape of gap this whole seam exists to
        // close, rebuilt one level up.
        let metrics = Metrics::new("qip-edge-node");
        metrics.describe(names::EDGE_WORK_PASSES, "passes of the cell's loop");
        metrics.count(names::EDGE_WORK_PASSES, labels([("cell", "cell-a")]));

        let health_body = r#"{"cell":"cell-a","halted":false}"#;
        let (content_type, body) = respond(b"GET /metrics HTTP/1.1\r\n\r\n", &metrics, health_body);
        assert_eq!(
            content_type, "text/plain; version=0.0.4; charset=utf-8",
            "exposition served under a media type no scraper parses as exposition"
        );
        assert!(
            body.contains(names::EDGE_WORK_PASSES),
            "the scrape did not carry the recorded series: {body}"
        );
        assert!(
            !body.contains("\"halted\""),
            "the scrape carried the health body instead of the exposition: {body}"
        );
    }

    #[test]
    fn a_probe_still_gets_the_health_body_after_the_scrape_route_was_added() {
        // The other half, and the one a routing change breaks silently: an
        // orchestrator's liveness probe is what decides whether this cell is
        // restarted.
        let metrics = Metrics::new("qip-edge-node");
        let health_body = r#"{"cell":"cell-a","halted":false}"#;
        let (content_type, body) = respond(b"GET / HTTP/1.1\r\n\r\n", &metrics, health_body);
        assert_eq!(content_type, "application/json");
        assert_eq!(body, health_body);
    }
}
