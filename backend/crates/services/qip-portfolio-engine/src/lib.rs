//! `qip-portfolio-engine` — turning approved theses into a checked proposal.
//!
//! [`construction::PortfolioConstructor`] sizes approved hypotheses through the
//! compute router, so the same concentration limits and cost model apply
//! whichever solver runs. What it produces is a
//! [`proposal::Proposal`]: intent plus reasoning plus the checks it has
//! passed, and explicitly *not* an order.
//!
//! Three rules are structural here:
//!
//! * A leg with no hypothesis behind it fails validation. A trade nobody can
//!   trace to a reviewed thesis is a trade nobody reviewed.
//! * Approval needs two controls. Risk and compliance both sign, or the
//!   proposal stays a draft — a single approver is a single point of failure,
//!   and the platform has two control functions precisely so that neither is
//!   one.
//! * Only an approved proposal can be released. A veto works from any state
//!   before release, because a control that can be locked out by timing is not
//!   a control.
//!
//! Where a concentration cap makes the exposure target unreachable, the cap
//! wins and the shortfall is stated in the proposal's own compromises rather
//! than quietly relaxed.

pub mod construction;
pub mod proposal;

pub use construction::{ApprovedThesis, ConstructionOutcome, Mandate, PortfolioConstructor};
pub use proposal::{Proposal, ProposalLeg, ProposalStatus, Side};
