//! Memorystore: a RESP client, and the cache adapter built on it.
//!
//! Memorystore speaks Redis. The Redis wire protocol (RESP) is a
//! length-prefixed text protocol over a plain TCP socket — a type byte, a
//! line, and for bulk payloads a byte count followed by exactly that many
//! bytes. Speaking it needs a socket and a parser, both of which this
//! workspace already has, so this adapter is written here rather than pulled
//! in. That is the same trade the HTTP client in `qip-transport` makes, and it
//! is only defensible because the protocol really is this small: the whole
//! encoder is [`encode_command`] and the whole decoder is [`read_reply`].
//!
//! # What this module does not promise
//!
//! **Nothing stored here survives anything.** This is the point of the target,
//! not a limitation of the adapter. [`crate::StorageTarget::Memorystore`]'s
//! rationale is "sub-millisecond reads of values that can be recomputed if
//! lost", and the Terraform that provisions the instance sets
//! `persistence_mode = DISABLED` on purpose, so that a cache which survived a
//! restart could never be mistaken for a source of record. Concretely, this
//! adapter does not promise:
//!
//! * **Durability.** A `put` that returns `Ok` means the server acknowledged
//!   it in memory. A failover, a restart, an eviction under `maxmemory`, or an
//!   expiry can each take it back, and none of them is an error.
//!   [`RedisKeyValueStore::is_crash_safe`] returns `false` and
//!   [`crate::StorageTarget::is_crash_safe`] excludes `Memorystore`, so the
//!   start-up banner assembled by [`crate::StorageSettings::banner_lines`]
//!   tells an operator that nothing here survives a restart.
//! * **Atomicity across keys.** Each call is one command. There are no
//!   transactions, no `MULTI`, no compare-and-set. A caller needing several
//!   keys to move together wants [`crate::DurableStore`].
//! * **Consistency after a failover.** Memorystore's `STANDARD_HA` tier
//!   promotes a replica that may be behind the primary. Reads after a failover
//!   can be stale or missing.
//! * **A complete Redis client.** Six commands are implemented — `AUTH`,
//!   `PING`, `GET`, `SET`, `DEL` and `SCAN` — which is what
//!   [`crate::KeyValueStore`] needs and no more. There is no pipelining, no
//!   pub/sub, no cluster redirection, no `MULTI`, and no RESP3.
//!
//! Anything the platform cannot afford to lose belongs in
//! [`crate::DurableStore`]; anything large belongs in a [`crate::BlobStore`].
//! [`StorageProvider::blobs`](crate::StorageProvider::blobs) refuses
//! `Memorystore` outright for that reason.
//!
//! # There is no TLS here
//!
//! This client speaks RESP over a plaintext TCP socket. The Terraform
//! provisions the instance with `transit_encryption_mode =
//! "SERVER_AUTHENTICATION"`, which means that instance expects TLS and this
//! client cannot reach it directly. That is stated in
//! [`crate::StorageTarget::required_configuration`] rather than left for a
//! deployment to discover as a connection reset: reaching such an instance
//! needs a TLS-terminating proxy inside the VPC, or an instance provisioned
//! without transit encryption. The AUTH string is sent in the clear over
//! whatever hop it is given, which is why it must be a private one.
//!
//! # One connection, and one retry
//!
//! The store holds a single connection behind a mutex and reuses it, because a
//! cache whose selling point is a sub-millisecond read cannot afford a TCP
//! handshake and an `AUTH` round trip per operation. Concurrency therefore
//! serialises: a caller that needs parallel access constructs more stores.
//!
//! A pooled connection can go stale — Redis closes idle clients, and a
//! failover closes everything. A socket-level failure on a *reused* connection
//! is therefore retried exactly once on a fresh connection. That is only sound
//! because every command this adapter sends is idempotent: `GET`, `DEL` and
//! `SCAN` inherently, and `SET` because it rewrites the same key with the same
//! bytes. A retry is deliberately *not* attempted after a timeout (the server
//! may simply be slow, and retrying doubles the wait a caller already declined
//! to make) nor after a protocol error (the peer will produce the same bytes
//! again), nor on a connection that was opened fresh for this very command.

use crate::kv::KeyValueStore;
use qip_core::error::{Error, Result};
use std::collections::BTreeSet;
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

/// The environment variable naming the Memorystore instance address.
///
/// `host` or `host:port`; the port defaults to [`DEFAULT_PORT`]. Memorystore
/// publishes this as the `redis_host` Terraform output.
pub const ADDRESS_VARIABLE: &str = "QIP_MEMORYSTORE_ADDRESS";

/// The environment variable carrying the instance's AUTH string.
///
/// The instance is provisioned with `auth_enabled = true`, so this is not
/// optional in a real deployment. It is resolved by the composition root
/// through [`crate::managed::ManagedSettings::from_env`] — never read here,
/// and never from a file in the repository — so `QIP_MEMORYSTORE_AUTH_FILE`
/// is honoured the way every other secret's `_FILE` variant is, and it is
/// redacted from every `Debug` output and every error message this module
/// produces.
pub const AUTH_VARIABLE: &str = "QIP_MEMORYSTORE_AUTH";

/// The port Redis listens on when an address does not name one.
pub const DEFAULT_PORT: u16 = 6379;

/// The first segment of every key this adapter writes.
///
/// Every key is `{prefix}:{namespace}:{key}`. The fixed first segment means a
/// prefix scan can never walk keys some other tenant of the same instance
/// wrote, and that [`RedisKeyValueStore::len`] counts this platform's keys
/// rather than the instance's.
pub const DEFAULT_KEY_PREFIX: &str = "qip";

/// Limits applied to every command and every reply.
///
/// The peer is the untrusted party: a server — or anything that has taken over
/// its address — can answer with a bulk string declaring four gigabytes, an
/// array of a billion elements, or a reply that never ends. Each limit below
/// is one of those, and each is checked *before* the allocation it bounds
/// rather than after.
///
/// The two size limits that face the *caller* rather than the peer
/// ([`Self::max_key_bytes`] and [`Self::max_value_bytes`]) are here for a
/// different reason: a cache entry large enough to matter belongs in a blob
/// store, and a store that accepted it would make the instance's `maxmemory`
/// eviction policy the thing deciding what the platform remembers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedisLimits {
    /// How long the TCP handshake may take.
    pub connect_timeout: Duration,
    /// How long the server may go silent mid-reply before the read fails.
    ///
    /// Short by the standards of an HTTP client, because the entire claim this
    /// target makes is a sub-millisecond read. A cache that has stopped
    /// answering should fail over to recomputation in milliseconds, not block
    /// a trading cycle for fifteen seconds.
    pub read_timeout: Duration,
    /// How long writing a command may take.
    pub write_timeout: Duration,
    /// Largest single reply accepted, counting every byte of framing.
    pub max_reply_bytes: usize,
    /// Largest single CRLF-terminated line. Bulk payloads are not lines and
    /// are bounded by [`Self::max_reply_bytes`] instead.
    pub max_line_bytes: usize,
    /// Most elements one array reply may declare.
    pub max_array_elements: usize,
    /// How deeply arrays may nest. `SCAN` answers two deep; anything deeper
    /// than this is a peer trying to exhaust the stack.
    pub max_nesting_depth: usize,
    /// Largest key, measured on the wire *after* the prefix and namespace are
    /// applied — because that is the string the server stores.
    pub max_key_bytes: usize,
    /// Largest serialized value a caller may store.
    pub max_value_bytes: usize,
    /// The `COUNT` hint given to `SCAN`. A hint, not a limit: Redis may return
    /// more or fewer.
    pub scan_batch: usize,
    /// Most `SCAN` round trips one prefix scan may make before giving up. A
    /// cursor that never returns to zero is otherwise an infinite loop driven
    /// by the peer.
    pub max_scan_rounds: usize,
    /// Most distinct keys one prefix scan may accumulate.
    pub max_scan_keys: usize,
}

impl Default for RedisLimits {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(2),
            write_timeout: Duration::from_secs(2),
            max_reply_bytes: 8 * 1024 * 1024,
            max_line_bytes: 64 * 1024,
            max_array_elements: 100_000,
            max_nesting_depth: 4,
            max_key_bytes: 1024,
            max_value_bytes: 1024 * 1024,
            scan_batch: 256,
            max_scan_rounds: 4096,
            max_scan_keys: 100_000,
        }
    }
}

/// Where the instance is, how to authenticate to it, and the limits to hold it
/// to.
///
/// The AUTH string is held here and deliberately never rendered: [`Debug`] is
/// implemented by hand so that a configuration dumped into a log cannot leak
/// it, and no error message in this module interpolates it.
#[derive(Clone)]
pub struct RedisConfig {
    address: String,
    username: Option<String>,
    secret: Option<String>,
    key_prefix: String,
    entry_lifetime: Option<Duration>,
    limits: RedisLimits,
}

