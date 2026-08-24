//! A chain adapter that opens a socket to a node.
//!
//! [`crate::adapter::SyntheticChain`] invents blocks and
//! [`crate::adapter::NodeChainAdapter`] declares what a full archive
//! integration would need without connecting to anything. This module is the
//! one that connects: it speaks JSON-RPC over
//! [`qip_transport::HttpClient`] to a node, decodes what comes back into
//! [`Block`]s and [`PendingTransaction`]s, and hands them to
//! [`crate::state::ChainState`], [`crate::gas`] and [`crate::mempool`] exactly
//! as the synthetic feed does.
//!
//! # Finality is the whole problem
//!
//! An exchange fill that has printed has happened. A block that is the head has
//! *probably* happened, and the difference is a reorg away. This adapter
//! therefore does not report the head. It reports blocks at or beyond
//! [`RpcChainConfig::confirmations`] below the head, and counts everything
//! shallower in [`RpcStats::withheld`] until the chain buries it. A deployment
//! that wants the head configures [`Confirmations::AT_RISK`], which is spelled
//! out rather than defaulted precisely so that accepting reorg risk is a line
//! in a diff.
//!
//! Two more rules follow from the same place, and both are enforced rather than
//! documented:
//!
//! * **A block with no hash is not a block.** `eth_getBlockByNumber` answers
//!   the `pending` tag with an object that has a null `hash` and a null
//!   `number`: a proposal, not a block. It is refused here, because a proposal
//!   admitted as a block would enter [`ChainState`](crate::state::ChainState)
//!   as settled history and every reserve derived from it would inherit that
//!   claim. Pending transactions reach the platform the only way they honestly
//!   can, as [`ChainUpdate::Pending`], which the mempool already models as a
//!   prediction nobody owes.
//! * **A log the node has retracted is not a trace.** A log carrying
//!   `removed: true` belongs to a block that has already been reorganised out.
//!   It is skipped, not decoded.
//!
//! # Why receipts and not `eth_getLogs`
//!
//! The obvious three-call shape is `eth_blockNumber`, `eth_getBlockByNumber`
//! and `eth_getLogs`, and it cannot be made honest. A log range tells you what
//! succeeded; it cannot distinguish a transaction that reverted from one that
//! emitted nothing, because the EVM discards the logs of a reverted call
//! either way. [`crate::block`] exists to make exactly that distinction — a
//! reverted transaction burned gas and moved nothing, and counting one as a
//! fill invents liquidity — and [`TxStatus`] has no variant for "the node did
//! not say". So the per-block call here is `eth_getBlockReceipts`, which
//! carries the status, the gas actually burned, the price actually paid and
//! the logs in one answer, and a receipt with no `status` field at all is
//! refused rather than assumed successful.
//!
//! # What this adapter does not promise
//!
//! It does not detect a reorganisation. It refuses to look shallower than the
//! configured depth, and a reorg deeper than that is
//! [`ChainState`](crate::state::ChainState)'s problem — every block carries its
//! parent hash so that layer can see the fork. An adapter that tried to
//! adjudicate branches would be a second, quieter copy of the state machine.
//!
//! It does not resolve tokens it was not told about. A `Transfer` log from a
//! contract outside [`TokenBinding`] produces no trace and is counted, because
//! an [`ObjectId`] invented from a contract address merges two assets.
//!
//! It cannot represent a gas price below one gwei. [`qip_core::Decimal`]
//! carries nine fractional digits of a whole native unit, which is exactly one
//! gwei for an eighteen-decimal native token, and wei below that do not fit.
//! Every wei figure is rounded **away from zero** on the way in, so the
//! conversion can overstate a cost by up to one gwei and can never understate
//! one. On a chain where sub-gwei fees are material this is the wrong
//! instrument, and saying so here is cheaper than discovering it in a
//! reconciliation.

use qip_contracts::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use crate::adapter::{ChainAdapter, ChainDescriptor, ChainUpdate};
use crate::block::{
    Address, Block, BlockHash, BlockNumber, ChainId, Hash32, Trace, TraceKind, Transaction, TxHash,
    TxStatus,
};
use crate::finality::Confirmations;
use crate::mempool::PendingTransaction;
use crate::units::TokenAmount;

/// Default header a provider credential travels in. Never a query parameter: a
/// URL is written to every access log on the path, and a node provider's key is
/// a bearer credential for an account that pays per request.
const DEFAULT_CREDENTIAL_HEADER: &str = "authorization";
/// Headers `qip_transport::HttpRequest` writes itself and drops a caller's copy
/// of. Naming them is what turns "the credential quietly vanished" into a
/// configuration error.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];
/// `keccak256("Transfer(address,address,uint256)")`, the ERC-20 transfer topic.
///
/// A literal rather than a computed digest: this crate has SHA-256 in tree and
/// no Keccak, and a constant that can be checked against any block explorer is
/// more auditable than an implementation of a hash used once.
const ERC20_TRANSFER_TOPIC: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// Nine fractional digits of a whole native unit is exactly one gwei, so a wei
/// figure divides by this to reach [`Decimal`]'s raw representation.
const WEI_PER_DECIMAL_UNIT: i128 = 1_000_000_000;

