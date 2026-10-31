//! `qip_transport::dns` — the fabric client's SRV bootstrap (ADR 0135).
//!
//! The broker module publishes `_qip-event-fabric._tcp.<zone>` as an SRV
//! record pointing at the brokers' A record, and until this module nothing
//! read it: every client was handed one endpoint by configuration and a
//! broker added or replaced in a region was found by redeploying every
//! client (R12-FABRIC-10016, R12-FABRIC-8098). `std` resolves A and AAAA
//! only, and no crate on the allow-list does SRV, so the query is written
//! here, in the one library licensed to own sockets (ADR 0100).
//!
//! Three pieces, each usable without the others:
//!
//! * [`encode_query`] and [`decode_answer`] — one RFC 1035 message with a
//!   QTYPE SRV question (RFC 2782), and its answer's SRV records with any A
//!   records the additional section carries. Pure functions over bytes, so
//!   the tests drive them with hand-built wire vectors.
//! * [`SrvResolver`] — the UDP exchange with the host's configured resolver,
//!   with an explicit timeout and a bounded number of attempts.
//! * [`Bootstrap`] — the fallback ladder: the answer just resolved, the last
//!   good answer while it is not too stale, then the configured addresses,
//!   which stay required so a client with no DNS at all still starts. Which
//!   rung answered is an [`AddressSource`], meant for a metric label.
//!
//! # Refuse rather than guess
//!
//! The decoder refuses a truncated answer (TC set) instead of falling back
//! to TCP: a three-broker region fits a UDP message, and a truncated one is
//! a configuration fault to surface, not to route around. It refuses an
//! answer whose ID is not one this query sent or whose question does not
//! match, a compression pointer that points forward or revisits an offset
//! (a loop), a label type that is neither plain nor a pointer, a name over
//! 253 characters, a response code other than no-error, and a target of
//! `.`, which RFC 2782 defines as "the service is decidedly not available".
//! Every refusal names what it saw. A plain label cannot exceed 63 octets
//! on the wire — the two high bits of a longer length are a label type — so
//! that limit is enforced where a name is encoded, not where one is read.
//!
//! # Discovery is never trust
//!
//! A name from DNS selects an address and nothing more. The mTLS handshake
//! still authenticates the broker by its certificate (ADR 0112), so a
//! poisoned answer can make a client fail to connect and cannot make it talk
//! to an unauthenticated broker. That is why the query ID is drawn from a
//! process-seeded hasher rather than a cryptographic source, and why this
//! module validates no signatures: it is not where trust lives.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use qip_core::error::{Error, Result};
use qip_core::time::{Clock, Timestamp};

/// The service label the broker module publishes
/// (`infrastructure/terraform/modules/event-fabric-broker/main.tf`,
/// `google_dns_record_set.event_fabric_broker_srv_tcp`). The owner name a
/// client queries is this followed by the zone — see [`fabric_srv_owner`].
pub const FABRIC_SRV_SERVICE: &str = "_qip-event-fabric._tcp";

/// The SRV owner name for the fabric in `zone` — `zone` with or without its
/// trailing dot, so a value copied from a zone resource and one typed by
/// hand both work.
pub fn fabric_srv_owner(zone: &str) -> String {
    let zone = zone.trim().trim_end_matches('.');
    format!("{FABRIC_SRV_SERVICE}.{zone}")
}

/// QTYPE SRV (RFC 2782).
const TYPE_SRV: u16 = 33;
/// TYPE A (RFC 1035 §3.2.2).
const TYPE_A: u16 = 1;
/// CLASS IN.
const CLASS_IN: u16 = 1;
/// The largest classic DNS message over UDP (RFC 1035 §4.2.1). No EDNS is
/// negotiated, so a server truncates past this and sets TC, which the
/// decoder refuses.
pub const MAX_UDP_MESSAGE: usize = 512;
/// RFC 1035 §2.3.4: a label is at most 63 octets and a name 255 on the wire,
/// which is 253 in presentation form.
const MAX_LABEL: usize = 63;
const MAX_NAME: usize = 253;
/// How many compression pointers one name may follow. Every pointer must
/// point backward and may not revisit an offset, so a chain this long is a
/// strictly descending walk through the message that no honest answer
/// contains; the name-length limit would catch it anyway, and this names
/// the fault for what it is.
const MAX_POINTERS: usize = 127;

