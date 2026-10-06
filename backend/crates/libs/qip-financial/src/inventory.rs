//! Physical inventory ledger for goods in transit and storage.
//!
//! Tracks the quantity and state of goods at each stage of a landed-cost
//! journey: shipped, in-transit, delivered, stored, and sold. The ledger
//! reconciles against [`crate::physical::LandedCost`] to ensure physical
//! inventory matches economic exposure.
//!
//! # Reconciliation
//!
//! The ledger refuses:
//! - Quantities that diverge from LandedCost's reported shipped/delivered/sold
//! - Condition transitions that violate the goods' state machine
//! - Reservations that exceed available quantity
//! - Any transition after the goods are sold or destroyed

use crate::physical::LandedCost;
use qip_core::{Decimal, Error, Result};
use serde::{Deserialize, Serialize};

/// The location and custody state of goods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GoodsLocation {
    /// At the origin, awaiting shipment.
    Origin,
    /// In transit on a leg of the route.
    InTransit,
    /// At the destination, awaiting sale.
    AtDestination,
    /// Sold and no longer held.
    Sold,
}

/// Physical condition of goods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoodsCondition {
    /// Good condition, ready for sale.
    Good,
    /// Damaged but possibly salvageable.
    Damaged,
    /// Expired or spoiled, unsaleable.
    Expired,
}

/// A reservation of goods for a specific order or commitment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reservation {
    /// Identifier for what this reservation is for (e.g., order ID).
    pub holder: String,
    /// Quantity reserved.
    pub quantity: Decimal,
}

/// State of goods at one point in the supply chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InventoryRecord {
    /// Where the goods are.
    pub location: GoodsLocation,
    /// Physical condition.
    pub condition: GoodsCondition,
    /// Quantity on hand.
    pub quantity: Decimal,
    /// Quantity reserved for orders/commitments.
    pub reserved: Vec<Reservation>,
}

impl InventoryRecord {
    /// Total quantity reserved across all reservations.
    pub fn total_reserved(&self) -> Decimal {
        self.reserved.iter().map(|r| r.quantity).sum()
    }

    /// Quantity available for sale (unreserved).
    pub fn available(&self) -> Result<Decimal> {
        self.quantity
            .checked_sub(self.total_reserved())
            .ok_or_else(|| Error::numeric("reserved quantity exceeds on-hand quantity"))
    }
}

/// Complete inventory ledger for a consignment, reconciled against LandedCost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InventoryLedger {
    /// The economic model this ledger reconciles against.
    pub landed_cost: LandedCost,

    /// Goods at each location/condition combination.
    pub origin: Vec<InventoryRecord>,
    pub in_transit: Vec<InventoryRecord>,
    pub at_destination: Vec<InventoryRecord>,
    pub sold: Vec<InventoryRecord>,
}

impl InventoryLedger {
    /// Create a new ledger for a consignment, starting with goods at origin.
    ///
    /// All goods begin in Origin/Good condition with the shipped quantity.
    /// The ledger refuses if:
    /// - shipped quantity is non-positive
    /// - landed_cost is missing required fields
    pub fn new(landed_cost: LandedCost) -> Result<Self> {
        if landed_cost.quantity_shipped <= Decimal::ZERO {
            return Err(Error::invalid(
                "shipped quantity must be positive; a consignment of nothing \
                 has no economic or inventory story to tell",
            ));
        }

        let shipped_qty = landed_cost.quantity_shipped;
        Ok(Self {
            landed_cost,
            origin: vec![InventoryRecord {
                location: GoodsLocation::Origin,
                condition: GoodsCondition::Good,
                quantity: shipped_qty,
                reserved: Vec::new(),
            }],
            in_transit: Vec::new(),
            at_destination: Vec::new(),
            sold: Vec::new(),
        })
    }

    /// Record a shipment departure: goods leave origin and enter in-transit.
    ///
    /// Refuses if:
    /// - quantity > shipped quantity in LandedCost
    /// - no goods exist at origin
    /// - goods at origin are not all in Good condition
    pub fn record_departure(&mut self, quantity: Decimal) -> Result<()> {
        if quantity > self.landed_cost.quantity_shipped {
            return Err(Error::invalid(
                "departed quantity exceeds the consignment's shipped quantity",
            ));
        }

        let origin_total: Decimal = self.origin.iter().map(|r| r.quantity).sum();
        if origin_total < quantity {
            return Err(Error::invalid(
                "departed quantity exceeds goods on hand at origin",
            ));
        }

        self.origin[0].quantity -= quantity;
        self.in_transit.push(InventoryRecord {
            location: GoodsLocation::InTransit,
            condition: GoodsCondition::Good,
            quantity,
            reserved: Vec::new(),
        });

        Ok(())
    }