/// One token this adapter is allowed to decode transfers of.
///
/// The mapping is configuration, not inference. Two chains' USDC are two
/// contracts, and an [`ObjectId`] minted from a contract address would either
/// merge them or split one asset in two depending on which way the guess went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenBinding {
    /// Contract address, matched case-insensitively because a node may answer
    /// in either checksum or lower-case form.
    pub contract: Address,
    /// The platform instrument this token is.
    pub object_id: ObjectId,
    /// Fractional digits the contract uses. Read from configuration rather than
    /// from the chain: a wrong number here is a transfer wrong by a factor of a
    /// million, so it is stated where a human reviews it.
    pub decimals: u8,
}

impl TokenBinding {
    pub fn new(contract: impl Into<String>, object_id: ObjectId, decimals: u8) -> Self {
        Self {
            contract: Address::new(contract),
            object_id,
            decimals,
        }
    }
}

/// Everything a deployment has to decide before this adapter can fetch.
///
/// Not `Serialize`, and its [`std::fmt::Debug`] redacts the credential.
#[derive(Clone)]
pub struct RpcChainConfig {
    /// Stable adapter name, for the descriptor and for log lines.
    pub name: String,
    pub chain: ChainId,
    pub venue: VenueId,
    /// `http://host[:port]` of the node. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    pub endpoint: Option<String>,
    /// Path under the endpoint. Providers usually put the project key here; it
    /// may not carry a query, since this adapter builds none.
    pub path: String,
    /// The provider credential. `None` is unconfigured, not "this node is
    /// open": a public endpoint is one nobody can rate-limit or attribute, and
    /// this adapter will not treat one as a production feed.
    pub credential: Option<String>,
    /// Header the credential travels in.
    pub credential_header: String,
    /// How deep a block must be buried before this adapter will report it.
    ///
    /// The single most important field here. [`Confirmations::AT_RISK`] means
    /// the head, with every reorg it implies, and is a decision rather than a
    /// default.
    pub confirmations: Confirmations,
    /// Height to begin from on the first poll. `None` starts at the first block
    /// that is already settled, so a fresh deployment does not silently
    /// backfill a chain's history through a per-request billing meter.
    pub start_block: Option<u64>,
    /// Most blocks one poll will fetch. Bounds both the work per call and the
    /// catch-up burst after an outage; what is left waits for the next poll.
    pub max_blocks_per_poll: u32,
    /// Most transactions this decoder will expand one block into.
    pub max_transactions_per_block: usize,
    /// Whether to ask for the pending set as well.
    pub include_pending: bool,
    /// Nominal block time, reported on the descriptor. Not a promise: the
    /// interval between two blocks is a random variable and the tail is what
    /// costs money.
    pub block_time: Duration,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for RpcChainConfig {
    fn default() -> Self {
        Self {
            name: "chain-json-rpc".into(),
            chain: ChainId::new("unconfigured"),
            venue: VenueId::new("UNCONFIGURED"),
            endpoint: None,
            path: "/".into(),
            credential: None,
            credential_header: DEFAULT_CREDENTIAL_HEADER.into(),
            // Twelve blocks is the conventional depth at which an
            // eighteen-second-a-block layer one is treated as settled by
            // custodians. It is deliberately not zero: a deployment that has
            // thought about reorg risk lowers this and says why in the diff.
            confirmations: Confirmations::exactly(12),
            start_block: None,
            max_blocks_per_poll: 16,
            max_transactions_per_block: 2_000,
            include_pending: false,
            block_time: Duration::from_secs(12),
            http: ClientLimits {
                // A full block with receipts is megabytes on a busy chain.
                max_body: 8 * 1024 * 1024,
                max_headers: 32,
                connect_timeout: StdDuration::from_secs(2),
                read_timeout: StdDuration::from_secs(20),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

impl std::fmt::Debug for RpcChainConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcChainConfig")
            .field("name", &self.name)
            .field("chain", &self.chain)
            .field("venue", &self.venue)
            .field("endpoint", &self.endpoint)
            .field("path", &self.path)
            // Present or absent is worth knowing; the value never is.
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .field("credential_header", &self.credential_header)
            .field("confirmations", &self.confirmations)
            .field("start_block", &self.start_block)
            .field("max_blocks_per_poll", &self.max_blocks_per_poll)
            .field(
                "max_transactions_per_block",
                &self.max_transactions_per_block,
            )
            .field("include_pending", &self.include_pending)
            .field("block_time", &self.block_time)
            .field("http", &self.http)
            .finish()
    }
}

/// What this adapter has done, for metrics and for tests that assert a fetch
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RpcStats {
    /// JSON-RPC calls made.
    pub requests: u64,
    /// Blocks decoded and handed over.
    pub blocks: u64,
    /// Transactions decoded across those blocks.
    pub transactions: u64,
    /// Reverted transactions seen. Reported rather than dropped: a revert is a
    /// cost that happened, and a chain whose revert rate has risen is telling a
    /// strategy something.
    pub reverts: u64,
    /// Blocks the node had but this adapter would not report, because they were
    /// shallower than the required depth or stamped after the caller's clock.
    /// Not a loss: the next poll asks for them again.
    pub withheld: u64,
    /// Transfer logs from contracts this deployment has not mapped. Counted
    /// because the symptom of a missing binding is silence.
    pub unmapped_transfers: u64,
    /// Logs the node itself marked retracted. A non-zero count means blocks are
    /// being reorganised at the depth this adapter is reading.
    pub retracted_logs: u64,
    /// Pending transactions reported.
    pub pending: u64,
}

/// Polls a node's JSON-RPC endpoint and produces chain updates.
#[derive(Debug)]
pub struct JsonRpcChainAdapter {
    config: RpcChainConfig,
    /// Prebuilt endpoint, parsed once so a malformed address fails where it was
    /// configured rather than on the first poll.
    endpoint: Option<Url>,
    /// Keyed by lower-cased contract address, which is how a node may or may
    /// not write it.
    tokens: BTreeMap<String, TokenBinding>,
    client: HttpClient,
    /// Next height to ask for. `None` until the first poll learns the head.
    cursor: Option<u64>,
    /// Monotone JSON-RPC request id, so a response can be matched to its call.
    next_id: u64,
    stats: RpcStats,
}

impl JsonRpcChainAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why the descriptor's
    /// requirement is never `None`.
    pub const REQUIREMENTS: [&'static str; 5] = [
        "a TLS-terminating egress proxy in front of this adapter, or a node on the cluster \
         network: `qip_transport::http` has no TLS stack and refuses `https` by name rather than \
         downgrading it, so a provider credential sent straight to a public endpoint would cross \
         the internet in clear text",
        "a node that serves `eth_blockNumber`, `eth_getBlockByNumber` and `eth_getBlockReceipts`, \
         with receipts carrying a `status` field: a pre-Byzantium receipt has a state root \
         instead, and this decoder refuses one rather than assuming the transaction succeeded",
        "a confirmation depth in `confirmations` that matches the chain's observed reorg depth \
         and the deployment's tolerance. The default of twelve is a convention, not a \
         measurement, and `Confirmations::AT_RISK` means the head with every reorg it implies",
        "a token binding for every contract whose transfers matter, since a contract with no \
         binding produces no trace and an ObjectId invented from an address would merge two \
         assets",
        "an alert on the withheld, retracted-log and unmapped-transfer counts: a chain \
         reorganising at the depth this adapter reads, or a binding that was never added, looks \
         from the outside like a quiet chain rather than a broken feed",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot fetch still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong.
    pub fn new(config: RpcChainConfig, tokens: Vec<TokenBinding>) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::invalid(
                "a chain feed needs a name: it appears on every descriptor and every refusal it \
                 produces",
            ));
        }
        if config.max_blocks_per_poll == 0 {
            return Err(Error::invalid(
                "max_blocks_per_poll is zero, which would make every poll return nothing while \
                 looking like a chain that had stopped",
            ));
        }
        if config.max_transactions_per_block == 0 {
            return Err(Error::invalid(
                "max_transactions_per_block is zero, which would refuse every non-empty block",
            ));
        }
        let header = config.credential_header.trim().to_ascii_lowercase();
        if header.is_empty() {
            return Err(Error::invalid(
                "the credential needs a header to travel in; it is never put in the URL",
            ));
        }
        if CLIENT_OWNED_HEADERS.contains(&header.as_str()) {
            return Err(Error::invalid(format!(
                "the credential cannot travel in the `{header}` header: the transport writes that \
                 one itself and drops a caller's copy, so the request would leave without a \
                 credential at all"
            )));
        }
        if !header.chars().all(|c| c.is_ascii_graphic() && c != ':') {
            return Err(Error::invalid(format!(
                "{header:?} is not a usable header name: a space, a colon or a control character \
                 in one would end the header and let the rest be read as another"
            )));
        }
        if let Some(credential) = &config.credential {
            if credential.trim().is_empty() {
                return Err(Error::invalid(
                    "the provider credential is blank; an unconfigured credential is `None`, not \
                     an empty string, so that the adapter reports itself unavailable instead of \
                     sending an empty header",
                ));
            }
            if credential.chars().any(char::is_control) {
                return Err(Error::invalid(
                    "the provider credential contains a control character; sent as a header value \
                     it would end the header and let the rest be read as another one",
                ));
            }
        }
        if config.path.contains('?') || config.path.contains('#') {
            return Err(Error::invalid(format!(
                "the endpoint path {:?} carries a query or a fragment; a JSON-RPC call is a POST \
                 body and this adapter builds no query at all",
                config.path
            )));
        }

        let endpoint = match &config.endpoint {
            Some(base) => {
                let url = Url::parse(base).map_err(Error::from)?;
                Some(url.with_path(&config.path).map_err(Error::from)?)
            }
            None => None,
        };

        let mut mapped: BTreeMap<String, TokenBinding> = BTreeMap::new();
        for token in tokens {
            let key = token.contract.as_str().to_ascii_lowercase();
            if key.trim().is_empty() {
                return Err(Error::invalid(
                    "a token binding with an empty contract address",
                ));
            }
            if let Some(existing) = mapped.insert(key, token) {
                return Err(Error::invalid(format!(
                    "two token bindings claim the contract {}: a transfer from it could not be \
                     resolved to one of them",
                    existing.contract
                )));
            }
        }

        let client = HttpClient::new(config.http);
        Ok(Self {
            config,
            endpoint,
            tokens: mapped,
            client,
            // No cursor until the first poll learns the head: where to begin
            // depends on a height this process does not yet know.
            cursor: None,
            next_id: 1,
            stats: RpcStats::default(),
        })
    }

