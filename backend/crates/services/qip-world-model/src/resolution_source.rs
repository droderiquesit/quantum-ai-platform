//! The authority a falsifiable claim is settled against.
//!
//! §8.1 asks the world model to hold the objects the platform reasons about,
//! and the resolution source was the one that existed only in
//! `qip-prediction`: a field on a `Proposition`, reachable by whatever
//! already held the proposition and by nothing else. A thesis whose authority
//! is not a node is a thesis whose provenance cannot be *traversed* — "who
//! said so" has to be answered by knowing which struct to open rather than by
//! following an edge, and an explanation that depends on knowing where to
//! look is not an explanation the audit trail can reproduce.
//!
//! # How the type crosses the crate boundary
//!
//! It does not, and that is deliberate. `qip-world-model` and
//! `qip-prediction` are both services, and dependencies point inward only
//! (`.claude/rules/architecture/00-boundaries.md`), so naming
//! `qip_prediction::resolution::ResolutionSource` here would be a service
//! depending on a service — an edge that needs an ADR and would make the
//! UNDERSTAND stage's knowledge graph unbuildable without the prediction
//! engine. What crosses is the **value**, not the type: the runtime is the one
//! place two services meet, it already holds both, and it reads the
//! proposition's source and hands this crate the fields.
//!
//! The closed set of authorities therefore stays exactly where it is defined —
//! `SourceKind::as_str` — and this crate stores the token that produced
//! rather than re-enumerating it. Two enumerations of one closed set disagree
//! eventually and the louder one wins.
//!
//! # Both instants, and neither of them the clock
//!
//! [`ResolutionSourceClaim`] carries the instant the authority began to hold
//! and the instant the platform could know it, and takes both from the
//! proposition's own provenance. Nothing here reads a clock, and
//! [`crate::WorldModel::record_resolution_source`] has no write instant in its
//! signature to read one from. That is the structural half of the guarantee:
//! a node stamped at write time reports every authority as first known when
//! the replay ran, a point-in-time query then returns facts the platform did
//! not hold, and the backtest that results looks better than the platform
//! was.

use qip_core::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The node-id namespace for a resolving authority.
///
/// Namespaced because an authority and an instrument may share a name — a
/// source called `XNYS` and a venue called `XNYS` are different objects — and
/// a graph that let them collide would answer "who settles this thesis" with
/// an order book.
pub const RESOLUTION_SOURCE_PREFIX: &str = "resolution-source";

/// The delimiter [`crate::Relationship::key`] joins a fact key on.
///
/// A name carrying one would make two distinct edges share a key, and the
/// second would be read as a later version of the first — a silent merge of
/// two claims, which is the failure a refusal here prevents.
const KEY_DELIMITER: char = '|';

/// A resolving authority as the world model holds it, with the provenance
/// that makes it replayable.
///
/// Constructed through [`ResolutionSourceClaim::new`], which refuses rather
/// than repairs, so every value of this type is one the graph can record
/// without a further check. The fields are private for that reason: a
/// validated invariant a caller can reach around is a comment, not an
/// invariant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionSourceClaim {
    name: String,
    authority: String,
    publishes: BTreeSet<String>,
    settles: String,
    authoritative_from: Timestamp,
    knowable_at: Timestamp,
}

impl ResolutionSourceClaim {
    /// Refuses a source the graph could not hold honestly.
    ///
    /// Each refusal names a fact that would otherwise be silently wrong:
    ///
    /// - an unnamed authority is a node nothing can traverse to and nobody can
    ///   hold to what it published;
    /// - a name or a subject carrying the fact-key delimiter merges two edges
    ///   onto one key, and the graph then reports the second as a revision of
    ///   the first;
    /// - an authority publishing nothing can settle nothing, which mirrors the
    ///   refusal the proposition itself already makes;
    /// - a claim settling no thesis is an orphan node, which is the shape of
    ///   the gap this type was written to close.
    ///
    /// Nothing is defaulted. A source the caller cannot describe is a caller
    /// bug, and clamping it to a placeholder is that bug surviving into the
    /// replay.
    pub fn new(
        name: impl Into<String>,
        authority: impl Into<String>,
        publishes: impl IntoIterator<Item = String>,
        settles: impl Into<String>,
        authoritative_from: Timestamp,
        knowable_at: Timestamp,
    ) -> Result<Self> {
        let name = name.into();
        let authority = authority.into();
        let settles = settles.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a resolution source must be named: name the authority the proposition settles \
                 against",
            ));
        }
        if name.contains(KEY_DELIMITER) {
            return Err(Error::invalid(format!(
                "resolution source `{name}` carries the fact-key delimiter `{KEY_DELIMITER}`, \
                 which would merge it with another edge: name it without one"
            )));
        }
        if authority.trim().is_empty() {
            return Err(Error::invalid(format!(
                "resolution source `{name}` states no kind of authority: pass the token the \
                 source kind renders itself as"
            )));
        }
        if settles.trim().is_empty() {
            return Err(Error::invalid(format!(
                "resolution source `{name}` settles nothing: name the thesis it resolves, or do \
                 not record it"
            )));
        }
        if settles.contains(KEY_DELIMITER) {
            return Err(Error::invalid(format!(
                "thesis `{settles}` carries the fact-key delimiter `{KEY_DELIMITER}`, which \
                 would merge its edge with another: name it without one"
            )));
        }
        let publishes: BTreeSet<String> = publishes.into_iter().collect();
        if publishes.is_empty() {
            return Err(Error::invalid(format!(
                "resolution source `{name}` publishes nothing, so it can settle nothing: name \
                 the metrics it publishes"
            )));
        }
        Ok(Self {
            name,
            authority,
            publishes,
            settles,
            authoritative_from,
            knowable_at,
        })
    }

    /// The node id this claim is recorded under.
    pub fn node_id(&self) -> String {
        format!("{RESOLUTION_SOURCE_PREFIX}:{}", self.name)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The token the source's own kind renders itself as.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// What the source publishes. A `BTreeSet`, so the attribute the graph
    /// renders is the same string on every replay.
    pub fn publishes(&self) -> &BTreeSet<String> {
        &self.publishes
    }

    /// The metrics as one attribute value, in set order.
    pub fn published_list(&self) -> String {
        self.publishes
            .iter()
            .cloned()
            .collect::<Vec<String>>()
            .join(",")
    }

    /// The thesis this authority settles.
    pub fn settles(&self) -> &str {
        &self.settles
    }

    /// When the authority began to hold — the valid-time half.
    pub fn authoritative_from(&self) -> Timestamp {
        self.authoritative_from
    }

    /// When the platform could know it — the transaction-time half, and the
    /// instant the node is stamped with.
    pub fn knowable_at(&self) -> Timestamp {
        self.knowable_at
    }
}
