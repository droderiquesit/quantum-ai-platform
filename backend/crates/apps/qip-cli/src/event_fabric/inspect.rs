//! ADR 0100 §1: `qip event-fabric health` and `qip event-fabric replay`, the
//! two read views of a partition (R12-FABRIC-7042, ADR 0156 §4b).
//!
//! * **health** asks the broker's `metadata` route and prints what the broker
//!   computed: the leader epoch, the high watermark, `archived_through` and
//!   whether an operator has isolated the partition. Nothing is recomputed
//!   here, so the command and the broker cannot disagree. A response that
//!   does not carry the isolation field is **refused**, not printed: a view
//!   that read a missing field as "not isolated" would show a parked
//!   partition as healthy whenever the broker predated the field.
//! * **replay** fetches a partition from an offset through the same
//!   transport a consumer uses, decodes every batch and checks the chain the
//!   broker wrote: each batch's `previous_batch_hash` must be the SHA-256 of
//!   the batch before it as it travelled. It prints a verdict in
//!   [`crate::replay`]'s discipline, `AGREES` (exit 0) or `DIFFERS` (exit 3),
//!   and re-drives nothing: it reads the fabric and writes nothing to it.
//!
//! The peer must be loopback and the token a file, for the reasons
//! [`super::admin`] gives. The mirror-lag view is not here: the mirror is not
//! built (ADR 0119), and its checkpoint is what that view will read.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, ContentHash, PrefixDecodeOutcome};
use qip_transport::event_fabric::protocol::{
    FetchRequest, MetadataRequest, Refusal, Request, Response,
};
use qip_transport::event_fabric::transport::Timeouts;

use super::admin::{local_peer, read_token};
use super::{Environment, Outcome};

/// `qip event-fabric health`.
pub const HEALTH: &str = "health";
/// `qip event-fabric replay`.
pub const REPLAY: &str = "replay";

/// Every batch is in the range and links to the one before it.
pub const AGREES: u8 = 0;
/// A partition that is isolated (health) or a broken link (replay). Three,
/// as `qip replay`'s DIFFERS is.
pub const DIFFERS: u8 = 3;

/// The most batches one replay reads before it stops and says so. A bound
/// because an unbounded read of a partition is a read of everything it
/// retains, and the operator who wants more can name a later `--from`.
pub const MAX_BATCHES: u64 = 10_000;

/// The bytes of credit each fetch carries.
const FETCH_CREDIT: u32 = 1024 * 1024;

fn usage(action: &str) -> String {
    let extra = if action == REPLAY {
        " [--from <offset>] [--max-batches <n>]"
    } else {
        ""
    };
    format!(
        "usage: qip event-fabric {action} --peer <loopback-address:port> --stream <name> \
         --partition <number>{extra}"
    )
}

struct Arguments {
    peer: String,
    stream: String,
    partition: u32,
    from: u64,
    max_batches: u64,
}

fn parse_arguments(action: &str, arguments: &[String]) -> Result<Arguments> {
    let flags: &[&str] = if action == REPLAY {
        &[
            "--peer",
            "--stream",
            "--partition",
            "--from",
            "--max-batches",
        ]
    } else {
        &["--peer", "--stream", "--partition"]
    };
    let mut found: BTreeMap<&str, &str> = BTreeMap::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let Some(flag) = flags.iter().find(|flag| **flag == argument.as_str()) else {
            return Err(Error::invalid(format!(
                "unknown argument {argument:?}; {}",
                usage(action)
            )));
        };
        let Some(value) = rest.next() else {
            return Err(Error::invalid(format!(
                "{flag} needs a value; {}",
                usage(action)
            )));
        };
        if found.insert(flag, value.as_str()).is_some() {
            return Err(Error::invalid(format!(
                "{flag} was given twice; {}",
                usage(action)
            )));
        }
    }
    let required = |flag: &str| {
        found
            .get(flag)
            .filter(|value| !value.trim().is_empty())
            .map(|value| (*value).to_string())
            .ok_or_else(|| Error::invalid(format!("{flag} is required; {}", usage(action))))
    };
    let number = |flag: &str, value: &str| {
        value.parse::<u64>().map_err(|_| {
            Error::invalid(format!(
                "{flag} must be a whole number, not {value:?}; {}",
                usage(action)
            ))
        })
    };
    let partition_text = required("--partition")?;
    let partition = u32::try_from(number("--partition", &partition_text)?).map_err(|_| {
        Error::invalid(format!(
            "--partition {partition_text} is not a partition number; {}",
            usage(action)
        ))
    })?;
    let from = match found.get("--from") {
        Some(value) => number("--from", value)?,
        None => 0,
    };
    let max_batches = match found.get("--max-batches") {
        Some(value) => {
            let n = number("--max-batches", value)?;
            if n == 0 || n > MAX_BATCHES {
                return Err(Error::invalid(format!(
                    "--max-batches must be between 1 and {MAX_BATCHES}, not {n}; {}",
                    usage(action)
                )));
            }
            n
        }
        None => MAX_BATCHES,
    };
    Ok(Arguments {
        peer: required("--peer")?,
        stream: required("--stream")?,
        partition,
        from,
        max_batches,
    })
}