    pub fn stats(&self) -> RpcStats {
        self.stats
    }

    pub fn config(&self) -> &RpcChainConfig {
        &self.config
    }

    /// The next height this adapter will ask for, once it has decided one.
    pub fn cursor(&self) -> Option<BlockNumber> {
        self.cursor.map(BlockNumber::new)
    }

    /// Whether this adapter can fetch at all.
    pub fn is_available(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// Configuration a deployment has not supplied, each named on its own.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.endpoint.is_none() {
            missing.push(
                "no endpoint: set `endpoint` to the node's JSON-RPC address. This adapter has no \
                 default node and will not reach for a public one"
                    .into(),
            );
        }
        if self.config.credential.is_none() {
            missing.push(format!(
                "no credential: set `credential`, which is sent in the `{}` header. A node \
                 endpoint that needs no credential is one nobody can attribute or rate-limit",
                self.config.credential_header
            ));
        }
        missing
    }

    /// The full text of what production must supply: what is missing now,
    /// followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        if self.config.confirmations == Confirmations::AT_RISK {
            parts.push(
                "this feed is configured to report the chain head with no confirmations, so every \
                 block it produces can be reorganised away and everything derived from one is \
                 void rather than stale. That is a legitimate choice for a strategy that hedges \
                 the risk and never for anything that moves custody"
                    .into(),
            );
        }
        if self.tokens.is_empty() {
            parts.push(
                "no token bindings, so no transfer in any block will decode into a trace: the \
                 blocks are reported for their headers, their gas and their finality only"
                    .into(),
            );
        }
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the adapter cannot fetch.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "{} cannot reach a node and will not substitute generated blocks: {}",
            self.config.name,
            self.requirement()
        ))
    }

    /// One JSON-RPC call, returning the `result` member.
    fn call(&mut self, method: &str, params: &str) -> Result<serde_json::Value> {
        let Some(endpoint) = &self.endpoint else {
            return Err(self.unavailable());
        };
        let Some(credential) = &self.config.credential else {
            return Err(self.unavailable());
        };
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let body =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#);
        let request = HttpRequest::json(Method::Post, &endpoint.to_string(), body.into_bytes())
            .map_err(Error::from)?
            .with_header(&self.config.credential_header, credential);

        let response = self.client.send(&request).map_err(Error::from)?;
        self.stats.requests += 1;
        if !response.is_success() {
            return Err(self.status_refusal(method, response.status, &response.body_excerpt()));
        }
        let text = response.body_as_str().map_err(Error::from)?;
        let envelope: RpcEnvelope = serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{}: the answer to {method} is not a JSON-RPC envelope: {error}. The first bytes \
                 of it were: {}",
                self.config.name,
                response.body_excerpt()
            ))
        })?;
        if let Some(failure) = envelope.error {
            // A JSON-RPC error is HTTP 200 with a failure inside, which is
            // exactly the shape that gets read as success by a client that only
            // checks the status line.
            return Err(Error::invalid(format!(
                "{}: the node refused {method} with JSON-RPC error {} ({})",
                self.config.name, failure.code, failure.message
            )));
        }
        match envelope.result {
            Some(serde_json::Value::Null) | None => Err(Error::not_found(format!(
                "{}: the node answered {method} with no result. For a block request that means \
                 the height does not exist on this node, which is what a pruned or lagging node \
                 looks like",
                self.config.name
            ))),
            Some(value) => Ok(value),
        }
    }

    /// What a non-2xx status means here.
    fn status_refusal(&self, method: &str, status: u16, excerpt: &str) -> Error {
        let name = &self.config.name;
        match status {
            401 | 403 => Error::denied(format!(
                "{name} rejected this deployment's credential with HTTP {status} on {method}. The \
                 credential itself is not quoted here, and is not written to any log by this \
                 adapter"
            )),
            404 => Error::not_found(format!(
                "{name} has no JSON-RPC endpoint at the configured path (HTTP 404): {excerpt}"
            )),
            402 | 429 => Error::unavailable(format!(
                "{name} is rate-limiting or refusing to bill this deployment (HTTP {status}) on \
                 {method}: {excerpt}"
            )),
            500..=599 => Error::unavailable(format!(
                "{name} failed to serve {method} (HTTP {status}): {excerpt}"
            )),
            other => Error::invalid(format!(
                "{name} answered {method} with HTTP {other}, which this adapter does not know how \
                 to read: {excerpt}"
            )),
        }
    }

    /// The chain head, as the node reports it.
    pub fn head(&mut self) -> Result<BlockNumber> {
        let value = self.call("eth_blockNumber", "[]")?;
        let text = value.as_str().ok_or_else(|| {
            Error::schema(format!(
                "{}: eth_blockNumber answered with something that is not a hex quantity",
                self.config.name
            ))
        })?;
        Ok(BlockNumber::new(hex_u64("the chain head", text)?))
    }

    /// The deepest height this adapter is willing to report, given the head.
    ///
    /// `None` when the chain is younger than the required depth, which is a
    /// real state on a fresh devnet and must not underflow into a height near
    /// `u64::MAX`.
    fn settled_height(&self, head: BlockNumber) -> Option<u64> {
        head.get()
            .checked_sub(u64::from(self.config.confirmations.depth()))
    }

    /// Fetch and decode one block by height, without the finality gate.
    ///
    /// Public so an operator can exercise the endpoint and the credential —
    /// the two things a deployment gets wrong — without the result depending on
    /// where the head is.
    pub fn block_at(&mut self, number: BlockNumber) -> Result<Block> {
        let params = format!(r#"["0x{:x}",true]"#, number.get());
        let block_value = self.call("eth_getBlockByNumber", &params)?;
        let wire: WireBlock = serde_json::from_value(block_value).map_err(|error| {
            Error::schema(format!(
                "{}: block {number} is not a shape this decoder reads: {error}",
                self.config.name
            ))
        })?;
        let receipts_value = self.call(
            "eth_getBlockReceipts",
            &format!(r#"["0x{:x}"]"#, number.get()),
        )?;
        let receipts: Vec<WireReceipt> =
            serde_json::from_value(receipts_value).map_err(|error| {
                Error::schema(format!(
                    "{}: the receipts for block {number} are not a shape this decoder reads: \
                     {error}",
                    self.config.name
                ))
            })?;
        let block = self.decode_block(wire, receipts)?;
        if block.number != number {
            // A node that answers one height with another is either behind a
            // load balancer serving two chains or reading a different tag than
            // the one asked for. Either way the height this adapter recorded
            // would not be the height it fetched.
            return Err(Error::schema(format!(
                "{}: asked for block {number} and was answered with block {}",
                self.config.name, block.number
            )));
        }
        Ok(block)
    }

    /// Turn one block and its receipts into a [`Block`].
    fn decode_block(&mut self, wire: WireBlock, receipts: Vec<WireReceipt>) -> Result<Block> {
        // A pending proposal has a null hash and a null number. Refused before
        // anything else, because everything below would happily decode it.
        let (Some(hash), Some(number)) = (wire.hash.as_deref(), wire.number.as_deref()) else {
            return Err(Error::invalid(format!(
                "{}: the node answered with a block that has no hash or no number. That is a \
                 pending proposal, not a block, and admitting one would put a height into the \
                 chain state that nothing ever built on",
                self.config.name
            )));
        };
        let number = BlockNumber::new(hex_u64("a block number", number)?);
        let hash = BlockHash::new(hash32("a block hash", hash)?);
        let parent_hash = BlockHash::new(hash32("a parent hash", &wire.parent_hash)?);

        if wire.transactions.len() > self.config.max_transactions_per_block {
            return Err(Error::guard(format!(
                "{}: block {number} carries {} transactions and the cap is {}: a block small \
                 enough to read is not automatically one worth expanding",
                self.config.name,
                wire.transactions.len(),
                self.config.max_transactions_per_block
            )));
        }
        if receipts.len() != wire.transactions.len() {
            return Err(Error::schema(format!(
                "{}: block {number} carries {} transactions and {} receipts. Two answers that do \
                 not line up are two different blocks stitched together, and a status taken from \
                 the wrong one is a revert counted as a fill",
                self.config.name,
                wire.transactions.len(),
                receipts.len()
            )));
        }

        let mut transactions = Vec::with_capacity(wire.transactions.len());
        for (position, (tx, receipt)) in wire.transactions.into_iter().zip(receipts).enumerate() {
            transactions.push(self.decode_transaction(number, position, tx, receipt)?);
        }

        let block = Block {
            chain: self.config.chain.clone(),
            number,
            hash,
            parent_hash,
            // The proposer's stamp. Not this process's clock, and not
            // necessarily monotone across a reorg — which is why the caller's
            // `until` gates on it rather than trusting it to be recent.
            timestamp: Timestamp::from_secs(
                i64::try_from(hex_u64("a block timestamp", &wire.timestamp)?).map_err(|_| {
                    Error::schema(format!(
                        "{}: block {number} is stamped beyond the representable range",
                        self.config.name
                    ))
                })?,
            ),
            base_fee: match &wire.base_fee_per_gas {
                Some(text) => wei_to_native("a base fee", text)?,
                // A chain with no base fee is a pre-1559 chain, where zero is
                // the true answer rather than a stand-in for one.
                None => Decimal::ZERO,
            },
            gas_used: hex_u64("a block's gas used", &wire.gas_used)?,
            gas_limit: hex_u64("a block's gas limit", &wire.gas_limit)?,
            transactions,
        };
        // The one structural check made here rather than left to a consumer:
        // `Block::validate` requires transaction indices to equal their
        // positions, and a node that serves them out of order produces a block
        // no downstream ordering can be trusted on.
        block.validate()?;
        Ok(block)
    }

    fn decode_transaction(
        &mut self,
        number: BlockNumber,
        position: usize,
        tx: WireTransaction,
        receipt: WireReceipt,
    ) -> Result<Transaction> {
        let hash = TxHash::new(hash32("a transaction hash", &tx.hash)?);
        let receipt_hash = TxHash::new(hash32(
            "a receipt's transaction hash",
            &receipt.transaction_hash,
        )?);
        if hash != receipt_hash {
            return Err(Error::schema(format!(
                "{}: in block {number}, transaction {position} is {hash} and its receipt is for \
                 {receipt_hash}. A status read off the wrong receipt is a revert counted as a fill",
                self.config.name
            )));
        }
        let index = u32::try_from(position).map_err(|_| {
            Error::guard(format!(
                "{}: block {number} has more transactions than a u32 index can hold",
                self.config.name
            ))
        })?;

        // No `status` at all is a pre-Byzantium receipt, which carries a state
        // root instead. Refused rather than assumed successful: the whole point
        // of this crate's transaction model is that a revert is not a fill.
        let status_text = receipt.status.as_deref().ok_or_else(|| {
            Error::schema(format!(
                "{}: the receipt for {hash} in block {number} carries no status field, so whether \
                 it succeeded is unknown. It is refused rather than assumed successful",
                self.config.name
            ))
        })?;
        let status = match hex_u64("a receipt status", status_text)? {
            1 => TxStatus::Succeeded,
            0 => {
                self.stats.reverts += 1;
                TxStatus::Reverted {
                    // The node reports a revert without decoding its reason
                    // unless asked to trace, and inventing one would put words
                    // in a contract's mouth.
                    reason: "the node reported a failed status and no reason".into(),
                }
            }
            other => {
                return Err(Error::schema(format!(
                    "{}: the receipt for {hash} reports status {other}, which is neither success \
                     nor failure",
                    self.config.name
                )));
            }
        };

        let effective_gas_price = match (&receipt.effective_gas_price, &tx.gas_price) {
            (Some(text), _) | (None, Some(text)) => wei_to_native("an effective gas price", text)?,
            (None, None) => {
                return Err(Error::schema(format!(
                    "{}: neither the receipt nor the transaction {hash} states a gas price, so \
                     what it cost cannot be computed. Gas is charged on a revert exactly as on a \
                     success, so a missing price is a missing cost rather than a free transaction",
                    self.config.name
                )));
            }
        };

        let succeeded = status.succeeded();
        let traces = self.decode_traces(&receipt, succeeded)?;
        self.stats.transactions += 1;
        Ok(Transaction {
            hash,
            index,
            from: Address::new(tx.from),
            to: tx.to.map(Address::new),
            status,
            gas_used: hex_u64("a receipt's gas used", &receipt.gas_used)?,
            effective_gas_price,
            traces,
        })
    }

    /// Traces for one transaction, decoded from its logs.
    ///
    /// Only ERC-20 transfers, and only for bound contracts. The trace index is
    /// the log's position within the receipt, so skipping a log this deployment
    /// cannot resolve leaves a gap rather than renumbering the ones around it:
    /// the index is what orders traces, and closing the gap would claim an
    /// order the chain did not have.
    fn decode_traces(&mut self, receipt: &WireReceipt, succeeded: bool) -> Result<Vec<Trace>> {
        let mut traces = Vec::new();
        for (position, log) in receipt.logs.iter().enumerate() {
            if log.removed {
                // The node has already retracted this log; the block it belongs
                // to is being reorganised out from under this read.
                self.stats.retracted_logs += 1;
                continue;
            }
            let Some(topic) = log.topics.first() else {
                continue;
            };
            if !topic.eq_ignore_ascii_case(ERC20_TRANSFER_TOPIC) {
                continue;
            }
            let Some(binding) = self.tokens.get(&log.address.to_ascii_lowercase()) else {
                self.stats.unmapped_transfers += 1;
                continue;
            };
            let (Some(from), Some(to)) = (log.topics.get(1), log.topics.get(2)) else {
                return Err(Error::schema(format!(
                    "{}: a transfer log from {} carries {} topics; an ERC-20 transfer has three",
                    self.config.name,
                    log.address,
                    log.topics.len()
                )));
            };
            let amount =
                TokenAmount::new(hex_i128("a transfer amount", &log.data)?, binding.decimals)?;
            let index = u32::try_from(position).unwrap_or(u32::MAX);
            traces.push(Trace::new(
                index,
                TraceKind::Transfer {
                    object_id: binding.object_id.clone(),
                    from: Address::new(address_from_topic(from)?),
                    to: Address::new(address_from_topic(to)?),
                    amount,
                },
            ));
        }
        if !succeeded && !traces.is_empty() {
            // A reverted transaction has no logs on any node that follows the
            // specification, so this means the two halves of the answer
            // disagree about what happened.
            return Err(Error::schema(format!(
                "{}: a receipt reports a failed status and {} logs. The EVM discards the logs of \
                 a reverted transaction, so one answer here is wrong and this decoder will not \
                 pick which",
                self.config.name,
                traces.len()
            )));
        }
        Ok(traces)
    }

    /// The pending set, as one call to `eth_getBlockByNumber` with the
    /// `pending` tag.
    ///
    /// Deliberately not decoded into a [`Block`]: see the module docs. What
    /// comes back is a proposal, and it reaches the platform as pending
    /// transactions or not at all.
    fn poll_pending(&mut self, until: Timestamp) -> Result<Vec<ChainUpdate>> {
        let value = self.call("eth_getBlockByNumber", r#"["pending",true]"#)?;
        let wire: WireBlock = serde_json::from_value(value).map_err(|error| {
            Error::schema(format!(
                "{}: the pending set is not a shape this decoder reads: {error}",
                self.config.name
            ))
        })?;
        let mut updates = Vec::new();
        for tx in wire
            .transactions
            .into_iter()
            .take(self.config.max_transactions_per_block)
        {
            let (Some(max_fee), Some(max_priority)) =
                (&tx.max_fee_per_gas, &tx.max_priority_fee_per_gas)
            else {
                // A legacy transaction prices itself with one number and the
                // mempool model wants two. Skipped rather than guessed: a
                // priority fee invented from a gas price would drive an
                // ordering prediction.
                continue;
            };
            updates.push(ChainUpdate::Pending(Box::new(PendingTransaction {
                hash: TxHash::new(hash32("a pending transaction hash", &tx.hash)?),
                from: Address::new(tx.from),
                nonce: hex_u64("a pending nonce", &tx.nonce)?,
                gas_limit: hex_u64("a pending gas limit", &tx.gas)?,
                max_fee_per_gas: wei_to_native("a pending max fee", max_fee)?,
                max_priority_fee_per_gas: wei_to_native("a pending priority fee", max_priority)?,
                // The caller's clock, not the wall clock: this is the earliest
                // instant this deployment could have known about the
                // transaction, which is all "first seen" can honestly mean when
                // the pool was polled rather than subscribed to.
                first_seen: until,
                // Calldata is not decoded here. An intent guessed from an
                // opaque blob would drive an ordering prediction, and `None` is
                // the field's documented answer for exactly that.
                intent: None,
            })));
        }
        self.stats.pending += updates.len() as u64;
        Ok(updates)
    }
}

impl ChainAdapter for JsonRpcChainAdapter {
    fn descriptor(&self) -> ChainDescriptor {
        ChainDescriptor {
            name: self.config.name.clone(),
            chain: self.config.chain.clone(),
            venue: self.config.venue.clone(),
            class: VenueClass::DecentralisedExchange,
            block_time: self.config.block_time,
            is_synthetic: false,
            // Never `None`, even fully configured: see [`Self::REQUIREMENTS`].
            production_requirement: Some(self.requirement()),
        }
    }

    /// Refuse at startup rather than at the first tick, so a deployment missing
    /// a credential fails while somebody is watching the rollout.
    fn start(&mut self, _at: Timestamp) -> Result<()> {
        if self.is_available() {
            Ok(())
        } else {
            Err(self.unavailable())
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<ChainUpdate>> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        let head = self.head()?;
        let mut updates = Vec::new();

        let Some(settled) = self.settled_height(head) else {
            // A chain shorter than the required depth has nothing settled in
            // it. Reported as withheld rather than as an error: it is a real
            // state, and it resolves itself as the chain grows.
            self.stats.withheld += 1;
            if self.config.include_pending {
                updates.extend(self.poll_pending(until)?);
            }
            return Ok(updates);
        };

        let start = match self.cursor {
            Some(cursor) => cursor,
            // A fresh adapter begins at the deepest block it is allowed to
            // report and does not walk backwards through the chain's history:
            // a backfill is a decision with a bill attached, and `start_block`
            // is where it is made.
            None => self.config.start_block.unwrap_or(settled),
        };
        if start > settled {
            self.stats.withheld += 1;
            self.cursor = Some(start);
            if self.config.include_pending {
                updates.extend(self.poll_pending(until)?);
            }
            return Ok(updates);
        }

        let mut height = start;
        let mut fetched = 0u32;
        while height <= settled && fetched < self.config.max_blocks_per_poll {
            let block = self.block_at(BlockNumber::new(height))?;
            if block.timestamp > until {
                // The proposer stamped it after the instant the caller is
                // allowed to know about. Withheld and the cursor left where it
                // is, so the next poll asks again rather than skipping it.
                self.stats.withheld += 1;
                break;
            }
            self.stats.blocks += 1;
            updates.push(ChainUpdate::Block(Box::new(block)));
            height += 1;
            fetched += 1;
        }
        self.cursor = Some(height);

        if self.config.include_pending {
            updates.extend(self.poll_pending(until)?);
        }
        Ok(updates)
    }
}

// --- hex, which is the only encoding a node speaks ---------------------------

/// A `0x`-prefixed quantity as a `u64`.
///
/// Every one of these refusals names what was being read, because "invalid
/// digit found in string" with no subject is the least useful error a decoder
/// can produce.
fn hex_u64(subject: &str, text: &str) -> Result<u64> {
    let digits = strip_hex(subject, text)?;
    u64::from_str_radix(digits, 16).map_err(|error| {
        Error::schema(format!(
            "{subject} is {text:?}, which is not a 64-bit hex quantity: {error}"
        ))
    })
}

/// A `0x`-prefixed quantity as an `i128`, for a token amount.
///
/// A `uint256` beyond `i128` is refused rather than truncated: a truncated
/// balance is not an approximate balance, it is a different number.
fn hex_i128(subject: &str, text: &str) -> Result<i128> {
    let digits = strip_hex(subject, text)?;
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return Ok(0);
    }
    i128::from_str_radix(trimmed, 16).map_err(|error| {
        Error::schema(format!(
            "{subject} is {text:?}, which does not fit a 128-bit integer: {error}. It is refused \
             rather than truncated, because a truncated amount is a different amount"
        ))
    })
}

fn strip_hex<'a>(subject: &str, text: &'a str) -> Result<&'a str> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .ok_or_else(|| {
            Error::schema(format!(
                "{subject} is {text:?}, which is not a `0x`-prefixed hex quantity. A node that has \
                 started answering in decimal is answering a different protocol"
            ))
        })?;
    if digits.is_empty() {
        return Err(Error::schema(format!("{subject} is `0x` with no digits")));
    }
    Ok(digits)
}

