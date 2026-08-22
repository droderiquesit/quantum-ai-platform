//! `qip-web` — the operator interface.
//!
//! Nine server-rendered surfaces with no JavaScript at all. That is a decision
//! rather than an omission: the API's content-security policy forbids script
//! entirely, so a page that needed it would not run, and everything the
//! operator does here is a link or a form submission a server can answer.
//!
//! [`html::Element::text`] escapes what it is given and there is no method
//! that inserts raw markup. Cross-site scripting is not something this crate
//! tries to catch; it is something it has no way to express.
//!
//! Every page carries a banner stating whether the platform is paper trading,
//! live, or halted. Whether real money is moving is the one thing that must
//! never be ambiguous, so it is the first thing rendered and it has its own
//! colour.

pub mod html;
pub mod pages;
pub mod style;
pub mod view;

pub use html::{Element, escape};
pub use pages::{Surface, render};
pub use view::{
    AgentRow, GovernanceRow, LimitRow, OpportunityRow, OrderRow, ProposalRow, StageRow, ThesisRow,
    ViewModel,
};
