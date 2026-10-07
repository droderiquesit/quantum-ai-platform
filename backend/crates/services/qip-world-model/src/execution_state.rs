//! Execution state tracking: current and historical execution record.
//!
//! Tracks fill rates, execution quality, and time-in-force patterns to help
//! the execution engine understand market microstructure behavior.

use qip_contracts::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum fill history to retain
const EXECUTION_HISTORY_LIMIT: usize = 1000;

/// Execution quality metrics
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub executed_at: Timestamp,
    /// Total size requested
    pub requested_size: Decimal,
    /// Size actually filled
    pub filled_size: Decimal,
    /// Average execution price
    pub avg_price: Decimal,
    /// Initial mid-price
    pub entry_mid: Decimal,
    /// Whether this was buy (true) or sell (false)
    pub is_buy: bool,
    /// Whether order was fully filled
    pub fully_filled: bool,
    /// Time from entry to full fill
    pub fill_time_ms: u64,
}

impl ExecutionRecord {
    /// Fill rate as percentage
    pub fn fill_rate(&self) -> f64 {
        if self.requested_size.is_zero() {
            0.0
        } else {
            (self.filled_size.to_f64() / self.requested_size.to_f64()) * 100.0
        }
    }

    /// Execution price vs entry mid
    pub fn price_improvement_bps(&self) -> f64 {
        let mid = self.entry_mid.to_f64();
        let executed = self.avg_price.to_f64();
        if mid == 0.0 {
            0.0
        } else {
            if self.is_buy {
                // Improvement is when we paid less than mid
                ((mid - executed) / mid) * 10000.0
            } else {
                // Improvement is when we received more than mid
                ((executed - mid) / mid) * 10000.0
            }
        }
    }
}

/// Execution statistics for a venue
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueExecutionStats {
    pub venue: VenueId,
    /// Number of orders executed
    pub order_count: u64,
    /// Average fill rate percentage
    pub avg_fill_rate: f64,
    /// Fully filled orders / total orders
    pub full_fill_percentage: f64,
    /// Median execution time in ms
    pub median_fill_time_ms: u64,
    /// Average price improvement in bps
    pub avg_improvement_bps: f64,
    /// Std dev of fill rates
    pub fill_rate_std_dev: f64,
}

/// Execution state for one instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionState {
    /// Execution history per venue (bounded)
    history: BTreeMap<VenueId, Vec<ExecutionRecord>>,
    /// Current stats per venue (updated from history)
    stats: BTreeMap<VenueId, VenueExecutionStats>,
}

impl Default for ExecutionState {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionState {
    pub fn new() -> Self {
        Self {
            history: BTreeMap::new(),
            stats: BTreeMap::new(),
        }
    }

    /// Record an execution
    pub fn record_execution(&mut self, record: ExecutionRecord) {
        let venue_history = self.history.entry(record.venue.clone()).or_default();
        venue_history.push(record.clone());
        if venue_history.len() > EXECUTION_HISTORY_LIMIT {
            venue_history.remove(0);
        }

        // Update stats
        self.update_stats(record.venue);
    }

    /// Recalculate statistics for a venue
    fn update_stats(&mut self, venue: VenueId) {
        if let Some(records) = self.history.get(&venue) {
            if records.is_empty() {
                return;
            }

            let order_count = records.len() as u64;
            let fill_rates: Vec<f64> = records.iter().map(|r| r.fill_rate()).collect();
            let avg_fill_rate = fill_rates.iter().sum::<f64>() / fill_rates.len() as f64;

            let full_fills = records.iter().filter(|r| r.fully_filled).count();
            let full_fill_percentage = (full_fills as f64 / records.len() as f64) * 100.0;

            let mut fill_times: Vec<u64> = records.iter().map(|r| r.fill_time_ms).collect();
            fill_times.sort();
            let median_fill_time_ms = if fill_times.len().is_multiple_of(2) {
                (fill_times[fill_times.len() / 2 - 1] + fill_times[fill_times.len() / 2]) / 2
            } else {
                fill_times[fill_times.len() / 2]
            };

            let improvements: Vec<f64> =
                records.iter().map(|r| r.price_improvement_bps()).collect();
            let avg_improvement_bps = improvements.iter().sum::<f64>() / improvements.len() as f64;

            let variance = fill_rates
                .iter()
                .map(|x| (x - avg_fill_rate).powi(2))
                .sum::<f64>()
                / fill_rates.len() as f64;
            let fill_rate_std_dev = variance.sqrt();

            self.stats.insert(
                venue.clone(),
                VenueExecutionStats {
                    venue,
                    order_count,
                    avg_fill_rate,
                    full_fill_percentage,
                    median_fill_time_ms,
                    avg_improvement_bps,
                    fill_rate_std_dev,
                },
            );
        }
    }

    /// Get stats for a venue
    pub fn stats(&self, venue: VenueId) -> Option<VenueExecutionStats> {
        self.stats.get(&venue).cloned()
    }

    /// Get all current stats
    pub fn all_stats(&self) -> Vec<VenueExecutionStats> {
        self.stats.values().cloned().collect()
    }

    /// Get recent execution history for a venue
    pub fn recent_executions(&self, venue: VenueId, limit: usize) -> Vec<ExecutionRecord> {
        self.history
            .get(&venue)
            .map(|h| h.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fill_rate_calculation() {
        let record = ExecutionRecord {
            object_id: ObjectId::from_string("test-exec-1"),
            venue: VenueId::new("NYSE"),
            executed_at: Timestamp::from_secs(0),
            requested_size: Decimal::from(1000),
            filled_size: Decimal::from(800),
            avg_price: Decimal::parse("100.50").unwrap(),
            entry_mid: Decimal::parse("100.00").unwrap(),
            is_buy: true,
            fully_filled: false,
            fill_time_ms: 5000,
        };
        assert!((record.fill_rate() - 80.0).abs() < 0.1);
    }

    #[test]
    fn test_price_improvement_tracking() {
        let record = ExecutionRecord {
            object_id: ObjectId::from_string("test-exec-2"),
            venue: VenueId::new("NASDAQ"),
            executed_at: Timestamp::from_secs(0),
            requested_size: Decimal::from(1000),
            filled_size: Decimal::from(1000),
            avg_price: Decimal::parse("99.90").unwrap(),
            entry_mid: Decimal::parse("100.00").unwrap(),
            is_buy: true,
            fully_filled: true,
            fill_time_ms: 3000,
        };
        let improvement = record.price_improvement_bps();
        assert!(improvement > 0.0); // Better price than mid
    }

    #[test]
    fn test_execution_state_stats() {
        let mut state = ExecutionState::new();
        for i in 0..20 {
            let record = ExecutionRecord {
                object_id: ObjectId::from_string(format!("test-exec-{}", i)),
                venue: VenueId::new("NYSE"),
                executed_at: Timestamp::from_secs(0),
                requested_size: Decimal::from(1000),
                filled_size: Decimal::from(900 + i),
                avg_price: Decimal::parse("100.50").unwrap(),
                entry_mid: Decimal::parse("100.00").unwrap(),
                is_buy: true,
                fully_filled: true,
                fill_time_ms: 5000,
            };
            state.record_execution(record);
        }
        let stats = state.stats(VenueId::new("NYSE"));
        assert!(stats.is_some());
        assert_eq!(stats.unwrap().order_count, 20);
    }
}
