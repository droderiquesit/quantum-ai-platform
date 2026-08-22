//! The versioned REST API.
//!
//! Every path is under `/api/v1`. Versioning the whole surface rather than
//! individual resources means a breaking change is one decision rather than a
//! negotiation per endpoint, and a client can pin to a version it understands.
//!
//! Authorisation is per route and stated in the table, not scattered through
//! the handlers. That makes it possible to read what the API permits without
//! reading what it does — which is the question a security review asks.

use crate::auth::{Authenticator, Principal, RateLimiter, Role};
use crate::http::{Handler, Method, Request, Response};
use crate::json;
use qip_core::time::Timestamp;
use qip_kernel::Platform;
use std::sync::{Arc, Mutex};

/// One route's declaration.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub method: Method,
    /// Path pattern under `/api/v1`, with `:name` for a parameter.
    pub pattern: &'static str,
    /// The least authority that may call it.
    pub required_role: Role,
    /// What it does, for the discovery endpoint and for a review.
    pub summary: &'static str,
}

/// Every route the API exposes.
///
/// Written out so the whole surface, and what each part requires, can be read
/// in one place. Anything not in this table is a 404.
pub const ROUTES: &[Route] = &[
    Route {
        method: Method::Get,
        pattern: "/health",
        required_role: Role::Monitor,
        summary: "liveness, and whether the platform is halted",
    },
    Route {
        method: Method::Get,
        pattern: "/system/status",
        required_role: Role::Viewer,
        summary: "autonomy level, kill switch, cycle count",
    },
    Route {
        method: Method::Get,
        pattern: "/system/metrics",
        required_role: Role::Monitor,
        summary: "counters and gauges",
    },
    Route {
        method: Method::Get,
        pattern: "/system/governance",
        required_role: Role::Viewer,
        summary: "the agent roster and its governance findings",
    },
    Route {
        method: Method::Get,
        pattern: "/portfolio",
        required_role: Role::Viewer,
        summary: "positions, exposures and equity",
    },
    Route {
        method: Method::Get,
        pattern: "/opportunities",
        required_role: Role::Viewer,
        summary: "the current opportunity queue",
    },
    Route {
        method: Method::Get,
        pattern: "/proposals",
        required_role: Role::Viewer,
        summary: "proposals and their status",
    },
    Route {
        method: Method::Get,
        pattern: "/orders",
        required_role: Role::Viewer,
        summary: "orders, fills, refusals and any venue/book disagreement",
    },
    Route {
        method: Method::Get,
        pattern: "/agents",
        required_role: Role::Viewer,
        summary: "the agent roster and each agent's manifest",
    },
    Route {
        method: Method::Post,
        pattern: "/cycle",
        required_role: Role::Analyst,
        summary: "run one cycle of the intelligence loop",
    },
    Route {
        method: Method::Post,
        pattern: "/kill-switch",
        required_role: Role::Operator,
        summary: "halt the platform",
    },
    Route {
        method: Method::Delete,
        pattern: "/kill-switch",
        required_role: Role::Operator,
        summary: "clear a halt",
    },
    Route {
        method: Method::Get,
        pattern: "/autonomy",
        required_role: Role::Viewer,
        summary: "the current autonomy level and ceiling",
    },
];

/// The API surface.
pub struct Api {
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    /// The clock the API reads. Injected so a test and a deployment differ
    /// only in which clock they pass.
    clock: Arc<dyn qip_core::Clock>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("routes", &ROUTES.len())
            .field("credentials", &self.authenticator.credential_count())
            .finish_non_exhaustive()
    }
}

