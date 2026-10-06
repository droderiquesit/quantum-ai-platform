//! ADR 0100 §1: `qip event-fabric isolate` and `qip event-fabric release`,
//! the operator's emergency controls on one partition (FABRIC-028).
//!
//! Isolating a partition stops every produce to it and deletes nothing: the
//! records it holds stay readable, the broker records who isolated it and
//! why, and the isolation survives a broker restart until somebody releases
//! it. These two subcommands are the only way an operator reaches the
//! broker's `admin/isolate` and `admin/release` routes without composing a
//! request by hand, which during an incident is the moment nobody should be
//! composing requests by hand.
//!
//! # Who the action is attributed to
//!
//! `--operator` names the identity the action is recorded under, and the
//! broker refuses the request unless it is the identity the presented token
//! verifies as. It is an argument rather than something this command
//! derives because a token does not say whose it is — only the broker's
//! identities file does — and a command that guessed would print one name
//! while the broker recorded another.
//!
//! # Why the peer must be loopback
//!
//! The fabric speaks plaintext TCP (ADR 0100 §7; mutual TLS is BLOCKED(C2)),
//! so an operator token sent across a network is a token anything on the
//! path can read and replay, and this token parks partitions. The command
//! therefore reaches a broker on its own host only. Run it there.
//!
//! This does not place, amend or cancel an order and changes no autonomy
//! level. It stops records being written to a log.

use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};

use qip_core::error::{Error, Result};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::protocol::{
    AdminIsolateRequest, AdminReleaseRequest, Refusal, Request, Response,
};
use qip_transport::event_fabric::transport::Timeouts;

use super::{Environment, Outcome};

/// `qip event-fabric isolate`.
pub const ISOLATE: &str = "isolate";
/// `qip event-fabric release`.
pub const RELEASE: &str = "release";

/// The variable whose `_FILE` form names the operator's token file. Never
/// read in its direct form: see [`super::grant`]'s `file_only`.
pub const TOKEN_VARIABLE: &str = "QIP_EVENT_FABRIC_OPERATOR_TOKEN";

/// What the two subcommands take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminArguments {
    pub peer: String,
    pub stream: String,
    pub partition: u32,
    pub operator: String,
    /// Required to isolate, refused on a release: a release needs no reason
    /// beyond the isolation's own, and accepting one would suggest it was
    /// recorded.
    pub reason: Option<String>,
}

fn usage(action: &str) -> String {
    let reason = if action == ISOLATE {
        " --reason <why>"
    } else {
        ""
    };
    format!(
        "usage: qip event-fabric {action} --peer <loopback-address:port> --stream <name> \
         --partition <number> --operator <identity>{reason}"
    )
}

/// Parse the arguments after `isolate` or `release`, refusing an unknown or
/// repeated flag, a missing one, and a partition that is not a number.
pub fn parse_arguments(action: &str, arguments: &[String]) -> Result<AdminArguments> {
    const FLAGS: [&str; 5] = [
        "--peer",
        "--stream",
        "--partition",
        "--operator",
        "--reason",
    ];
    let mut found: BTreeMap<&str, &str> = BTreeMap::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let Some(flag) = FLAGS.iter().find(|flag| **flag == argument.as_str()) else {
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
    let partition_text = required("--partition")?;
    let partition = partition_text.parse::<u32>().map_err(|_| {
        Error::invalid(format!(
            "--partition must be a partition number, not {partition_text:?}; {}",
            usage(action)
        ))
    })?;
    let reason = match (action, found.contains_key("--reason")) {
        (ISOLATE, _) => Some(required("--reason")?),
        (_, true) => {
            return Err(Error::invalid(format!(
                "--reason belongs to isolate; a release records who released the partition, \
                 and the reason on record stays the isolation's own. {}",
                usage(action)
            )));
        }
        (_, false) => None,
    };
    Ok(AdminArguments {
        peer: required("--peer")?,
        stream: required("--stream")?,
        partition,
        operator: required("--operator")?,
        reason,
    })
}

/// Resolve `peer`, refusing anything that is not this host. See the module
/// documentation for why.
pub fn local_peer(peer: &str) -> Result<SocketAddr> {
    let resolved: Vec<SocketAddr> = peer
        .to_socket_addrs()
        .map_err(|error| {
            Error::invalid(format!(
                "--peer {peer:?} is not an address:port this command can resolve: {error}"
            ))
        })?
        .collect();
    if let Some(outside) = resolved.iter().find(|address| !address.ip().is_loopback()) {
        return Err(Error::denied(format!(
            "--peer {peer:?} resolves to {outside}, which is not loopback. The fabric speaks \
             plaintext TCP, so an operator token sent there could be read and replayed by \
             anything on the path; run this command on the broker's own host"
        )));
    }
    resolved.first().copied().ok_or_else(|| {
        Error::invalid(format!(
            "--peer {peer:?} resolved to no address; name the broker's loopback address:port"
        ))
    })
}

/// The operator's token, from the file `QIP_EVENT_FABRIC_OPERATOR_TOKEN_FILE`
/// names and from nowhere else.
pub fn read_token(environment: &Environment) -> Result<BearerToken> {
    let path = super::grant::file_only(environment, TOKEN_VARIABLE)?;
    BearerToken::resolve(TOKEN_VARIABLE, None, path)
}

/// `qip event-fabric isolate …` or `qip event-fabric release …`.
///
/// The order is [`super::grant::run`]'s and for its reason: the arguments
/// are refused before anything is read, and the peer is refused before the
/// token file is opened, so a run that could not have gone anywhere safe
/// never touches credential material.
pub fn run(action: &str, arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(action, arguments)?;
    let peer = local_peer(&arguments.peer)?;
    let token = read_token(environment)?;
    let at = format!("{}:{}", arguments.stream, arguments.partition);

    let request = match &arguments.reason {
        Some(reason) => Request::AdminIsolate(AdminIsolateRequest {
            stream: arguments.stream.clone(),
            partition: arguments.partition,
            operator: arguments.operator.clone(),
            reason: reason.clone(),
        }),
        None => Request::AdminRelease(AdminReleaseRequest {
            stream: arguments.stream.clone(),
            partition: arguments.partition,
            operator: arguments.operator.clone(),
        }),
    };
    let answer = environment
        .connect(peer, token)
        .call(request, Timeouts::default())?;
    let lines = match answer {
        Response::AdminIsolate(isolated) => vec![
            format!(
                "ISOLATED {at} at offset {} by {}: {}",
                isolated.isolated_at_offset,
                arguments.operator,
                arguments.reason.as_deref().unwrap_or_default()
            ),
            format!(
                "Every produce to {at} is now refused, naming this isolation. Nothing was \
                 deleted, its records can still be read, and it stays isolated across a broker \
                 restart until `qip event-fabric release` lifts it."
            ),
        ],
        Response::AdminRelease(released) => vec![format!(
            "RELEASED {at} at offset {} by {}: it accepts produce again",
            released.released_at_offset, arguments.operator
        )],
        Response::Refused(Refusal::AclDenied | Refusal::KeyOutOfScope) => {
            return Err(Error::denied(format!(
                "the broker refused: the identity this token verifies as holds no admin grant \
                 covering {at}. Add one to the stream catalogue's grants, or use the token of \
                 an identity that holds one"
            )));
        }
        other => {
            return Err(Error::invalid(format!(
                "the broker answered the {action} of {at} with {other:?}, which is not an \
                 answer to it"
            )));
        }
    };
    Ok(Outcome { lines, code: 0 })
}