/// A `0x`-prefixed 32-byte digest.
fn hash32(subject: &str, text: &str) -> Result<Hash32> {
    let digits = strip_hex(subject, text)?;
    if digits.len() != 64 {
        return Err(Error::schema(format!(
            "{subject} is {} hex digits and a 32-byte digest is 64. A short hash is a truncated \
             identity, and two blocks that share one are one block to everything downstream",
            digits.len()
        )));
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        let pair = digits
            .get(start..start + 2)
            .ok_or_else(|| Error::schema(format!("{subject} is not whole bytes of hex")))?;
        *byte = u8::from_str_radix(pair, 16).map_err(|error| {
            Error::schema(format!(
                "{subject} contains {pair:?}, which is not hex: {error}"
            ))
        })?;
    }
    Ok(Hash32::from_bytes(bytes))
}

/// The 20-byte address held in the low bytes of a 32-byte log topic.
fn address_from_topic(topic: &str) -> Result<String> {
    let digits = strip_hex("a transfer participant", topic)?;
    if digits.len() != 64 {
        return Err(Error::schema(format!(
            "a transfer participant is {} hex digits and a topic is 64",
            digits.len()
        )));
    }
    let tail = digits.get(24..).ok_or_else(|| {
        Error::schema("a transfer participant has no address in its low 20 bytes".to_string())
    })?;
    Ok(format!("0x{}", tail.to_ascii_lowercase()))
}