/// One SRV record as RFC 2782 lays it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    /// Lower is tried first.
    pub priority: u16,
    /// Among equal priorities, higher is preferred; this module orders by
    /// weight descending rather than drawing, so a replay is a replay.
    pub weight: u16,
    pub port: u16,
    /// The target host, lower-cased, without a trailing dot.
    pub target: String,
}

/// A decoded SRV answer: the records in the order a client should try them,
/// the A records the additional section volunteered for their targets, and
/// the shortest TTL among the records, which bounds how long the answer may
/// be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvAnswer {
    pub records: Vec<SrvRecord>,
    /// Target name (lower-cased) to the IPv4 addresses the answer carried
    /// for it. Empty for a target the answer did not resolve; the client
    /// then connects by name.
    pub addresses: BTreeMap<String, Vec<Ipv4Addr>>,
    pub ttl: Duration,
}

impl SrvAnswer {
    /// The `host:port` authorities to try, in record order, each target
    /// expanded to the addresses the answer carried for it or kept as a
    /// name when it carried none.
    pub fn authorities(&self) -> Vec<String> {
        let mut out = Vec::new();
        for record in &self.records {
            match self.addresses.get(&record.target) {
                Some(addresses) if !addresses.is_empty() => {
                    for address in addresses {
                        out.push(format!("{address}:{}", record.port));
                    }
                }
                _ => out.push(format!("{}:{}", record.target, record.port)),
            }
        }
        out
    }
}

/// One RFC 1035 query message asking for `owner`'s SRV records, class IN,
/// recursion desired, under `id`. Refuses an owner with an empty label, a
/// label over 63 octets or a name over 253.
pub fn encode_query(id: u16, owner: &str) -> Result<Vec<u8>> {
    let mut message = Vec::with_capacity(12 + owner.len() + 6);
    message.extend_from_slice(&id.to_be_bytes());
    // QR=0 OPCODE=0 AA=0 TC=0 RD=1 | RA=0 Z=0 RCODE=0
    message.extend_from_slice(&0x0100u16.to_be_bytes());
    message.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    message.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    message.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    message.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    write_name(&mut message, owner)?;
    message.extend_from_slice(&TYPE_SRV.to_be_bytes());
    message.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(message)
}

fn write_name(out: &mut Vec<u8>, name: &str) -> Result<()> {
    let name = name.trim_end_matches('.');
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(Error::invalid(format!(
            "the DNS name '{name}' is empty or over {MAX_NAME} characters"
        )));
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > MAX_LABEL {
            return Err(Error::invalid(format!(
                "the DNS name '{name}' has a label that is empty or over {MAX_LABEL} octets"
            )));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

/// Reads a possibly compressed name at `at` and returns it lower-cased with
/// the offset just past it in the message (past the first pointer, when
/// there is one). Refuses a pointer that does not point backward from where
/// it stands, a pointer to an offset this name already visited (a loop: a
/// label followed by a pointer back to that label passes the backward check
/// and would otherwise cycle until the length limit), an unreasonably long
/// pointer chain, a label type other than plain or pointer, a name over 253,
/// or a name running off the end of the message.
pub fn read_name(message: &[u8], at: usize) -> Result<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut length = 0usize;
    let mut position = at;
    let mut next = None;
    let mut visited: Vec<usize> = Vec::new();
    loop {
        let &len = message
            .get(position)
            .ok_or_else(|| Error::invalid("a DNS name runs past the end of the message"))?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    let end = next.unwrap_or(position + 1);
                    return Ok((labels.join("."), end));
                }
                let len = len as usize;
                let label = message
                    .get(position + 1..position + 1 + len)
                    .ok_or_else(|| {
                        Error::invalid("a DNS label runs past the end of the message")
                    })?;
                length += len + usize::from(!labels.is_empty());
                if length > MAX_NAME {
                    return Err(Error::invalid(format!(
                        "a DNS name is over {MAX_NAME} characters"
                    )));
                }
                labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
                position += 1 + len;
            }
            0xC0 => {
                let &low = message.get(position + 1).ok_or_else(|| {
                    Error::invalid("a DNS compression pointer runs past the end of the message")
                })?;
                let target = (usize::from(len & 0x3F) << 8) | usize::from(low);
                if target >= position {
                    return Err(Error::invalid(format!(
                        "a DNS compression pointer at offset {position} points forward to \
                         {target}; a pointer may only refer to an earlier occurrence"
                    )));
                }
                if visited.contains(&target) {
                    return Err(Error::invalid(format!(
                        "a DNS compression pointer at offset {position} returns to offset \
                         {target}, which this name already read; the answer loops"
                    )));
                }
                visited.push(target);
                if visited.len() > MAX_POINTERS {
                    return Err(Error::invalid(
                        "a DNS name follows more compression pointers than a name can have \
                         labels; no honest answer is built this way",
                    ));
                }
                next.get_or_insert(position + 2);
                position = target;
            }
            other => {
                return Err(Error::invalid(format!(
                    "a DNS label type {other:#04x} is neither plain nor a compression pointer"
                )));
            }
        }
    }
}

