//! Where a source lives and what it takes to poll it.
//!
//! The access mechanism is not decoration. Ingestion runs on
//! [`qip_market_ingestion::adapter::DataAdapter`], whose `poll(until)` leaves
//! the clock with the caller — that is what lets one adapter serve a live run,
//! a backtest and a replay. Half the mechanisms below push rather than answer
//! (a websocket, a multicast group), so an adapter over them has to buffer
//! arrivals and drain the buffer on `poll`. A mechanism recorded without that
//! distinction produces an adapter that blocks or drops, and neither failure
//! shows up until a replay disagrees with the live run it is meant to
//! reproduce.

use qip_core::Duration;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Transport scheme of an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    Https,
    Http,
    Wss,
    Ws,
    Git,
    File,
    /// UDP multicast, for exchange-style streaming feeds.
    Udp,
    /// Model Context Protocol server.
    Mcp,
}

impl Scheme {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
            Self::Wss => "wss",
            Self::Ws => "ws",
            Self::Git => "git",
            Self::File => "file",
            Self::Udp => "udp",
            Self::Mcp => "mcp",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "https" => Some(Self::Https),
            "http" => Some(Self::Http),
            "wss" => Some(Self::Wss),
            "ws" => Some(Self::Ws),
            "git" => Some(Self::Git),
            "file" => Some(Self::File),
            "udp" => Some(Self::Udp),
            "mcp" => Some(Self::Mcp),
            _ => None,
        }
    }

    /// Whether the transport authenticates the server and encrypts the body.
    ///
    /// Recorded rather than enforced here: cleartext lowers the reliability
    /// score and is stated in the decision, because a feed a network can
    /// rewrite is a feed that can be made to say anything.
    pub const fn is_encrypted(&self) -> bool {
        matches!(self, Self::Https | Self::Wss)
    }
}

/// Whether a mechanism answers a request or arrives on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// The adapter asks and the source answers. Maps straight onto `poll`.
    Pull,
    /// The source pushes. An adapter must buffer arrivals and hand over
    /// everything up to `until` on `poll`, or the caller stops owning the
    /// clock and replay stops being reproducible.
    PushBuffered,
}

/// How a candidate's records are authenticated at the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "auth", rename_all = "snake_case")]
pub enum AuthRequirement {
    /// Anyone may read it.
    None,
    /// A shared key in a header or query parameter.
    ApiKey { header: String },
    /// A bearer token obtained from a token endpoint.
    OAuth2 { token_endpoint: String },
    /// A client certificate presented at the TLS handshake.
    MutualTls,
}

impl AuthRequirement {
    /// What a deployment must be given before this endpoint can be reached.
    ///
    /// `None` means nothing has to be supplied — not that nothing is needed.
    pub fn credential_required(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::ApiKey { header } => Some(format!("an API key presented in `{header}`")),
            Self::OAuth2 { token_endpoint } => {
                Some(format!("OAuth2 client credentials for {token_endpoint}"))
            }
            Self::MutualTls => Some("a client certificate and its private key".to_string()),
        }
    }
}

/// Syndication feed dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedFormat {
    Rss,
    Atom,
}

/// Encoding of a bulk file drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Csv,
    Json,
    Parquet,
    Xml,
    FixedWidth,
}

/// Which part of an MCP server a source is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mcp", rename_all = "snake_case")]
pub enum McpTarget {
    /// A callable tool, invoked with arguments.
    Tool { name: String },
    /// A resource read by URI.
    Resource { uri: String },
    /// The whole server, enumerated at poll time.
    Server,
}

/// How a source is reached, carrying what a poll of it needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mechanism", rename_all = "snake_case")]
pub enum AccessMechanism {
    /// Request/response over HTTP.
    Rest {
        auth: AuthRequirement,
        /// Query parameter that bounds a request to records after a cursor.
        /// `None` means every poll refetches, which is what makes a REST
        /// source expensive rather than what makes it unusable.
        incremental_parameter: Option<String>,
        page_size: u32,
    },
    /// A long-lived socket the source writes to.
    WebSocket {
        auth: AuthRequirement,
        /// The frame sent after connect to select what arrives.
        subscribe_frame: String,
        /// How long the socket may be silent before it is treated as gone.
        heartbeat_interval: Duration,
    },
    /// RSS or Atom, re-fetched whole.
    Feed {
        format: FeedFormat,
        /// How often the publisher says it republishes.
        published_every: Duration,
    },
    /// A file dropped on a schedule and fetched entire.
    BulkFile {
        format: FileFormat,
        published_every: Duration,
        auth: AuthRequirement,
    },
    /// A repository, polled by fetching a ref.
    GitRepository {
        branch: String,
        /// Path within the tree that holds the data.
        path: String,
    },
    /// A Model Context Protocol server, tool or resource.
    Mcp {
        target: McpTarget,
        auth: AuthRequirement,
    },
    /// A page with no machine interface, read by extraction.
    HtmlPage {
        /// The element the values are read from. Extraction is brittle by
        /// construction, which is why an HTML source scores low on
        /// reliability rather than being rejected outright.
        selector: String,
    },
    /// A multicast group carrying an exchange-style stream.
    StreamingMulticast {
        group: String,
        port: u16,
        /// Wire protocol name, decoded by `qip-protocols`.
        wire_protocol: String,
    },
}