impl std::fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConfig")
            .field("address", &self.address)
            .field("username", &self.username)
            // Not `self.secret`: a configuration is exactly the kind of value
            // that ends up in a start-up log.
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("key_prefix", &self.key_prefix)
            .field("entry_lifetime", &self.entry_lifetime)
            .field("limits", &self.limits)
            .finish()
    }
}

impl RedisConfig {
    /// A configuration for an instance at `address`, with no authentication.
    ///
    /// Useful for a local Redis and for the tests in this module. A real
    /// Memorystore instance has `auth_enabled = true` and will refuse every
    /// command from a connection built this way.
    pub fn at(address: impl AsRef<str>) -> Result<Self> {
        let address = normalise_address(address.as_ref())?;
        Ok(Self {
            address,
            username: None,
            secret: None,
            key_prefix: DEFAULT_KEY_PREFIX.to_string(),
            entry_lifetime: None,
            limits: RedisLimits::default(),
        })
    }

    /// Resolve from explicit values.
    ///
    /// Values, not variables: this crate does not read the process
    /// environment. The composition root resolves [`ADDRESS_VARIABLE`] and
    /// [`AUTH_VARIABLE`] — the latter through `qip_core::secret`, so it may be
    /// a mounted file — and passes them in, which is also what lets a test of
    /// a refusal run without mutating a process-global that every other test
    /// in the binary shares.
    ///
    /// An empty or whitespace-only address is treated as unset. A deployment
    /// template that expands a missing value to `""` is common enough that
    /// reading it as "the operator asked for the empty address" would turn a
    /// templating mistake into a connection to nowhere. An empty AUTH string
    /// is refused before it reaches here, by the caller that resolved it.
    pub fn from_values(address: Option<&str>, auth: Option<&str>) -> Result<Self> {
        let address = address
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::unavailable(format!(
                    "{ADDRESS_VARIABLE} is unset, so this build has no Memorystore instance to \
                     talk to. The RESP adapter is compiled in; what is missing is the deployment: \
                     {}. See docs/operations/external-dependencies.md",
                    crate::StorageTarget::Memorystore
                        .required_configuration()
                        .unwrap_or("an instance address")
                ))
            })?;
        let mut config = Self::at(address)?;
        if let Some(secret) = auth.map(str::trim).filter(|value| !value.is_empty()) {
            config = config.with_auth(secret);
        }
        Ok(config)
    }

    /// Authenticate with an AUTH string (Memorystore's default: no username).
    pub fn with_auth(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    /// Authenticate as a named ACL user, for a Redis 6+ deployment that has
    /// them. Memorystore's own AUTH string has no user, so this is separate
    /// from [`Self::with_auth`] rather than parsed out of it — splitting a
    /// secret on a `:` that a password is allowed to contain is how a working
    /// credential becomes a mysterious `WRONGPASS`.
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Override the fixed first key segment. See [`DEFAULT_KEY_PREFIX`].
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Result<Self> {
        let prefix = prefix.into();
        if prefix.is_empty() {
            return Err(Error::invalid(
                "the Memorystore key prefix may not be empty; it is what keeps a prefix scan from \
                 walking keys this platform did not write",
            ));
        }
        self.key_prefix = prefix;
        Ok(self)
    }

    /// Expire every value this store writes after `lifetime`.
    ///
    /// Off by default, because an expiry a caller did not ask for is a value
    /// that vanishes for a reason nobody wrote down. Where it is set, it is
    /// sent as `PX` milliseconds on every `SET`, so a cache cannot slowly turn
    /// into the only copy of something by never being evicted.
    pub fn with_entry_lifetime(mut self, lifetime: Duration) -> Result<Self> {
        if lifetime.is_zero() {
            return Err(Error::invalid(
                "a Memorystore entry lifetime of zero would delete every value as it was written; \
                 leave it unset for no expiry",
            ));
        }
        self.entry_lifetime = Some(lifetime);
        Ok(self)
    }

    pub fn with_limits(mut self, limits: RedisLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn limits(&self) -> RedisLimits {
        self.limits
    }

    /// Whether an AUTH string was supplied. The string itself is not readable.
    pub fn is_authenticated(&self) -> bool {
        self.secret.is_some()
    }
}

/// Add the default port to an address that does not name one.
///
/// A bare host is what an operator copies out of the `redis_host` Terraform
/// output, and defaulting the port here means the deployment does not have to
/// know that Redis is 6379.
fn normalise_address(address: &str) -> Result<String> {
    let address = address.trim();
    if address.is_empty() {
        return Err(Error::invalid("a Memorystore address may not be empty"));
    }
    if address.contains(':') {
        Ok(address.to_string())
    } else {
        Ok(format!("{address}:{DEFAULT_PORT}"))
    }
}

/// One decoded RESP reply.
///
/// Public because a reply is the whole of what the server says, and a test
/// that can only assert on the adapter's interpretation of a reply cannot
/// assert that the interpretation is right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RespValue {
    /// `+OK`
    Simple(String),
    /// `-WRONGPASS ...`. An answer, not a transport failure: the server
    /// understood the command and declined it.
    ServerError(String),
    /// `:1`
    Integer(i64),
    /// `$3\r\nabc`, or `$-1` for the null bulk string that means "no such key".
    Bulk(Option<Vec<u8>>),
    /// `*2\r\n...`, or `*-1` for a null array.
    Array(Option<Vec<RespValue>>),
}

impl RespValue {
    /// The RESP type name, for error messages that have to say what arrived
    /// instead of what was expected.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Simple(_) => "a simple string",
            Self::ServerError(_) => "an error",
            Self::Integer(_) => "an integer",
            Self::Bulk(None) => "a null bulk string",
            Self::Bulk(Some(_)) => "a bulk string",
            Self::Array(None) => "a null array",
            Self::Array(Some(_)) => "an array",
        }
    }
}

/// How many bytes of one reply remain readable.
///
/// Carried through the decoder rather than checked at the end, so that a
/// declared length larger than the whole budget is refused at the header —
/// before the allocation it would have caused.
#[derive(Debug)]
struct Budget {
    limit: usize,
    remaining: usize,
}

impl Budget {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            remaining: limit,
        }
    }

    fn spend(&mut self, bytes: usize) -> Result<()> {
        match self.remaining.checked_sub(bytes) {
            Some(left) => {
                self.remaining = left;
                Ok(())
            }
            None => Err(Error::schema(format!(
                "a Redis reply wanted more than the {} bytes this client will hold for one reply; \
                 refused at the length header rather than after the allocation",
                self.limit
            ))),
        }
    }
}

/// Encode a command as a RESP array of bulk strings.
///
/// Every argument is length-prefixed, which is why this adapter does not have
/// to escape or reject anything a caller puts in a key or a value: a `\r\n`
/// inside an argument is data, not framing. That is a property of RESP worth
/// naming, because the equivalent inline-command form has exactly the
/// injection problem this form does not.
fn encode_command(arguments: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("*{}\r\n", arguments.len()).as_bytes());
    for argument in arguments {
        out.extend_from_slice(format!("${}\r\n", argument.len()).as_bytes());
        out.extend_from_slice(argument);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Classify a failed read.
///
/// The distinction matters to the retry rule: a timeout must not be retried,
/// an end-of-stream on a pooled connection is exactly the case that must be.
fn read_failure(error: &std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::timeout(format!(
            "the Redis server accepted the command and then went silent: {error}"
        )),
        std::io::ErrorKind::UnexpectedEof => Error::io(
            "the Redis server closed the connection without finishing its reply".to_string(),
        ),
        _ => Error::io(format!("reading a Redis reply failed: {error}")),
    }
}

fn read_byte<R: Read>(reader: &mut R, budget: &mut Budget) -> Result<u8> {
    budget.spend(1)?;
    let mut byte = [0u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|error| read_failure(&error))?;
    Ok(byte[0])
}

/// Read one CRLF-terminated line, without its terminator.
fn read_line<R: Read>(
    reader: &mut R,
    budget: &mut Budget,
    limits: &RedisLimits,
) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let byte = read_byte(reader, budget)?;
        if byte == b'\n' {
            // A bare newline is not RESP. Accepting one would mean this client
            // and the server disagree about where a reply ends, which is worse
            // than refusing: every later reply would be read from the wrong
            // offset and interpreted as something it is not.
            if line.pop() == Some(b'\r') {
                return Ok(line);
            }
            return Err(Error::schema(
                "a Redis reply line ended with a bare newline; RESP terminates every line with \
                 CRLF",
            ));
        }
        line.push(byte);
        if line.len() > limits.max_line_bytes {
            return Err(Error::schema(format!(
                "a Redis reply line passed {} bytes without a CRLF",
                limits.max_line_bytes
            )));
        }
    }
}

