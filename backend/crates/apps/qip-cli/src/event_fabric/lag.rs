//! ADR 0100 §1: `qip event-fabric lag`, how far a consumer group is behind
//! one partition (R12-FABRIC-7042).
//!
//! The broker already answers the question on its `group/lag` route — the
//! group's committed offset against the partition's high watermark — and a
//! consumer reads it there; what an operator lacked was a way to ask it
//! without composing a request by hand during the incident where lag is the
//! first thing anybody wants to know. The figures printed are the broker's,
//! not recomputed here, so the command and the consumer cannot disagree.
//!
//! The peer must be loopback and the token comes from a file only, for the
//! reasons [`super::admin`] gives: the same operator token reaches it. A group
//! that has never committed on the partition is reported as the broker's
//! refusal, not as a lag of zero — no checkpoint is not the same as caught up.
//!
//! Mirroring has no view: the fabric does not mirror (ADR 0100), and a view
//! of a mechanism that does not exist would read as one that is idle.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use qip_transport::event_fabric::protocol::{GroupLagRequest, Refusal, Request, Response};
use qip_transport::event_fabric::transport::Timeouts;

use super::admin::{local_peer, read_token};
use super::{Environment, Outcome};

/// `qip event-fabric lag`.
pub const SUBCOMMAND: &str = "lag";

const USAGE: &str = "usage: qip event-fabric lag --peer <loopback-address:port> --stream <name> \
                     --partition <number> --group <consumer-group>";

/// What the subcommand takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LagArguments {
    pub peer: String,
    pub stream: String,
    pub partition: u32,
    pub group: String,
}

/// Parse the arguments after `lag`, refusing an unknown or repeated flag, a
/// missing one, and a partition that is not a number.
pub fn parse_arguments(arguments: &[String]) -> Result<LagArguments> {
    const FLAGS: [&str; 4] = ["--peer", "--stream", "--partition", "--group"];
    let mut found: BTreeMap<&str, &str> = BTreeMap::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let Some(flag) = FLAGS.iter().find(|flag| **flag == argument.as_str()) else {
            return Err(Error::invalid(format!(
                "unknown argument {argument:?}; {USAGE}"
            )));
        };
        let Some(value) = rest.next() else {
            return Err(Error::invalid(format!("{flag} needs a value; {USAGE}")));
        };
        if found.insert(flag, value.as_str()).is_some() {
            return Err(Error::invalid(format!("{flag} was given twice; {USAGE}")));
        }
    }
    let required = |flag: &str| {
        found
            .get(flag)
            .filter(|value| !value.trim().is_empty())
            .map(|value| (*value).to_string())
            .ok_or_else(|| Error::invalid(format!("{flag} is required; {USAGE}")))
    };
    let partition_text = required("--partition")?;
    let partition = partition_text.parse::<u32>().map_err(|_| {
        Error::invalid(format!(
            "--partition must be a partition number, not {partition_text:?}; {USAGE}"
        ))
    })?;
    Ok(LagArguments {
        peer: required("--peer")?,
        stream: required("--stream")?,
        partition,
        group: required("--group")?,
    })
}

pub fn run(arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(arguments)?;
    let peer = local_peer(&arguments.peer)?;
    let token = read_token(environment)?;
    let at = format!("{}:{}", arguments.stream, arguments.partition);
    let answer = environment.connect(peer, token).call(
        Request::GroupLag(GroupLagRequest {
            group_id: arguments.group.clone(),
            stream: arguments.stream.clone(),
            partition: arguments.partition,
        }),
        Timeouts::default(),
    )?;
    match answer {
        Response::GroupLag(lag) => Ok(Outcome {
            lines: vec![format!(
                "LAG {at} group {}: {} (committed offset {}, high watermark {})",
                arguments.group,
                lag.lag(),
                lag.committed_offset(),
                lag.high_watermark()
            )],
            code: 0,
        }),
        Response::Refused(Refusal::AclDenied | Refusal::KeyOutOfScope) => {
            Err(Error::denied(format!(
                "the broker refused: the identity this token verifies as holds no consume or \
                 admin grant covering {at}. Add one to the stream catalogue's grants, or use \
                 the token of an identity that holds one"
            )))
        }
        other => Err(Error::invalid(format!(
            "the broker answered the lag of {at} with {other:?}, which is not an answer to it"
        ))),
    }
}