impl AccessMechanism {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Rest { .. } => "rest",
            Self::WebSocket { .. } => "websocket",
            Self::Feed { .. } => "feed",
            Self::BulkFile { .. } => "bulk_file",
            Self::GitRepository { .. } => "git_repository",
            Self::Mcp { .. } => "mcp",
            Self::HtmlPage { .. } => "html_page",
            Self::StreamingMulticast { .. } => "streaming_multicast",
        }
    }

    /// Everything an adapter author needs in order to write `poll`.
    pub fn poll_plan(&self) -> PollPlan {
        match self {
            Self::Rest {
                incremental_parameter,
                ..
            } => PollPlan {
                delivery: Delivery::Pull,
                natural_interval: Duration::from_secs(60),
                incremental: incremental_parameter.is_some(),
                credential_required: self.credential_required(),
            },
            Self::WebSocket {
                heartbeat_interval,
                auth,
                ..
            } => PollPlan {
                delivery: Delivery::PushBuffered,
                natural_interval: *heartbeat_interval,
                incremental: true,
                credential_required: auth.credential_required(),
            },
            Self::Feed {
                published_every, ..
            }
            | Self::BulkFile {
                published_every, ..
            } => PollPlan {
                delivery: Delivery::Pull,
                natural_interval: *published_every,
                incremental: false,
                credential_required: self.credential_required(),
            },
            Self::GitRepository { .. } => PollPlan {
                delivery: Delivery::Pull,
                natural_interval: Duration::from_mins(15),
                // A fetch names the commit it moved from, so a poll can ask
                // for the difference rather than the tree.
                incremental: true,
                credential_required: None,
            },
            Self::Mcp { auth, .. } => PollPlan {
                delivery: Delivery::Pull,
                natural_interval: Duration::from_mins(5),
                incremental: false,
                credential_required: auth.credential_required(),
            },
            Self::HtmlPage { .. } => PollPlan {
                delivery: Delivery::Pull,
                natural_interval: Duration::from_mins(30),
                incremental: false,
                credential_required: None,
            },
            Self::StreamingMulticast { .. } => PollPlan {
                delivery: Delivery::PushBuffered,
                natural_interval: Duration::from_millis(1),
                incremental: true,
                credential_required: None,
            },
        }
    }

    fn credential_required(&self) -> Option<String> {
        match self {
            Self::Rest { auth, .. }
            | Self::WebSocket { auth, .. }
            | Self::BulkFile { auth, .. }
            | Self::Mcp { auth, .. } => auth.credential_required(),
            Self::Feed { .. }
            | Self::GitRepository { .. }
            | Self::HtmlPage { .. }
            | Self::StreamingMulticast { .. } => None,
        }
    }

    /// Whether robots.txt governs this mechanism.
    ///
    /// robots.txt is a statement about a web origin. It does not reach a
    /// multicast group or a licensed bulk drop, and pretending it does would
    /// make the legality verdict unknown for sources whose terms are actually
    /// settled by a contract.
    pub const fn is_governed_by_robots(&self) -> bool {
        matches!(
            self,
            Self::Rest { .. } | Self::Feed { .. } | Self::HtmlPage { .. }
        )
    }
}

/// What polling this mechanism involves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PollPlan {
    pub delivery: Delivery,
    /// How often a poll can usefully return new work.
    pub natural_interval: Duration,
    /// Whether a poll can ask only for what changed.
    pub incremental: bool,
    /// What production must supply before the first poll succeeds.
    pub credential_required: Option<String>,
}

/// A reachable address plus the mechanism that reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceEndpoint {
    scheme: Scheme,
    host: String,
    port: Option<u16>,
    path: String,
    mechanism: AccessMechanism,
}

impl SourceEndpoint {
    /// Parse `scheme://host[:port]/path`.
    ///
    /// Deliberately strict and small rather than a general URL parser: the
    /// host is what allowlists, denylists and robots.txt are keyed on, and a
    /// permissive parser that guesses a host is a permissive parser that
    /// guesses past a denylist.
    pub fn parse(url: &str, mechanism: AccessMechanism) -> Result<Self> {
        let url = url.trim();
        let Some((scheme_text, rest)) = url.split_once("://") else {
            return Err(Error::invalid(format!(
                "`{url}` has no scheme; an endpoint must say how it is reached"
            )));
        };
        let scheme = Scheme::parse(&scheme_text.to_ascii_lowercase())
            .ok_or_else(|| Error::invalid(format!("`{scheme_text}` is not a known scheme")))?;
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        if authority.is_empty() {
            return Err(Error::invalid(format!("`{url}` names no host")));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port_text)) => {
                let port: u16 = port_text.parse().map_err(|_| {
                    Error::invalid(format!("`{port_text}` in `{url}` is not a port"))
                })?;
                (host, Some(port))
            }
            None => (authority, None),
        };
        if host.is_empty() {
            return Err(Error::invalid(format!("`{url}` names no host")));
        }
        Ok(Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            path,
            mechanism,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The host, lowercased. What every legal check is keyed on.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The path, always starting with `/`. What robots.txt rules match on.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn mechanism(&self) -> &AccessMechanism {
        &self.mechanism
    }

    pub fn url(&self) -> String {
        match self.port {
            Some(port) => format!(
                "{}://{}:{port}{}",
                self.scheme.as_str(),
                self.host,
                self.path
            ),
            None => format!("{}://{}{}", self.scheme.as_str(), self.host, self.path),
        }
    }

    /// Where robots.txt would be for this endpoint's origin.
    pub fn robots_url(&self) -> String {
        match self.port {
            Some(port) => format!("{}://{}:{port}/robots.txt", self.scheme.as_str(), self.host),
            None => format!("{}://{}/robots.txt", self.scheme.as_str(), self.host),
        }
    }
}