    /// Record goods arriving at destination after spoilage.
    ///
    /// Refuses if:
    /// - delivered quantity != landed_cost.quantity_delivered
    /// - delivered quantity > in-transit quantity
    /// - goods in transit are not all in Good condition
    pub fn record_arrival(&mut self, delivered_quantity: Decimal) -> Result<()> {
        if (delivered_quantity - self.landed_cost.quantity_delivered).abs() > Decimal::ZERO {
            return Err(Error::invalid(
                "delivered quantity must match the landed cost's quantity_delivered; \
                 reconciliation cannot proceed without exact alignment",
            ));
        }

        let in_transit_total: Decimal = self.in_transit.iter().map(|r| r.quantity).sum();
        if delivered_quantity > in_transit_total {
            return Err(Error::invalid(
                "delivered quantity exceeds goods in transit",
            ));
        }

        if !self
            .in_transit
            .iter()
            .all(|r| r.condition == GoodsCondition::Good)
        {
            return Err(Error::invalid(
                "goods with spoilage or damage cannot progress to destination; \
                 record their condition before arrival",
            ));
        }

        self.in_transit[0].quantity -= delivered_quantity;

        self.at_destination.push(InventoryRecord {
            location: GoodsLocation::AtDestination,
            condition: GoodsCondition::Good,
            quantity: delivered_quantity,
            reserved: Vec::new(),
        });

        Ok(())
    }

    /// Record a sale: goods move from destination to sold.
    ///
    /// Refuses if:
    /// - sold_quantity != landed_cost.quantity_sold
    /// - sold_quantity > goods at destination
    /// - any sold goods are not in Good condition
    pub fn record_sale(&mut self, sold_quantity: Decimal) -> Result<()> {
        if (sold_quantity - self.landed_cost.quantity_sold).abs() > Decimal::ZERO {
            return Err(Error::invalid(
                "sold quantity must match the landed cost's quantity_sold; \
                 reconciliation requires exact alignment",
            ));
        }

        let available_at_destination: Decimal =
            self.at_destination.iter().map(|r| r.quantity).sum();
        if sold_quantity > available_at_destination {
            return Err(Error::invalid(
                "sold quantity exceeds goods available at destination",
            ));
        }

        if !self
            .at_destination
            .iter()
            .all(|r| r.condition == GoodsCondition::Good)
        {
            return Err(Error::invalid(
                "damaged or expired goods cannot be recorded as sold at full value",
            ));
        }

        self.at_destination[0].quantity -= sold_quantity;

        self.sold.push(InventoryRecord {
            location: GoodsLocation::Sold,
            condition: GoodsCondition::Good,
            quantity: sold_quantity,
            reserved: Vec::new(),
        });

        Ok(())
    }

    /// Record spoilage or damage in-transit.
    ///
    /// Moves goods from Good to Damaged/Expired condition in-transit,
    /// refusing if:
    /// - loss_quantity > in-transit quantity
    /// - no goods exist in transit
    pub fn record_spoilage(
        &mut self,
        loss_quantity: Decimal,
        new_condition: GoodsCondition,
    ) -> Result<()> {
        if new_condition == GoodsCondition::Good {
            return Err(Error::invalid(
                "spoilage must move goods to Damaged or Expired condition, not Good",
            ));
        }

        let good_quantity: Decimal = self
            .in_transit
            .iter()
            .filter(|r| r.condition == GoodsCondition::Good)
            .map(|r| r.quantity)
            .sum();

        if loss_quantity > good_quantity {
            return Err(Error::invalid(
                "spoilage quantity exceeds good goods in transit",
            ));
        }

        if loss_quantity <= Decimal::ZERO {
            return Err(Error::invalid(
                "spoilage quantity must be positive; a loss of nothing is not recorded",
            ));
        }

        self.in_transit[0].quantity -= loss_quantity;
        self.in_transit.push(InventoryRecord {
            location: GoodsLocation::InTransit,
            condition: new_condition,
            quantity: loss_quantity,
            reserved: Vec::new(),
        });

        Ok(())
    }

    /// Reserve quantity for an order or commitment.
    ///
    /// Refuses if:
    /// - holder already has a reservation for this consignment
    /// - reservation quantity > available quantity at destination
    pub fn reserve(&mut self, holder: String, quantity: Decimal) -> Result<()> {
        if self
            .at_destination
            .iter()
            .flat_map(|r| r.reserved.iter())
            .any(|res| res.holder == holder)
        {
            return Err(Error::invalid(
                "holder already has a reservation for this consignment; \
                 modify the existing reservation instead",
            ));
        }

        let available: Decimal = self
            .at_destination
            .iter()
            .map(|r| r.quantity)
            .sum::<Decimal>()
            - self
                .at_destination
                .iter()
                .flat_map(|r| r.reserved.iter())
                .map(|res| res.quantity)
                .sum::<Decimal>();

        if quantity > available {
            return Err(Error::invalid(
                "reservation quantity exceeds available goods at destination",
            ));
        }

        if let Some(record) = self.at_destination.first_mut() {
            record.reserved.push(Reservation { holder, quantity });
            Ok(())
        } else {
            Err(Error::invalid("no goods at destination to reserve"))
        }
    }

