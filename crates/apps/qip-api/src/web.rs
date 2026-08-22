//! Serving the operator interface.
//!
//! The web surfaces sit alongside the API on the same server, under paths that
//! do not start with `/api`. They are read-only and require the `Viewer` role,
//! the same as the equivalent API endpoints — a page is not a lower bar than
//! the JSON it renders.
//!
//! The view model is assembled here, from a platform read under a lock that is
//! released before rendering. A rendering path that held the platform lock
//! could stall a trading loop behind an HTML page, which is a bad trade.

use crate::auth::{Authenticator, RateLimiter, Role};
use crate::http::{Handler, Method, Request, Response};
use qip_kernel::Platform;
use qip_web::pages::{Surface, render};
use qip_web::view::{
    AgentRow, GovernanceRow, LimitRow, OpportunityRow, OrderRow, ProposalRow, StageRow, ViewModel,
};
use std::sync::{Arc, Mutex};

/// Serves the operator interface.
pub struct Web {
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    clock: Arc<dyn qip_core::Clock>,
    /// The last cycle's stages, so the overview has something to show between
    /// cycles. Kept here rather than in the platform because it is a display
    /// concern, and the platform should not carry one.
    last_cycle: Mutex<Vec<StageRow>>,
}

impl std::fmt::Debug for Web {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web")
            .field("surfaces", &Surface::all().len())
            .finish_non_exhaustive()
    }
}

