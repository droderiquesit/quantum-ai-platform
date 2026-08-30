//! Serving the operator console.
//!
//! Nine server-rendered views under `/console`, assembled from the platform's
//! existing accessors and from [`crate::cells`]. The rules the console is
//! built around are stated in `qip_web::console`; this module is where they
//! meet the platform, and two of them shape almost every line here.
//!
//! **A view never invents a number.** Every panel is assembled as one of three
//! things: reported (with an as-of time), stale (with its age and the bound it
//! exceeded), or absent (with the reason, from [`crate::missing`]). Where the
//! platform holds something behind a capability gate, or where a subsystem is
//! not wired into this process at all, the panel says so. It never renders a
//! zero it did not observe.
//!
//! **Nothing here can act, except one thing.** The console serves `GET` for
//! every view and accepts exactly one `POST`: tripping the kill switch. There
//! is no handler that clears one, because
//! [`qip_risk_engine::autonomy::KillSwitch::clear_global`] requires an
//! operator identity verified within the last fifteen minutes and a page
//! cannot establish one. Stopping is easy and restarting is not; the console
//! is built so that restarting is not merely refused but unreachable.
//!
//! The platform lock is taken, everything needed is copied out, and it is
//! released before rendering. Holding it across rendering would let an HTML
//! page stall a trading loop.

use crate::auth::{Authenticator, RateLimiter, Role};
use crate::cells::{CellObservation, CellRegistry, describe_age};
use crate::http::{Handler, Method, Request, Response};
use crate::missing;
use qip_core::Decimal;
use qip_core::time::Timestamp;
use qip_kernel::Platform;
use qip_web::console::model::{
    AgentCallRow, CapitalRow, CellRow, ConsoleModel, ExposureRow, FillRow, KillSwitchState, Metric,
    RefusalRow, ServiceRow, StrategyRow,
};
use qip_web::console::{TRIP_PATH, View, render};
use qip_web::panel::Panel;
use qip_web::view::{OpportunityRow, OrderRow, Posture};
use std::sync::{Arc, Mutex};

/// Serves the operator console.
pub struct Console {
    platform: Arc<Mutex<Platform>>,
    cells: Arc<CellRegistry>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<RateLimiter>,
    /// The clock the console reads. Injected, like everywhere else, so a test
    /// and a deployment differ only in which clock they pass — and so
    /// staleness is computed against a clock a test can move.
    clock: Arc<dyn qip_core::Clock>,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console")
            .field("views", &View::all().len())
            .finish_non_exhaustive()
    }
}