/// `qip event-fabric health …` or `qip event-fabric replay …`.
pub fn run(action: &str, arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(action, arguments)?;
    let peer = local_peer(&arguments.peer)?;
    let token = read_token(environment)?;
    let mut transport = environment.connect(peer, token);
    let at = format!("{}:{}", arguments.stream, arguments.partition);
    match action {
        HEALTH => {
            let answer = transport.call(
                Request::Metadata(MetadataRequest {
                    stream: arguments.stream.clone(),
                    partition: arguments.partition,
                }),
                Timeouts::default(),
            )?;
            match answer {
                Response::Metadata(metadata) => {
                    let Some(isolation) = metadata.isolation() else {
                        return Err(Error::invalid(format!(
                            "the broker's metadata answer for {at} carries no isolation \
                             field, so whether the partition is parked is not known. Refusing \
                             to print it as healthy; run a broker that reports isolation"
                        )));
                    };
                    let state = if isolation.isolated {
                        format!(
                            "ISOLATED by {}: {}",
                            isolation.operator.as_deref().unwrap_or("unnamed"),
                            isolation.reason.as_deref().unwrap_or("no reason recorded")
                        )
                    } else {
                        "not isolated".to_string()
                    };
                    Ok(Outcome {
                        lines: vec![format!(
                            "HEALTH {at}: leader epoch {}, high watermark {}, archived through \
                             {}, {state}",
                            metadata.leader_epoch(),
                            metadata.high_watermark(),
                            metadata.archived_through()
                        )],
                        code: if isolation.isolated { DIFFERS } else { AGREES },
                    })
                }
                Response::Refused(Refusal::AclDenied | Refusal::KeyOutOfScope) => {
                    Err(Error::denied(format!(
                        "the broker refused: the identity this token verifies as holds no grant \
                         covering {at}. Add one to the stream catalogue's grants, or use the \
                         token of an identity that holds one"
                    )))
                }
                other => Err(Error::invalid(format!(
                    "the broker answered the health of {at} with {other:?}, which is not an \
                     answer to it"
                ))),
            }
        }
        REPLAY => replay(&mut *transport, &arguments, &at),
        other => Err(Error::invalid(format!(
            "unknown inspect view {other:?}; the views are {HEALTH} and {REPLAY}"
        ))),
    }
}

fn replay(
    transport: &mut (dyn qip_transport::event_fabric::transport::FabricTransport + Send),
    arguments: &Arguments,
    at: &str,
) -> Result<Outcome> {
    let mut offset = arguments.from;
    let mut read: u64 = 0;
    let mut previous: Option<(u64, ContentHash)> = None;
    let mut first_offset = None;
    let mut high_watermark = 0;
    loop {
        if read >= arguments.max_batches {
            break;
        }
        let answer = transport.call(
            Request::Fetch(FetchRequest {
                stream: arguments.stream.clone(),
                partition: arguments.partition,
                offset,
                max_bytes: FETCH_CREDIT,
            }),
            Timeouts::default(),
        )?;
        let fetched = match answer {
            Response::Fetch(fetched) => fetched,
            Response::Refused(Refusal::AclDenied | Refusal::KeyOutOfScope) => {
                return Err(Error::denied(format!(
                    "the broker refused: the identity this token verifies as holds no consume \
                     grant covering {at}"
                )));
            }
            other => {
                return Err(Error::invalid(format!(
                    "the broker answered a fetch of {at} with {other:?}, which is not an \
                     answer to it"
                )));
            }
        };
        high_watermark = fetched.high_watermark();
        let bytes = qip_core::hash::from_hex(fetched.batches())
            .ok_or_else(|| Error::invalid("the broker's fetch carried batches that are not hex"))?;
        if bytes.is_empty() {
            break;
        }
        let mut cursor = &bytes[..];
        while !cursor.is_empty() && read < arguments.max_batches {
            let (batch, consumed) = match Batch::decode_prefix(cursor)? {
                PrefixDecodeOutcome::Complete { batch, consumed } => (batch, consumed),
                PrefixDecodeOutcome::Torn => {
                    return Ok(differs(
                        at,
                        format!("the batch after offset {offset} arrived torn"),
                    ));
                }
            };
            let encoded = batch.encode()?;
            let base = batch.base_offset;
            if first_offset.is_none() {
                first_offset = Some(base);
            }
            match &previous {
                Some((previous_base, previous_hash)) => {
                    if batch.previous_batch_hash.as_ref() != Some(previous_hash) {
                        return Ok(differs(
                            at,
                            format!(
                                "the batch at offset {base} does not chain to the batch at \
                                 offset {previous_base}"
                            ),
                        ));
                    }
                }
                None if base == 0 => {
                    // The first batch of a partition has nothing before it.
                    if batch.previous_batch_hash.is_some() {
                        return Ok(differs(
                            at,
                            "the batch at offset 0 claims a predecessor".to_string(),
                        ));
                    }
                }
                None => {}
            }
            previous = Some((base, ContentHash::sha256_of(&encoded)));
            read += 1;
            // Offsets count batches, as a consumer's do (`advance_past`).
            offset = base.saturating_add(1);
            cursor = &cursor[consumed..];
        }
    }
    let Some(first) = first_offset else {
        return Err(Error::not_found(format!(
            "{at} holds no batch at or after offset {}; the high watermark is {high_watermark}. \
             Nothing was verified, which is not the same as agreeing",
            arguments.from
        )));
    };
    let anchored = if first == 0 {
        "anchored at the partition's first batch"
    } else {
        "the first batch's own predecessor is outside the range and was not checked"
    };
    Ok(Outcome {
        lines: vec![format!(
            "AGREES {at}: {read} batch(es) from offset {first} chain unbroken; {anchored} \
             (high watermark {high_watermark})"
        )],
        code: AGREES,
    })
}

fn differs(at: &str, why: String) -> Outcome {
    Outcome {
        lines: vec![format!("DIFFERS {at}: {why}")],
        code: DIFFERS,
    }
}