fn read_u16(message: &[u8], at: usize) -> Result<u16> {
    let bytes = message
        .get(at..at + 2)
        .ok_or_else(|| Error::invalid("a DNS field runs past the end of the message"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(message: &[u8], at: usize) -> Result<u32> {
    let bytes = message
        .get(at..at + 4)
        .ok_or_else(|| Error::invalid("a DNS field runs past the end of the message"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Decodes the answer to the query [`encode_query`] built under `id` for
/// `owner`. Refuses, naming what it saw: an ID that is not `id`; a message
/// that is not a response; a truncated answer; a response code other than
/// no-error; a question count other than one, or a question that is not
/// `owner` SRV IN; and every malformation [`read_name`] refuses. Records of
/// other types and classes in the answer are skipped by their declared
/// length; A records in the additional section are kept by owner.
pub fn decode_answer(id: u16, owner: &str, message: &[u8]) -> Result<SrvAnswer> {
    if message.len() < 12 {
        return Err(Error::invalid(format!(
            "a DNS message of {} bytes is shorter than its 12-byte header",
            message.len()
        )));
    }
    let seen_id = read_u16(message, 0)?;
    if seen_id != id {
        return Err(Error::invalid(format!(
            "the DNS answer carries ID {seen_id:#06x} and the query was {id:#06x}; it answers \
             another question"
        )));
    }
    let flags = read_u16(message, 2)?;
    if flags & 0x8000 == 0 {
        return Err(Error::invalid("the DNS message is a query, not a response"));
    }
    if flags & 0x0200 != 0 {
        return Err(Error::invalid(format!(
            "the DNS answer is truncated (TC set): it does not fit {MAX_UDP_MESSAGE} bytes, \
             which for a fabric region means more brokers than the record should name"
        )));
    }
    let rcode = flags & 0x000F;
    if rcode != 0 {
        let meaning = match rcode {
            1 => "format error",
            2 => "server failure",
            3 => "no such name",
            4 => "not implemented",
            5 => "refused",
            _ => "an unassigned code",
        };
        return Err(Error::unavailable(format!(
            "the resolver answered RCODE {rcode} ({meaning}) for '{owner}' SRV"
        )));
    }
    let qdcount = read_u16(message, 4)?;
    let ancount = read_u16(message, 6)?;
    let nscount = read_u16(message, 8)?;
    let arcount = read_u16(message, 10)?;
    if qdcount != 1 {
        return Err(Error::invalid(format!(
            "the DNS answer carries {qdcount} questions and the query asked one"
        )));
    }
    let (qname, mut at) = read_name(message, 12)?;
    let qtype = read_u16(message, at)?;
    let qclass = read_u16(message, at + 2)?;
    at += 4;
    let expected = owner.trim_end_matches('.').to_ascii_lowercase();
    if qname != expected || qtype != TYPE_SRV || qclass != CLASS_IN {
        return Err(Error::invalid(format!(
            "the DNS answer's question is '{qname}' type {qtype} class {qclass}; the query was \
             '{expected}' type {TYPE_SRV} class {CLASS_IN}"
        )));
    }

    let mut records = Vec::new();
    let mut addresses: BTreeMap<String, Vec<Ipv4Addr>> = BTreeMap::new();
    let mut ttl: Option<u32> = None;
    let mut unavailable_marker = false;
    let total = usize::from(ancount) + usize::from(nscount) + usize::from(arcount);
    for index in 0..total {
        let (name, after_name) = read_name(message, at)?;
        let rtype = read_u16(message, after_name)?;
        let rclass = read_u16(message, after_name + 2)?;
        let rttl = read_u32(message, after_name + 4)?;
        let rdlength = usize::from(read_u16(message, after_name + 8)?);
        let rdata_at = after_name + 10;
        let rdata_end = rdata_at + rdlength;
        if rdata_end > message.len() {
            return Err(Error::invalid(format!(
                "a DNS record's data of {rdlength} bytes runs past the end of the message"
            )));
        }
        let in_answer = index < usize::from(ancount);
        if rclass == CLASS_IN && rtype == TYPE_SRV && in_answer && name == expected {
            if rdlength < 7 {
                return Err(Error::invalid(format!(
                    "an SRV record's data of {rdlength} bytes is shorter than its three \
                     fields and a target"
                )));
            }
            let priority = read_u16(message, rdata_at)?;
            let weight = read_u16(message, rdata_at + 2)?;
            let port = read_u16(message, rdata_at + 4)?;
            let (target, target_end) = read_name(message, rdata_at + 6)?;
            if target_end != rdata_end {
                return Err(Error::invalid(format!(
                    "an SRV record's target ends at byte {target_end} of a record declared to \
                     end at {rdata_end}; the record is malformed"
                )));
            }
            if target.is_empty() {
                // RFC 2782: a target of "." means the service is decidedly
                // not available at this domain. Not an address, and not
                // silently an empty one. The record is still stepped over.
                unavailable_marker = true;
                at = rdata_end;
                continue;
            }
            ttl = Some(ttl.map_or(rttl, |t| t.min(rttl)));
            records.push(SrvRecord {
                priority,
                weight,
                port,
                target,
            });
        } else if rclass == CLASS_IN && rtype == TYPE_A && rdlength == 4 {
            let bytes = &message[rdata_at..rdata_end];
            addresses
                .entry(name)
                .or_default()
                .push(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
        }
        at = rdata_end;
    }
    if records.is_empty() {
        if unavailable_marker {
            return Err(Error::unavailable(format!(
                "the zone publishes '{owner}' with target '.', which RFC 2782 defines as the \
                 service being decidedly not available there"
            )));
        }
        return Err(Error::unavailable(format!(
            "the resolver answered no SRV record for '{owner}'; the broker module's record is \
             not published in this zone, or the zone is not the one configured"
        )));
    }
    // Priority ascending, weight descending, then target and port, so two
    // clients reading one answer try the brokers in the same order.
    records.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(b.weight.cmp(&a.weight))
            .then(a.target.cmp(&b.target))
            .then(a.port.cmp(&b.port))
    });
    Ok(SrvAnswer {
        records,
        addresses,
        ttl: Duration::from_secs(u64::from(ttl.unwrap_or(0))),
    })
}

/// The UDP exchange with one resolver. Every call has an explicit timeout
/// and at most `attempts` sends; an attempt that times out is retried. An
/// answer to any query this call sent is accepted — a resolver that is
/// slower than one timeout answers the first query while the second is
/// pending, and refusing that answer for carrying the first ID would turn
/// the retry into a way to fail — and any datagram carrying an ID this call
/// never sent, or that fails to decode, is a refusal, not a retry: a stray
/// or forged datagram is surfaced, never waited past.
#[derive(Debug)]
pub struct SrvResolver {
    resolver: SocketAddr,
    timeout: Duration,
    attempts: u8,
    hasher: std::hash::RandomState,
}

impl SrvResolver {
    /// `resolver` is the host's configured nameserver; the composition root
    /// reads it (from its configuration or the host's `resolv.conf`) and
    /// hands it in, because this library reads no environment and no file.
    /// Refuses a zero timeout or zero attempts: either would make every
    /// query fail at once while looking like a resolver outage.
    pub fn new(resolver: SocketAddr, timeout: Duration, attempts: u8) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::invalid(
                "the SRV resolver's timeout is zero; every query would time out before it \
                 was sent",
            ));
        }
        if attempts == 0 {
            return Err(Error::invalid(
                "the SRV resolver is allowed zero attempts; it could never ask",
            ));
        }
        Ok(Self {
            resolver,
            timeout,
            attempts,
            hasher: std::hash::RandomState::new(),
        })
    }

    pub fn resolver(&self) -> SocketAddr {
        self.resolver
    }

    fn next_id(&self, attempt: u8) -> u16 {
        let mut hasher = self.hasher.build_hasher();
        hasher.write_u8(attempt);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        (hasher.finish() & 0xFFFF) as u16
    }

    /// Asks the resolver for `owner`'s SRV records.
    pub fn query(&self, owner: &str) -> Result<SrvAnswer> {
        let bind: SocketAddr = if self.resolver.is_ipv4() {
            "0.0.0.0:0".parse().map_err(|e| Error::io(format!("{e}")))?
        } else {
            "[::]:0".parse().map_err(|e| Error::io(format!("{e}")))?
        };
        let socket = UdpSocket::bind(bind)
            .map_err(|e| Error::io(format!("cannot open a UDP socket for the SRV query: {e}")))?;
        socket
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| socket.set_write_timeout(Some(self.timeout)))
            .map_err(|e| Error::io(format!("cannot set the SRV query's timeout: {e}")))?;
        socket.connect(self.resolver).map_err(|e| {
            Error::io(format!(
                "cannot address the resolver {} for the SRV query: {e}",
                self.resolver
            ))
        })?;
        let mut last_timeout = None;
        let mut sent: Vec<u16> = Vec::with_capacity(usize::from(self.attempts));
        for attempt in 0..self.attempts {
            let id = self.next_id(attempt);
            let query = encode_query(id, owner)?;
            socket.send(&query).map_err(|e| {
                Error::io(format!(
                    "cannot send the SRV query to the resolver {}: {e}",
                    self.resolver
                ))
            })?;
            sent.push(id);
            let mut buffer = [0u8; MAX_UDP_MESSAGE];
            match socket.recv(&mut buffer) {
                Ok(read) => {
                    let answer = &buffer[..read];
                    let answered = read_u16(answer, 0)?;
                    let Some(&matching) = sent.iter().find(|&&sent_id| sent_id == answered) else {
                        return Err(Error::invalid(format!(
                            "the DNS answer carries ID {answered:#06x}, which this query never \
                             sent ({} sent); it answers another question",
                            sent.len()
                        )));
                    };
                    return decode_answer(matching, owner, answer);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    last_timeout = Some(e);
                }
                Err(e) => {
                    return Err(Error::io(format!(
                        "the SRV query to the resolver {} failed: {e}",
                        self.resolver
                    )));
                }
            }
        }
        Err(Error::timeout(format!(
            "the resolver {} did not answer the SRV query for '{owner}' within {:?} on any of {} \
             attempts{}",
            self.resolver,
            self.timeout,
            self.attempts,
            last_timeout.map(|e| format!(" ({e})")).unwrap_or_default()
        )))
    }
}

/// Which rung of the bootstrap ladder supplied the addresses a client is
/// about to use. Meant as a metric label, so an operator can see a region
/// running on stale discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSource {
    /// The answer resolved on this call.
    Srv,
    /// The last good answer, kept because the resolver failed and the
    /// answer is not yet too stale.
    Cached,
    /// The addresses configuration names, because there is no answer fresh
    /// enough to use.
    Configured,
}