impl Console {
    pub fn new(
        platform: Arc<Mutex<Platform>>,
        cells: Arc<CellRegistry>,
        authenticator: Arc<Authenticator>,
        rate_limiter: Arc<RateLimiter>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Self {
        Self {
            platform,
            cells,
            authenticator,
            rate_limiter,
            clock,
        }
    }

    /// Whether a path belongs to the console.
    pub fn owns(path: &str) -> bool {
        path == "/console" || path.starts_with("/console/")
    }

    /// Build the model. See the module documentation for the rules it follows.
    pub fn model(&self, now: Timestamp) -> ConsoleModel {
        let Ok(platform) = self.platform.lock() else {
            // A poisoned lock means a thread panicked while holding the
            // platform. Every panel is absent and the banner says halted:
            // nothing here is known, and saying so is the only honest option.
            return ConsoleModel {
                posture: Posture {
                    halted: true,
                    halt_reason:
                        "the platform is in an inconsistent state after an internal failure"
                            .to_string(),
                    ..Posture::default()
                },
                rendered_at: now.to_rfc3339(),
                ..ConsoleModel::default()
            };
        };
        let observations = self.cells.observations();
        assemble(&platform, &observations, self.cells.freshness_bound(), now)
    }

    /// Trip the kill switch.
    ///
    /// Offered from the console because tripping needs no authority beyond a
    /// component noticing something wrong, and an operator watching this
    /// screen is exactly that. The API's own `POST /api/v1/kill-switch`
    /// requires an operator role instead, and the difference is deliberate: a
    /// machine calling the API with a token should be held to the role its
    /// token declares, while a human who can already see the halt is not made
    /// to find a second credential before stopping the platform.
    fn trip(&self, subject: &str, now: Timestamp) -> Response {
        let Ok(mut platform) = self.platform.lock() else {
            return Response::text(
                503,
                "the platform is in an inconsistent state and is not serving",
            );
        };
        platform.autonomy_mut().kill_switch_mut().trip_global(
            now,
            format!("console:{subject}"),
            "tripped from the operator console",
        );
        // See Other rather than a rendered page: a refresh after a POST must
        // not trip it again.
        Response::text(303, "halted").with_header("location", View::Risk.path())
    }
}

impl Handler for Console {
    fn handle(&self, request: &Request) -> Response {
        let now = self.clock.now();

        // Authentication first, so an unauthenticated caller learns nothing
        // about which console paths exist.
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

        match request.method {
            Method::Get | Method::Head => match View::from_path(&request.path) {
                Some(view) => Response::html(200, render(view, &self.model(now))),
                None => Response::text(404, "no such console view"),
            },
            // The one action the console has. Every other path, and every
            // other method, is refused — there is no handler that clears a
            // halt, so no request can reach one.
            Method::Post if request.path == TRIP_PATH => self.trip(&principal.subject, now),
            Method::Post => Response::text(404, "no such console action"),
            _ => Response::text(
                405,
                "the operator console is read-only apart from tripping the kill switch",
            ),
        }
    }
}

// --- assembly ---------------------------------------------------------------

fn money(value: Decimal) -> String {
    value.to_string()
}

/// Build every panel from the platform.
///
/// One function rather than one per view: the nine views overlap heavily —
/// exposure appears on three of them — and assembling a panel twice is how two
/// views end up disagreeing.
fn assemble(
    platform: &Platform,
    observations: &[CellObservation],
    freshness: qip_core::Duration,
    now: Timestamp,
) -> ConsoleModel {
    let as_of = now.to_rfc3339();
    let controller = platform.autonomy();
    let switch = controller.kill_switch();

    let regions = region_panel(platform, observations, freshness, now);
    let cells_reporting = !observations.is_empty();

    ConsoleModel {
        posture: Posture {
            autonomy_level: controller.level().as_str().to_string(),
            autonomy_ceiling: controller.ceiling().as_str().to_string(),
            live: controller.is_live(),
            halted: switch.is_globally_tripped(),
            halt_reason: switch
                .global_trip()
                .map(|trip| format!("{} ({})", trip.reason, trip.tripped_by))
                .unwrap_or_default(),
        },
        rendered_at: as_of.clone(),
        cycle: platform.cycle_count(),
        events_logged: platform.event_log().len(),
        chain_intact: platform.event_log().verify_chain().is_ok(),

        // --- global ---
        regions: regions.clone(),
        market_state: Panel::absent(missing::DESK_GATED),
        opportunities: opportunity_panel(platform, &as_of),
        strategies: strategy_panel(platform, &as_of),
        capital_distribution: grant_panel(platform, &as_of),
        system_health: health_panel(platform, now, &as_of),

        // --- regional ---
        cell_brains: Panel::absent(missing::NO_CELL_REPORTS),
        local_opportunities: Panel::absent(
            "a CellReport carries a cell's book, its capital utilisation and its \
             reconciliation breaks. It does not carry the cell's own opportunity queue, so \
             the centre has no local queue to show; the queue on the global view is this \
             process's own.",
        ),
        cell_latency: Panel::absent(missing::NO_CELL_REPORTS),
        brokers: Panel::absent(missing::NO_CELL_REPORTS),
        venues: Panel::absent(missing::NO_CELL_REPORTS),
        inventory: exposure_panel(platform, cells_reporting, &as_of),
        cash: Panel::absent(missing::DESK_GATED),
        cell_models: Panel::absent(missing::NO_MODEL_REGISTRY),

        // --- trading ---
        positions: exposure_panel(platform, cells_reporting, &as_of),
        pending_orders: pending_order_panel(platform, &as_of),
        fills: fill_panel(platform, &as_of),
        pnl: Panel::absent(missing::NO_ATTRIBUTION),
        alpha: Panel::absent(missing::NO_ATTRIBUTION),
        refusals: refusal_panel(platform, &as_of),

        // --- arbitrage ---
        three_arm: Panel::absent(missing::NO_ARBITRAGE_ENGINE),
        n_leg: Panel::absent(missing::NO_ARBITRAGE_ENGINE),
        arbitrage_capital: Panel::absent(missing::NO_ARBITRAGE_ENGINE),

        // --- ai ---
        models: Panel::absent(missing::NO_MODEL_REGISTRY),
        model_reputation: Panel::absent(missing::NO_MODEL_REGISTRY),
        agent_calls: agent_call_panel(platform, &as_of),
        training: Panel::absent(missing::NO_TRAINING_SERVICE),

        // --- quantum ---
        quantum_jobs: Panel::absent(missing::NO_QUANTUM_JOBS),
        quantum_routing: Panel::current(
            vec![
                Metric::new(
                    "Quantum provider",
                    if platform.config().quantum_enabled {
                        "attached (simulated)"
                    } else {
                        "none"
                    },
                )
                .with_note("from the platform configuration this process was assembled with"),
                Metric::new("Classical baseline", "always").with_note(
                    "every routed problem is solved classically as well; a quantum result \
                     is never reported alone",
                ),
            ],
            as_of.clone(),
        ),

        // --- data finder ---
        sources: Panel::absent(missing::NO_DATA_FINDER),
        source_health: Panel::current(
            vec![
                Metric::new(
                    "Datasets licensed for production",
                    platform.config().licensed_datasets.len().to_string(),
                )
                .with_note(if platform.config().licensed_datasets.is_empty() {
                    "none configured; agents may use no dataset in a production decision"
                        .to_string()
                } else {
                    platform.config().licensed_datasets.join(", ")
                }),
            ],
            as_of.clone(),
        ),

        // --- risk ---
        limits: Panel::absent(missing::NO_LIMIT_UTILISATION),
        exposure: exposure_panel(platform, cells_reporting, &as_of),
        tail_risk: Panel::absent(missing::DESK_GATED),
        concentration: concentration_panel(platform, cells_reporting, &as_of),
        regional_limits: budget_panel(platform, &as_of),
        kill_switch: KillSwitchState {
            halted: switch.is_globally_tripped(),
            halted_scopes: switch
                .halted_scopes()
                .into_iter()
                .map(str::to_string)
                .collect(),
            tripped_by: switch
                .global_trip()
                .map(|trip| trip.tripped_by.clone())
                .unwrap_or_default(),
            tripped_at: switch
                .global_trip()
                .map(|trip| trip.at.to_rfc3339())
                .unwrap_or_default(),
            reason: switch
                .global_trip()
                .map(|trip| trip.reason.clone())
                .unwrap_or_default(),
            clearances: switch.clearances().len(),
        },

        // --- operations ---
        services: health_panel(platform, now, &as_of),
        transports: Panel::absent(missing::NO_TRANSPORT_HEALTH),
        clusters: Panel::absent(missing::NO_CLUSTER_HEALTH),
        model_health: Panel::absent(missing::NO_MODEL_REGISTRY),
        source_outages: Panel::absent(missing::NO_DATA_FINDER),
        operating_cost: cost_panel(platform, &as_of),
        governance: governance_panel(platform, now, &as_of),
    }
}

fn region_panel(
    platform: &Platform,
    observations: &[CellObservation],
    freshness: qip_core::Duration,
    now: Timestamp,
) -> Panel<CellRow> {
    if observations.is_empty() {
        return Panel::absent(missing::NO_CELL_REPORTS);
    }
    let switch = platform.autonomy().kill_switch();
    let exposure = platform.central().exposure();

    let rows: Vec<CellRow> = observations
        .iter()
        .map(|observation| {
            let stale = observation.is_stale(now, freshness);
            let halted = switch.is_halted(&observation.cell);
            CellRow {
                cell: observation.cell.clone(),
                status: if halted {
                    "halted".to_string()
                } else if stale {
                    "stale".to_string()
                } else {
                    "reporting".to_string()
                },
                reported_at: observation.at.to_rfc3339(),
                age: describe_age(observation.age(now)),
                positions: observation.positions,
                gross: money(exposure.by_cell.gross_of(&observation.cell)),
                net: money(exposure.by_cell.net_of(&observation.cell)),
                strategies: observation.strategies,
                reconciliation_breaks: observation.reconciliation_breaks,
                halted,
            }
        })
        .collect();

    // The panel is stale if *any* cell is, not only if all of them are. Part
    // of the picture being old makes the picture old: an aggregate mixing a
    // current book with an hour-old one is not a current aggregate.
    let oldest = observations
        .iter()
        .max_by_key(|observation| observation.age(now).as_nanos());
    match oldest {
        Some(observation) if observation.is_stale(now, freshness) => Panel::stale(
            rows,
            observation.at.to_rfc3339(),
            describe_age(observation.age(now)),
            describe_age(freshness),
        ),
        _ => Panel::current(rows, now.to_rfc3339()),
    }
}

fn opportunity_panel(platform: &Platform, as_of: &str) -> Panel<OpportunityRow> {
    // The queue is this process's own and is always readable, so an empty
    // queue here is an observed zero: the detectors ran and found nothing.
    Panel::current(
        platform
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
        as_of,
    )
}

fn strategy_panel(platform: &Platform, as_of: &str) -> Panel<StrategyRow> {
    let factory = platform.central().factory();
    Panel::current(
        factory
            .candidates()
            .map(|candidate| StrategyRow {
                id: candidate.strategy().as_str().to_string(),
                cell: candidate.cell().to_string(),
                venue: candidate.venue().as_str().to_string(),
                stage: factory.stage_of(candidate.strategy()).as_str().to_string(),
                holds_capital: factory.holds_capital(candidate.strategy()),
                registered_at: candidate.registered_at().to_rfc3339(),
            })
            .collect(),
        as_of,
    )
}

fn grant_panel(platform: &Platform, as_of: &str) -> Panel<CapitalRow> {
    let central = platform.central();
    let rows: Vec<CapitalRow> = central
        .factory()
        .candidates()
        .filter_map(|candidate| {
            let envelope = central.envelope(candidate.cell(), candidate.strategy())?;
            let used = central
                .utilisation(candidate.cell(), candidate.strategy())
                .map(|utilisation| money(utilisation.gross_committed))
                // No utilisation reported is not zero utilisation: the cell
                // has not told the centre what it has committed.
                .unwrap_or_else(|| "not reported".to_string());
            Some(CapitalRow {
                subject: "grant".to_string(),
                cell: candidate.cell().to_string(),
                strategy: candidate.strategy().as_str().to_string(),
                granted: money(envelope.gross_limit()),
                used,
                utilisation: central
                    .utilisation(candidate.cell(), candidate.strategy())
                    .map(|utilisation| share(utilisation.gross_committed, envelope.gross_limit()))
                    .unwrap_or_else(|| "not reported".to_string()),
                expires_at: envelope.expires_at().to_rfc3339(),
            })
        })
        .collect();
    // Grants are held in this process, so an empty list is an observed zero:
    // nothing has been issued.
    Panel::current(rows, as_of)
}

fn share(part: Decimal, whole: Decimal) -> String {
    if !whole.is_positive() {
        return "n/a".to_string();
    }
    format!("{:.1}%", 100.0 * part.to_f64() / whole.to_f64())
}

fn budget_panel(platform: &Platform, as_of: &str) -> Panel<CapitalRow> {
    let config = platform.central().config();
    let bound = |subject: &str, value: Decimal| CapitalRow {
        subject: subject.to_string(),
        cell: "all".to_string(),
        strategy: "all".to_string(),
        granted: money(value),
        // Deliberately not a number. The centre knows what it may allocate; it
        // does not know what is committed against these bounds until the cells
        // report, and a zero here would read as unused headroom.
        used: "not reported".to_string(),
        utilisation: "not reported".to_string(),
        expires_at: "n/a".to_string(),
    };
    Panel::current(
        vec![
            bound("total budget", config.total_budget),
            bound("per strategy", config.per_strategy),
            bound("per cell", config.per_cell),
            bound("per venue", config.per_venue),
        ],
        as_of,
    )
}

fn exposure_panel(platform: &Platform, cells_reporting: bool, as_of: &str) -> Panel<ExposureRow> {
    if !cells_reporting {
        return Panel::absent(missing::NO_CELL_REPORTS);
    }
    let exposure = platform.central().exposure();
    let limits = platform.central().concentration_limits();
    let mut rows = Vec::new();
    for (axis, view, limit) in [
        ("instrument", &exposure.by_instrument, limits.per_instrument),
        ("sector", &exposure.by_sector, limits.per_sector),
        ("venue", &exposure.by_venue, limits.per_venue),
        ("currency", &exposure.by_currency, limits.per_currency),
        ("cell", &exposure.by_cell, limits.per_cell),
    ] {
        let shares = view.shares();
        for (bucket, gross) in view.ranked() {
            let bucket_share = shares.get(&bucket).copied().unwrap_or(0.0);
            rows.push(ExposureRow {
                axis: axis.to_string(),
                bucket: bucket.clone(),
                gross: money(gross),
                net: money(view.net_of(&bucket)),
                share: bucket_share,
                limit,
                breached: bucket_share > limit,
            });
        }
    }
    Panel::current(rows, as_of)
}

fn concentration_panel(
    platform: &Platform,
    cells_reporting: bool,
    as_of: &str,
) -> Panel<ExposureRow> {
    if !cells_reporting {
        return Panel::absent(missing::NO_CELL_REPORTS);
    }
    let limits = platform.central().concentration_limits();
    Panel::current(
        platform
            .central()
            .exposure()
            .concentrations(&limits)
            .into_iter()
            .map(|finding| ExposureRow {
                axis: finding.axis.to_string(),
                bucket: finding.bucket.clone(),
                gross: money(finding.gross),
                net: String::new(),
                share: finding.share,
                limit: finding.limit,
                breached: true,
            })
            .collect(),
        as_of,
    )
}

fn pending_order_panel(platform: &Platform, as_of: &str) -> Panel<OrderRow> {
    Panel::current(
        platform
            .orders()
            .open_orders()
            .into_iter()
            .map(|order| OrderRow {
                id: order.order_id.as_str().to_string(),
                instrument: order.object_id.as_str().to_string(),
                side: order.side.as_str().to_string(),
                quantity: order.quantity.to_string(),
                state: order.state.as_str().to_string(),
                // An order with no fills has not been proved real, so it is
                // shown as paper until a live fill says otherwise.
                simulated: order.is_paper() || order.fills.is_empty(),
            })
            .collect(),
        as_of,
    )
}

fn fill_panel(platform: &Platform, as_of: &str) -> Panel<FillRow> {
    let manager = platform.orders();
    Panel::current(
        manager
            .fills()
            .into_iter()
            .map(|fill| {
                let order = manager.order(&fill.order_id);
                FillRow {
                    order: fill.order_id.as_str().to_string(),
                    instrument: order
                        .map(|order| order.object_id.as_str().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    side: order
                        .map(|order| order.side.as_str().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    quantity: fill.quantity.to_string(),
                    price: fill.price.to_string(),
                    venue: fill.venue.clone(),
                    simulated: fill.simulated,
                }
            })
            .collect(),
        as_of,
    )
}

/// Every submission the order manager refused, one row each.
///
/// A row per refusal rather than a count of them: the question an operator
/// asks of this panel is "why is that position not on", and a figure reading
/// `3` answers a different question. The refusals are held by this process's
/// own order manager and are always readable, so an empty list here is an
/// observed zero — nothing was refused — and renders as one.
fn refusal_panel(platform: &Platform, as_of: &str) -> Panel<RefusalRow> {
    Panel::current(
        platform
            .orders()
            .refusals()
            .iter()
            .map(|refusal| RefusalRow {
                order: refusal.order_id.as_str().to_string(),
                at: refusal.at.to_rfc3339(),
                kind: match &refusal.refusal {
                    // A control refused it, and a control's refusal is never
                    // retried automatically.
                    Some(reason) if reason.is_safety_control() => "safety control".to_string(),
                    Some(_) => "fault".to_string(),
                    // Not "fault": calling an unrecorded refusal transient
                    // would invite exactly the retry nobody can justify.
                    None => "not recorded".to_string(),
                },
                reason: refusal
                    .refusal
                    .as_ref()
                    .map(|reason| reason.describe())
                    .unwrap_or_else(|| "refused without a recorded reason".to_string()),
            })
            .collect(),
        as_of,
    )
}

fn agent_call_panel(platform: &Platform, as_of: &str) -> Panel<AgentCallRow> {
    Panel::current(
        platform
            .organisation()
            .audit()
            .records()
            .iter()
            .map(|record| AgentCallRow {
                agent: record.agent_id.clone(),
                run: record.run_id.as_str().to_string(),
                status: run_status(&record.status).to_string(),
                tool_calls: record.spend.tool_calls,
                model_calls: record.spend.language_model_calls,
                tokens: record.spend.tokens,
                cost: format!("${:.4}", record.spend.cost_micros as f64 / 1_000_000.0),
                utilisation: record.utilisation,
                // `None` renders as "no finding", not as zero conviction. An
                // agent that produced nothing has not expressed a weak view.
                conviction: record.finding.as_ref().map(|finding| finding.conviction),
            })
            .collect(),
        as_of,
    )
}

fn run_status(status: &qip_agents::runtime::RunStatus) -> &'static str {
    match status {
        qip_agents::runtime::RunStatus::Succeeded => "ok",
        qip_agents::runtime::RunStatus::Escalated { .. } => "degraded",
        qip_agents::runtime::RunStatus::Failed { .. } => "down",
        qip_agents::runtime::RunStatus::Refused { .. } => "rejected",
    }
}

fn cost_panel(platform: &Platform, as_of: &str) -> Panel<Metric> {
    let records = platform.organisation().audit().records();
    let cost: u64 = records.iter().map(|record| record.spend.cost_micros).sum();
    let tokens: u64 = records
        .iter()
        .map(|record| u64::from(record.spend.tokens))
        .sum();
    let calls: u64 = records
        .iter()
        .map(|record| u64::from(record.spend.language_model_calls))
        .sum();
    Panel::current(
        vec![
            Metric::new("Agent runs", records.len().to_string())
                .with_note("from this process's audit trail"),
            Metric::new("Model calls", calls.to_string()),
            Metric::new("Tokens", tokens.to_string()),
            Metric::new("Spend", format!("${:.4}", cost as f64 / 1_000_000.0)).with_note(
                "charged against agent budgets before each call, so this is spend, \
                            not an estimate",
            ),
        ],
        as_of,
    )
}

fn health_panel(platform: &Platform, now: Timestamp, as_of: &str) -> Panel<ServiceRow> {
    let breaks = platform.orders().reconciliation_breaks().len();
    let chain = platform.event_log().verify_chain();
    let switch = platform.autonomy().kill_switch();

    let mut rows = vec![
        ServiceRow {
            name: "event-log".to_string(),
            state: if chain.is_ok() { "ok" } else { "down" }.to_string(),
            detail: match &chain {
                Ok(()) => format!(
                    "{} record(s), hash chain intact",
                    platform.event_log().len()
                ),
                Err(sequence) => describe_chain_break(platform.event_log(), *sequence),
            },
        },
        ServiceRow {
            name: "order-manager".to_string(),
            state: if breaks == 0 { "ok" } else { "down" }.to_string(),
            detail: if breaks == 0 {
                "the book agrees with every venue report".to_string()
            } else {
                format!(
                    "{breaks} venue/book disagreement(s); none of the platform's own numbers should be believed until they are resolved"
                )
            },
        },
        ServiceRow {
            name: "autonomy".to_string(),
            state: if switch.is_globally_tripped() {
                "halted"
            } else {
                "ok"
            }
            .to_string(),
            detail: format!(
                "level {}, ceiling {}",
                platform.autonomy().level().as_str(),
                platform.autonomy().ceiling().as_str()
            ),
        },
        ServiceRow {
            name: "agent-organisation".to_string(),
            state: "ok".to_string(),
            detail: format!("{} agent(s) on the roster", platform.organisation().len()),
        },
    ];

    let outstanding = platform.central().recalls().outstanding(now).len();
    rows.push(ServiceRow {
        name: "capital-recalls".to_string(),
        state: if outstanding == 0 { "ok" } else { "degraded" }.to_string(),
        detail: if outstanding == 0 {
            "no recall is outstanding".to_string()
        } else {
            format!(
                "{outstanding} recall(s) issued and not acknowledged; the reliable bound on \
                 each is its envelope's expiry, which the cell enforces"
            )
        },
    });

    // A reproducible signing key is not a fault, but it is not production
    // either, and a deployment that forgot to supply real key material looks
    // exactly like one that did unless this says so.
    rows.push(ServiceRow {
        name: "central-signing-key".to_string(),
        state: if platform.central().signing_key_is_reproducible() {
            "degraded"
        } else {
            "ok"
        }
        .to_string(),
        detail: if platform.central().signing_key_is_reproducible() {
            "reproducible from configuration; anyone who knows the seed can mint a capital \
             envelope. Not a production key."
                .to_string()
        } else {
            "assembled from supplied key material".to_string()
        },
    });

    Panel::current(rows, as_of)
}

/// What is actually broken at the record `verify_chain` names.
///
/// [`qip_events::EventLog::verify_chain`] returns the sequence of the first
/// record whose linkage does not hold and nothing else. That is enough to know
/// the log has been altered and not enough to say how, and two different
/// findings arrive as the same number:
///
/// * the record's stored predecessor hash is not the hash of the record before
///   it — something was removed, reordered or inserted; or
/// * the link is intact but the record no longer hashes to the hash stored
///   with it — the record was edited in place.
///
/// They are told apart here by comparing the link the log itself carries, and
/// the two hashes are printed in full. An operator reading this line is
/// reading it because the audit trail is not trustworthy, which is the one
/// moment where a summary is worth less than the values.
fn describe_chain_break(log: &qip_events::EventLog, sequence: u64) -> String {
    let records = log.records();
    let Some(position) = records
        .iter()
        .position(|record| record.sequence == sequence)
    else {
        return format!(
            "hash chain broken: the verifier names record {sequence}, which is no longer \
             in the log. The log changed while it was being read."
        );
    };
    let record = &records[position];
    // The first record commits to the genesis hash rather than to a
    // predecessor, so that is what it is checked against. The predecessor is
    // named by its own sequence rather than by `sequence - 1`, which is not
    // the same thing in a log that has dropped a record.
    let (predecessor, expected) = match position.checked_sub(1) {
        Some(previous) => (
            format!("record {}", records[previous].sequence),
            records[previous].record_hash.as_str(),
        ),
        None => (
            "the genesis hash".to_string(),
            qip_events::log::GENESIS_HASH,
        ),
    };
    if record.previous_hash == expected {
        format!(
            "hash chain broken at record {sequence}: its link to {predecessor} holds, but \
             its content no longer hashes to the hash stored with it ({}). The record was \
             edited in place.",
            record.record_hash
        )
    } else {
        format!(
            "hash chain broken at record {sequence}: it commits to predecessor hash {}, \
             and {predecessor} hashes to {}. A record was removed, reordered or inserted.",
            record.previous_hash, expected
        )
    }
}

fn governance_panel(
    platform: &Platform,
    now: Timestamp,
    as_of: &str,
) -> Panel<qip_web::view::GovernanceRow> {
    Panel::current(
        platform
            .review_governance(now)
            .iter()
            .map(|finding| qip_web::view::GovernanceRow {
                severity: match finding.severity {
                    qip_agents::governance::Severity::Error => "error".to_string(),
                    qip_agents::governance::Severity::Warning => "warning".to_string(),
                },
                rule: finding.rule.clone(),
                detail: finding.detail.clone(),
            })
            .collect(),
        as_of,
    )
}

#[cfg(test)]
mod tests {
    //! Unit tests for the parts of the console that cannot be reached through
    //! a [`Platform`].
    //!
    //! A platform's event log is append-only and owns its own storage, so
    //! there is no way to hand one a broken chain from outside the crate — and
    //! the report a broken chain produces is the last thing anyone wants to
    //! find untested.

    use super::describe_chain_break;
    use qip_core::{CorrelationId, EventId, Lineage, Timestamp};
    use qip_events::topic::Topic;
    use qip_events::{AnyEvent, EventLog};

    fn event(id: &str) -> AnyEvent {
        AnyEvent {
            event_id: EventId::from_string(id),
            topic: Topic::MarketTick,
            schema_version: 1,
            occurred_at: Timestamp::from_secs(1_760_000_000),
            recorded_at: Timestamp::from_secs(1_760_000_000),
            sequence: 0,
            lineage: Lineage::root(CorrelationId::from_string("corr-1"), "test"),
            idempotency_key: None,
            payload: serde_json::json!({ "symbol": "AAA", "price": 100.0 }),
            payload_hash: String::new(),
        }
    }

    /// A file-backed log with three valid records, at a path of its own.
    fn log_at(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "qip-api-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("events.jsonl");
        let mut log = EventLog::open(&path).expect("opens");
        for index in 0..3 {
            log.append(&event(&format!("evt-{index}")))
                .expect("appends");
        }
        assert!(log.verify_chain().is_ok(), "the log starts intact");
        (dir, path)
    }

    fn lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("readable")
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn a_record_edited_in_place_is_named_with_the_hash_it_no_longer_matches() {
        let (dir, path) = log_at("edited");

        // Change what the second record says, leaving its stored hashes alone.
        // Its link to the record before it still holds; its own content no
        // longer does.
        let mut all = lines(&path);
        let mut record: serde_json::Value = serde_json::from_str(&all[1]).expect("parses");
        record["event"]["payload"]["price"] = serde_json::json!(999_999.0);
        let stored_hash = record["record_hash"].as_str().expect("a hash").to_string();
        all[1] = serde_json::to_string(&record).expect("serialises");
        std::fs::write(&path, all.join("\n") + "\n").expect("writes");

        let reopened = EventLog::open(&path).expect("reopens");
        let sequence = reopened.verify_chain().expect_err("tampering is detected");
        let report = describe_chain_break(&reopened, sequence);

        assert!(report.contains("record 2"), "{report}");
        assert!(report.contains("edited in place"), "{report}");
        assert!(
            report.contains(&stored_hash),
            "the hash the content no longer matches is named: {report}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_link_names_both_the_hash_committed_to_and_the_hash_found() {
        let (dir, path) = log_at("relinked");

        // Point the third record at a predecessor that is not the one in front
        // of it — what removing or reordering a record looks like on disk.
        let mut all = lines(&path);
        let first: serde_json::Value = serde_json::from_str(&all[0]).expect("parses");
        let expected = first["record_hash"].as_str().expect("a hash").to_string();
        let second: serde_json::Value = serde_json::from_str(&all[1]).expect("parses");
        let real_predecessor = second["record_hash"].as_str().expect("a hash").to_string();
        let mut third: serde_json::Value = serde_json::from_str(&all[2]).expect("parses");
        third["previous_hash"] = serde_json::json!(expected);
        all[2] = serde_json::to_string(&third).expect("serialises");
        std::fs::write(&path, all.join("\n") + "\n").expect("writes");

        let reopened = EventLog::open(&path).expect("reopens");
        let sequence = reopened.verify_chain().expect_err("tampering is detected");
        let report = describe_chain_break(&reopened, sequence);

        assert!(report.contains("record 3"), "{report}");
        assert!(
            report.contains(&expected) && report.contains(&real_predecessor),
            "both the hash committed to and the hash actually found are named: {report}"
        );
        assert!(
            report.contains("removed, reordered or inserted"),
            "{report}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