fn line_as_text(line: Vec<u8>) -> Result<String> {
    String::from_utf8(line).map_err(|error| {
        Error::schema(format!(
            "a Redis reply line was not valid UTF-8: {}",
            error.utf8_error()
        ))
    })
}

fn line_as_integer(line: &[u8]) -> Result<i64> {
    std::str::from_utf8(line)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| {
            Error::schema(format!(
                "a Redis reply declared {:?} where a number belongs",
                String::from_utf8_lossy(line)
            ))
        })
}

/// Decode one reply, bounded in size, element count and nesting depth.
///
/// Generic over [`Read`] rather than taking the socket directly so that the
/// malformed-reply cases can be tested against a byte slice as well as against
/// a real server — the failures worth asserting on are the ones a socket makes
/// awkward to produce on demand.
fn read_reply<R: Read>(
    reader: &mut R,
    budget: &mut Budget,
    depth: usize,
    limits: &RedisLimits,
) -> Result<RespValue> {
    if depth > limits.max_nesting_depth {
        return Err(Error::schema(format!(
            "a Redis reply nested arrays more than {} deep; SCAN, the deepest reply this client \
             asks for, is two",
            limits.max_nesting_depth
        )));
    }
    let tag = read_byte(reader, budget)?;
    match tag {
        b'+' => Ok(RespValue::Simple(line_as_text(read_line(
            reader, budget, limits,
        )?)?)),
        b'-' => Ok(RespValue::ServerError(line_as_text(read_line(
            reader, budget, limits,
        )?)?)),
        b':' => Ok(RespValue::Integer(line_as_integer(&read_line(
            reader, budget, limits,
        )?)?)),
        b'$' => {
            let declared = line_as_integer(&read_line(reader, budget, limits)?)?;
            if declared < 0 {
                return Ok(RespValue::Bulk(None));
            }
            let length = usize::try_from(declared).map_err(|_| {
                Error::schema("a Redis bulk string declared a length this platform cannot address")
            })?;
            // Both the payload and its CRLF, charged before the allocation.
            budget.spend(length + 2)?;
            let mut payload = vec![0u8; length];
            reader
                .read_exact(&mut payload)
                .map_err(|error| read_failure(&error))?;
            let mut terminator = [0u8; 2];
            reader
                .read_exact(&mut terminator)
                .map_err(|error| read_failure(&error))?;
            if &terminator != b"\r\n" {
                return Err(Error::schema(format!(
                    "a Redis bulk string of {length} bytes was not terminated by CRLF; the \
                     declared length and the payload disagree"
                )));
            }
            Ok(RespValue::Bulk(Some(payload)))
        }
        b'*' => {
            let declared = line_as_integer(&read_line(reader, budget, limits)?)?;
            if declared < 0 {
                return Ok(RespValue::Array(None));
            }
            let count = usize::try_from(declared).map_err(|_| {
                Error::schema("a Redis array declared a length this platform cannot address")
            })?;
            if count > limits.max_array_elements {
                return Err(Error::schema(format!(
                    "a Redis array declared {count} elements; this client holds at most {}",
                    limits.max_array_elements
                )));
            }
            // One byte per element is the cheapest any element can be, so
            // charging that up front refuses an array whose *declaration*
            // already exceeds the budget without reading a single element.
            budget.spend(count)?;
            let mut elements = Vec::with_capacity(count);
            for _ in 0..count {
                elements.push(read_reply(reader, budget, depth + 1, limits)?);
            }
            Ok(RespValue::Array(Some(elements)))
        }
        other => Err(Error::schema(format!(
            "{:?} does not begin a RESP reply; this client speaks RESP2, whose replies begin with \
             '+', '-', ':', '$' or '*'",
            char::from(other)
        ))),
    }
}

/// One authenticated socket to the instance.
#[derive(Debug)]
struct Connection {
    reader: BufReader<TcpStream>,
}

impl Connection {
    /// Connect, apply the timeouts, and authenticate.
    ///
    /// Authentication happens here rather than lazily so that a wrong AUTH
    /// string is a failure to open a connection, not a puzzling `NOAUTH` on
    /// the first read of a value that was supposed to be there.
    fn open(config: &RedisConfig) -> Result<Self> {
        let limits = &config.limits;
        let addresses: Vec<SocketAddr> = config
            .address
            .to_socket_addrs()
            .map_err(|error| {
                Error::io(format!(
                    "the Memorystore address {:?} did not resolve: {error}",
                    config.address
                ))
            })?
            .collect();
        let mut last: Option<Error> = None;
        let mut connected: Option<TcpStream> = None;
        for address in &addresses {
            match TcpStream::connect_timeout(address, limits.connect_timeout) {
                Ok(stream) => {
                    // A cache read is one small command and one small reply.
                    // Waiting 40ms for a second segment that is never coming
                    // is the whole of Nagle's downside here.
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(limits.read_timeout));
                    let _ = stream.set_write_timeout(Some(limits.write_timeout));
                    connected = Some(stream);
                    break;
                }
                Err(error) if is_timeout(&error) => {
                    last = Some(Error::timeout(format!(
                        "connecting to Memorystore at {address} did not complete within {:?}. \
                         The instance is reachable only from the peered VPC",
                        limits.connect_timeout
                    )));
                }
                Err(error) => {
                    last = Some(Error::io(format!(
                        "connecting to Memorystore at {address} failed: {error}"
                    )));
                }
            }
        }
        let stream = match connected {
            Some(stream) => stream,
            None => {
                return Err(last.unwrap_or_else(|| {
                    Error::io(format!(
                        "the Memorystore address {:?} resolved to no addresses",
                        config.address
                    ))
                }));
            }
        };

        let mut connection = Self {
            reader: BufReader::new(stream),
        };
        if let Some(secret) = config.secret.as_deref() {
            let reply = match config.username.as_deref() {
                Some(username) => connection.command(
                    &[b"AUTH", username.as_bytes(), secret.as_bytes()],
                    &config.limits,
                )?,
                None => connection.command(&[b"AUTH", secret.as_bytes()], &config.limits)?,
            };
            expect_ok(interpret(reply, "AUTH")?, "AUTH")?;
        }
        Ok(connection)
    }

    /// Write one command and read exactly one reply.
    fn command(&mut self, arguments: &[&[u8]], limits: &RedisLimits) -> Result<RespValue> {
        let request = encode_command(arguments);
        let mut sink: &TcpStream = self.reader.get_ref();
        sink.write_all(&request)
            .and_then(|()| sink.flush())
            .map_err(|error| {
                if is_timeout(&error) {
                    Error::timeout(format!("writing a Redis command timed out: {error}"))
                } else {
                    Error::io(format!("writing a Redis command failed: {error}"))
                }
            })?;
        let mut budget = Budget::new(limits.max_reply_bytes);
        read_reply(&mut self.reader, &mut budget, 0, limits)
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Turn a `-ERR` reply into a platform error, leaving every other reply alone.
///
/// A refused authentication is [`Error::Denied`] rather than [`Error::Io`]
/// because retrying it is pointless and the fix is a credential, not a
/// network. Everything else the server declines is [`Error::Io`]: an
/// out-of-memory instance, a read-only replica, a command the server does not
/// have.
fn interpret(reply: RespValue, command: &str) -> Result<RespValue> {
    match reply {
        RespValue::ServerError(message) => {
            let code = message.split_whitespace().next().unwrap_or_default();
            if command == "AUTH" || matches!(code, "NOAUTH" | "WRONGPASS" | "NOPERM") {
                Err(Error::denied(format!(
                    "Memorystore refused {command}: {message}. The AUTH string is read from \
                     {AUTH_VARIABLE} and the instance is provisioned with auth_enabled = true; a \
                     missing variable and a wrong one look the same from here"
                )))
            } else {
                Err(Error::io(format!(
                    "Memorystore refused {command}: {message}"
                )))
            }
        }
        other => Ok(other),
    }
}

fn expect_ok(reply: RespValue, command: &str) -> Result<()> {
    match reply {
        RespValue::Simple(ref text) if text == "OK" => Ok(()),
        other => Err(Error::schema(format!(
            "Memorystore answered {command} with {} where +OK belongs",
            other.kind()
        ))),
    }
}

fn expect_integer(reply: RespValue, command: &str) -> Result<i64> {
    match reply {
        RespValue::Integer(value) => Ok(value),
        other => Err(Error::schema(format!(
            "Memorystore answered {command} with {} where an integer belongs",
            other.kind()
        ))),
    }
}

fn expect_optional_bulk(reply: RespValue, command: &str) -> Result<Option<Vec<u8>>> {
    match reply {
        RespValue::Bulk(payload) => Ok(payload),
        other => Err(Error::schema(format!(
            "Memorystore answered {command} with {} where a bulk string belongs",
            other.kind()
        ))),
    }
}

fn expect_array(reply: RespValue, command: &str) -> Result<Vec<RespValue>> {
    match reply {
        RespValue::Array(Some(elements)) => Ok(elements),
        other => Err(Error::schema(format!(
            "Memorystore answered {command} with {} where an array belongs",
            other.kind()
        ))),
    }
}

/// Escape the glob metacharacters Redis's `MATCH` understands.
///
/// `SCAN ... MATCH` takes a glob, and a caller's prefix is a literal. Without
/// this, a namespace or key prefix containing `*`, `?` or a character class
/// would silently match keys the caller did not ask for — the scan equivalent
/// of an injection, and one that returns *more* data rather than failing.
fn glob_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len() + 8);
    for character in literal.chars() {
        if matches!(character, '*' | '?' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// A [`KeyValueStore`] over Memorystore, for values that can be recomputed.
///
/// Read the module documentation before using it: this store is not durable,
/// not transactional, and not a source of record, and none of those is a
/// defect to be fixed later. [`Self::is_crash_safe`] returns `false` and will
/// keep returning `false`.
pub struct RedisKeyValueStore {
    config: RedisConfig,
    namespace: String,
    /// `{key_prefix}:{namespace}:` — computed once, since it is prepended to
    /// every key and stripped off every scan result.
    wire_prefix: String,
    /// The pooled connection, absent until the first use and after any
    /// failure. A failure means the stream's framing can no longer be trusted,
    /// so the socket is dropped rather than reused.
    connection: Mutex<Option<Connection>>,
}

impl std::fmt::Debug for RedisKeyValueStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisKeyValueStore")
            .field("address", &self.config.address)
            .field("namespace", &self.namespace)
            .field("authenticated", &self.config.is_authenticated())
            // Stated in the Debug output because this type is held behind
            // `Arc<dyn KeyValueStore>`, where nothing else distinguishes it
            // from the engine.
            .field("durability", &Self::DURABILITY_NOTICE)
            .finish_non_exhaustive()
    }
}