impl AddressSource {
    pub const fn as_label(self) -> &'static str {
        match self {
            AddressSource::Srv => "srv",
            AddressSource::Cached => "cached",
            AddressSource::Configured => "configured",
        }
    }
}

/// What a [`Bootstrap`] resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub source: AddressSource,
    /// `host:port` authorities in the order to try them.
    pub authorities: Vec<String>,
    /// Why the lookup did not answer, when `source` is not
    /// [`AddressSource::Srv`]: a truncated answer and a resolver that never
    /// answered both land a client on a lower rung, and an operator reading
    /// the rung alone could not tell a zone misconfigured from a resolver
    /// down. The ladder degrades; it does not hide what it degraded from.
    pub lookup_failure: Option<String>,
}

#[derive(Debug, Clone)]
struct LastGood {
    answer: SrvAnswer,
    resolved_at: Timestamp,
}

/// The fallback ladder of ADR 0135 decision 4. A resolver outage degrades
/// to the last good answer for at most its TTL times [`Bootstrap::STALENESS_FACTOR`],
/// capped at [`Bootstrap::STALENESS_CAP`], and past that to the configured
/// addresses, which are required: a client with no DNS at all still starts.
#[derive(Debug)]
pub struct Bootstrap {
    configured: Vec<String>,
    last_good: Option<LastGood>,
    clock: Arc<dyn Clock>,
}

