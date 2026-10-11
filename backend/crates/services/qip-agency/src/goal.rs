//! AGENCY-003 / AGENCY-041: a desired outcome is a typed `GoalSpec`, never a
//! free-form instruction.
//!
//! A free-form goal ("grow the book") carries no budget, no envelope and no
//! identity, so a planner reading it has nothing to stay inside. `GoalSpec`
//! is only constructible through [`GoalSpecDraft::build`] and its serde form
//! routes through the same constructor, so a JSON document missing a field
//! is refused with that field's name rather than defaulted.
//!
//! The type lives in `qip_contracts::expansion` and is re-exported here. There
//! used to be two `GoalSpec`s, this crate's and the contracts crate's, with
//! different fields and different refusals; a goal one of them validated was
//! not a goal the other had checked (R12 W0.4). There is now one.

pub use qip_contracts::expansion::{GoalClass, GoalSpec, GoalSpecDraft};
