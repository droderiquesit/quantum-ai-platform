//! The API server.
//!
//! Reads its configuration from the environment, assembles a platform, and
//! serves. Every safety default is applied here and stated in the start-up
//! banner, so an operator can read what the process will and will not do
//! before it does anything.
//!
//! Credentials come from the environment, never from a file in the repository
//! and never from a default. A server with no credential configured refuses to
//! start: an API that would otherwise be unauthenticated is worse than one
//! that is down.
//!
//! Storage is read the same way and refused the same way. `QIP_STORAGE_TARGET`
//! and `QIP_STORAGE_ROOT` name the store, and a configuration that does not
//! describe one this process can write to stops the start-up rather than
//! falling back to memory — a deployment that believed it was durable and was
//! not would pass every smoke test it has and lose everything at the restart.

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::cells::CellRegistry;
use qip_api::console::Console;
use qip_api::http::{Server, ServerLimits};
use qip_api::routes::Api;
use qip_api::web::{Router, Web};
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_core::{Clock, SystemClock};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
use std::sync::{Arc, Mutex};

fn main() {
    if let Err(error) = run() {
        eprintln!("qip-api: {}", error.message());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let now = clock.now();

    let address = std::env::var("QIP_API_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    // The autonomy ceiling. Paper trading unless the environment explicitly
    // raises it, and raising it is a deployment decision.
    let ceiling = match std::env::var("QIP_AUTONOMY_CEILING") {
        Ok(value) => AutonomyLevel::parse(&value)?,
        Err(_) => AutonomyLevel::PaperTrading,
    };

    // Resolved and proven writable before anything else is built. Failing here
    // costs a restart; failing at the first archived cycle costs the record of
    // everything that happened up to it.
    let storage = StorageSettings::from_env()?;
    storage.preflight()?;
    let archive = Arc::new(ChainArchive::open(storage.key_value("event-log")?)?);

    let config = PlatformConfig::default().with_live_ceiling(ceiling);
    let context = qip_core::Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::new("qip-api", clock.clone()),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;

    // The trust root, before anything is served: install the operator's
    // envelope key when the deployment provides one, and refuse to run
    // live-capable on the seed-derived default. This process is the one that
    // dispatches envelopes down the mesh, so the key installed here is the
    // key every cell verifies those grants against. See `trust.rs` for why a
    // refusal and not a warning.
    //
    // Read through `qip_core::secret`, so the deployment may supply the key in
    // a file rather than in the process environment. That is what the Secret
    // Manager CSI driver projects into the pod, and a signing key in
    // `/proc/<pid>/environ` is one every child process and every crash dump
    // also has.
    let envelope_key = qip_core::secret::from_environment(qip_api::trust::ENVELOPE_KEY_VARIABLE)
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let provenance = qip_api::trust::harden_central(&mut platform, envelope_key.as_deref())
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    // The mesh backbone, where the deployment names cells to serve. Absent
    // configuration means the routes are absent: no listener binds, and the
    // banner says the deltas have nowhere to land here.
    let mesh_settings = qip_api::mesh::MeshSettings::from_env()?;
    let mesh = match &mesh_settings {
        Some(settings) => Some(Arc::new(Mutex::new(qip_api::mesh::MeshBackbone::open(
            settings,
            storage.key_value("mesh")?,
            clock.clone(),
        )?))),
        None => None,
    };

    // Credentials from the environment. Nothing is defaulted: an API that
    // starts unauthenticated because a variable was missing is worse than one
    // that does not start.
    let mut credentials = Vec::new();
    for (variable, role) in [
        ("QIP_TOKEN_MONITOR", Role::Monitor),
        ("QIP_TOKEN_VIEWER", Role::Viewer),
        ("QIP_TOKEN_ANALYST", Role::Analyst),
        ("QIP_TOKEN_APPROVER", Role::Approver),
        ("QIP_TOKEN_OPERATOR", Role::Operator),
    ] {
        // Through `qip_core::secret`, so a deployment may mount the token as
        // a file. A refusal here is fatal rather than skipped: a token whose
        // file is named and unreadable must not become a role that quietly
        // does not exist, which would leave the API running with fewer
        // credentials than the operator configured and no indication of it.
        let Some(token) = qip_core::secret::from_environment(variable)? else {
            continue;
        };
        if token.len() < 32 {
            return Err(Error::invalid(format!(
                "{variable} is shorter than 32 characters; a token that can be guessed is not a credential"
            )));
        }
        credentials.push(Credential::from_token(
            format!("{}@env", role.as_str()),
            role,
            token,
            now,
            now.saturating_add(Duration::from_days(30)),
        ));
    }
    if credentials.is_empty() {
        return Err(Error::denied(
            "no credential is configured; set at least QIP_TOKEN_OPERATOR. An API that would otherwise be unauthenticated does not start.",
        ));
    }

    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(credentials));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 600));

    // One registry, shared. The console reads staleness off it and `/regions`
    // serves the same observations, so a page and the JSON behind it cannot
    // disagree about which cells have gone quiet.
    let cells = Arc::new(CellRegistry::default());

    let mut api = Api::new(
        platform.clone(),
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    )
    .with_cells(cells.clone())
    .with_archive(archive.clone());
    if let Some(mesh) = &mesh {
        api = api.with_mesh(mesh.clone());
    }
    let api = Arc::new(api);
    let console = Arc::new(Console::new(
        platform.clone(),
        cells,
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    ));
    let web = Arc::new(Web::new(platform, authenticator, rate_limiter, clock));
    let router = Router::new(api, web).with_console(console);

    let server = Server::bind(&address, Arc::new(router), ServerLimits::default())?;
    let bound = server.local_address()?;

    // The start-up banner. An operator should be able to read what this
    // process will do before it does anything.
    println!("qip-api listening on {bound}");
    println!("  autonomy ceiling: {ceiling} ({})", ceiling.describe());
    println!(
        "  live trading:     {}",
        if ceiling.is_live() {
            "REACHABLE — requires two authenticated operators to enable"
        } else {
            "unreachable in this deployment"
        }
    );
    println!("  api version:      v1");
    println!(
        "  api surface:      {} route(s), described at {}",
        qip_api::ROUTES.len(),
        qip_api::routes::OPENAPI_PATH
    );
    println!(
        "  operator ui:      {} surface(s) and {} console view(s), no JavaScript",
        qip_web::Surface::all().len(),
        qip_web::View::all().len()
    );
    println!(
        "  console:          read-only; it can trip the kill switch and has no path that \
         clears one"
    );
    println!("  capital trust:    {}", provenance.describe());
    match &mesh {
        Some(mesh) => {
            // The addresses come from the bound sockets rather than from the
            // configuration, so what the banner names is what is actually
            // listening — including a port the operating system chose.
            let Ok(mesh) = mesh.lock() else {
                return Err(Error::invalid(
                    "the mesh backbone is in an inconsistent state before serving began",
                ));
            };
            println!(
                "  mesh:             serving {} cell(s); deltas drain and capital dispatches \
                 on each POST /cycle",
                mesh.listeners()
                    .iter()
                    .filter(|listener| listener.role == "cells")
                    .count()
            );
            for listener in mesh.listeners() {
                match listener.role {
                    "cells" => println!(
                        "                    {} publishes deltas and polls capital at {} \
                         (its QIP_MESH_PEER)",
                        listener.cell, listener.address
                    ),
                    _ => println!(
                        "                    {} capital feed on loopback {} (internal)",
                        listener.cell, listener.address
                    ),
                }
            }
        }
        None => println!(
            "  mesh:             not served ({} is not set); cells pointed here are \
             partitioned and stop when their envelopes expire",
            qip_api::mesh::CELLS_VARIABLE
        ),
    }
    for line in storage.banner_lines(
        &[
            "the event log's hash chain, at each completed cycle",
            "undelivered capital envelopes in the mesh spool, and their dead letters \
             (when the mesh is served)",
        ],
        &[
            "the in-memory event index and everything queried from it",
            "rate-limit counters",
            "which cells have reported and when",
            "the mesh delta inbox and its absorption counters",
        ],
    ) {
        println!("{line}");
    }
    println!("  event chain:      {}", archive.describe());

    server.serve()
}