impl RedisKeyValueStore {
    /// What this store guarantees about a write it has acknowledged.
    ///
    /// A constant rather than prose in a doc comment so that a caller, a log
    /// line and a test can all quote the same sentence.
    pub const DURABILITY_NOTICE: &'static str = "none: a cache with persistence \
         disabled. An acknowledged write can be lost to a restart, a failover, an \
         eviction or an expiry, and none of those is an error";

    /// Connect, authenticate, and prove the instance answers.
    ///
    /// The `PING` is what makes this a start-up failure rather than a failure
    /// during the first trading cycle. A store that constructed cleanly and
    /// only failed on first use would be believed by a health check.
    pub fn connect(config: RedisConfig, namespace: &str) -> Result<Self> {
        let namespace = namespace.trim();
        if namespace.is_empty() {
            return Err(Error::invalid(
                "a Memorystore namespace may not be empty; it is the middle segment of every key \
                 this store writes",
            ));
        }
        let wire_prefix = format!("{}:{}:", config.key_prefix, namespace);
        if wire_prefix.len() >= config.limits.max_key_bytes {
            return Err(Error::invalid(format!(
                "the Memorystore key prefix {wire_prefix:?} already fills the {}-byte key budget, \
                 leaving no room for a key",
                config.limits.max_key_bytes
            )));
        }
        let store = Self {
            config,
            namespace: namespace.to_string(),
            wire_prefix,
            connection: Mutex::new(None),
        };
        store.ping()?;
        Ok(store)
    }

    /// Whether an acknowledged write survives loss of power.
    ///
    /// Always `false`. It is a method rather than a doc sentence because the
    /// caller that most needs to know is one holding this as
    /// `Arc<dyn KeyValueStore>`, and because a constant answer that can be
    /// asserted on is harder to quietly change than a paragraph.
    pub const fn is_crash_safe(&self) -> bool {
        false
    }

    /// The namespace every key in this store sits under.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The address this store is connected to.
    pub fn address(&self) -> &str {
        self.config.address()
    }

    /// `PING`, for a health check.
    pub fn ping(&self) -> Result<()> {
        let reply = interpret(self.call(&[b"PING"])?, "PING")?;
        match reply {
            RespValue::Simple(ref text) if text == "PONG" => Ok(()),
            // `PING` inside a subscription answers with an array, and a proxy
            // may answer with a bulk string. Neither is what this client asked
            // for, and treating either as healthy is how a health check comes
            // to pass against something that is not Redis.
            other => Err(Error::io(format!(
                "Memorystore answered PING with {} rather than +PONG",
                other.kind()
            ))),
        }
    }

    /// Send one command, opening or reopening the connection as needed.
    ///
    /// The retry rule is stated in the module documentation: once, only on a
    /// reused connection, and only for a socket-level failure.
    fn call(&self, arguments: &[&[u8]]) -> Result<RespValue> {
        let mut guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pooled = guard.take();
        let reused = pooled.is_some();
        let mut connection = match pooled {
            Some(existing) => existing,
            None => Connection::open(&self.config)?,
        };

        match connection.command(arguments, &self.config.limits) {
            Ok(reply) => {
                *guard = Some(connection);
                Ok(reply)
            }
            Err(error) => {
                // Whatever happened, this socket's framing is no longer
                // trustworthy: a half-read reply would be parsed as the next
                // command's answer.
                drop(connection);
                if reused && matches!(error, Error::Io(_)) {
                    let mut fresh = Connection::open(&self.config)?;
                    let reply = fresh.command(arguments, &self.config.limits)?;
                    *guard = Some(fresh);
                    Ok(reply)
                } else {
                    Err(error)
                }
            }
        }
    }

    /// The key as the server stores it, bounds checked.
    fn wire_key(&self, key: &str) -> Result<String> {
        if key.is_empty() {
            return Err(Error::invalid(
                "a Memorystore key may not be empty; an empty key is legal in Redis and is always \
                 a caller's mistake",
            ));
        }
        let wire = format!("{}{key}", self.wire_prefix);
        if wire.len() > self.config.limits.max_key_bytes {
            return Err(Error::invalid(format!(
                "the key {wire:?} is {} bytes on the wire, over the {}-byte limit",
                wire.len(),
                self.config.limits.max_key_bytes
            )));
        }
        Ok(wire)
    }

    /// Every key under `prefix`, deduplicated and ordered.
    ///
    /// `SCAN` rather than `KEYS`: `KEYS` walks the whole keyspace in one
    /// blocking pass, which on a shared cache stalls every other client for as
    /// long as it takes. `SCAN` is incremental, and its cost is paid by this
    /// caller. The price is that `SCAN` may return the same key twice and may
    /// miss a key added during the walk — so the results go into a
    /// [`BTreeSet`], which deduplicates and gives the lexicographic order
    /// [`KeyValueStore::keys_with_prefix`] promises, and the walk is honestly
    /// a snapshot of nothing in particular.
    fn scan(&self, prefix: &str) -> Result<BTreeSet<String>> {
        let literal = format!("{}{prefix}", self.wire_prefix);
        if literal.len() > self.config.limits.max_key_bytes {
            return Err(Error::invalid(format!(
                "the scan prefix {literal:?} is longer than the {}-byte key limit, so no key can \
                 match it",
                self.config.limits.max_key_bytes
            )));
        }
        let pattern = format!("{}*", glob_escape(&literal));
        let batch = self.config.limits.scan_batch.to_string();

        let mut cursor = String::from("0");
        let mut found = BTreeSet::new();
        let mut rounds = 0usize;
        loop {
            rounds += 1;
            if rounds > self.config.limits.max_scan_rounds {
                return Err(Error::schema(format!(
                    "a Memorystore prefix scan made {} round trips without the cursor returning \
                     to zero",
                    self.config.limits.max_scan_rounds
                )));
            }
            let reply = self.call(&[
                b"SCAN",
                cursor.as_bytes(),
                b"MATCH",
                pattern.as_bytes(),
                b"COUNT",
                batch.as_bytes(),
            ])?;
            let mut parts = expect_array(interpret(reply, "SCAN")?, "SCAN")?.into_iter();
            let (Some(next_cursor), Some(batch_reply), None) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(Error::schema(
                    "SCAN must answer with exactly two elements: a cursor and a batch of keys",
                ));
            };
            let next_cursor = expect_optional_bulk(next_cursor, "SCAN")?.ok_or_else(|| {
                Error::schema("SCAN answered with a null cursor, which cannot be sent back")
            })?;

            for element in expect_array(batch_reply, "SCAN")? {
                let key = expect_optional_bulk(element, "SCAN")?.ok_or_else(|| {
                    Error::schema("SCAN returned a null element where a key belongs")
                })?;
                let key = String::from_utf8(key).map_err(|error| {
                    Error::schema(format!(
                        "SCAN returned a key that is not valid UTF-8: {}. Something other than \
                         this platform is writing under {:?}",
                        error.utf8_error(),
                        self.wire_prefix
                    ))
                })?;
                let Some(caller_key) = key
                    .strip_prefix(&self.wire_prefix)
                    .filter(|rest| rest.starts_with(prefix))
                else {
                    return Err(Error::schema(format!(
                        "SCAN returned {key:?}, which is outside the pattern {pattern:?} it was \
                         given"
                    )));
                };
                found.insert(caller_key.to_string());
                if found.len() > self.config.limits.max_scan_keys {
                    return Err(Error::schema(format!(
                        "a Memorystore prefix scan passed {} keys; a cache namespace this large \
                         is being used as a database",
                        self.config.limits.max_scan_keys
                    )));
                }
            }

            cursor = line_as_text(next_cursor)?;
            if cursor == "0" {
                return Ok(found);
            }
        }
    }
}

