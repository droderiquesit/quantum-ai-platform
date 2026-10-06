//! Funding and borrow state per instrument.
//!
//! The node maintains funding rates and borrow availability for instruments it
//! trades. These facts determine whether a position is worth holding (funding
//! cost) and whether a position can be held (borrow availability). A node that
//! must ask for them waits for the answer or trades without it; holding them
//! locally makes them current to the last message processed.
//!
//! # Updates
//!
//! Funding-rate and borrow-availability updates are applied on the next pass
//! after the node processes them. They are exposed to the feasibility gate so
//! that sizing and risk decisions account for the actual carry and borrow costs.

use qip_core::Decimal;
use std::collections::BTreeMap;

/// The funding rate (as a decimal percentage per time period) for an instrument.
///
/// A positive rate means the holder pays; negative means they receive.
/// Typically quoted as an annual percentage but the semantics are left to the
/// caller to track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FundingRate(pub Decimal);

impl FundingRate {
    pub fn new(rate: Decimal) -> Self {
        FundingRate(rate)
    }

    pub fn rate(&self) -> Decimal {
        self.0
    }
}

/// The quantity of an instrument available to borrow.
///
/// `None` means the quantity is unknown or unavailable.
/// `Some(quantity)` means up to `quantity` units can be borrowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BorrowAvailability(pub Option<Decimal>);

impl BorrowAvailability {
    pub fn available(quantity: Decimal) -> Self {
        BorrowAvailability(Some(quantity))
    }

    pub fn unavailable() -> Self {
        BorrowAvailability(None)
    }

    pub fn quantity(&self) -> Option<Decimal> {
        self.0
    }
}

/// Funding and borrow state for all instruments.
///
/// Keyed by instrument name as a string. If an instrument is not present,
/// its funding and borrow state is unknown locally.
#[derive(Clone, Debug, Default)]
pub struct FundingState {
    /// Funding rates per instrument.
    funding_rates: BTreeMap<String, FundingRate>,
    /// Borrow availability per instrument.
    borrow_availability: BTreeMap<String, BorrowAvailability>,
}

impl FundingState {
    pub fn new() -> Self {
        FundingState {
            funding_rates: BTreeMap::new(),
            borrow_availability: BTreeMap::new(),
        }
    }

    /// Update the funding rate for an instrument.
    pub fn set_funding_rate(&mut self, instrument: String, rate: FundingRate) {
        self.funding_rates.insert(instrument, rate);
    }

    /// Update the borrow availability for an instrument.
    pub fn set_borrow_availability(
        &mut self,
        instrument: String,
        availability: BorrowAvailability,
    ) {
        self.borrow_availability.insert(instrument, availability);
    }

    /// Get the funding rate for an instrument, if known.
    pub fn funding_rate(&self, instrument: &str) -> Option<FundingRate> {
        self.funding_rates.get(instrument).copied()
    }

    /// Get the borrow availability for an instrument, if known.
    pub fn borrow_availability(&self, instrument: &str) -> Option<BorrowAvailability> {
        self.borrow_availability.get(instrument).copied()
    }

    /// Clear all funding and borrow state.
    pub fn clear(&mut self) {
        self.funding_rates.clear();
        self.borrow_availability.clear();
    }

    /// Get a snapshot of all funding rates.
    pub fn all_funding_rates(&self) -> &BTreeMap<String, FundingRate> {
        &self.funding_rates
    }

    /// Get a snapshot of all borrow availability.
    pub fn all_borrow_availability(&self) -> &BTreeMap<String, BorrowAvailability> {
        &self.borrow_availability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_funding_rate_can_be_set_and_retrieved() {
        let mut state = FundingState::new();
        let rate = FundingRate::new(Decimal::parse("0.05").unwrap());

        state.set_funding_rate("BTC".to_string(), rate);
        assert_eq!(state.funding_rate("BTC"), Some(rate));
    }

    #[test]
    fn borrow_availability_tracks_quantity() {
        let mut state = FundingState::new();
        let availability = BorrowAvailability::available(Decimal::parse("100").unwrap());

        state.set_borrow_availability("ETH".to_string(), availability);
        assert_eq!(state.borrow_availability("ETH"), Some(availability));
    }

    #[test]
    fn missing_instrument_returns_none() {
        let state = FundingState::new();
        assert_eq!(state.funding_rate("MISSING"), None);
        assert_eq!(state.borrow_availability("MISSING"), None);
    }

    #[test]
    fn borrow_availability_can_be_unavailable() {
        let mut state = FundingState::new();
        let unavailable = BorrowAvailability::unavailable();

        state.set_borrow_availability("UNKNOWN".to_string(), unavailable);
        assert_eq!(state.borrow_availability("UNKNOWN"), Some(unavailable));
        assert_eq!(
            state.borrow_availability("UNKNOWN").unwrap().quantity(),
            None
        );
    }

    #[test]
    fn clearing_state_removes_all_data() {
        let mut state = FundingState::new();
        state.set_funding_rate(
            "BTC".to_string(),
            FundingRate::new(Decimal::parse("0.05").unwrap()),
        );
        state.set_borrow_availability(
            "ETH".to_string(),
            BorrowAvailability::available(Decimal::parse("100").unwrap()),
        );

        state.clear();
        assert_eq!(state.funding_rate("BTC"), None);
        assert_eq!(state.borrow_availability("ETH"), None);
    }

    #[test]
    fn mutation_funding_rate_actually_stores_value() {
        // Break: remove the set_funding_rate call
        let mut state = FundingState::new();
        let rate1 = FundingRate::new(Decimal::parse("0.05").unwrap());
        let rate2 = FundingRate::new(Decimal::parse("0.10").unwrap());

        state.set_funding_rate("BTC".to_string(), rate1);
        state.set_funding_rate("BTC".to_string(), rate2);

        // Should be the second rate, not the first
        assert_eq!(state.funding_rate("BTC"), Some(rate2));
        assert_ne!(state.funding_rate("BTC"), Some(rate1));
    }

    #[test]
    fn mutation_borrow_availability_quantity_actually_matters() {
        // Break: swap available/unavailable
        let mut state = FundingState::new();
        let qty1 = Decimal::parse("50").unwrap();
        let qty2 = Decimal::parse("100").unwrap();

        state.set_borrow_availability("ETH".to_string(), BorrowAvailability::available(qty1));
        state.set_borrow_availability("ETH".to_string(), BorrowAvailability::available(qty2));

        let result = state.borrow_availability("ETH").unwrap();
        assert_eq!(result.quantity(), Some(qty2));
        assert_ne!(result.quantity(), Some(qty1));
    }
}
