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
use qip_data_finder::registration::RegistrationRecord;
use qip_kernel::central::ArbitragePolicy;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::journal::StreamJournal;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
use std::sync::{Arc, Mutex};

/// Variables this binary used to read, with the refusal each now produces.
///
/// A retired variable has three possible fates and only one of them is safe.
/// Deleting the read leaves a deployment setting it silently ineffective —
/// the exact defect `manifest_wiring.rs` exists to catch, where an operator
/// "sees a diff, a new revision and no effect whatsoever". Accepting it and
/// mapping it onto something else guesses at intent. Refusing it by name, with
/// the replacement stated, is the third: the process stops, the message says
/// what to do, and `no_deployment_sets_a_variable_its_binary_refuses_to_start_with`
/// proves no deployment in the tree still sets either spelling.
///
/// Both messages are phrased "`<NAME>` is no longer read" deliberately. That
/// wording is what the acceptance walk reads a retired variable out of, so the
/// refusal and the test that no deployment trips it are the same fact written
/// once.
const RETIRED_CREDENTIAL_VARIABLES: [(&str, &str); 2] = [
    (
        "QIP_TOKEN_APPROVER",
        "configuration: QIP_TOKEN_APPROVER is no longer read. The approver role was removed \
         because no route required it, so the token it minted authorised nothing beyond what an \
         analyst token already grants. Unset this variable. Approving a venue registration is \
         POST /registrations/:source/approve and takes QIP_TOKEN_OPERATOR; approving a proposal \
         is not an API action at all and takes two kernel controls, risk and compliance.",
    ),
    (
        "QIP_TOKEN_APPROVER_FILE",
        "configuration: QIP_TOKEN_APPROVER_FILE is no longer read. The approver role was removed \
         because no route required it. Remove the token-approver secret mount from this \
         workload's catalogue entry and its rendered manifest: the Secret Manager container it \
         projects is no longer created, so the volume would fail to mount before this process \
         ever started. QIP_TOKEN_OPERATOR_FILE is the credential that approves a venue \
         registration.",
    ),
];

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

    // The autonomy ceiling, read through the one constructor that refuses a
    // live one. This used to be `AutonomyLevel::parse`, which accepts all six
    // levels because the domain model has six — so `QIP_AUTONOMY_CEILING=
    // autonomous_live` started this process live-capable, and the only thing
    // standing between it and a real venue was that nobody had set it.
    //
    // Nothing is silently lowered: a live value is a refusal to start, not a
    // demotion to paper. An operator who configured live trading and got a
    // running paper process would believe something false about their
    // deployment, which is worse than a pod that will not start and says why.
    let ceiling = AutonomyLevel::deployable(std::env::var("QIP_AUTONOMY_CEILING").ok().as_deref())?;

    // Resolved and proven writable before anything else is built. Failing here
    // costs a restart; failing at the first archived cycle costs the record of
    // everything that happened up to it. The environment is passed in rather
    // than read by the library: this is the composition root, the one place
    // that may read it, and the managed-target credentials it resolves go
    // through `qip_core::secret` so a deployment may mount them as files.
    let storage = StorageSettings::from_env(&|name| std::env::var(name).ok())?;
    storage.preflight()?;
    let archive = Arc::new(ChainArchive::open(storage.key_value("event-log")?)?);

    // The universe this process sizes against, read and journaled before the
    // platform exists — see `load_universe` for why an unset path is a
    // refusal and not an empty universe.
    let catalogue = load_universe(&storage, now)?;

    // The arbitrage desk's policy, read here because this is the one process
    // that ships the cycle whitelist down the mesh — see `load_arbitrage_policy`
    // for why an unset path is an empty whitelist and a set one that does not
    // read is a refusal. Set on the configuration before the platform exists,
    // so the plane `harden_central` rebuilds from that configuration carries
    // it too; a policy attached after the rebuild would be lost in the swap.
    let (arbitrage, arbitrage_banner) = load_arbitrage_policy()?;

    // Who has registered with a venue, from the committed file the deployment
    // names — see `load_venue_registrations` for why an unset path is a
    // registry that records nobody and a named file that does not parse is a
    // refusal to start. Read before the configuration is built because the
    // registry the feed's admission gate consults is built from that
    // configuration a few lines below, and a record applied afterwards would
    // admit a source the connector had already been refused.
    let (venue_registrations, registrations_source) = load_venue_registrations()?;

    let mut config = PlatformConfig::default()
        .with_live_ceiling(ceiling)
        .with_venue_registrations(venue_registrations);
    config.central.arbitrage = arbitrage;

    // The source `POST /cycle` senses, chosen before the platform exists
    // because a tape owns the clock the platform must be assembled on. A
    // connector is admitted by the data finder's licensing catalogue inside
    // `ApiFeed::open`, before any socket is touched, and refused rather than
    // opened when its posture is not evaluated — see `feed.rs` for why the
    // absent case stays absent instead of falling back to generated prices.
    let feed_settings = qip_api::feed::FeedSettings::from_env()
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    // The registration registry the connector is admitted against is the one
    // this configuration stands for — the same function the platform builds
    // its own from at assembly — so the feed cannot admit a source on a
    // record the platform then does not hold. With no registrations file
    // named the registry is the shipped table with no record, and every
    // source that needs an account is refused here until an operator approves
    // it through `POST /registrations/{source}/approve`, which reaches the
    // platform's registry and the event log but not a connector already
    // opened — a connector is admitted once, at start. A committed file is
    // how a deployment starts with those records already in place.
    let registrations = config
        .registration_registry()
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let mut feed =
        qip_api::feed::ApiFeed::open(&feed_settings, &registrations, config.seed, now)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    // The stream's durable record, opened on the same storage the event log
    // archives to and before anything is served. Until this call every restart
    // began with an empty dedup window, so a table-shaped source — the ECB
    // reference rates serve their whole table on every poll — republished all
    // of it as new observations at each rollout, and the session and span
    // figures that turn "streamed for a week" into a checkable claim reset to
    // zero at every start-up.
    //
    // `StreamJournal::open` writes the ledger as it opens, so a store this
    // process cannot write to stops it here rather than at the first
    // `POST /cycle`, after the process has reported ready. A zero resume is a
    // first session, not a failure, and is said out loud because the next poll
    // then republishes whatever the source re-serves.
    let journal_banner = match feed.as_mut() {
        Some(feed) => feed
            .journal_to(storage.key_value(StreamJournal::NAMESPACE)?)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?
            .map(|resumed| match resumed {
                0 => format!(
                    "on {}; nothing resumed, so this is a first session (or the last checkpoint \
                     carried nothing) and the next poll republishes whatever the source re-serves",
                    StreamJournal::NAMESPACE
                ),
                resumed => format!(
                    "on {}; {resumed} fingerprint(s) resumed from the previous session, so a \
                     redelivery of them is absorbed rather than republished",
                    StreamJournal::NAMESPACE
                ),
            }),
        None => None,
    };
    // The clock the platform reasons on. A tape owns its own, and the
    // platform must be assembled on it: opportunities expire at tape time,
    // and a router asked for a latency budget measured from the wall clock
    // against a deadline in 2025 would refuse every panel as already late.
    // Everything operational — credentials, rate limits, the console's
    // staleness, telemetry timestamps — stays on the wall clock, which is
    // what an operator is on.
    let platform_clock: Arc<dyn Clock> = match feed.as_ref().and_then(|feed| feed.owned_clock()) {
        Some(tape_clock) => tape_clock,
        None => clock.clone(),
    };
    let context = qip_core::Context::new(platform_clock, config.seed);
    // Cloned before the platform takes it: `Telemetry` holds `Arc`s over its
    // registry, tracer and logger, so the clone shares the same underlying
    // state rather than starting a second, disconnected one. The OpenObserve
    // drain thread, if configured below, reads from this handle; the platform
    // records into the same one.
    let telemetry = Telemetry::new("qip-api", clock.clone());
    let telemetry_for_export = telemetry.clone();
    let mut platform = Platform::new(
        config,
        context,
        telemetry,
        catalogue.universe,
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

    // A tape must end before the roster's authorisation does, and the
    // assembled organisation is asked directly. See
    // `ApiFeed::refuse_tape_beyond_authorisation` for the run that showed why.
    if let Some(feed) = &feed {
        feed.refuse_tape_beyond_authorisation(&platform)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    }
    let feed_banner = feed.as_ref().map_or_else(
        || {
            format!(
                "none ({} and {} are not set); POST /cycle senses nothing and reasons over what \
                 the platform already holds, and no research route will show an instrument until \
                 a source is chosen",
                qip_api::feed::TAPE_PATH_VARIABLE,
                qip_api::feed::CONNECTOR_SOURCE_VARIABLE
            )
        },
        qip_api::feed::ApiFeed::describe,
    );
    // The standing gate, named so an operator can see it is consulted rather
    // than take the claim on trust: its check count rises once per admitted
    // cycle and can be held against the stream ledger's poll count.
    let licensing_banner = feed
        .as_ref()
        .and_then(qip_api::feed::ApiFeed::licensing_standing);
    let feed = feed.map(|feed| Arc::new(Mutex::new(feed)));

    // The durable trial book, on the same storage the event log archives to.
    // The factory the plane was built with charges holdout evaluations to an
    // in-process book, so until this call every restart forgot every
    // family's lifetime trial count — the per-run accounting the deflated
    // Sharpe gate is corrected against and the one the blueprint forbids.
    // Opened after the plane is hardened, and carried across by
    // `set_central` if it ever were not. A journal that does not verify
    // stops the process here: a count rebuilt over a broken chain is the
    // understated count the chain exists to catch.
    platform
        .open_trial_book(storage.key_value("trial-book")?, "trial-book")
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    // The wallet statement, read and observed into the platform before
    // anything is served, on the wall clock — a statement is a document a
    // person dated. See `load_wallet_statement` for why unset is no feed and
    // set-but-unreadable is a refusal to start. Observed here rather than in
    // a route so that the very first cycle's LEARN stage reconciles it, and
    // wrapped around the router below so each admitted `POST /cycle`
    // re-reads a file that has changed.
    let (statement_feed, statement_banner) = load_wallet_statement(&mut platform, now)?;

    // The capital-fabric declaration, applied before anything is served, on
    // the same wall clock and for the same reason: a destination proposal is
    // an act a person dated. Until this existed, `decide_fabric`'s only
    // production caller passed `Wallet` commands, so §38.4's registry,
    // §37.1's corridor lifecycle and §37.3's seven-check gate were built and
    // reached by nothing, and `GET /transfer-gate` answered
    // `last_assessment: null` permanently rather than transiently. Applied
    // here rather than in a route so the first cycle sees the corridors the
    // desk declared, and wrapped around the router below so a corridor
    // appended while the process serves does not need a restart.
    let (fabric_feed, fabric_banner) = load_fabric_declaration(&mut platform, now)?;

    // The mesh backbone, where the deployment names cells to serve. Absent
    // configuration means the routes are absent: no listener binds, and the
    // banner says the deltas have nowhere to land here.
    let mesh_settings = qip_api::mesh::MeshSettings::from_env()?;
    let mesh = match &mesh_settings {
        Some(settings) => Some(Arc::new(Mutex::new(qip_api::mesh::MeshBackbone::open(
            settings,
            storage.key_value("mesh")?,
            clock.clone(),
            // The same trust root the plane signs envelopes with, so a cell
            // verifies policy and capital against one key and a rotation is
            // one secret rolled in one place. Absent here means policy
            // distribution is off and the backbone counts it, rather than
            // signing with something nobody verifies.
            envelope_key.as_ref().map(|key| key.as_bytes().to_vec()),
        )?))),
        None => None,
    };

    // The OpenObserve drain (ADR 0028): absent configuration means this
    // process's telemetry stays local, exactly as it always has. Set means a
    // thread starts that POSTs this process's metrics and spans on an
    // interval, and the handle must outlive `serve()` below — dropping it
    // would stop the thread while the process still runs.
    let openobserve_config = qip_api::openobserve::OpenObserveConfig::from_env()?;
    let _openobserve_drain = match &openobserve_config {
        Some(config) => Some(qip_api::openobserve::spawn(
            telemetry_for_export,
            config.clone(),
            clock.clone(),
        )?),
        None => None,
    };

    // A variable that used to mint a credential and now mints none.
    //
    // `QIP_TOKEN_APPROVER` produced a fifth bearer token, for a role no route
    // ever required — `required_role: Role::Approver` was zero rows of a table
    // in which the other four roles appeared 4, 38, 2 and 5 times. The role is
    // gone (see `qip_api::auth::Role` for why it was removed rather than given
    // a route), and with it the secret container and the mount in all four
    // environments.
    //
    // Ignoring the variable would be the quieter failure and the worse one: an
    // operator who had put a token behind it would believe a credential was
    // issued, and every rotation of it would change nothing. So it is refused
    // with the replacement named, the way `qip-edge-node` refuses
    // `QIP_MIRROR_PATH`. Both spellings, because `qip_core::secret` resolves a
    // mounted file through the `_FILE` variant and a deployment that mounted
    // the secret sets only that one — a refusal naming the bare variable alone
    // would miss every deployment that ever actually set it.
    for (variable, refusal) in RETIRED_CREDENTIAL_VARIABLES {
        if std::env::var(variable).is_ok_and(|value| !value.trim().is_empty()) {
            return Err(Error::invalid(refusal));
        }
    }

    // Credentials from the environment. Nothing is defaulted: an API that
    // starts unauthenticated because a variable was missing is worse than one
    // that does not start.
    let mut credentials = Vec::new();
    for (variable, role) in [
        ("QIP_TOKEN_MONITOR", Role::Monitor),
        ("QIP_TOKEN_VIEWER", Role::Viewer),
        ("QIP_TOKEN_ANALYST", Role::Analyst),
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

    // Built before the API so the API can be handed the store the overview
    // page reads from. An API without it runs cycles the page never shows.
    let web = Arc::new(Web::new(
        platform.clone(),
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    ));
    let mut api = Api::new(
        platform.clone(),
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    )
    .with_cells(cells.clone())
    .with_archive(archive.clone())
    .with_cycle_overview(web.cycle_overview());
    if let Some(mesh) = &mesh {
        api = api.with_mesh(mesh.clone());
    }
    // `with_feed` also admits the feed's source to the platform's reference
    // ledger — see its doc for why that lives there and not here.
    if let Some(feed) = &feed {
        api = api
            .with_feed(feed.clone())
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    }
    let api = Arc::new(api);
    let console = Arc::new(Console::new(
        platform.clone(),
        cells,
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    ));
    let router = Router::new(api, web).with_console(console);
    // With a statement feed, the router is wrapped so an admitted
    // `POST /cycle` re-reads the file before the API runs the cycle. Without
    // one the router is served bare: no code on the cycle path can re-read
    // anything, which is the structural form of "no feed".
    let handler: Arc<dyn qip_api::http::Handler> = match statement_feed {
        Some(feed) => Arc::new(qip_api::statement::StatementRefresh::new(
            router,
            Arc::new(Mutex::new(feed)),
            platform.clone(),
            authenticator.clone(),
            clock.clone(),
        )),
        None => Arc::new(router),
    };
    // Nested outside the statement wrapper, so an admitted cycle applies any
    // appended fabric command before the statement is re-read and before the
    // cycle runs. The order matters only in that both happen first; neither
    // reads what the other writes.
    let handler: Arc<dyn qip_api::http::Handler> = match fabric_feed {
        Some(feed) => Arc::new(qip_api::fabric::FabricRefresh::new(
            handler,
            Arc::new(Mutex::new(feed)),
            platform.clone(),
            authenticator.clone(),
            clock.clone(),
        )),
        None => handler,
    };

    let server = Server::bind(&address, handler, ServerLimits::default())?;
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
    // The live surface, in the same terms as the rest of the banner. An
    // operator reading it should know how long a stream stays open and what a
    // client is expected to do when it closes, before a dashboard is pointed
    // at this process and found to reconnect every five minutes.
    {
        let limits = qip_api::stream::StreamLimits::default();
        println!(
            "  live streams:     {} server-sent-event stream(s) under {}{}; heartbeat every {}s, \
             each connection closes after {}s and resumes on Last-Event-ID",
            qip_api::stream::StreamKind::ALL.len(),
            qip_api::routes::VERSION_PREFIX,
            qip_api::stream::STREAM_PREFIX,
            limits.heartbeat_after.as_secs(),
            limits.max_duration.as_secs()
        );
    }
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
    println!("  feed:             {feed_banner}");
    if let Some(line) = &journal_banner {
        println!("  stream journal:   {line}");
    }
    if let Some(line) = &licensing_banner {
        println!("  licensing:        {line}");
    }
    if let Some(feed) = &feed {
        let Ok(feed) = feed.lock() else {
            return Err(Error::invalid(
                "the feed is in an inconsistent state before serving began",
            ));
        };
        if let Some(tape_clock) = feed.owned_clock() {
            println!(
                "  platform clock:   tape time, at {} until the first POST /cycle; credentials, \
                 rate limits and the console stay on the wall clock",
                tape_clock.now().to_rfc3339()
            );
        }
    }
    println!("  arbitrage desk:   {arbitrage_banner}");
    println!("  wallet statement: {statement_banner}");
    println!("  capital fabric:   {fabric_banner}");
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
    match &openobserve_config {
        Some(config) => println!("  openobserve:      draining to {}", config.describe()),
        None => println!(
            "  openobserve:      not draining ({} is not set); telemetry stays local to \
             this process",
            qip_api::openobserve::URL_VARIABLE
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
    {
        let platform = platform.lock().map_err(|_| {
            Error::invalid("the platform lock is poisoned before the first request")
        })?;
        println!("  registrations:    {registrations_source}");
        println!("  source standing:  {}", registrations_banner(&platform));
    }
    println!(
        "  universe:         {}; sector and country buckets are fed from it. Note ADR 0027: under the \
         conservative default the first desk order into an empty book is refused by \
         sector-concentration, and the decision is the risk desk's, not this process's",
        catalogue.manifest.describe()
    );

    server.serve()
}

/// One banner line: which catalogued sources stand registered, keyless or
/// pending, as the platform's own registry answers at start.
///
/// Read off the assembled platform rather than the configuration so the line
/// describes the registry a route will read, and every source in the
/// finder's catalogue is named so an operator sees what is refused as well
/// as what is admitted — a banner that listed only the admitted sources would
/// read as a platform with nothing to register.
fn registrations_banner(platform: &Platform) -> String {
    let catalogue = match qip_data_finder::admission::catalogue() {
        Ok(catalogue) => catalogue,
        Err(error) => return format!("the catalogue did not build: {}", error.message()),
    };
    let lines: Vec<String> = catalogue
        .iter()
        .map(|entry| {
            let standing = match platform.registration_standing(entry.source_id) {
                Ok(standing) => standing.describe(),
                Err(_) => "pending; refused until an operator approves it".to_string(),
            };
            format!("{} ({standing})", entry.source_id)
        })
        .collect();
    lines.join("; ")
}

/// The instrument universe, from the committed catalogue the deployment names.
///
/// Refused when unset. Every root used to assemble `Universe::new()`, so the
/// exposure buckets the kernel projects from the universe at assembly —
/// sector, country, asset class, venue — received nothing in any deployed
/// process and the two bucket limits in the default set could never fire;
/// an empty universe is the state that hid that, and a process that fell
/// back to one on a missing variable would hide it again. The catalogue's
/// hash is recorded in the `universe` namespace of the same storage the
/// event log archives to, under its hash and as `current`, so a run can say
/// which catalogue it sized against.
fn load_universe(
    storage: &StorageSettings,
    now: qip_core::Timestamp,
) -> Result<qip_financial::LoadedCatalogue> {
    let path = std::env::var("QIP_UNIVERSE_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            Error::invalid(
                "configuration: QIP_UNIVERSE_PATH is not set. Point it at the committed instrument \
                 catalogue — data/datasets/universe.json in the repository, mounted at \
                 /etc/qip/universe.json by the deployment; this process does not start on an \
                 empty universe, because an empty universe feeds no exposure bucket and \
                 nothing would say so",
            )
        })?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: QIP_UNIVERSE_PATH names {path}, which cannot be read: {error}"
        ))
    })?;
    let catalogue = qip_financial::catalogue::load(&text, now)
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    qip_financial::catalogue::record_manifest(
        storage.key_value("universe")?.as_ref(),
        &catalogue.manifest,
    )?;
    Ok(catalogue)
}

