//! Where capital sits, expressed as a key a limit can be written against.
//!
//! Capital is not fungible across places in the way a single balance sheet
//! number suggests. A dollar at a US prime broker, a dollar in a euro account
//! in Frankfurt and a dollar of yen collateral posted at a Tokyo clearing house
//! are three different assets with three different funding rates, three
//! settlement conventions and three sets of hours during which they can be
//! turned into anything else. Netting them into one figure is exactly the
//! arithmetic that produces a plan requiring capital to be in Tokyo on Monday
//! morning that is, in fact, in New York on Friday afternoon.
//!
//! So the fabric keys everything on the triple that actually determines those
//! properties — region, currency, venue — and nothing in this crate accepts a
//! bare amount without one.

use qip_contracts::venue::VenueId;
use qip_core::Currency;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A funding and settlement jurisdiction.
///
/// A newtype rather than a string so a region cannot be silently compared
/// against a venue or a free-text label. Regions are the axis funding rates,
/// cut-off times and non-settlement days vary along, and a mis-keyed region
/// reads as a place the fabric has no calendar for — which the settlement
/// module refuses rather than defaulting to same-day.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Region(String);

impl Region {
    /// Name a region.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The region's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One place capital can be, and the three properties that make it that place.
///
/// Ordered, so plans keyed on a location iterate in a stable sequence and two
/// runs on the same inputs produce byte-identical output. That is not a
/// convenience: it is what makes a pre-positioning plan diffable against the
/// previous one during an incident.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CapitalLocation {
    /// The jurisdiction whose calendar and funding curve apply.
    pub region: Region,
    /// The currency the balance is denominated in.
    pub currency: Currency,
    /// The venue, broker or custodian holding it.
    pub venue: VenueId,
}

impl CapitalLocation {
    /// Name a location.
    pub fn new(region: Region, currency: Currency, venue: VenueId) -> Self {
        Self {
            region,
            currency,
            venue,
        }
    }

    /// Whether moving between two locations crosses a currency boundary.
    ///
    /// The expensive boundary. A same-currency transfer between two custodians
    /// is a wire; a cross-currency one is a wire plus an FX trade whose market
    /// impact scales with size, and pricing the second as though it were the
    /// first understates the cost of exactly the large transfers that matter.
    pub fn requires_conversion(&self, other: &Self) -> bool {
        self.currency != other.currency
    }

    /// Whether two locations differ only in which venue holds the balance.
    pub fn is_same_pocket(&self, other: &Self) -> bool {
        self.region == other.region && self.currency == other.currency
    }
}

impl fmt::Display for CapitalLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.region, self.currency, self.venue)
    }
}
