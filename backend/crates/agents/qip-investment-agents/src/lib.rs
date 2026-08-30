//! `qip-investment-agents` — the machine investment organisation.
//!
//! Eighteen governed specialists, a shared [`desk::Desk`] of capability-gated
//! facilities, and a coordinator that dispatches to them.
//!
//! The organisation is shaped like one that would pass an audit rather than one
//! that is convenient to build. Research cannot reach the market. Whoever
//! proposes cannot approve. The adversarial reviewer reports to a different
//! owner from the analysts it reviews and cannot publish theses of its own.
//! Execution reaches venues but originates nothing. Two control agents hold the
//! veto, and no agent — none of the eighteen — can raise its own autonomy
//! level.
//!
//! Those are not conventions. [`manifests::roster`] is checked by
//! `Roster::validate` in the tests, and the individual combinations are refused
//! by `AgentManifest::validate` in `qip-agents`.

pub mod analysts;
pub mod chief;
pub mod construction;
pub mod control;
pub mod desk;
pub mod execution;
pub mod fast;
pub mod learning;
pub mod manifests;
pub mod reasoning;
pub mod review;
pub mod simulation;
pub mod support;

pub use chief::{ChiefInvestmentIntelligence, Organisation, OrganisationReport};
pub use desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
pub use manifests::{ids, owners, roster};
