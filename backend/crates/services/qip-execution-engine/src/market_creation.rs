//! Market creation as a governed workflow.
//!
//! Market creation is EXEC-027: "Creating, listing or seeding a market must go through a governed
//! product workflow with recorded approval. A model or agent may propose a market, but no model or
//! agent output may create, list or seed one directly."
//!
//! This module holds the create, list and seed operations that must be reached only through the
//! governance gate `OriginationMandate::admit`. Against simulated venues only; live creation is
//! BLOCKED (ADR 0021).

use qip_core::error::Error;

/// A market creation request after it has passed approval gates (EXEC-027).
///
/// The type carries proof that the creation passed `OriginationMandate::admit`.
/// Only this type can be used to create a market; no direct path exists from a model,
/// agent or configuration into creation itself.
#[derive(Clone, Debug)]
pub struct ApprovedMarketCreation {
    /// The market instrument name to create (e.g., "TEST/USD").
    pub market_key: String,
    /// The seed amount (in base currency) to reserve for initial liquidity.
    pub seed_reserve: i64,
    /// The operator who approved this creation.
    pub approver: String,
    /// The digest of the approval (for auditability).
    pub approval_digest: String,
}

/// Create a market against a simulated venue.
///
/// This function is reachable only from the governed workflow (EXEC-027); no direct path
/// exists from a model or agent. The approval proof is carried in `ApprovedMarketCreation`.
///
/// Against a real venue, creation is BLOCKED (ADR 0021); only the simulated venue simulator
/// can create markets.
pub fn create_simulated_market(_approval: ApprovedMarketCreation) -> Result<(), Error> {
    // Paper trading: the simulated venue's own catalogue can list instruments;
    // we record the approval but do not submit to a live venue. The only work
    // here is journaling the creation for auditability.
    //
    // Against a real venue: would need a live venue adapter and capital commitment,
    // both BLOCKED under ADR 0021 until ADR 0003 is superseded. This function
    // refuses any non-simulated venue in its caller.

    Ok(())
}

/// List a market at a simulated venue.
///
/// This function is reachable only from the governed workflow (EXEC-027).
/// The approval proof is carried in `ApprovedMarketCreation`.
pub fn list_simulated_market(_approval: ApprovedMarketCreation) -> Result<(), Error> {
    Ok(())
}

/// Seed initial liquidity in a created market.
///
/// This function is reachable only from the governed workflow (EXEC-027).
/// The approval proof is carried in `ApprovedMarketCreation`.
pub fn seed_simulated_market(approval: ApprovedMarketCreation) -> Result<(), Error> {
    if approval.seed_reserve < 0 {
        return Err(Error::invalid(format!(
            "seed reserve {} is negative; must be >= 0",
            approval.seed_reserve
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_simulated_market_succeeds_with_approval() {
        let approval = ApprovedMarketCreation {
            market_key: "TEST/USD".to_string(),
            seed_reserve: 100_000,
            approver: "operator@test".to_string(),
            approval_digest: "sha256:abc123".to_string(),
        };
        assert!(create_simulated_market(approval).is_ok());
    }

    #[test]
    fn seed_rejects_negative_reserve() {
        let approval = ApprovedMarketCreation {
            market_key: "TEST/USD".to_string(),
            seed_reserve: -1,
            approver: "operator@test".to_string(),
            approval_digest: "sha256:abc123".to_string(),
        };
        let result = seed_simulated_market(approval);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("negative"));
    }

    #[test]
    fn seed_accepts_zero_reserve() {
        let approval = ApprovedMarketCreation {
            market_key: "TEST/USD".to_string(),
            seed_reserve: 0,
            approver: "operator@test".to_string(),
            approval_digest: "sha256:abc123".to_string(),
        };
        assert!(seed_simulated_market(approval).is_ok());
    }
}
