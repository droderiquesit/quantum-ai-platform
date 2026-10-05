//! Lifecycles as data, and whether a sequence of states is admissible
//! (REASON-011).
//!
//! The order state machine and the strategy rungs are each a hand-written
//! `match`: correct for the one lifecycle they encode, and unable to answer
//! the question for a contract, a position or anything declared later. A
//! [`Lifecycle`] is the transition relation itself, declared and versioned,
//! and [`Lifecycle::judge`] decides a whole observed or proposed sequence
//! against it.
//!
//! **The first break is named, not merely detected.** A bare "inadmissible"
//! sends a reader to diff the sequence against the relation by hand, and a
//! checker that reported the *last* bad move would point past the one that
//! made every later state meaningless.

use std::collections::BTreeSet;

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A declared lifecycle: where a thing may start and which moves it may make.
///
/// The states are exactly those the two sets name. There is no separate state
/// list to drift out of step with the relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub name: String,
    pub version: u32,
    /// States a sequence may open in.
    pub initial: BTreeSet<String>,
    /// Permitted moves, as `(from, to)`.
    pub transitions: BTreeSet<(String, String)>,
}

/// The first move in a sequence the lifecycle does not allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Break {
    /// Index, in the judged sequence, of the state that could not be entered.
    pub index: usize,
    /// The state it was entered from; `None` when the sequence opened in a
    /// state the lifecycle does not start in.
    pub from: Option<String>,
    pub to: String,
}

/// What [`Lifecycle::judge`] decided, citing the definition it decided under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admissibility {
    pub lifecycle: String,
    pub version: u32,
    /// `None` when every move is allowed.
    pub first_break: Option<Break>,
}

impl Admissibility {
    pub fn is_admissible(&self) -> bool {
        self.first_break.is_none()
    }
}

impl Lifecycle {
    /// Decide whether `sequence` is a path the lifecycle allows.
    ///
    /// Refuses an empty sequence: nothing was observed, and answering
    /// "admissible" would let a reader who failed to load the history report
    /// it clean.
    pub fn judge<S: AsRef<str>>(&self, sequence: &[S]) -> Result<Admissibility> {
        let Some(first) = sequence.first() else {
            return Err(Error::invalid(format!(
                "no states were supplied to judge against lifecycle '{}'; supply the observed \
                 sequence, starting with the state it opened in",
                self.name
            )));
        };
        let opening = (!self.initial.contains(first.as_ref())).then(|| Break {
            index: 0,
            from: None,
            to: first.as_ref().to_string(),
        });
        let first_break = opening.or_else(|| {
            sequence.windows(2).enumerate().find_map(|(i, pair)| {
                let (from, to) = (pair[0].as_ref(), pair[1].as_ref());
                (!self
                    .transitions
                    .contains(&(from.to_string(), to.to_string())))
                .then(|| Break {
                    index: i + 1,
                    from: Some(from.to_string()),
                    to: to.to_string(),
                })
            })
        });
        Ok(Admissibility {
            lifecycle: self.name.clone(),
            version: self.version,
            first_break,
        })
    }
}
