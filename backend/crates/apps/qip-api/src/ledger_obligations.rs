//! The ledger's obligations, read from `qip-ledgerd` before each cycle
//! (R12-CAPITAL-4086).
//!
//! The survival waterfall reserves margin, collateral and unsettled cash
//! before it sizes anything, and the ledger is the only source of those
//! three figures. `qip-ledgerd` serves them at
//! `GET /api/v1/ledger/obligations?unit=&as_of=`, and
//! `Platform::observe_ledger_obligations` takes them, but nothing in a
//! shipped binary connected the two, so every cycle sized against a
//! waterfall that reserved nothing the ledger held.
//!
//! Each `POST /cycle` reads the route at the cycle's instant in the
//! settlement currency and installs the reading before the cycle runs. A
//! timeout, a non-200 answer or a body that does not decode installs no
//! reading and is recorded as a cycle problem: the cycle runs on the last
//! reading the platform holds, said aloud, rather than on a figure nobody
//! read. Unset [`URL_VARIABLE`] reads nothing and says so at start-up, as
//! the desk shipper does.
//!
//! The read happens under the platform lock, because the instant it is
//! taken at is the cycle's, which only exists once the feed has sensed under
//! that lock. The timeouts are therefore tight and explicit: a ledger that
//! does not answer in a second costs the cycle a second, not a hang.

use qip_contracts::ledger::LedgerObligations;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_transport::http::{ClientLimits, HttpClient};
use std::time::Duration as StdDuration;

/// The variable naming `qip-ledgerd`'s read API, as `http://host:port`.
pub const URL_VARIABLE: &str = "QIP_LEDGERD_URL";

/// The settlement currency the survival waterfall sizes in, which is the
/// unit the reading is taken in.
pub const UNIT: &str = "USD";

/// Reads `qip-ledgerd`'s obligations, or nothing when no URL is set.
#[derive(Clone, Debug)]
pub struct LedgerObligationsReader {
    base: Option<String>,
    client: HttpClient,
}

impl LedgerObligationsReader {
    /// From the environment. A URL that is not plaintext `http://` is
    /// refused at start rather than at the first cycle: the client speaks
    /// plaintext only, and the egress proxy is where TLS lives.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let base = lookup(URL_VARIABLE)
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        if let Some(base) = &base
            && !base.starts_with("http://")
        {
            return Err(Error::invalid(format!(
                "configuration: {URL_VARIABLE}={base} is not a plaintext http:// URL; this \
                 client speaks plaintext only, so point it at qip-ledgerd's read listener or \
                 its local egress proxy"
            )));
        }
        Ok(Self::new(base))
    }

    /// A reader for `base`, or one that reads nothing for `None`.
    pub fn new(base: Option<String>) -> Self {
        Self {
            base,
            client: HttpClient::new(ClientLimits {
                connect_timeout: StdDuration::from_secs(1),
                read_timeout: StdDuration::from_secs(1),
                write_timeout: StdDuration::from_secs(1),
                max_body: 64 * 1024,
                ..ClientLimits::default()
            }),
        }
    }

    /// The start-up line.
    pub fn banner(&self) -> String {
        match &self.base {
            Some(base) => {
                format!("ledger obligations: read from {base} before every cycle, in {UNIT}")
            }
            None => format!(
                "ledger obligations: NOT READ — {URL_VARIABLE} is unset, so the survival \
                 waterfall reserves no margin, collateral or unsettled cash the ledger holds"
            ),
        }
    }

    /// The ledger's obligations as of `at`, `Ok(None)` when no URL is set,
    /// or the reason no reading was taken.
    pub fn read(&self, at: Timestamp) -> std::result::Result<Option<LedgerObligations>, String> {
        let Some(base) = &self.base else {
            return Ok(None);
        };
        let url = format!(
            "{base}/api/v1/ledger/obligations?unit={UNIT}&as_of={}",
            at.as_secs()
        );
        let response = self.client.get(&url).map_err(|error| {
            format!("the ledger's obligations were not read from {url}: {error}")
        })?;
        if response.status != 200 {
            return Err(format!(
                "the ledger answered {} {} for its obligations at {url}",
                response.status, response.reason
            ));
        }
        let obligations: LedgerObligations =
            serde_json::from_slice(&response.body).map_err(|error| {
                format!("the ledger's obligations from {url} did not decode: {error}")
            })?;
        Ok(Some(obligations))
    }
}