/// Where the venue registrations are read from: a JSON array of
/// `RegistrationRecord`s, mounted the way the universe is. Unset means nobody
/// has registered.
const VENUE_REGISTRATIONS_VARIABLE: &str = "QIP_VENUE_REGISTRATIONS_PATH";

/// The registrations the deployment commits, from the JSON file it names, and
/// the banner line that says where they came from.
///
/// `QIP_VENUE_REGISTRATIONS_PATH` unset is not a refusal: it is a deployment
/// where nobody has registered with a venue, and the platform's answer to that
/// is the fail-closed one — the shipped requirement table records nobody, and
/// every source needing an account is refused by name until an operator
/// approves it at runtime. Set and unreadable, or readable and not an array of
/// records, is a refusal to start: a process that fell back to "nobody
/// registered" because the file the operator pointed at was missing would run
/// healthy with every account-bearing source silently refused, and the
/// operator would look for the fault at the venue.
///
/// Nothing here can invent an attribution. `RegistrationRecord` has one
/// constructor, it refuses a blank operator, a blank terms citation and a
/// `secret` that is not a variable name, and the `Deserialize` impl goes
/// through that same constructor — so a malformed record is refused at load
/// with serde naming the field, and a pasted key is refused without being
/// echoed. That is the whole reason this reads records rather than accepting
/// a looser shape and validating it afterwards.
///
/// An empty array is refused too. A mounted file that registers nobody is a
/// configured feed that grants nothing, which reads to the next operator as a
/// registration that failed rather than as a deployment where nobody
/// registered; unsetting the variable says the second thing, and says it in
/// the plan as well as in the banner.
fn load_venue_registrations() -> Result<(Vec<RegistrationRecord>, String)> {
    let Some(path) = std::env::var(VENUE_REGISTRATIONS_VARIABLE)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok((
            Vec::new(),
            format!(
                "none ({VENUE_REGISTRATIONS_VARIABLE} is not set); the registry records nobody \
                 and every source that needs an account stays refused until an operator \
                 approves it"
            ),
        ));
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: {VENUE_REGISTRATIONS_VARIABLE} names {path}, which cannot be read: \
             {error}. Unset it to run with no registrations; a named file that does not read is \
             not a deployment where nobody registered"
        ))
    })?;
    let records: Vec<RegistrationRecord> = serde_json::from_str(&text).map_err(|error| {
        Error::invalid(format!(
            "configuration: {VENUE_REGISTRATIONS_VARIABLE} names {path}, which is not an array \
             of venue registrations: {error}. Each record carries source_id, operator, \
             terms_read_at, terms and secret.variable — the deployment variable the credential \
             is read under, never the credential; see docs/operations/registering-a-venue.md"
        ))
    })?;
    if records.is_empty() {
        return Err(Error::invalid(format!(
            "configuration: {VENUE_REGISTRATIONS_VARIABLE} names {path}, which registers nobody. \
             Unset the variable — the deployment then says out loud that nobody registered, \
             rather than mounting a file that grants nothing"
        )));
    }
    let banner = format!(
        "{} record(s) from {path}: {}",
        records.len(),
        records
            .iter()
            .map(|record| format!("{} by {}", record.source_id(), record.operator()))
            .collect::<Vec<_>>()
            .join("; ")
    );
    Ok((records, banner))
}

