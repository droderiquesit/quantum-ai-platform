//! The journalled statement of the limit in force (ADR 0405 D8).
//!
//! The operator's limit set is a ceiling, and since ADR 0405 the pre-trade
//! check runs against that ceiling narrowed by the desk mandate's risk
//! tolerance. A decision is only reproducible from the event log if the log
//! says which bounds it was checked against, and until this record the log
//! could not: the ceiling comes from a file and the tolerance from the
//! mandate registry, and neither was written down beside the decisions they
//! governed.
//!
//! One record per process start, written at assembly, and only when the
//! tolerance is below one. At one the limit in force is the ceiling itself
//! and there is nothing to state. The desk's mandate cannot be replaced while
//! a process runs — the registry has no such path — so one record covers every
//! decision of that boot.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_risk::limits::LimitSet;
use serde::{Deserialize, Serialize};

/// The producer of [`LimitsNarrowed`]. Its own origin, so a reader of the
/// other `RiskEvaluated` producers never meets this body.
pub const LIMIT_NARROWING_ORIGIN: &str = "kernel/limit-narrowing";

/// One limit, as the operator wrote it and as it is in force.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NarrowedLimit {
    pub name: String,
    /// [`qip_risk::limits::LimitKind::label`].
    pub kind: String,
    /// A minimum is not narrowed, and says so here rather than leaving a
    /// reader to infer it from two equal numbers.
    pub is_minimum: bool,
    /// The operator's bound.
    pub ceiling: f64,
    /// The bound an order is checked against.
    pub in_force: f64,
}

/// The limit set in force for this process, and what it was derived from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitsNarrowed {
    /// The name of the operator's limit set.
    pub ceiling_set: String,
    /// The desk mandate's risk tolerance, read from the mandate registry.
    pub tolerance: Decimal,
    /// Every limit of the set, in the set's own order.
    pub limits: Vec<NarrowedLimit>,
    pub at: Timestamp,
}

impl LimitsNarrowed {
    /// The record for a ceiling and the set in force derived from it.
    ///
    /// Refuses two sets that do not line up rule for rule. The record's whole
    /// use is that a reader can put each bound in force beside the ceiling it
    /// came from; a record that paired one rule's ceiling with another's
    /// bound would be worse than none.
    pub fn between(
        ceiling: &LimitSet,
        in_force: &LimitSet,
        tolerance: Decimal,
        at: Timestamp,
    ) -> Result<Self> {
        if ceiling.len() != in_force.len() {
            return Err(Error::invalid(format!(
                "the limit set in force has {} limits and its ceiling {} has {}; the set in \
                 force must be the ceiling narrowed, rule for rule",
                in_force.len(),
                ceiling.name,
                ceiling.len()
            )));
        }
        let mut limits = Vec::with_capacity(ceiling.len());
        for (above, below) in ceiling.limits.iter().zip(&in_force.limits) {
            if above.name != below.name || above.kind.label() != below.kind.label() {
                return Err(Error::invalid(format!(
                    "the limit in force {} ({}) does not line up with the ceiling's {} ({}); \
                     the set in force must be the ceiling narrowed, rule for rule",
                    below.name,
                    below.kind.label(),
                    above.name,
                    above.kind.label()
                )));
            }
            limits.push(NarrowedLimit {
                name: above.name.clone(),
                kind: above.kind.label().to_string(),
                is_minimum: above.kind.is_minimum(),
                ceiling: above.kind.bound(),
                in_force: below.kind.bound(),
            });
        }
        Ok(Self {
            ceiling_set: ceiling.name.clone(),
            tolerance,
            limits,
            at,
        })
    }
}

impl EventBody for LimitsNarrowed {
    const TOPIC: Topic = Topic::RiskEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}