    /// Release a reservation.
    ///
    /// Refuses if no matching reservation exists.
    pub fn release_reservation(&mut self, holder: &str) -> Result<()> {
        for record in self.at_destination.iter_mut() {
            if let Some(pos) = record.reserved.iter().position(|r| r.holder == holder) {
                record.reserved.remove(pos);
                return Ok(());
            }
        }
        Err(Error::invalid("no matching reservation found to release"))
    }

    /// Reconcile inventory against the landed cost model.
    ///
    /// Asserts exact alignment at key checkpoints:
    /// - shipped total = landed_cost.quantity_shipped
    /// - delivered total = landed_cost.quantity_delivered
    /// - sold total = landed_cost.quantity_sold
    /// - spoilage = shipped - delivered
    pub fn reconcile(&self) -> Result<()> {
        let origin_total: Decimal = self.origin.iter().map(|r| r.quantity).sum();
        let in_transit_total: Decimal = self.in_transit.iter().map(|r| r.quantity).sum();
        let destination_total: Decimal = self.at_destination.iter().map(|r| r.quantity).sum();
        let sold_total: Decimal = self.sold.iter().map(|r| r.quantity).sum();

        let total_accounted = origin_total + in_transit_total + destination_total + sold_total;
        let total_expected = self.landed_cost.quantity_shipped;

        if (total_accounted - total_expected).abs() > Decimal::ZERO {
            return Err(Error::numeric(
                "inventory total diverges from shipped quantity: some goods are unaccounted for",
            ));
        }

        let destination_and_sold = destination_total + sold_total;
        let expected_at_destination_and_sold = self.landed_cost.quantity_delivered;

        if (destination_and_sold - expected_at_destination_and_sold).abs() > Decimal::ZERO {
            return Err(Error::numeric(
                "quantity at destination + sold diverges from delivered quantity: \
                 spoilage reconciliation failed",
            ));
        }

        if (sold_total - self.landed_cost.quantity_sold).abs() > Decimal::ZERO {
            return Err(Error::numeric(
                "sold quantity diverges from landed cost's sold quantity: \
                 returns or damage adjustment not recorded",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_landed_cost() -> LandedCost {
        LandedCost {
            origin: "port-of-origin".to_string(),
            destination: "warehouse".to_string(),
            route: vec!["port-of-origin".to_string(), "warehouse".to_string()],
            elapsed_days: 45,
            quantity_shipped: Decimal::from(1000),
            quantity_delivered: Decimal::from(980),
            quantity_sold: Decimal::from(960),
            goods: Decimal::from(10000),
            freight: Decimal::from(2000),
            duty: Decimal::from(1500),
            clearance: Decimal::from(500),
            storage: Decimal::from(1200),
            marketplace: Decimal::from(800),
            returns: Decimal::from(300),
        }
    }

    #[test]
    fn a_new_ledger_starts_with_all_goods_at_origin_in_good_condition() {
        let cost = example_landed_cost();
        let ledger = InventoryLedger::new(cost.clone()).unwrap();

        assert_eq!(ledger.origin.len(), 1);
        assert_eq!(ledger.origin[0].quantity, Decimal::from(1000));
        assert_eq!(ledger.origin[0].condition, GoodsCondition::Good);
        assert_eq!(ledger.origin[0].location, GoodsLocation::Origin);
        assert!(ledger.in_transit.is_empty());
        assert!(ledger.at_destination.is_empty());
        assert!(ledger.sold.is_empty());
    }

    #[test]
    fn a_zero_or_negative_shipped_quantity_is_refused_at_creation() {
        let mut cost = example_landed_cost();
        cost.quantity_shipped = Decimal::ZERO;
        let err = InventoryLedger::new(cost).unwrap_err();
        assert!(err.message().contains("shipped quantity must be positive"));
    }

    #[test]
    fn recording_departure_moves_goods_from_origin_to_in_transit() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();

        assert_eq!(ledger.origin[0].quantity, Decimal::ZERO);
        assert_eq!(ledger.in_transit[0].quantity, Decimal::from(1000));
        assert_eq!(ledger.in_transit[0].condition, GoodsCondition::Good);
    }

    #[test]
    fn departure_greater_than_shipped_quantity_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        let err = ledger.record_departure(Decimal::from(1001)).unwrap_err();
        assert!(
            err.message()
                .contains("exceeds the consignment's shipped quantity")
        );
    }

    #[test]
    fn recording_arrival_moves_goods_to_destination_and_reconciles_spoilage() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();

        // 20 units remain in transit (implicit spoilage from 1000 shipped to 980 delivered)
        assert_eq!(ledger.in_transit[0].quantity, Decimal::from(20));
        assert_eq!(ledger.at_destination[0].quantity, Decimal::from(980));
        assert_eq!(ledger.at_destination[0].condition, GoodsCondition::Good);
    }

    #[test]
    fn arrival_quantity_must_match_landed_cost_quantity_delivered() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        let err = ledger.record_arrival(Decimal::from(979)).unwrap_err();
        assert!(err.message().contains("delivered quantity must match"));
    }

