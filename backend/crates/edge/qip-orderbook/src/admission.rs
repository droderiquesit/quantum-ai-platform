//! Order-level capture is admitted per venue, on two recorded facts.
//!
//! Order-by-order depth is the dearest feed a venue sells and the most tightly
//! licensed. Capturing it because a feed happened to publish it is the failure
//! this gate prevents: it is admitted only for a venue whose configuration
//! records both the licence entitlement that covers it and the justification
//! for its cost. Either missing refuses the whole configuration at load, naming
//! the venue; nothing is defaulted and nothing is partially admitted.
//!
//! A venue absent from the configuration is simply not admitted, so the safe
//! state is the default one.

use crate::book::Book;
use crate::snapshot::BookKind;
use crate::venue::VenueState;
use qip_contracts::{VenueId, VenueStatus};
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

/// One venue's configuration entry.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueDepthConfig {
    pub venue: String,
    /// `aggregated` or `order_by_order`.
    pub depth: String,
    /// The licence entitlement that covers order-level data.
    #[serde(default)]
    pub l3_entitlement: Option<String>,
    /// Why the cost of order-level data is worth paying.
    #[serde(default)]
    pub l3_cost_justification: Option<String>,
}

/// Proof that order-level capture was admitted for one venue.
///
/// Held by value, not constructible outside this module, and required to build
/// an order-by-order [`VenueState`] through [`L3Admission::open_state`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3Admission {
    venue: VenueId,
    entitlement: String,
    justification: String,
}

impl L3Admission {
    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub fn entitlement(&self) -> &str {
        &self.entitlement
    }

    /// A state that captures order-level events for the admitted venue.
    pub fn open_state(&self, object_id: ObjectId, status: VenueStatus) -> VenueState {
        VenueState::new(
            object_id,
            self.venue.clone(),
            Book::of_kind(BookKind::OrderByOrder),
            status,
        )
    }

    pub fn justification(&self) -> &str {
        &self.justification
    }
}

/// The venues admitted for order-level capture, as the configuration loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DepthAdmissions {
    admitted: BTreeMap<String, L3Admission>,
}

impl DepthAdmissions {
    /// Load the per-venue depth configuration, refusing it as a whole when any
    /// venue asks for order-level depth without both recorded facts.
    pub fn load(json: &str) -> Result<Self> {
        let entries: Vec<VenueDepthConfig> = serde_json::from_str(json)?;
        let mut admitted = BTreeMap::new();
        for entry in entries {
            let tracks_orders = match entry.depth.as_str() {
                "order_by_order" => true,
                "aggregated" => false,
                other => {
                    return Err(Error::invalid(format!(
                        "venue {} names depth {other:?}; use \"aggregated\" or \"order_by_order\"",
                        entry.venue
                    )));
                }
            };
            fn present(v: &Option<String>) -> Option<&str> {
                v.as_deref().filter(|s| !s.trim().is_empty())
            }
            if !tracks_orders {
                if entry.l3_entitlement.is_some() || entry.l3_cost_justification.is_some() {
                    return Err(Error::invalid(format!(
                        "venue {} is configured aggregated but carries order-level terms; \
                         remove them or set depth to order_by_order",
                        entry.venue
                    )));
                }
                continue;
            }
            let (Some(entitlement), Some(justification)) = (
                present(&entry.l3_entitlement),
                present(&entry.l3_cost_justification),
            ) else {
                return Err(Error::denied(format!(
                    "venue {} asks for order-level capture without both a licence entitlement \
                     (l3_entitlement) and a cost justification (l3_cost_justification); \
                     record both, or capture aggregated depth",
                    entry.venue
                )));
            };
            if admitted
                .insert(
                    entry.venue.clone(),
                    L3Admission {
                        venue: VenueId::new(entry.venue.clone()),
                        entitlement: entitlement.to_string(),
                        justification: justification.to_string(),
                    },
                )
                .is_some()
            {
                return Err(Error::invalid(format!(
                    "venue {} appears twice in the depth configuration",
                    entry.venue
                )));
            }
        }
        Ok(Self { admitted })
    }

    /// The admission for a venue, if order-level capture was admitted there.
    pub fn for_venue(&self, venue: &VenueId) -> Option<&L3Admission> {
        self.admitted.get(venue.as_str())
    }
}
