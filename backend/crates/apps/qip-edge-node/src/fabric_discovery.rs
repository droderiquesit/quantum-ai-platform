//! Fabric bootstrap from the broker's SRV record (R12-FABRIC-10016, ADR 0135).
//!
//! Hard-coded broker addresses break when a broker is replaced; discovery
//! through `_qip-event-fabric._tcp.<zone>` survives it, and the configured
//! `QIP_FABRIC_URL` remains the last rung so a node with no DNS still starts
//! and DNS loss never stops it (`qip_transport::dns::Bootstrap`).
//!
//! The resolver, the zone, the per-query timeout and the attempt count are
//! all required together: no timeout or attempt figure is chosen here, and a
//! resolver named without them would be a lookup this node tuned by guess.
//! One of the four without the others stops the process.

use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_observability::metrics::{Metrics, names};
use qip_transport::dns::{AddressSource, Bootstrap, SrvResolver, fabric_srv_owner};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub const RESOLVER_VARIABLE: &str = "QIP_FABRIC_SRV_RESOLVER";
pub const ZONE_VARIABLE: &str = "QIP_FABRIC_SRV_ZONE";
pub const TIMEOUT_VARIABLE: &str = "QIP_FABRIC_SRV_TIMEOUT_MS";
pub const ATTEMPTS_VARIABLE: &str = "QIP_FABRIC_SRV_ATTEMPTS";

/// Where discovery asks, when it is configured.
#[derive(Debug)]
pub struct FabricDiscovery {
    resolver: SrvResolver,
    owner: String,
}

/// The endpoint a node will use and which rung supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredEndpoint {
    pub url: String,
    pub source: AddressSource,
    pub lookup_failure: Option<String>,
}

impl FabricDiscovery {
    /// `None` when none of the four is set.
    pub fn read(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let set = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());
        let (resolver, zone, timeout, attempts) = (
            set(RESOLVER_VARIABLE),
            set(ZONE_VARIABLE),
            set(TIMEOUT_VARIABLE),
            set(ATTEMPTS_VARIABLE),
        );
        if resolver.is_none() && zone.is_none() && timeout.is_none() && attempts.is_none() {
            return Ok(None);
        }
        let (Some(resolver), Some(zone), Some(timeout), Some(attempts)) =
            (resolver, zone, timeout, attempts)
        else {
            return Err(Error::invalid(format!(
                "configuration: {RESOLVER_VARIABLE}, {ZONE_VARIABLE}, {TIMEOUT_VARIABLE} and \
                 {ATTEMPTS_VARIABLE} are set together or not at all; a resolver with no zone \
                 asks nothing, and a timeout or attempt count is never guessed"
            )));
        };
        let address: SocketAddr = resolver.trim().parse().map_err(|_| {
            Error::invalid(format!(
                "configuration: {RESOLVER_VARIABLE}={resolver} is not an ip:port"
            ))
        })?;
        let millis: u64 = timeout.trim().parse().map_err(|_| {
            Error::invalid(format!(
                "configuration: {TIMEOUT_VARIABLE}={timeout} is not a whole number of milliseconds"
            ))
        })?;
        let attempts: u8 = attempts.trim().parse().map_err(|_| {
            Error::invalid(format!(
                "configuration: {ATTEMPTS_VARIABLE}={attempts} is not a count from 1 to 255"
            ))
        })?;
        Ok(Some(Self {
            resolver: SrvResolver::new(address, Duration::from_millis(millis), attempts)?,
            owner: fabric_srv_owner(zone.trim()),
        }))
    }

    /// Resolve the endpoint for `configured_url` (`scheme://host:port`, no
    /// path). The record supplies the authority; the scheme is the
    /// configured one, because the link security was chosen from it. A
    /// failed lookup lands on the cached answer or the configured address —
    /// never an error: DNS loss must not stop the node.
    pub fn resolve(
        &self,
        configured_url: &str,
        clock: Arc<dyn Clock>,
    ) -> Result<DiscoveredEndpoint> {
        let (scheme, authority) = configured_url.split_once("://").ok_or_else(|| {
            Error::invalid(format!(
                "configuration: the fabric endpoint {configured_url} has no scheme"
            ))
        })?;
        let mut bootstrap = Bootstrap::new(vec![authority.to_string()], clock)?;
        let resolved = bootstrap.resolve(|| self.resolver.query(&self.owner));
        let first = resolved.authorities.first().ok_or_else(|| {
            Error::invalid("the fabric bootstrap resolved to no authority".to_string())
        })?;
        Ok(DiscoveredEndpoint {
            url: format!("{scheme}://{first}"),
            source: resolved.source,
            lookup_failure: resolved.lookup_failure,
        })
    }
}

/// Record which rung answered, one series per rung, `1` on the one in force
/// and `0` on the others, so a region on stale discovery reads as such.
pub fn record_source(metrics: &Metrics, source: AddressSource) {
    metrics.describe(
        names::FABRIC_ADDRESS_SOURCE,
        "which rung supplied the fabric endpoint, by source: srv, cached, configured",
    );
    for candidate in [
        AddressSource::Srv,
        AddressSource::Cached,
        AddressSource::Configured,
    ] {
        let mut labels = qip_observability::metrics::Labels::new();
        labels.insert("source".to_string(), candidate.as_label().to_string());
        metrics.gauge(
            names::FABRIC_ADDRESS_SOURCE,
            labels,
            if candidate == source { 1.0 } else { 0.0 },
        );
    }
}