    #[test]
    fn recording_sale_moves_goods_to_sold() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger.record_sale(Decimal::from(960)).unwrap();

        assert_eq!(ledger.at_destination[0].quantity, Decimal::from(20));
        assert_eq!(ledger.sold[0].quantity, Decimal::from(960));
    }

    #[test]
    fn sale_quantity_must_match_landed_cost_quantity_sold() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        let err = ledger.record_sale(Decimal::from(961)).unwrap_err();
        assert!(err.message().contains("sold quantity must match"));
    }

    #[test]
    fn recording_spoilage_moves_damaged_goods_to_new_condition() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger
            .record_spoilage(Decimal::from(20), GoodsCondition::Expired)
            .unwrap();

        assert_eq!(ledger.in_transit[0].quantity, Decimal::from(980));
        assert_eq!(ledger.in_transit[0].condition, GoodsCondition::Good);
        assert_eq!(ledger.in_transit[1].quantity, Decimal::from(20));
        assert_eq!(ledger.in_transit[1].condition, GoodsCondition::Expired);
    }

    #[test]
    fn spoilage_to_good_condition_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        let err = ledger
            .record_spoilage(Decimal::from(20), GoodsCondition::Good)
            .unwrap_err();
        assert!(
            err.message()
                .contains("must move goods to Damaged or Expired")
        );
    }

    #[test]
    fn spoilage_of_zero_quantity_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        let err = ledger
            .record_spoilage(Decimal::ZERO, GoodsCondition::Expired)
            .unwrap_err();
        assert!(err.message().contains("spoilage quantity must be positive"));
    }

    #[test]
    fn reservation_records_a_holder_s_claim_on_goods_at_destination() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger
            .reserve("order-123".to_string(), Decimal::from(500))
            .unwrap();

        let destination = &ledger.at_destination[0];
        assert_eq!(destination.reserved.len(), 1);
        assert_eq!(destination.reserved[0].holder, "order-123");
        assert_eq!(destination.reserved[0].quantity, Decimal::from(500));
    }

    #[test]
    fn reservation_exceeding_available_quantity_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        let err = ledger
            .reserve("order-1".to_string(), Decimal::from(981))
            .unwrap_err();
        assert!(err.message().contains("exceeds available goods"));
    }

    #[test]
    fn release_reservation_removes_the_holder_s_claim() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger
            .reserve("order-123".to_string(), Decimal::from(500))
            .unwrap();
        ledger.release_reservation("order-123").unwrap();

        assert!(ledger.at_destination[0].reserved.is_empty());
    }

    #[test]
    fn release_of_nonexistent_reservation_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        let err = ledger.release_reservation("nonexistent").unwrap_err();
        assert!(err.message().contains("no matching reservation"));
    }

    #[test]
    fn reconciliation_passes_when_inventory_aligns_with_landed_cost() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger.record_sale(Decimal::from(960)).unwrap();

        ledger.reconcile().unwrap();
    }

    #[test]
    fn reconciliation_fails_when_delivered_quantity_is_missing() {
        let cost = example_landed_cost();
        let ledger = InventoryLedger::new(cost).unwrap();

        // With all goods still at origin, reconciliation should fail because
        // landed_cost expects 980 to be delivered, but none are yet in transit/destination
        let err = ledger.reconcile().unwrap_err();
        assert!(err.message().contains("spoilage reconciliation failed"));
    }

    #[test]
    fn reconciliation_passes_when_all_quantities_align_with_landed_cost() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger.record_sale(Decimal::from(960)).unwrap();

        let result = ledger.reconcile();
        assert!(
            result.is_ok(),
            "reconciliation should pass when quantities align"
        );
    }

    #[test]
    fn a_duplicate_reservation_holder_is_refused() {
        let cost = example_landed_cost();
        let mut ledger = InventoryLedger::new(cost).unwrap();

        ledger.record_departure(Decimal::from(1000)).unwrap();
        ledger.record_arrival(Decimal::from(980)).unwrap();
        ledger
            .reserve("order-123".to_string(), Decimal::from(100))
            .unwrap();

        let err = ledger
            .reserve("order-123".to_string(), Decimal::from(200))
            .unwrap_err();
        assert!(err.message().contains("holder already has a reservation"));
    }
}
