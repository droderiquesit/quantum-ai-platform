//! `qip-web` — the operator interface.
//!
//! Two sets of server-rendered pages with no JavaScript at all: the nine
//! investment surfaces in [`pages`], and the nine-view operator console in
//! [`console`]. Having no JavaScript is a decision rather than an omission: the API's content-security policy forbids script
//! entirely, so a page that needed it would not run, and everything the
//! operator does here is a link or a form submission a server can answer.
//!
//! [`html::Element::text`] escapes what it is given and there is no method
//! that inserts raw markup. Cross-site scripting is not something this crate
//! tries to catch; it is something it has no way to express.
//!
//! [`panel::Panel`] is the console's other load-bearing type. A view must
//! never invent a number, so no collection reaches a page as a bare `Vec`: a
//! panel carries whether its contents were reported, are stale, or were never
//! reported at all. An empty table that reads as "zero exposure" when it means
//! "no cell is reporting" is the most dangerous thing a trading console can
//! render, and the type is what keeps the two apart. [`view::Fact`] is the
//! same rule for a single figure: a value the platform never recorded reaches
//! a page as the reason it is missing, never as `0`.
//!
//! Every page carries a banner stating whether the platform is paper trading,
//! live, or halted. Whether real money is moving is the one thing that must
//! never be ambiguous, so it is the first thing rendered and it has its own
//! colour.

pub mod console;
pub mod html;
pub mod pages;
pub mod panel;
pub mod style;
pub mod view;

pub use console::{ConsoleModel, View};
pub use html::{Element, escape};
pub use pages::{Surface, render};
pub use panel::{Freshness, Panel};
pub use view::{
    AgentRow, EdgeCellRow, Fact, FactRow, GovernanceRow, LimitRow, OpportunityRow, OrderRow,
    Posture, ProposalRow, ShippedPolicyRow, StageRow, ThesisRow, UniverseExclusionRow,
    UniverseView, ViewModel,
};
