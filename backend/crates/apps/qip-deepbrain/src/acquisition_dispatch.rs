//! Each information request goes to the handler its kind names
//! (R12-DATA-3105, blueprint §9.10).
//!
//! Every launched request used to become one scout job for the discovery desk
//! whatever it asked for, so a request to re-measure, to simulate or to ask a
//! specialist was answered by a search for a new source, or by nothing.
//! [`route`] reads [`AcquisitionKind`] and names the handler; the node hands
//! only the new-source queries to the discovery desk.
//!
//! Two kinds have no handler in this tree: an alternative provider and a
//! public filing fetch both need a vendor or connector nobody has configured.
//! They are routed to [`Routing::Unhandled`] with the reason, never to the
//! discovery desk as if a search answered them, and never dropped silently.

use crate::research::ScoutRequest;
use qip_contracts::ambient::AcquisitionKind;

/// The handler a kind is dispatched to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    /// The discovery desk's scout queue.
    DiscoveryDesk,
    /// The tick desk's microstructure campaigns.
    TickDesk,
    /// The simulation constructed for a started research item.
    SimulationConstruction,
    /// The specialist roster.
    SpecialistRoster,
    /// A fresh sweep of what the own stream already observes.
    Resweep,
}

/// Where one request went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routing {
    Handled(Handler),
    /// No handler exists; the reason names what is missing.
    Unhandled(&'static str),
}

/// The handler for `kind`. Total over [`AcquisitionKind`]: a new kind is a
/// compile error here rather than a request that goes nowhere.
pub const fn route(kind: AcquisitionKind) -> Routing {
    match kind {
        AcquisitionKind::NewSourceQuery => Routing::Handled(Handler::DiscoveryDesk),
        AcquisitionKind::MicrostructureProbe => Routing::Handled(Handler::TickDesk),
        AcquisitionKind::Simulation => Routing::Handled(Handler::SimulationConstruction),
        AcquisitionKind::SpecialistAgent => Routing::Handled(Handler::SpecialistRoster),
        AcquisitionKind::AdditionalMeasurement => Routing::Handled(Handler::Resweep),
        AcquisitionKind::AlternativeProvider => Routing::Unhandled(
            "no alternative-provider handler: it needs a second vendor connector no environment configures",
        ),
        AcquisitionKind::PublicFilingFetch => Routing::Unhandled(
            "no filing-fetch handler: it needs a public-filing connector and its egress route",
        ),
    }
}

/// A launched request and where it was routed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatched {
    pub request_id: String,
    pub kind: AcquisitionKind,
    pub routing: Routing,
}

/// Route every scout job; the first list is what the discovery desk takes.
pub fn dispatch(scouted: &[ScoutRequest]) -> (Vec<ScoutRequest>, Vec<Dispatched>) {
    let mut to_desk = Vec::new();
    let mut routed = Vec::new();
    for scout in scouted {
        let routing = route(scout.kind);
        if routing == Routing::Handled(Handler::DiscoveryDesk) {
            to_desk.push(scout.clone());
        }
        routed.push(Dispatched {
            request_id: scout.request_id.clone(),
            kind: scout.kind,
            routing,
        });
    }
    (to_desk, routed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scout(id: &str, kind: AcquisitionKind) -> ScoutRequest {
        ScoutRequest {
            request_id: id.to_string(),
            question: "q".to_string(),
            rank_bp: 1,
            entities: vec!["E".to_string()],
            kind,
        }
    }

    /// Every kind dispatches to its own handler, and the two without one are
    /// refused with a reason. Mutation: route `Simulation` to
    /// `Handler::DiscoveryDesk` — the simulation row fails.
    #[test]
    fn each_request_kind_dispatches_to_its_handler() {
        let expected = [
            (AcquisitionKind::NewSourceQuery, Handler::DiscoveryDesk),
            (AcquisitionKind::MicrostructureProbe, Handler::TickDesk),
            (AcquisitionKind::Simulation, Handler::SimulationConstruction),
            (AcquisitionKind::SpecialistAgent, Handler::SpecialistRoster),
            (AcquisitionKind::AdditionalMeasurement, Handler::Resweep),
        ];
        for (kind, handler) in expected {
            assert_eq!(route(kind), Routing::Handled(handler), "{kind:?}");
        }
        for kind in [
            AcquisitionKind::AlternativeProvider,
            AcquisitionKind::PublicFilingFetch,
        ] {
            assert!(
                matches!(route(kind), Routing::Unhandled(r) if !r.is_empty()),
                "{kind:?}"
            );
        }
        assert_eq!(
            expected.len() + 2,
            AcquisitionKind::ALL.len(),
            "premise: every kind is covered above"
        );
    }

    /// Only new-source queries reach the discovery desk; every request is
    /// accounted for in the routing. Mutation: drop the `== DiscoveryDesk`
    /// filter in `dispatch` — the desk receives the simulation too.
    #[test]
    fn only_new_source_queries_reach_the_discovery_desk_and_every_request_is_routed() {
        let jobs = [
            scout("a", AcquisitionKind::NewSourceQuery),
            scout("b", AcquisitionKind::Simulation),
            scout("c", AcquisitionKind::PublicFilingFetch),
        ];
        let (desk, routed) = dispatch(&jobs);
        assert_eq!(desk.len(), 1);
        assert_eq!(desk[0].request_id, "a");
        assert_eq!(routed.len(), 3);
    }
}