/// A wei quantity as whole native units.
///
/// Rounds **away from zero**, so the conversion can overstate a cost by up to
/// one gwei and can never understate one. See the module docs: nine fractional
/// digits of a native unit is exactly one gwei, and wei below that do not fit
/// at all.
fn wei_to_native(subject: &str, text: &str) -> Result<Decimal> {
    let wei = hex_i128(subject, text)?;
    let units = wei / WEI_PER_DECIMAL_UNIT;
    let remainder = wei % WEI_PER_DECIMAL_UNIT;
    let rounded = if remainder > 0 {
        units.checked_add(1).ok_or_else(|| {
            Error::schema(format!(
                "{subject} is too large to represent in native units"
            ))
        })?
    } else {
        units
    };
    // `Decimal` is a raw i128 of 10^-9 units, so `rounded` is already in its
    // representation; `hex_i128` above is what refused anything that did not
    // fit an i128 in the first place.
    Ok(Decimal::from_raw(rounded))
}

// --- the wire schema --------------------------------------------------------
//
// The subset of the Ethereum JSON-RPC shape this decoder reads, and the whole
// of what it promises. Unknown fields are ignored, because a node adding one is
// not a fault. Unknown or absent *values* in a field this decoder reads are
// refused, because those change what the block means.

#[derive(Debug, Deserialize)]
struct RpcEnvelope {
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct WireBlock {
    /// Null for the `pending` tag, which is what makes a proposal detectable.
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(rename = "parentHash")]
    parent_hash: String,
    /// Seconds since the epoch, as the proposer stamped them.
    timestamp: String,
    #[serde(rename = "baseFeePerGas", default)]
    base_fee_per_gas: Option<String>,
    #[serde(rename = "gasUsed")]
    gas_used: String,
    #[serde(rename = "gasLimit")]
    gas_limit: String,
    /// Full transaction objects, which is what the `true` second parameter
    /// asks for. A list of hashes would deserialise as nothing here and be
    /// refused, which is the right answer for a call made with the wrong
    /// parameter.
    #[serde(default)]
    transactions: Vec<WireTransaction>,
}

#[derive(Debug, Deserialize)]
struct WireTransaction {
    hash: String,
    from: String,
    #[serde(default)]
    to: Option<String>,
    nonce: String,
    /// The gas limit the sender set, not the gas burned.
    gas: String,
    #[serde(rename = "gasPrice", default)]
    gas_price: Option<String>,
    #[serde(rename = "maxFeePerGas", default)]
    max_fee_per_gas: Option<String>,
    #[serde(rename = "maxPriorityFeePerGas", default)]
    max_priority_fee_per_gas: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireReceipt {
    #[serde(rename = "transactionHash")]
    transaction_hash: String,
    /// `0x1` or `0x0`. Absent on a pre-Byzantium receipt, which this decoder
    /// refuses rather than reading as a success.
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "gasUsed")]
    gas_used: String,
    #[serde(rename = "effectiveGasPrice", default)]
    effective_gas_price: Option<String>,
    #[serde(default)]
    logs: Vec<WireLog>,
}

#[derive(Debug, Deserialize)]
struct WireLog {
    address: String,
    #[serde(default)]
    topics: Vec<String>,
    data: String,
    /// True when the node has already retracted the log because its block was
    /// reorganised away.
    #[serde(default)]
    removed: bool,
}
