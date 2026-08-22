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

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
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

    let config = PlatformConfig::default().with_live_ceiling(ceiling);
    let context = qip_core::Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::new("qip-api", clock.clone()),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;

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
        let Ok(token) = std::env::var(variable) else {
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

    let api = Arc::new(Api::new(
        platform.clone(),
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    ));
    let web = Arc::new(Web::new(platform, authenticator, rate_limiter, clock));
    let router = Router::new(api, web);

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
    println!("  operator ui:      9 surface(s), no JavaScript");

    server.serve()
}
