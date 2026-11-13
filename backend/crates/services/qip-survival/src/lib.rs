//! The survival kernel and the confidence (model-risk) governor, outside the
//! runtime (R12-CAPITAL-7204, ADR 0127 decision 3, ADR 0464).
//!
//! Both lived in `qip-kernel`, whose closure holds every alpha crate, so
//! their independence from alpha could only be asserted by a scan of their
//! source for an alpha crate's name — which an aliased import or a `crate::`
//! path into a module wrapping an alpha component evades. As a crate their
//! independence is a property of the dependency graph:
//! `qip-acceptance`'s `survival_independence` refuses any path from this
//! crate to a crate that generates alpha or holds a model, and any path from
//! such a crate to this one. An alpha crate that cannot name
//! [`survival::SurvivalGovernor`] or [`confidence::ConfidenceSignals`] cannot
//! construct, relax or bypass a ceiling either of them computes.
//!
//! The kernel composes both and hands them data: the worlds a stress priced
//! ([`survival::StressedWorlds`]) and the feature statistics' shifts
//! ([`confidence::DistributionReading`]), never a component that could be
//! asked a question.

pub mod confidence;
pub mod survival;