/// Where the arbitrage desk's policy is read from: a JSON `ArbitragePolicy`,
/// mounted the way the universe is. Unset means no desk.
const ARBITRAGE_POLICY_VARIABLE: &str = "QIP_ARBITRAGE_POLICY_PATH";

/// The arbitrage desk's policy, from the JSON file the deployment names, and
/// the banner line that says what was read.
///
/// `QIP_ARBITRAGE_POLICY_PATH` unset is not a refusal: it is the operator
/// saying there is no desk, and the platform's answer to that is the
/// fail-closed one — `CentralConfig::arbitrage` stays `None`, every cycle
/// whitelist ships empty, and each cell's installer declines by name. Set
/// and unreadable, or readable and not a policy, is a refusal to start: a
/// process that fell back to no desk because the file the operator pointed
/// at was missing would run healthy with the desk silently off, which is the
/// state that hid the empty universe once already. The content is only
/// parsed here; whether it says something the cell would refuse is judged
/// when the plane assembles, which also stops the process, naming the entry.
fn load_arbitrage_policy() -> Result<(Option<ArbitragePolicy>, String)> {
    let Some(path) = std::env::var(ARBITRAGE_POLICY_VARIABLE)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok((
            None,
            format!(
                "none ({ARBITRAGE_POLICY_VARIABLE} is not set); every cycle whitelist ships \
                 empty and no cell installs a desk"
            ),
        ));
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: {ARBITRAGE_POLICY_VARIABLE} names {path}, which cannot be read: \
             {error}. Unset it to run with no desk; a named policy that does not read is not \
             a desk that is off"
        ))
    })?;
    let policy: ArbitragePolicy = serde_json::from_str(&text).map_err(|error| {
        Error::invalid(format!(
            "configuration: {ARBITRAGE_POLICY_VARIABLE} names {path}, which is not an \
             arbitrage policy: {error}. See docs/operations/arbitrage-policy.md for the fields"
        ))
    })?;
    let banner = format!(
        "policy from {path}: strategy {}, {} market(s) across {} venue(s), funded in {}; \
         sized against that strategy's live grant at each cell",
        policy.strategy,
        policy.markets.len(),
        policy.venues.len(),
        policy.funding_instrument
    );
    Ok((Some(policy), banner))
}

