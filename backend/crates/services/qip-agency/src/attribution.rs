//! AGENCY-048: an effect attribution keeps the causal chain, not just a KPI.
//!
//! A bare "KPI moved +3%" cannot be audited: it names no mechanism, no
//! confounder it was adjusted for and no rival explanation, and one number
//! cannot show an effect that appeared at a minute and reversed by a day.
//!
//! The contract is `qip_contracts::expansion::EffectAttribution`, re-exported
//! here. This crate used to hold a second one with the chain and rivals but no
//! horizons, beside the contracts crate's with horizons but no chain; each
//! admitted what the other refused (R12-AGENCY-6104). The merged type requires
//! both halves, and its `validate` refuses an unidentified effect recorded at
//! any identifiability above zero (R12-AGENCY-6103), which is the check
//! [`crate::autonomy::ActionPolicy::widen`] and [`crate::engine::learn`] run
//! before they read one.

pub use qip_contracts::expansion::{Effect, EffectAttribution, HorizonObservation, Identification};