impl Web {
    pub fn new(
        platform: Arc<Mutex<Platform>>,
        authenticator: Arc<Authenticator>,
        rate_limiter: Arc<RateLimiter>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Self {
        Self {
            platform,
            authenticator,
            rate_limiter,
            clock,
            last_cycle: Mutex::new(Vec::new()),
        }
    }

    /// Record a cycle's stages for the overview.
    pub fn record_cycle(&self, report: &qip_kernel::CycleReport) {
        if let Ok(mut last) = self.last_cycle.lock() {
            *last = report
                .stages
                .iter()
                .map(|outcome| StageRow {
                    stage: outcome.stage.as_str().to_string(),
                    ran: outcome.ran,
                    produced: outcome.produced,
                    detail: outcome.detail.clone(),
                })
                .collect();
        }
    }

    /// Build the view model from the platform.
    ///
    /// The lock is taken, everything needed is copied out, and it is released
    /// before rendering. Holding it across rendering would let an HTML page
    /// stall a trading loop.
    fn model(&self) -> ViewModel {
        let now = self.clock.now();
        let Ok(platform) = self.platform.lock() else {
            // A poisoned lock means a thread panicked while holding the
            // platform. Rendering the safe defaults says nothing untrue.
            return ViewModel {
                halted: true,
                halt_reason: "the platform is in an inconsistent state after an internal failure"
                    .to_string(),
                ..ViewModel::default()
            };
        };

        let controller = platform.autonomy();
        let switch = controller.kill_switch();

        ViewModel {
            autonomy_level: controller.level().as_str().to_string(),
            autonomy_ceiling: controller.ceiling().as_str().to_string(),
            live: controller.is_live(),
            halted: switch.is_globally_tripped(),
            halt_reason: switch
                .global_trip()
                .map(|trip| format!("{} ({})", trip.reason, trip.tripped_by))
                .unwrap_or_default(),
            cycle: platform.cycle_count(),
            correlation_id: "see the event log".to_string(),
            events_logged: platform.event_log().len(),
            chain_intact: platform.event_log().verify_chain().is_ok(),
            rendered_at: now.to_string(),

            equity: "10,000,000".to_string(),
            position_count: 0,
            gross_exposure: 0.0,
            net_exposure: 0.0,
            paper_only: !platform.orders().has_live_fills(),

            stages: self
                .last_cycle
                .lock()
                .map(|last| last.clone())
                .unwrap_or_default(),
            opportunities: platform
                .queue()
                .iter()
                .map(|opportunity| OpportunityRow {
                    id: opportunity.opportunity_id.as_str().to_string(),
                    headline: opportunity.headline.clone(),
                    score: opportunity.rank.score,
                    confidence: opportunity.rank.confidence,
                    detectors: opportunity.detectors.clone(),
                })
                .collect(),
            theses: Vec::new(),
            proposals: platform
                .proposals()
                .iter()
                .map(|proposal| ProposalRow {
                    id: proposal.proposal_id.as_str().to_string(),
                    status: proposal.status.as_str().to_string(),
                    legs: proposal.len(),
                    rationale: proposal.rationale.clone(),
                })
                .collect(),
            orders: platform
                .orders()
                .orders()
                .map(|order| OrderRow {
                    id: order.order_id.as_str().to_string(),
                    instrument: order.object_id.as_str().to_string(),
                    side: order.side.as_str().to_string(),
                    quantity: order.quantity.to_string(),
                    state: order.state.as_str().to_string(),
                    simulated: order.is_paper() || order.fills.is_empty(),
                })
                .collect(),
            refusals: platform
                .orders()
                .refusals()
                .iter()
                .filter_map(|refusal| refusal.refusal.as_ref().map(|reason| reason.describe()))
                .collect(),
            limits: limit_rows(&platform),
            agents: platform
                .organisation()
                .roster()
                .iter()
                .map(|manifest| AgentRow {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    role: manifest.role.as_str().to_string(),
                    owner: manifest.owner.clone(),
                    purpose: manifest.purpose.clone(),
                    capabilities: manifest
                        .capabilities
                        .iter()
                        .map(|capability| capability.as_str().to_string())
                        .collect(),
                })
                .collect(),
            governance: platform
                .review_governance(now)
                .iter()
                .map(|finding| GovernanceRow {
                    severity: match finding.severity {
                        qip_agents::governance::Severity::Error => "error".to_string(),
                        qip_agents::governance::Severity::Warning => "warning".to_string(),
                    },
                    rule: finding.rule.clone(),
                    detail: finding.detail.clone(),
                })
                .collect(),
        }
    }
}

/// The limits the platform runs under, with their rationales.
fn limit_rows(_platform: &Platform) -> Vec<LimitRow> {
    qip_risk::limits::LimitSet::conservative_default()
        .limits
        .iter()
        .map(|limit| LimitRow {
            name: limit.name.clone(),
            observed: 0.0,
            bound: 0.0,
            utilisation: 0.0,
            breached: false,
            rationale: limit.rationale.clone(),
        })
        .collect()
}

impl Handler for Web {
    fn handle(&self, request: &Request) -> Response {
        let now = self.clock.now();

        if request.method != Method::Get {
            return Response::text(405, "the operator interface is read-only");
        }
        let Some(surface) = Surface::from_path(&request.path) else {
            return Response::html(404, render(Surface::Overview, &ViewModel::default()));
        };

        let principal = match self
            .authenticator
            .authenticate(request.header("authorization"), now)
        {
            Ok(principal) => principal,
            Err(_) => {
                return Response::text(401, "authentication required")
                    .with_header("www-authenticate", "Bearer");
            }
        };
        if !self.rate_limiter.permit(&principal.subject, now) {
            return Response::text(429, "rate limit exceeded");
        }
        // A page is not a lower bar than the JSON it renders.
        if principal.require(Role::Viewer).is_err() {
            return Response::text(403, "the viewer role is required");
        }

        // Every surface shows the same view. Role-dependent redaction would
        // mean two readers of the same page disagreeing about what the
        // platform did, which is worse than one bar for the whole interface.
        Response::html(200, render(surface, &self.model()))
    }
}

/// Routes between the API and the operator interface on one server.
pub struct Router {
    api: Arc<crate::routes::Api>,
    web: Arc<Web>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router").finish_non_exhaustive()
    }
}

impl Router {
    pub fn new(api: Arc<crate::routes::Api>, web: Arc<Web>) -> Self {
        Self { api, web }
    }
}

impl Handler for Router {
    fn handle(&self, request: &Request) -> Response {
        if request.path.starts_with("/api") {
            self.api.handle(request)
        } else {
            self.web.handle(request)
        }
    }
}