/// The wallet statement the deployment names, observed into `platform`, and
/// the banner line that says what was read.
///
/// `QIP_WALLET_STATEMENT_PATH` unset is not a refusal: it is the operator
/// saying no custodian has reported to this process, and the platform's
/// answer is the honest one — nothing is observed, LEARN reconciles nothing,
/// and `/wallet` answers `assembled: false`. Set and unreadable, or readable
/// and not a statement, or dated in the future, or past the kernel's bound,
/// is a refusal to start naming the field: a process that fell back to no
/// feed because the file the operator pointed at was wrong would run
/// healthy with the wallet silently unobserved, which is the state every
/// deployment was in before this feed existed. Nothing is clamped — a
/// statement of 257 holdings is refused, not truncated to 256.
fn load_wallet_statement(
    platform: &mut Platform,
    now: qip_core::Timestamp,
) -> Result<(Option<qip_api::statement::StatementFeed>, String)> {
    let Some(feed) =
        qip_api::statement::StatementFeed::from_env(&|name| std::env::var(name).ok(), now)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?
    else {
        return Ok((None, qip_api::statement::absent_banner()));
    };
    feed.statement()
        .observe_into(platform)
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let banner = feed.describe();
    Ok((Some(feed), banner))
}

/// The capital-fabric declaration the deployment names, applied into
/// `platform`, and the banner line that says what was read.
///
/// `QIP_CAPITAL_FABRIC_PATH` unset is not a refusal: it is the operator
/// saying no destination, corridor or transfer intent has been declared to
/// this process, and the platform's answer is the honest one — nothing is
/// journalled, the registry stays empty, and `/transfer-gate` answers
/// `last_assessment: null`. Set and unreadable, or readable and not a
/// declaration, or carrying a command the log refuses, is a refusal to start
/// naming the position: a process that fell back to no declaration because
/// the file the operator pointed at was wrong would serve with the fabric
/// silently unreached, which is the state every deployment was in before this
/// existed.
///
/// A command the *control* refuses is not a refusal to start. `decide_fabric`
/// answers a refused command with an `Outcome::Refused` inside an `Ok`
/// record, because the refusal is a decision and belongs on the chain — a
/// corridor proposed against a destination that was never verified is
/// evidence the desk should see, not a reason the API cannot serve.
fn load_fabric_declaration(
    platform: &mut Platform,
    now: qip_core::Timestamp,
) -> Result<(Option<qip_api::fabric::FabricFeed>, String)> {
    let Some(mut feed) = qip_api::fabric::FabricFeed::from_env(&|name| std::env::var(name).ok())
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?
    else {
        return Ok((None, qip_api::fabric::absent_banner()));
    };
    let applied = feed
        .apply_pending(platform, now)
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let banner = format!("{} ({applied} applied)", feed.describe());
    Ok((Some(feed), banner))
}