impl Bootstrap {
    /// How many TTLs a last good answer may be kept past the moment it was
    /// resolved. Three, because a TTL is the publisher's own promise of how
    /// long the answer holds and a short outage should not throw a region
    /// back onto its static list at the first missed query.
    pub const STALENESS_FACTOR: u32 = 3;
    /// The longest any answer is kept whatever its TTL: an hour. A record
    /// published with a day's TTL would otherwise hide a day-long outage.
    pub const STALENESS_CAP: Duration = Duration::from_secs(3600);

    /// `configured` is the `host:port` list configuration names. Refuses an
    /// empty one: the ladder's last rung must hold something.
    pub fn new(configured: Vec<String>, clock: Arc<dyn Clock>) -> Result<Self> {
        if configured.is_empty() {
            return Err(Error::invalid(
                "the fabric bootstrap has no configured broker address; configured addresses are \
                 required so that a client with no DNS still starts (ADR 0135)",
            ));
        }
        if let Some(blank) = configured.iter().position(|a| a.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "the fabric bootstrap's configured address at position {blank} is blank; a blank \
                 rung is a connect to nowhere, and refusing it beats guessing"
            )));
        }
        Ok(Self {
            configured,
            last_good: None,
            clock,
        })
    }

    /// Runs `lookup` and chooses the rung. A fresh answer replaces the last
    /// good one; a failed lookup keeps the last good one while it is not
    /// too stale; otherwise the configured addresses.
    pub fn resolve(&mut self, lookup: impl FnOnce() -> Result<SrvAnswer>) -> Resolved {
        let now = self.clock.now();
        match lookup() {
            Ok(answer) => {
                let authorities = answer.authorities();
                self.last_good = Some(LastGood {
                    answer,
                    resolved_at: now,
                });
                Resolved {
                    source: AddressSource::Srv,
                    authorities,
                    lookup_failure: None,
                }
            }
            Err(failure) => match &self.last_good {
                Some(last)
                    if now.since(last.resolved_at).as_nanos()
                        <= i64::try_from(Self::staleness(last.answer.ttl).as_nanos())
                            .unwrap_or(i64::MAX) =>
                {
                    Resolved {
                        source: AddressSource::Cached,
                        authorities: last.answer.authorities(),
                        lookup_failure: Some(failure.to_string()),
                    }
                }
                _ => Resolved {
                    source: AddressSource::Configured,
                    authorities: self.configured.clone(),
                    lookup_failure: Some(failure.to_string()),
                },
            },
        }
    }

    /// How long an answer with `ttl` may be kept.
    pub fn staleness(ttl: Duration) -> Duration {
        ttl.saturating_mul(Self::STALENESS_FACTOR)
            .min(Self::STALENESS_CAP)
    }

    /// The configured addresses, the ladder's last rung.
    pub fn configured(&self) -> &[String] {
        &self.configured
    }
}
