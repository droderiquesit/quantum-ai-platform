//! The Crypto/On-chain Brain (R12-ASSET-3017): the specialist that reads the
//! confirmed chain for protocol state, the bridge transfers a reorganisation
//! failed, pool liquidity and on-chain flow.
//!
//! The kernel holds on-chain state through `qip-chain` and counted
//! reorganisations and failed bridge transfers, and no specialist read any of
//! it, so a chain reorganised under the book reached the chief as nothing.
//! It reads the desk's chain slot, which the kernel writes from its confirmed
//! view every REASON (`Platform::brief_the_desk_on_chain`), so a reorg is the
//! kernel's fact and not this agent's inference.
//!
//! **No chain is no data, never a quiet chain.** On-chain capture waits on a
//! licensed source record (ADR 0157), so on every deployed binary today the
//! slot is unobserved and the finding says that, naming what is missing.
//! Token economics and smart-contract risk have no source in the tree; the
//! finding's caveats say so rather than reporting them as clean.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Reads the confirmed chain: reorgs, failed bridges, pools and flow.
#[derive(Debug)]
pub struct CryptoOnChainAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl CryptoOnChainAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for CryptoOnChainAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let chain = self.desk.chain.get(ctx)?;
        if !chain.observed {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no chain is observed: on-chain capture waits on a licensed source record \
                 (ADR 0157), so there is no protocol state, bridge, pool or flow to read",
            ));
        }
        // Statistics, not money: reserves are read only to say whether a
        // pool holds any liquidity, and nothing here sizes.
        let liquid = chain
            .pools
            .iter()
            .filter(|pool| pool.reserve_base.is_positive() && pool.reserve_quote.is_positive())
            .count();
        let claim = format!(
            "{} reorganisation(s), the deepest {} block(s); {} bridge transfer(s) failed by one; \
             {} pool(s), {liquid} holding liquidity",
            chain.reorgs,
            chain.deepest_reorg,
            chain.bridged_transfers_failed,
            chain.pools.len()
        );
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .fact(computed(
                ctx,
                "chain_reorg_depth",
                f64::from(chain.deepest_reorg),
                "blocks",
                &["chain"],
            ))
            .fact(computed(
                ctx,
                "chain_reorgs",
                chain.reorgs as f64,
                "count",
                &["chain"],
            ))
            .fact(computed(
                ctx,
                "bridge_transfers_failed",
                chain.bridged_transfers_failed as f64,
                "count",
                &["chain"],
            ))
            .fact(computed(
                ctx,
                "amm_pools",
                chain.pools.len() as f64,
                "count",
                &["chain"],
            ))
            .fact(computed(
                ctx,
                "amm_pools_with_liquidity",
                liquid as f64,
                "count",
                &["chain"],
            ))
            .caveats(vec![
                "reads the chain as the kernel last confirmed it into the desk's slot".to_string(),
                "token economics and smart-contract risk: no source in the tree carries either"
                    .to_string(),
            ]);
        // Flow only where the chain is deep enough to confirm: an unconfirmed
        // count is not a number anybody confirmed.
        if let Some(trades) = chain.confirmed_trades {
            builder = builder.fact(computed(
                ctx,
                "onchain_trades",
                trades as f64,
                "count",
                &["chain"],
            ));
        }
        builder.build()
    }
}