#[cfg(test)]
mod tests {
    //! Two things this root pins. The operator page's example policy is one
    //! the plane accepts, so the page cannot drift from the type it
    //! documents — a runbook whose example is refused at start-up is worse
    //! than none. And what the committed catalogue does to the first desk
    //! order under the conservative default, which ADR 0027 has now decided:
    //! the two entries are `MaxAxisWeight` against equity rather than
    //! `MaxConcentration` against gross, so one position in an empty book is
    //! no longer the whole of its axis and the first order is admitted. The
    //! test asserts both halves, because a cap that admits everything is the
    //! `MaxExpectedShortfall` defect wearing the other mask.

    // The workspace denies `panic_in_result_fn` for production code; in a
    // test the assertion is the deliverable and `?` keeps the setup readable.
    #![allow(clippy::panic_in_result_fn)]

    use qip_core::error::{Error, Result};
    use qip_core::{Context, Timestamp, dec};
    use qip_kernel::central::ArbitragePolicy;
    use qip_kernel::{Platform, PlatformConfig};
    use qip_observability::Telemetry;
    use qip_risk::AggregateFigures;
    use qip_risk::limits::LimitSet;

    #[test]
    fn the_operator_pages_example_policy_is_one_the_plane_accepts() -> Result<()> {
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../docs/operations/arbitrage-policy.md"
        ))
        .expect("the operator page is readable");
        let (_, rest) = page
            .split_once("```json\n")
            .ok_or_else(|| Error::not_found("the page's JSON example"))?;
        let (example, _) = rest
            .split_once("```")
            .ok_or_else(|| Error::not_found("the end of the page's JSON example"))?;
        let policy: ArbitragePolicy = serde_json::from_str(example)?;
        // Premise: the example says something, so acceptance below is of a
        // policy and not of an empty object.
        assert_eq!(policy.markets.len(), 1);
        policy.validate()?;
        let mut config = PlatformConfig::default();
        config.central.arbitrage = Some(policy);
        let (context, _clock) =
            Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            qip_financial::universe::Universe::new(),
            LimitSet::conservative_default(),
        )?;
        Ok(())
    }

    const COMMITTED: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../data/datasets/universe.json"
    );

    #[test]
    fn the_first_order_into_a_catalogued_universe_is_admitted_and_a_sector_past_the_cap_is_still_refused()
    -> Result<()> {
        let now = Timestamp::from_civil(2026, 9, 2);
        let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
        let catalogue = qip_financial::catalogue::load(&text, now)?;
        // Premise: the universe is not empty, and the instrument the order
        // is about carries a sector — the axis the refusal names.
        let first = catalogue
            .universe
            .iter()
            .find(|object| object.is_decision_grade())
            .expect("the committed catalogue carries a decision-grade instrument")
            .clone();
        assert!(!catalogue.universe.is_empty());
        assert_ne!(
            first.sector,
            qip_financial::asset_class::Sector::Unclassified,
            "the fixture instrument has no sector to be concentrated in"
        );

        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(now, config.seed);
        let mut platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            catalogue.universe,
            LimitSet::conservative_default(),
        )?;
        // Premise: the kernel projected the sector axis for this instrument,
        // and the book is empty, so the order below is the first position.
        let axes = platform.exposure_axes_for(first.object_id.as_str());
        assert_eq!(
            axes.get("sector").map(String::as_str),
            Some(
                serde_json::to_value(first.sector)?
                    .as_str()
                    .expect("sector is a string")
            ),
            "the kernel did not project the sector bucket from the catalogue"
        );
        assert!(platform.risk_figures().gross_exposure().is_zero());
        assert_eq!(platform.risk_figures().fills(), 0);

        // The side is read from its wire form because this root deliberately
        // does not link the execution engine — `api_boundary.rs` holds it to
        // that — and `Side` is nameable nowhere else.
        let side = serde_json::from_str("\"buy\"")?;
        let order = platform.order_from(
            first.object_id.clone(),
            side,
            dec!("10"),
            first.price,
            "prop-adr-0027",
            vec!["hyp-adr-0027".to_string()],
            now,
        );
        platform.submit_order(order, now).map_err(|error| {
            Error::invalid(format!(
                "the first order into a catalogued universe was refused: {}. ADR 0027 replaced \
                 the share-of-gross denominator precisely so that one position in an empty book \
                 is no longer the whole of its axis",
                error.message()
            ))
        })?;

        // Half the test. Admitting the first order is only an improvement if
        // the control that replaced the old one can still veto, and the
        // failure this second half prevents is the one ADR 0027 is a reaction
        // to: `MaxExpectedShortfall` shipped in every default set and could
        // never fire, and a cap that admits everything is that defect wearing
        // the other mask. So the same axis is driven past 0.35 of equity and
        // must be refused by name.
        let equity = platform.risk_figures().equity();
        // Whole lots of the catalogue's default lot of one: the central
        // feasibility gate now sits ahead of every other control and refuses
        // a fractional size by name, so an off-grid order would prove the
        // wrong veto. The premise of this half is the sector cap, and half
        // the book's equity floored to a whole share is still past it.
        let breaching_quantity =
            ((equity * dec!("0.5")) / first.price).floor_to_step(qip_core::Decimal::ONE);
        assert!(
            breaching_quantity.is_positive(),
            "the premise: half the equity buys at least one whole share"
        );
        let breach = platform.order_from(
            first.object_id.clone(),
            side,
            breaching_quantity,
            first.price,
            "prop-adr-0027",
            vec!["hyp-adr-0027".to_string()],
            now,
        );
        let refusal = match platform.submit_order(breach, now) {
            Ok(()) => panic!(
                "half of this book's equity in one sector was admitted under a 0.35 axis-weight \
                 cap; the control ADR 0027 installed cannot fire"
            ),
            Err(error) => error.message().to_string(),
        };
        // The delimited token, not a substring: the refusal names the limit
        // as `sector-concentration: …`, and a message that merely mentioned
        // the word would not prove which control fired.
        assert!(
            refusal
                .split_whitespace()
                .any(|token| token == "sector-concentration:"),
            "the refusal does not name sector-concentration as the control that fired: {refusal}"
        );
        Ok(())
    }
}
