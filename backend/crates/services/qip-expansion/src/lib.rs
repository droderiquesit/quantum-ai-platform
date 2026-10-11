//! `qip-expansion`: the curriculum of the Intelligence Expansion Engine
//! (blueprint §23, EXPAND domain).
//!
//! The engine sits above the brains (EXPAND-005): it reads what they found
//! wanting as [`gap::Observation`]s and turns admitted research candidates into
//! a ranked, budgeted [`curriculum::ResearchQueue`]. Nothing below a
//! composition root may depend on this crate; `qip-acceptance`'s
//! `architecture` suite holds that.
//!
//! Nothing here acts. A started item is a record that it may run within this
//! round's budget; it fetches nothing, registers no tool and reaches no venue.
//!
//! The loop, in the order it runs:
//!
//! * [`detectors`] reads plain-data readings of platform state and states
//!   what is wanting, with a measured severity and its origin;
//! * [`gap`] classifies each into one of nine classes or refuses it;
//! * [`desk`] derives the eleven ranking inputs from what it measured
//!   ([`desk::criteria_for`]), admits, schedules, closes and withdraws;
//! * [`plan`] names the acquisition, the derivation and the research plan
//!   each item carries;
//! * [`promotion`] holds the four-environment campaign, the nine-criterion
//!   evaluation, and the only road from closed research to a capability.
//!
//! Status: `qip-deepbrain`'s research loop holds an [`desk::ExpansionDesk`]
//! and sweeps platform state into it every cycle. The promotion road has no
//! production caller: nothing in the tree yet produces a nine-criterion
//! evaluation, and inventing one to exercise the road would be the failure
//! [`promotion`] exists to refuse.

pub mod campaigns;
pub mod curriculum;
pub mod desk;
pub mod detectors;
pub mod gap;
pub mod plan;
pub mod promotion;