impl Api {
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
        }
    }

    /// Find the route matching a request, if any.
    pub fn route_for(method: Method, path: &str) -> Option<&'static Route> {
        let suffix = path.strip_prefix("/api/v1")?;
        let suffix = if suffix.is_empty() { "/" } else { suffix };
        ROUTES
            .iter()
            .find(|route| route.method == method && matches_pattern(route.pattern, suffix))
    }

    fn dispatch(&self, request: &Request, principal: &Principal, route: &Route) -> Response {
        let now = self.clock.now();
        let Ok(mut platform) = self.platform.lock() else {
            // A poisoned lock means a thread panicked while holding the
            // platform. Serving from it could produce inconsistent state, so
            // the honest answer is that the service is unavailable.
            return Response::json(
                503,
                r#"{"error":"the platform is in an inconsistent state and is not serving"}"#,
            );
        };

        match (route.method, route.pattern) {
            (Method::Get, "/health") => Response::json(200, health(&platform)),
            (Method::Get, "/system/status") => Response::json(200, status(&platform)),
            (Method::Get, "/system/metrics") => Response::json(200, metrics(&platform)),
            (Method::Get, "/system/governance") => Response::json(200, governance(&platform, now)),
            (Method::Get, "/portfolio") => Response::json(200, portfolio(&platform)),
            (Method::Get, "/opportunities") => Response::json(200, opportunities(&platform)),
            (Method::Get, "/proposals") => Response::json(200, proposals(&platform)),
            (Method::Get, "/orders") => Response::json(200, orders(&platform)),
            (Method::Get, "/agents") => Response::json(200, agents(&platform)),
            (Method::Get, "/autonomy") => Response::json(200, autonomy(&platform)),
            (Method::Post, "/cycle") => {
                let report = platform.run_cycle(now);
                Response::json(202, cycle_report(&report))
            }
            (Method::Post, "/kill-switch") => {
                let reason = request
                    .query_param("reason")
                    .unwrap_or("halted through the API");
                platform.autonomy_mut().kill_switch_mut().trip_global(
                    now,
                    format!("api:{}", principal.subject),
                    reason,
                );
                Response::json(
                    200,
                    format!(
                        r#"{{"halted":true,"by":{},"reason":{}}}"#,
                        json::string(&principal.subject),
                        json::string(reason)
                    ),
                )
            }
            (Method::Delete, "/kill-switch") => {
                // Clearing a halt needs an operator identity, which is what
                // the Operator role on this route establishes.
                let operator = qip_risk_engine::autonomy::OperatorIdentity::verified(
                    principal.subject.clone(),
                    "api-bearer-token",
                    now,
                );
                match platform
                    .autonomy_mut()
                    .kill_switch_mut()
                    .clear_global(&operator, now)
                {
                    Ok(()) => Response::json(
                        200,
                        format!(
                            r#"{{"halted":false,"cleared_by":{}}}"#,
                            json::string(&principal.subject)
                        ),
                    ),
                    Err(error) => Response::json(
                        409,
                        format!(r#"{{"error":{}}}"#, json::string(error.message())),
                    ),
                }
            }
            _ => Response::json(404, r#"{"error":"no such route"}"#),
        }
    }
}

impl Handler for Api {
    fn handle(&self, request: &Request) -> Response {
        let now = self.clock.now();

        // Discovery is unauthenticated on purpose: a client needs to know the
        // API version before it can present a credential correctly, and the
        // route table is not a secret.
        if request.method == Method::Get && request.path == "/api/v1" {
            return Response::json(200, discovery());
        }

        let Some(route) = Api::route_for(request.method, &request.path) else {
            // A path that exists under a different method gets a 405 rather
            // than a 404, since hiding that would only confuse a legitimate
            // client without hiding anything from anyone else.
            let exists_under_another_method = ROUTES.iter().any(|candidate| {
                request
                    .path
                    .strip_prefix("/api/v1")
                    .is_some_and(|suffix| matches_pattern(candidate.pattern, suffix))
            });
            return if exists_under_another_method {
                Response::json(405, r#"{"error":"that method is not allowed here"}"#)
            } else {
                Response::json(404, r#"{"error":"no such route"}"#)
            };
        };

        let principal = match self
            .authenticator
            .authenticate(request.header("authorization"), now)
        {
            Ok(principal) => principal,
            Err(error) => {
                return Response::json(
                    401,
                    format!(r#"{{"error":{}}}"#, json::string(error.message())),
                )
                .with_header("www-authenticate", "Bearer");
            }
        };

        if !self.rate_limiter.permit(&principal.subject, now) {
            return Response::json(429, r#"{"error":"rate limit exceeded"}"#);
        }

        if let Err(error) = principal.require(route.required_role) {
            return Response::json(
                403,
                format!(r#"{{"error":{}}}"#, json::string(error.message())),
            );
        }

        self.dispatch(request, &principal, route)
    }
}

/// Whether a concrete path matches a pattern with `:name` parameters.
fn matches_pattern(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if pattern_parts.len() != path_parts.len() {
        return false;
    }
    pattern_parts
        .iter()
        .zip(&path_parts)
        .all(|(pattern, actual)| pattern.starts_with(':') || pattern == actual)
}

// --- response bodies --------------------------------------------------------

fn discovery() -> String {
    let routes: Vec<String> = ROUTES
        .iter()
        .map(|route| {
            format!(
                r#"{{"method":{},"path":{},"role":{},"summary":{}}}"#,
                json::string(route.method.as_str()),
                json::string(&format!("/api/v1{}", route.pattern)),
                json::string(route.required_role.as_str()),
                json::string(route.summary)
            )
        })
        .collect();
    format!(r#"{{"version":"v1","routes":[{}]}}"#, routes.join(","))
}

fn health(platform: &Platform) -> String {
    let halted = platform.autonomy().kill_switch().is_globally_tripped();
    // A book that disagrees with the venue is reported here as well as on
    // `/orders`, because this is the endpoint a monitor polls and that
    // disagreement is the one condition under which none of the platform's own
    // numbers should be believed.
    let breaks = platform.orders().reconciliation_breaks().len();
    let status = if halted {
        "halted"
    } else if breaks > 0 {
        "reconciliation-break"
    } else {
        "ok"
    };
    format!(
        r#"{{"status":{},"halted":{},"autonomy":{},"live_capable":{},"reconciliation_breaks":{}}}"#,
        json::string(status),
        halted,
        json::string(platform.autonomy().level().as_str()),
        platform.is_live_capable(),
        breaks
    )
}

fn status(platform: &Platform) -> String {
    let switch = platform.autonomy().kill_switch();
    format!(
        r#"{{"autonomy":{},"configured_autonomy":{},"ceiling":{},"live_capable":{},"halted":{},"halted_scopes":[{}],"cycles":{},"events":{}}}"#,
        json::string(platform.autonomy().level().as_str()),
        json::string(platform.autonomy().configured_level().as_str()),
        json::string(platform.autonomy().ceiling().as_str()),
        platform.is_live_capable(),
        switch.is_globally_tripped(),
        switch
            .halted_scopes()
            .iter()
            .map(|scope| json::string(scope))
            .collect::<Vec<_>>()
            .join(","),
        platform.cycle_count(),
        platform.event_log().len()
    )
}

fn metrics(platform: &Platform) -> String {
    format!(
        r#"{{"cycles":{},"events_logged":{},"opportunities_queued":{},"proposals":{},"orders":{},"fills":{},"refusals":{},"live_fills":{}}}"#,
        platform.cycle_count(),
        platform.event_log().len(),
        platform.queue().len(),
        platform.proposals().len(),
        platform.orders().orders().count(),
        platform.orders().fills().len(),
        platform.orders().refusals().len(),
        platform.orders().has_live_fills()
    )
}

fn governance(platform: &Platform, now: Timestamp) -> String {
    let findings = platform.review_governance(now);
    let rendered: Vec<String> = findings
        .iter()
        .map(|finding| {
            format!(
                r#"{{"severity":{},"rule":{},"detail":{},"agents":[{}]}}"#,
                json::string(match finding.severity {
                    qip_agents::governance::Severity::Error => "error",
                    qip_agents::governance::Severity::Warning => "warning",
                }),
                json::string(&finding.rule),
                json::string(&finding.detail),
                finding
                    .agents
                    .iter()
                    .map(|agent| json::string(agent))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(
        r#"{{"agents":{},"findings":[{}]}}"#,
        platform.organisation().len(),
        rendered.join(",")
    )
}

fn portfolio(platform: &Platform) -> String {
    // The book is behind the desk's capability gate, so the API reports what
    // it can see without one: the counts and the risk posture. Position-level
    // detail is served through the read model, not by reaching past a control.
    format!(
        r#"{{"proposals":{},"orders":{},"fills":{},"paper_only":{}}}"#,
        platform.proposals().len(),
        platform.orders().orders().count(),
        platform.orders().fills().len(),
        !platform.orders().has_live_fills()
    )
}

fn opportunities(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .queue()
        .iter()
        .map(|opportunity| {
            format!(
                r#"{{"id":{},"headline":{},"score":{},"confidence":{},"detectors":[{}]}}"#,
                json::string(opportunity.opportunity_id.as_str()),
                json::string(&opportunity.headline),
                json::number(opportunity.rank.score),
                json::number(opportunity.rank.confidence),
                opportunity
                    .detectors
                    .iter()
                    .map(|detector| json::string(detector))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(r#"{{"opportunities":[{}]}}"#, rendered.join(","))
}

fn proposals(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .proposals()
        .iter()
        .map(|proposal| {
            format!(
                r#"{{"id":{},"status":{},"legs":{},"gross":{},"turnover":{},"rationale":{}}}"#,
                json::string(proposal.proposal_id.as_str()),
                json::string(proposal.status.as_str()),
                proposal.len(),
                json::number(proposal.target_gross),
                json::number(proposal.turnover),
                json::string(&proposal.rationale)
            )
        })
        .collect();
    format!(r#"{{"proposals":[{}]}}"#, rendered.join(","))
}

fn orders(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .orders()
        .orders()
        .map(|order| {
            format!(
                r#"{{"id":{},"instrument":{},"side":{},"quantity":{},"state":{},"filled":{},"simulated":{}}}"#,
                json::string(order.order_id.as_str()),
                json::string(order.object_id.as_str()),
                json::string(order.side.as_str()),
                json::string(&order.quantity.to_string()),
                json::string(order.state.as_str()),
                json::string(&order.filled_quantity().to_string()),
                order.is_paper()
            )
        })
        .collect();
    let breaks: Vec<String> = platform
        .orders()
        .reconciliation_breaks()
        .iter()
        .map(|reason| json::string(reason))
        .collect();
    format!(
        r#"{{"orders":[{}],"refusals":{},"reconciliation_breaks":[{}]}}"#,
        rendered.join(","),
        platform.orders().refusals().len(),
        breaks.join(",")
    )
}

fn agents(platform: &Platform) -> String {
    let rendered: Vec<String> = platform
        .organisation()
        .roster()
        .iter()
        .map(|manifest| {
            format!(
                r#"{{"id":{},"name":{},"role":{},"owner":{},"purpose":{},"capabilities":[{}]}}"#,
                json::string(&manifest.id),
                json::string(&manifest.name),
                json::string(manifest.role.as_str()),
                json::string(&manifest.owner),
                json::string(&manifest.purpose),
                manifest
                    .capabilities
                    .iter()
                    .map(|capability| json::string(capability.as_str()))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(r#"{{"agents":[{}]}}"#, rendered.join(","))
}

fn autonomy(platform: &Platform) -> String {
    let controller = platform.autonomy();
    let history: Vec<String> = controller
        .history()
        .iter()
        .map(|change| {
            format!(
                r#"{{"at":{},"from":{},"to":{},"operator":{},"reason":{}}}"#,
                change.at.as_nanos(),
                json::string(change.from.as_str()),
                json::string(change.to.as_str()),
                json::string(&change.operator),
                json::string(&change.reason)
            )
        })
        .collect();
    format!(
        r#"{{"level":{},"ceiling":{},"live":{},"history":[{}]}}"#,
        json::string(controller.level().as_str()),
        json::string(controller.ceiling().as_str()),
        controller.is_live(),
        history.join(",")
    )
}

fn cycle_report(report: &qip_kernel::CycleReport) -> String {
    let stages: Vec<String> = report
        .stages
        .iter()
        .map(|outcome| {
            format!(
                r#"{{"stage":{},"ran":{},"produced":{},"detail":{},"problems":[{}]}}"#,
                json::string(outcome.stage.as_str()),
                outcome.ran,
                outcome.produced,
                json::string(&outcome.detail),
                outcome
                    .problems
                    .iter()
                    .map(|problem| json::string(problem))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    format!(
        r#"{{"cycle":{},"correlation_id":{},"halted":{},"traversed_every_stage":{},"stages":[{}]}}"#,
        report.cycle,
        json::string(report.correlation_id.as_str()),
        report.halted,
        report.traversed_every_stage(),
        stages.join(",")
    )
}