impl KeyValueStore for RedisKeyValueStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let wire = self.wire_key(key)?;
        let reply = interpret(self.call(&[b"GET", wire.as_bytes()])?, "GET")?;
        match expect_optional_bulk(reply, "GET")? {
            None => Ok(None),
            Some(payload) => {
                let value = serde_json::from_slice(&payload).map_err(|error| {
                    Error::schema(format!(
                        "the value at {wire:?} is not the JSON this store writes: {error}. A \
                         cache entry written by something else is not readable here"
                    ))
                })?;
                Ok(Some(value))
            }
        }
    }

    /// Store a value that the platform can afford to lose.
    ///
    /// Returning `Ok` means the server acknowledged the write in memory. See
    /// [`RedisKeyValueStore::DURABILITY_NOTICE`] for everything that can take
    /// it back afterwards.
    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let wire = self.wire_key(key)?;
        let payload = serde_json::to_vec(&value)?;
        if payload.len() > self.config.limits.max_value_bytes {
            return Err(Error::invalid(format!(
                "the value for {wire:?} serialises to {} bytes, over the {}-byte cache entry \
                 limit. A value this size belongs in a blob store, where it is durable and does \
                 not compete with hot keys for the instance's memory",
                payload.len(),
                self.config.limits.max_value_bytes
            )));
        }

        let lifetime = self.config.entry_lifetime.map(|lifetime| {
            u64::try_from(lifetime.as_millis())
                .unwrap_or(u64::MAX)
                .to_string()
        });
        let mut arguments: Vec<&[u8]> = vec![b"SET", wire.as_bytes(), payload.as_slice()];
        if let Some(milliseconds) = lifetime.as_deref() {
            arguments.push(b"PX");
            arguments.push(milliseconds.as_bytes());
        }
        expect_ok(interpret(self.call(&arguments)?, "SET")?, "SET")
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let wire = self.wire_key(key)?;
        let reply = interpret(self.call(&[b"DEL", wire.as_bytes()])?, "DEL")?;
        Ok(expect_integer(reply, "DEL")? > 0)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self.scan(prefix)?.into_iter().collect())
    }

    /// How many keys this namespace holds.
    ///
    /// A full `SCAN` of the namespace, not `DBSIZE`: `DBSIZE` counts the whole
    /// instance, including every other namespace and every other tenant. The
    /// cost is therefore proportional to the keyspace, which is worth knowing
    /// before calling it in a loop.
    fn len(&self) -> Result<usize> {
        Ok(self.scan("")?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::KeyValueStore;
    use crate::provider::{StorageProvider, StorageTarget};
    use std::collections::{BTreeMap, VecDeque};
    use std::io::{BufRead, BufReader as StdBufReader};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // --- a real RESP server on a loopback port -----------------------------
    //
    // The adapter is only interesting where it meets a socket, so the tests
    // give it one. Port 0 asks the operating system for a free port, so these
    // run alongside every other test binary without a hard-coded number.

    /// What the server should do when it receives a command.
    #[derive(Clone, Debug)]
    enum Reply {
        /// Exact bytes, so a test can send something that is not valid RESP.
        Raw(Vec<u8>),
        /// Say nothing for this long, then hang up. For the read timeout.
        Silent(Duration),
        /// Hang up without answering. For the pooled connection that has gone
        /// stale under the client.
        Close,
    }

    fn ok() -> Reply {
        Reply::Raw(b"+OK\r\n".to_vec())
    }

    fn integer(value: i64) -> Reply {
        Reply::Raw(format!(":{value}\r\n").into_bytes())
    }

    fn bulk(text: &str) -> Reply {
        Reply::Raw(format!("${}\r\n{text}\r\n", text.len()).into_bytes())
    }

    fn null_bulk() -> Reply {
        Reply::Raw(b"$-1\r\n".to_vec())
    }

    fn server_error(text: &str) -> Reply {
        Reply::Raw(format!("-{text}\r\n").into_bytes())
    }

    /// A `SCAN` answer: the cursor to send next, and the keys in this batch.
    fn scan_batch(cursor: &str, keys: &[&str]) -> Reply {
        let mut out = format!("*2\r\n${}\r\n{cursor}\r\n*{}\r\n", cursor.len(), keys.len());
        for key in keys {
            out.push_str(&format!("${}\r\n{key}\r\n", key.len()));
        }
        Reply::Raw(out.into_bytes())
    }

    struct RespServer {
        address: String,
        stop: Arc<AtomicBool>,
        connections: Arc<AtomicUsize>,
        commands: Arc<Mutex<Vec<Vec<String>>>>,
        scripted: Arc<Mutex<BTreeMap<String, VecDeque<Reply>>>>,
        standing: Arc<Mutex<BTreeMap<String, Reply>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl RespServer {
        /// A server that answers `PING` with `+PONG` and nothing else until a
        /// test says so.
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let address = listener
                .local_addr()
                .expect("the listener has a local address")
                .to_string();
            listener
                .set_nonblocking(true)
                .expect("the listener can poll");

            let stop = Arc::new(AtomicBool::new(false));
            let connections = Arc::new(AtomicUsize::new(0));
            let commands = Arc::new(Mutex::new(Vec::new()));
            let scripted = Arc::new(Mutex::new(BTreeMap::new()));
            let standing = Arc::new(Mutex::new(BTreeMap::from([(
                "PING".to_string(),
                Reply::Raw(b"+PONG\r\n".to_vec()),
            )])));

            let thread_stop = stop.clone();
            let thread_connections = connections.clone();
            let thread_commands = commands.clone();
            let thread_scripted = scripted.clone();
            let thread_standing = standing.clone();
            let handle = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            thread_connections.fetch_add(1, Ordering::SeqCst);
                            let commands = thread_commands.clone();
                            let scripted = thread_scripted.clone();
                            let standing = thread_standing.clone();
                            // Its own thread, so that a deliberately silent
                            // answer delays the client under test and not the
                            // next connection: a client that has timed out and
                            // reconnects must find the listener ready.
                            std::thread::spawn(move || {
                                serve(stream, &commands, &scripted, &standing);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                address,
                stop,
                connections,
                commands,
                scripted,
                standing,
                handle: Some(handle),
            }
        }

        /// Answer the next `command` this way, once.
        fn once(&self, command: &str, reply: Reply) -> &Self {
            self.scripted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(command.to_ascii_uppercase())
                .or_default()
                .push_back(reply);
            self
        }

        /// Answer every `command` this way, after any one-shot replies.
        fn always(&self, command: &str, reply: Reply) -> &Self {
            self.standing
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(command.to_ascii_uppercase(), reply);
            self
        }

        fn config(&self) -> RedisConfig {
            RedisConfig::at(&self.address)
                .expect("a loopback address is a valid address")
                .with_limits(RedisLimits {
                    read_timeout: Duration::from_millis(500),
                    connect_timeout: Duration::from_millis(500),
                    write_timeout: Duration::from_millis(500),
                    ..RedisLimits::default()
                })
        }

        fn store(&self, namespace: &str) -> RedisKeyValueStore {
            RedisKeyValueStore::connect(self.config(), namespace)
                .expect("the test server accepts a connection and answers PING")
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        fn connections(&self) -> usize {
            self.connections.load(Ordering::SeqCst)
        }
    }

    impl Drop for RespServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn serve(
        stream: TcpStream,
        commands: &Arc<Mutex<Vec<Vec<String>>>>,
        scripted: &Arc<Mutex<BTreeMap<String, VecDeque<Reply>>>>,
        standing: &Arc<Mutex<BTreeMap<String, Reply>>>,
    ) {
        let mut reader = StdBufReader::new(match stream.try_clone() {
            Ok(clone) => clone,
            Err(_) => return,
        });
        let mut sink = stream;
        while let Some(command) = read_command(&mut reader) {
            let name = command
                .first()
                .map(|first| first.to_ascii_uppercase())
                .unwrap_or_default();
            commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(command);

            let queued = scripted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_mut(&name)
                .and_then(VecDeque::pop_front);
            let reply = queued.or_else(|| {
                standing
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&name)
                    .cloned()
            });

            match reply {
                Some(Reply::Raw(bytes)) => {
                    if sink.write_all(&bytes).and_then(|()| sink.flush()).is_err() {
                        return;
                    }
                }
                Some(Reply::Silent(delay)) => {
                    std::thread::sleep(delay);
                    let _ = sink.shutdown(std::net::Shutdown::Both);
                    return;
                }
                Some(Reply::Close) | None => {
                    let _ = sink.shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        }
    }

    /// Read one RESP array of bulk strings, as a client sends it.
    fn read_command(reader: &mut StdBufReader<TcpStream>) -> Option<Vec<String>> {
        let count = match read_header(reader, b'*')? {
            count if count >= 0 => count,
            _ => return None,
        };
        let mut arguments = Vec::new();
        for _ in 0..count {
            let length = read_header(reader, b'$')?;
            let length = usize::try_from(length).ok()?;
            let mut payload = vec![0u8; length + 2];
            reader.read_exact(&mut payload).ok()?;
            payload.truncate(length);
            arguments.push(String::from_utf8(payload).ok()?);
        }
        Some(arguments)
    }

    fn read_header(reader: &mut StdBufReader<TcpStream>, expected: u8) -> Option<i64> {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        let rest = line.strip_prefix(char::from(expected))?;
        rest.parse::<i64>().ok()
    }

    /// An address nothing is listening on: bind a port, then drop it. The only
    /// way to name a free port without guessing one a parallel test has taken.
    fn address_with_no_listener() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let address = listener
            .local_addr()
            .expect("the listener has a local address")
            .to_string();
        drop(listener);
        address
    }

    fn decode(bytes: &[u8]) -> Result<RespValue> {
        let limits = RedisLimits::default();
        let mut budget = Budget::new(limits.max_reply_bytes);
        read_reply(&mut &bytes[..], &mut budget, 0, &limits)
    }

    // --- the protocol ------------------------------------------------------

    #[test]
    fn a_command_is_encoded_as_a_resp_array_of_length_prefixed_bulk_strings() {
        assert_eq!(
            encode_command(&[b"SET", b"k", b"v"]),
            b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\nv\r\n".to_vec()
        );
    }

    #[test]
    fn a_value_containing_crlf_is_framed_by_its_length_and_cannot_forge_a_second_command() {
        // The premise: the payload really does contain the bytes that would
        // end a line in an inline command.
        let payload: &[u8] = b"a\r\nDEL everything\r\n";
        assert!(payload.windows(2).any(|pair| pair == b"\r\n"));

        let encoded = encode_command(&[b"SET", b"k", payload]);
        let text = String::from_utf8_lossy(&encoded);
        assert!(
            text.starts_with("*3\r\n"),
            "still exactly three arguments: {text:?}"
        );
        assert!(
            text.contains(&format!("${}\r\n", payload.len())),
            "the payload is declared by length, not delimited: {text:?}"
        );
    }

    #[test]
    fn every_resp_reply_type_decodes_to_the_value_it_names() {
        assert_eq!(decode(b"+OK\r\n"), Ok(RespValue::Simple("OK".to_string())));
        assert_eq!(
            decode(b"-ERR nope\r\n"),
            Ok(RespValue::ServerError("ERR nope".to_string()))
        );
        assert_eq!(decode(b":-7\r\n"), Ok(RespValue::Integer(-7)));
        assert_eq!(
            decode(b"$3\r\nabc\r\n"),
            Ok(RespValue::Bulk(Some(b"abc".to_vec())))
        );
        assert_eq!(decode(b"$-1\r\n"), Ok(RespValue::Bulk(None)));
        assert_eq!(decode(b"*-1\r\n"), Ok(RespValue::Array(None)));
        assert_eq!(
            decode(b"*2\r\n:1\r\n$2\r\nhi\r\n"),
            Ok(RespValue::Array(Some(vec![
                RespValue::Integer(1),
                RespValue::Bulk(Some(b"hi".to_vec())),
            ])))
        );
    }

    #[test]
    fn a_malformed_reply_is_refused_rather_than_panicking() {
        // Four different ways for a peer to be wrong, each of which would be a
        // panic or a wrong answer in a decoder that trusted its input.
        for (bytes, what) in [
            (&b"!nope\r\n"[..], "an unknown type byte"),
            (&b":twelve\r\n"[..], "a non-numeric integer"),
            (&b"$3\r\nabcd\r\n"[..], "a bulk string longer than declared"),
            (&b"+bare\n"[..], "a line ended without a carriage return"),
        ] {
            let error = decode(bytes).expect_err(what);
            assert_eq!(error.code(), "schema", "{what}: {error}");
        }
    }

    #[test]
    fn a_reply_that_stops_half_way_is_an_io_failure_and_not_a_parse_of_what_arrived() {
        let error = decode(b"$10\r\nabc").expect_err("a truncated bulk string");
        assert_eq!(error.code(), "io", "{error}");
    }

    #[test]
    fn a_bulk_string_declaring_more_than_the_bounded_reply_size_is_refused_at_its_header() {
        // The header alone is eleven bytes; a decoder that allocated first
        // would ask for a hundred megabytes on the strength of them.
        let error = decode(b"$99999999\r\n").expect_err("an oversized bulk string");
        assert_eq!(error.code(), "schema", "{error}");
        assert!(
            error.message().contains("refused at the length header"),
            "{error}"
        );
    }

    #[test]
    fn an_array_declaring_more_elements_than_the_limit_is_refused_before_any_are_read() {
        let error = decode(b"*100000000\r\n").expect_err("an oversized array");
        assert_eq!(error.code(), "schema", "{error}");
    }

    #[test]
    fn arrays_nested_deeper_than_the_limit_are_refused_rather_than_recursed_into() {
        let mut bytes = Vec::new();
        for _ in 0..64 {
            bytes.extend_from_slice(b"*1\r\n");
        }
        bytes.extend_from_slice(b":1\r\n");
        let error = decode(&bytes).expect_err("a deeply nested reply");
        assert_eq!(error.code(), "schema", "{error}");
    }

    #[test]
    fn a_literal_prefix_containing_glob_metacharacters_is_escaped_for_scan() {
        assert_eq!(glob_escape("a*b?c[d]e\\f"), "a\\*b\\?c\\[d\\]e\\\\f");
        assert_eq!(glob_escape("plain/key"), "plain/key");
    }

    // --- the adapter, against a real socket --------------------------------

    #[test]
    fn a_value_round_trips_through_set_and_get_under_the_namespaced_key() {
        let server = RespServer::start();
        server.always("SET", ok());
        server.always("GET", bulk("{\"quote\":41.5}"));
        let store = server.store("quotes");

        store
            .put("AAPL", serde_json::json!({ "quote": 41.5 }))
            .expect("the server acknowledges the write");
        let read_back = store.get("AAPL").expect("the server answers the read");

        assert_eq!(read_back, Some(serde_json::json!({ "quote": 41.5 })));
        let commands = server.commands();
        assert_eq!(
            commands,
            vec![
                vec!["PING".to_string()],
                vec![
                    "SET".to_string(),
                    "qip:quotes:AAPL".to_string(),
                    "{\"quote\":41.5}".to_string(),
                ],
                vec!["GET".to_string(), "qip:quotes:AAPL".to_string()],
            ],
            "the key on the wire carries the fixed prefix and the namespace"
        );
    }

    #[test]
    fn a_missing_key_is_a_null_bulk_string_and_not_an_error() {
        let server = RespServer::start();
        server.always("GET", null_bulk());
        let store = server.store("quotes");
        assert_eq!(
            store.get("MSFT").expect("a null reply is not a failure"),
            None
        );
    }

    #[test]
    fn a_prefix_scan_walks_every_cursor_and_returns_keys_without_the_wire_prefix() {
        let server = RespServer::start();
        // Two rounds, and a duplicate across them: SCAN is allowed to return
        // the same key twice, and a caller must not see it twice.
        server.once(
            "SCAN",
            scan_batch("17", &["qip:fills:b", "qip:fills:a", "qip:fills:b"]),
        );
        server.once("SCAN", scan_batch("0", &["qip:fills:c", "qip:fills:a"]));
        let store = server.store("fills");

        let keys = store.keys_with_prefix("").expect("the scan completes");

        assert_eq!(
            keys,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "deduplicated, stripped of the wire prefix, and in lexicographic order"
        );
        let scans: Vec<Vec<String>> = server
            .commands()
            .into_iter()
            .filter(|command| command.first().map(String::as_str) == Some("SCAN"))
            .collect();
        assert_eq!(scans.len(), 2, "the second cursor was followed: {scans:?}");
        assert_eq!(
            scans.first().and_then(|command| command.get(1)),
            Some(&"0".to_string()),
            "the walk starts at cursor zero"
        );
        assert_eq!(
            scans.get(1).and_then(|command| command.get(1)),
            Some(&"17".to_string()),
            "and continues from the cursor the server returned"
        );
    }

    #[test]
    fn a_namespace_containing_a_glob_metacharacter_cannot_widen_the_scan_it_asks_for() {
        let server = RespServer::start();
        server.always("SCAN", scan_batch("0", &[]));
        let store = RedisKeyValueStore::connect(server.config(), "book*")
            .expect("the namespace is legal, if unwise");

        store.keys_with_prefix("a[0]").expect("the scan completes");

        let pattern = server
            .commands()
            .into_iter()
            .find(|command| command.first().map(String::as_str) == Some("SCAN"))
            .and_then(|command| command.get(3).cloned())
            .expect("the SCAN carried a MATCH pattern");
        assert_eq!(
            pattern, "qip:book\\*:a\\[0\\]*",
            "every metacharacter in the literal is escaped, and only the trailing star is a glob"
        );
    }

    #[test]
    fn a_deletion_reports_whether_the_key_was_there() {
        let server = RespServer::start();
        server.once("DEL", integer(1));
        server.once("DEL", integer(0));
        let store = server.store("quotes");

        assert!(store.delete("AAPL").expect("the first delete succeeds"));
        assert!(
            !store.delete("AAPL").expect("the second delete succeeds"),
            "a key that was not there is not an error, and is not a deletion either"
        );
        assert_eq!(
            server.commands().last(),
            Some(&vec!["DEL".to_string(), "qip:quotes:AAPL".to_string()])
        );
    }

    #[test]
    fn a_configured_entry_lifetime_is_sent_with_every_write() {
        let server = RespServer::start();
        server.always("SET", ok());
        let config = server
            .config()
            .with_entry_lifetime(Duration::from_millis(1500))
            .expect("a non-zero lifetime is accepted");
        let store = RedisKeyValueStore::connect(config, "features").expect("the server answers");

        store
            .put("vol", serde_json::json!(1))
            .expect("the write is acknowledged");

        assert_eq!(
            server.commands().last(),
            Some(&vec![
                "SET".to_string(),
                "qip:features:vol".to_string(),
                "1".to_string(),
                "PX".to_string(),
                "1500".to_string(),
            ]),
            "the expiry travels with the write rather than being set afterwards, so a crash \
             between the two cannot leave a value that never expires"
        );
    }

    #[test]
    fn a_lifetime_of_zero_is_refused_because_it_would_delete_every_value_as_it_was_written() {
        let error = RedisConfig::at("127.0.0.1:6379")
            .expect("a valid address")
            .with_entry_lifetime(Duration::ZERO)
            .expect_err("zero is refused");
        assert_eq!(error.code(), "invalid", "{error}");
    }

    // --- refusals ----------------------------------------------------------

    #[test]
    fn an_authentication_failure_is_refused_legibly_and_never_echoes_the_secret() {
        let server = RespServer::start();
        server.always(
            "AUTH",
            server_error("WRONGPASS invalid username-password pair"),
        );
        let config = server.config().with_auth("s3cr3t-instance-auth-string");

        let error = RedisKeyValueStore::connect(config, "quotes")
            .expect_err("a rejected AUTH must not produce a working store");

        assert_eq!(
            error.code(),
            "denied",
            "a wrong credential is not a transport failure: {error}"
        );
        assert!(
            error.message().contains("WRONGPASS"),
            "the server's own words survive: {error}"
        );
        assert!(
            error.message().contains(AUTH_VARIABLE),
            "and the reader is told which variable to fix: {error}"
        );
        assert!(
            !error.message().contains("s3cr3t"),
            "but the secret itself is never in the message: {error}"
        );
        assert_eq!(
            server.commands().first(),
            Some(&vec![
                "AUTH".to_string(),
                "s3cr3t-instance-auth-string".to_string()
            ]),
            "AUTH is sent before anything else, so a bad credential fails at connect"
        );
    }

    #[test]
    fn the_auth_string_is_redacted_from_every_debug_rendering() {
        let server = RespServer::start();
        server.always("AUTH", ok());
        let config = server.config().with_auth("s3cr3t-instance-auth-string");
        assert!(
            !format!("{config:?}").contains("s3cr3t"),
            "a configuration is exactly the value that ends up in a start-up log"
        );
        assert!(format!("{config:?}").contains("redacted"));

        let store = RedisKeyValueStore::connect(config, "quotes").expect("the server answers PING");
        let rendered = format!("{store:?}");
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
        assert!(
            rendered.contains("authenticated: true"),
            "whether a credential was supplied is still visible: {rendered}"
        );
    }

    #[test]
    fn a_value_over_the_configured_limit_is_refused_before_a_byte_reaches_the_socket() {
        let server = RespServer::start();
        server.always("SET", ok());
        let config = server.config().with_limits(RedisLimits {
            max_value_bytes: 64,
            ..server.config().limits()
        });
        let store = RedisKeyValueStore::connect(config, "quotes").expect("the server answers");
        let before = server.commands().len();

        let error = store
            .put("big", serde_json::json!("x".repeat(256)))
            .expect_err("an oversized value is refused");

        assert_eq!(error.code(), "invalid", "{error}");
        assert!(
            error.message().contains("blob store"),
            "the refusal says where the value does belong: {error}"
        );
        assert_eq!(
            server.commands().len(),
            before,
            "and nothing was sent: the limit is enforced before the command is written"
        );
    }

    #[test]
    fn a_key_over_the_configured_limit_is_refused_before_a_byte_reaches_the_socket() {
        let server = RespServer::start();
        let config = server.config().with_limits(RedisLimits {
            max_key_bytes: 32,
            ..server.config().limits()
        });
        let store = RedisKeyValueStore::connect(config, "quotes").expect("the server answers");
        let before = server.commands().len();

        let error = store
            .get(&"k".repeat(64))
            .expect_err("an oversized key is refused");

        assert_eq!(error.code(), "invalid", "{error}");
        assert_eq!(server.commands().len(), before, "and nothing was sent");
    }

    #[test]
    fn an_empty_key_is_refused_rather_than_stored_under_the_bare_namespace() {
        let server = RespServer::start();
        let store = server.store("quotes");
        assert_eq!(
            store.get("").expect_err("an empty key is refused").code(),
            "invalid"
        );
    }

    #[test]
    fn a_reply_larger_than_the_bounded_size_is_refused_rather_than_buffered() {
        let server = RespServer::start();
        // A declaration only: the server never sends the body, and a client
        // that had to read it before deciding would wait forever.
        server.always("GET", Reply::Raw(b"$99999999\r\n".to_vec()));
        let store = server.store("quotes");

        let error = store
            .get("AAPL")
            .expect_err("an oversized reply is refused");

        assert_eq!(error.code(), "schema", "{error}");
    }

    #[test]
    fn a_malformed_reply_from_a_real_socket_is_refused_rather_than_panicking() {
        let server = RespServer::start();
        server.always("GET", Reply::Raw(b"!not-resp\r\n".to_vec()));
        let store = server.store("quotes");

        let error = store.get("AAPL").expect_err("a malformed reply is refused");

        assert_eq!(error.code(), "schema", "{error}");
        assert!(error.message().contains("RESP2"), "{error}");
    }

    #[test]
    fn a_value_that_is_not_the_json_this_store_writes_is_refused_rather_than_guessed_at() {
        let server = RespServer::start();
        server.always("GET", bulk("not json at all"));
        let store = server.store("quotes");

        let error = store.get("AAPL").expect_err("a non-JSON value is refused");

        assert_eq!(error.code(), "schema", "{error}");
    }

    #[test]
    fn a_server_that_accepts_a_command_and_says_nothing_fails_with_a_timeout() {
        let server = RespServer::start();
        server.always("GET", Reply::Silent(Duration::from_secs(3)));
        let store = server.store("quotes");

        let started = std::time::Instant::now();
        let error = store.get("AAPL").expect_err("silence is a timeout");

        assert_eq!(error.code(), "timeout", "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the wait was bounded by the read timeout, not by the server: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_health_check_refuses_anything_that_is_not_pong() {
        let server = RespServer::start();
        server.always("PING", bulk("PONG"));

        let error = RedisKeyValueStore::connect(server.config(), "quotes")
            .expect_err("a bulk PONG is not the reply RESP2 PING gives");

        assert_eq!(error.code(), "io", "{error}");
        assert!(error.message().contains("PING"), "{error}");
    }

    #[test]
    fn a_refused_connection_is_reported_as_io_rather_than_as_missing_configuration() {
        // The distinction an operator acts on: "nothing is listening" sends
        // them to the VPC, "no address configured" sends them to the
        // deployment manifest.
        let config = RedisConfig::at(address_with_no_listener()).expect("a valid address");

        let error = RedisKeyValueStore::connect(config, "quotes")
            .expect_err("nothing is listening on that port");

        assert_eq!(error.code(), "io", "{error}");
        assert!(error.message().contains("Memorystore"), "{error}");
    }

    #[test]
    fn a_pooled_connection_that_the_server_has_closed_is_reopened_once() {
        let server = RespServer::start();
        server.once("GET", Reply::Close);
        server.always("GET", bulk("{\"quote\":1}"));
        let store = server.store("quotes");
        assert_eq!(
            server.connections(),
            1,
            "the premise: connect pooled exactly one connection"
        );

        let value = store
            .get("AAPL")
            .expect("the retry on a fresh connection succeeds");

        assert_eq!(value, Some(serde_json::json!({ "quote": 1 })));
        assert_eq!(
            server.connections(),
            2,
            "a second connection was opened, which is the retry"
        );
    }

    #[test]
    fn a_timeout_is_not_retried_because_the_caller_already_declined_to_wait_that_long() {
        let server = RespServer::start();
        server.always("GET", Reply::Silent(Duration::from_secs(3)));
        let store = server.store("quotes");
        assert_eq!(
            server.connections(),
            1,
            "the premise: one pooled connection"
        );

        let error = store.get("AAPL").expect_err("silence is a timeout");

        assert_eq!(error.code(), "timeout", "{error}");
        assert_eq!(
            server.connections(),
            1,
            "no second connection: a slow server is not a stale socket"
        );
    }

    // --- durability, told truthfully ---------------------------------------

    #[test]
    fn the_cache_adapter_reports_that_it_is_not_crash_safe() {
        let server = RespServer::start();
        let store = server.store("quotes");

        assert!(
            !store.is_crash_safe(),
            "a cache with persistence disabled must never claim otherwise"
        );
        assert!(
            RedisKeyValueStore::DURABILITY_NOTICE.contains("can be lost"),
            "the notice a caller quotes has to say so in words too"
        );
        assert!(
            format!("{store:?}").contains("durability"),
            "and it is visible on the type held behind Arc<dyn KeyValueStore>"
        );
    }

    #[test]
    fn memorystore_has_an_adapter_and_still_reports_that_nothing_it_holds_survives_a_restart() {
        assert!(
            StorageTarget::Memorystore.is_implemented(),
            "the RESP adapter is compiled into this build"
        );
        assert!(
            !StorageTarget::Memorystore.is_crash_safe(),
            "which changes nothing about durability: persistence_mode is DISABLED on the instance"
        );
        let settings = crate::StorageSettings::from_values(Some("memorystore"), None)
            .expect("memorystore resolves without a root");
        assert!(!settings.is_durable());
        let banner = settings.banner_lines(&["hot quotes"], &[]);
        assert!(
            banner
                .iter()
                .any(|line| line.contains("NOTHING SURVIVES A RESTART")),
            "the start-up banner an operator reads says it outright: {banner:?}"
        );
        assert!(
            banner
                .iter()
                .any(|line| line.contains("persists:         nothing")),
            "and does not claim to persist what it was told it persists: {banner:?}"
        );
    }

    #[test]
    fn the_unconfigured_adapter_opens_no_connection() {
        // A listener that would accept anything, and is never told to. If the
        // refusal path touched the network at all, this would catch it.
        let witness = RespServer::start();

        let from_values = RedisConfig::from_values(None, None)
            .expect_err("no address means no Memorystore to talk to");
        assert_eq!(from_values.code(), "unavailable", "{from_values}");
        assert!(
            from_values.message().contains(ADDRESS_VARIABLE),
            "the refusal names the variable to set: {from_values}"
        );
        assert!(
            from_values
                .message()
                .contains("docs/operations/external-dependencies.md"),
            "and where to read about it: {from_values}"
        );

        let from_provider = StorageProvider::new(StorageTarget::Memorystore, "/tmp")
            .key_value("quotes")
            .expect_err("an unconfigured provider builds no store");
        assert_eq!(from_provider.code(), "unavailable", "{from_provider}");

        assert_eq!(
            witness.connections(),
            0,
            "neither refusal opened a socket to anything"
        );
    }

    #[test]
    fn a_store_built_from_settings_authenticates_with_the_string_the_composition_root_resolved() {
        // The whole path a binary takes: an environment the root looks
        // variables up in, `StorageSettings::from_env`, the provider, and a
        // connection. Until this crate stopped reading `std::env` itself, the
        // AUTH string was fetched three calls below the root at the moment the
        // adapter was built, which is why no test could drive this path
        // without mutating the process environment — and why a deployment
        // could not mount the string as a file.
        let server = RespServer::start();
        server.always("AUTH", ok());
        let environment = |name: &str| -> Option<String> {
            match name {
                crate::settings::TARGET_VARIABLE => Some("memorystore".to_string()),
                ADDRESS_VARIABLE => Some(server.address.clone()),
                AUTH_VARIABLE => Some("the-string-the-root-resolved".to_string()),
                _ => None,
            }
        };

        let settings = crate::StorageSettings::from_values(Some("memorystore"), None)
            .expect("the premise: memorystore resolves without a root");
        assert!(
            settings.managed().is_empty(),
            "the premise: from_values alone carries no credential, so whatever the store \
             authenticates with below came through from_env"
        );

        let settings = crate::StorageSettings::from_env(&environment)
            .expect("an address and an AUTH string are a complete Memorystore configuration");
        assert!(
            settings
                .managed()
                .redis_config()
                .is_ok_and(|c| c.is_authenticated()),
            "the resolved settings carry the credential: {:?}",
            settings.managed()
        );
        let store = settings
            .key_value("quotes")
            .expect("the provider builds the store from what it was given");
        drop(store);

        let commands = server.commands();
        assert_eq!(
            commands.first(),
            Some(&vec![
                "AUTH".to_string(),
                "the-string-the-root-resolved".to_string()
            ]),
            "the first command on the wire is AUTH with the string the root resolved: \
             {commands:?}"
        );
    }

    #[test]
    fn memorystore_still_names_what_a_deployment_supplies_even_though_the_adapter_is_built_in() {
        let requirement = StorageTarget::Memorystore
            .required_configuration()
            .expect("a deployment still has to supply an instance");
        for expected in [ADDRESS_VARIABLE, AUTH_VARIABLE, "VPC", "TLS"] {
            assert!(
                requirement.contains(expected),
                "the requirement should mention {expected}: {requirement}"
            );
        }
    }

    #[test]
    fn the_ports_that_cannot_be_built_here_say_why_and_not_merely_that_credentials_are_missing() {
        // Kept beside the adapter because the contrast is the assertion: one
        // of these four ports became an adapter, and the other three did not,
        // and a reader is owed the reason rather than a shrug.
        for (target, expected) in [
            (StorageTarget::Bigtable, ["gRPC", "protobuf", "HTTP/2"]),
            (StorageTarget::Spanner, ["session", "gRPC", "REST"]),
            (
                StorageTarget::AlloyDb,
                ["PostgreSQL wire protocol", "SCRAM", "admin-only"],
            ),
        ] {
            let requirement = target
                .required_configuration()
                .unwrap_or_else(|| panic!("{target:?} must state what it needs"));
            for fragment in expected {
                assert!(
                    requirement.contains(fragment),
                    "{target:?} should explain {fragment}: {requirement}"
                );
            }
            assert!(
                requirement.len() > 200,
                "{target:?} needs enough words to let a reader decide what to do next: \
                 {requirement}"
            );
        }
    }
}
