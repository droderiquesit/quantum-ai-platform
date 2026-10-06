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
//! Status: library only. No composition root constructs a queue yet, because
//! no detector in the tree states a severity or an economic value for what it
//! finds, and no measure exists for most of the eleven criteria. Wiring it
//! would mean inventing those numbers, so the register rows this closes carry
//! `integrated = false`.

pub mod curriculum;
pub mod gap;
