//! Company and macro twins over a supply network (R12-WORLD-9037; §11.5).
//!
//! The future tree carries a world driver to instruments through their
//! betas. It cannot say that a strike at a supplier reaches the supplier's
//! customers, and their customers in turn, because it holds no supply chain.
//! A [`BusinessTwin`] does.
//!
//! # A compact state, with a source for every link
//!
//! The twin holds two kinds of links and nothing else:
//! - supply links: a customer takes a stated share of its input from a
//!   supplier;
//! - macro links: an entity's output moves by a stated elasticity of a macro
//!   driver.
//!
//! Its size depends on the links, not on how much history produced them.
//! Every link names the source it came from, and every effect the twin
//! reports names the links it passed through. A reader can therefore check
//! a propagated number against the claims it rests on, which is the
//! twin's source manifest.
//!
//! # Linear, bounded propagation
//!
//! A shock is a fractional change in an entity's output. A customer's
//! change is the share-weighted sum of its suppliers' changes: a customer
//! taking 40% of its input from a supplier that loses 10% of its output
//! loses 4%. Effects are computed as a bounded fixed point over at most
//! `max_hops` rounds, so a cycle in the network cannot run unbounded.
//! A customer's shares are refused if they sum to more than one, so a chain
//! whose shares sum to less than one attenuates; a cycle whose shares reach
//! one does not, and grows round by round until the cap. An output cannot
//! fall by more than all of itself, so a shock or a carried change below
//! -100% is refused rather than reported. The arithmetic is
//! statistics over fractions; no money amount crosses here.
//!
//! Everything is deterministic: the same links and shock give the same
//! effects, in entity order.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The tolerance on a customer's shares summing to one, for rounding in the
/// source data. Beyond it the shares are refused, because inputs over 100%
/// would make every round amplify a shock instead of carrying it.
pub const SHARE_TOLERANCE: f64 = 1e-6;

/// The most rounds a shock is carried. A cap on work, not a forecast about
/// how far a shock travels.
pub const MAX_TWIN_HOPS: usize = 8;

/// A change below this fraction of the original shock is not reported: it
/// cannot be told from rounding in the shares it was carried through.
pub const EFFECT_FLOOR: f64 = 1e-4;

/// A customer takes `share` of its input from a supplier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupplyLink {
    pub supplier: String,
    pub customer: String,
    /// The customer's share of its input from this supplier, in `(0, 1]`.
    pub share: f64,
    pub source: String,
}

/// An entity's output moves by `elasticity` times a macro driver's change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroLink {
    pub driver: String,
    pub entity: String,
    pub elasticity: f64,
    pub source: String,
}

/// What a shock did to one entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityEffect {
    /// The fractional change in the entity's output.
    pub change: f64,
    /// The round in which the shock first reached the entity: zero for an
    /// entity the shock landed on directly.
    pub hops: usize,
    /// The sources of every link the effect was carried through.
    pub sources: BTreeSet<String>,
}

/// What one shock did across the twin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TwinResponse {
    /// The entity or macro driver shocked.
    pub origin: String,
    pub change: f64,
    pub effects: BTreeMap<String, EntityEffect>,
    /// Whether effects were still changing when the hop cap was reached.
    pub truncated: bool,
}

/// A company and macro twin. See the module doc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BusinessTwin {
    /// Supply links by customer.
    supply: BTreeMap<String, Vec<SupplyLink>>,
    /// Macro links by driver.
    exposure: BTreeMap<String, Vec<MacroLink>>,
}

impl BusinessTwin {
    /// Build a twin. Refuses:
    /// - a blank name or source, and a self-supplying link;
    /// - a share outside `(0, 1]` and a non-finite elasticity;
    /// - a customer whose shares sum past one (beyond [`SHARE_TOLERANCE`]);
    /// - a link stated twice.
    ///
    /// A bad link is refused rather than clamped or dropped, so the twin
    /// never carries a number nobody stated.
    pub fn new(supply: Vec<SupplyLink>, exposure: Vec<MacroLink>) -> Result<Self> {
        let blank = |s: &str| s.trim().is_empty();
        let mut by_customer: BTreeMap<String, Vec<SupplyLink>> = BTreeMap::new();
        for link in supply {
            if blank(&link.supplier) || blank(&link.customer) || blank(&link.source) {
                return Err(Error::invalid(
                    "a supply link names its supplier, customer and source",
                ));
            }
            if link.supplier == link.customer {
                return Err(Error::invalid(format!(
                    "{} is stated as its own supplier; a self-link would carry every shock \
                     back to where it started",
                    link.supplier
                )));
            }
            if !(link.share.is_finite() && link.share > 0.0 && link.share <= 1.0) {
                return Err(Error::invalid(format!(
                    "{} takes a share of {} from {}, which is not a share in (0, 1]",
                    link.customer, link.share, link.supplier
                )));
            }
            let links = by_customer.entry(link.customer.clone()).or_default();
            if links.iter().any(|l| l.supplier == link.supplier) {
                return Err(Error::invalid(format!(
                    "the supply of {} to {} is stated twice; state it once",
                    link.supplier, link.customer
                )));
            }
            links.push(link);
        }
        for (customer, links) in &by_customer {
            let total: f64 = links.iter().map(|l| l.share).sum();
            if total > 1.0 + SHARE_TOLERANCE {
                return Err(Error::invalid(format!(
                    "{customer}'s stated supply shares sum to {total}, more than all of its \
                     input; correct the shares rather than have a shock amplified"
                )));
            }
        }
        let mut by_driver: BTreeMap<String, Vec<MacroLink>> = BTreeMap::new();
        for link in exposure {
            if blank(&link.driver) || blank(&link.entity) || blank(&link.source) {
                return Err(Error::invalid(
                    "a macro link names its driver, entity and source",
                ));
            }
            if !link.elasticity.is_finite() {
                return Err(Error::invalid(format!(
                    "{}'s elasticity to {} is {}, which is not a number",
                    link.entity, link.driver, link.elasticity
                )));
            }
            let links = by_driver.entry(link.driver.clone()).or_default();
            if links.iter().any(|l| l.entity == link.entity) {
                return Err(Error::invalid(format!(
                    "{}'s exposure to {} is stated twice; state it once",
                    link.entity, link.driver
                )));
            }
            links.push(link);
        }
        Ok(Self {
            supply: by_customer,
            exposure: by_driver,
        })
    }

    /// Every source the twin's links rest on.
    pub fn manifest(&self) -> BTreeSet<String> {
        self.supply
            .values()
            .flatten()
            .map(|l| l.source.clone())
            .chain(self.exposure.values().flatten().map(|l| l.source.clone()))
            .collect()
    }

    /// The number of links the twin holds: its whole state.
    pub fn len(&self) -> usize {
        self.supply.values().map(Vec::len).sum::<usize>()
            + self.exposure.values().map(Vec::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A shock of `change` (a fraction of output) to one entity, carried to
    /// its customers and theirs. Refuses a non-finite change and a hop cap
    /// past [`MAX_TWIN_HOPS`].
    pub fn shock_entity(&self, entity: &str, change: f64, max_hops: usize) -> Result<TwinResponse> {
        let seed = BTreeMap::from([(entity.to_string(), (change, BTreeSet::new()))]);
        self.carry(entity, change, seed, max_hops)
    }

    /// A shock of `change` to a macro driver, landing on each exposed entity
    /// at its elasticity and then carried through the supply network.
    /// Refuses a driver no entity is exposed to: a shock that lands nowhere
    /// would read as one with no effect.
    pub fn shock_macro(&self, driver: &str, change: f64, max_hops: usize) -> Result<TwinResponse> {
        let links = self.exposure.get(driver).ok_or_else(|| {
            Error::invalid(format!(
                "no entity in the twin is exposed to {driver}; state an exposure before \
                 shocking it"
            ))
        })?;
        let seed = links
            .iter()
            .map(|l| {
                (
                    l.entity.clone(),
                    (l.elasticity * change, BTreeSet::from([l.source.clone()])),
                )
            })
            .collect();
        self.carry(driver, change, seed, max_hops)
    }

    /// The bounded fixed point: each round, every customer's change is the
    /// share-weighted sum of its suppliers' changes, plus whatever the shock
    /// put on it directly.
    fn carry(
        &self,
        origin: &str,
        change: f64,
        seed: BTreeMap<String, (f64, BTreeSet<String>)>,
        max_hops: usize,
    ) -> Result<TwinResponse> {
        if !change.is_finite() {
            return Err(Error::invalid(format!(
                "a shock of {change} to {origin} is not a number"
            )));
        }
        if max_hops == 0 || max_hops > MAX_TWIN_HOPS {
            return Err(Error::invalid(format!(
                "a shock is carried between 1 and {MAX_TWIN_HOPS} hops, not {max_hops}"
            )));
        }
        let below_all = |entity: &str, delta: f64| {
            Error::invalid(format!(
                "the shock to {origin} would take {entity}'s output down by {:.1}%, more than \
                 all of it; the twin is linear and cannot carry a change that large",
                -delta * 100.0
            ))
        };
        let floor = EFFECT_FLOOR * change.abs();
        let mut effects: BTreeMap<String, EntityEffect> = seed
            .iter()
            .map(|(entity, (delta, sources))| {
                (
                    entity.clone(),
                    EntityEffect {
                        change: *delta,
                        hops: 0,
                        sources: sources.clone(),
                    },
                )
            })
            .collect();
        let mut truncated = false;
        // One round past the cap only measures whether effects were still
        // moving; its result is not kept.
        for round in 1..=max_hops + 1 {
            let mut next: BTreeMap<String, EntityEffect> = BTreeMap::new();
            for (customer, links) in &self.supply {
                let mut delta = seed.get(customer).map_or(0.0, |(d, _)| *d);
                let mut sources = seed
                    .get(customer)
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default();
                for link in links {
                    if let Some(upstream) = effects.get(&link.supplier) {
                        delta += link.share * upstream.change;
                        sources.insert(link.source.clone());
                        sources.extend(upstream.sources.iter().cloned());
                    }
                }
                if delta.abs() >= floor && delta != 0.0 {
                    let hops = effects.get(customer).map_or(round, |e| e.hops);
                    next.insert(
                        customer.clone(),
                        EntityEffect {
                            change: delta,
                            hops,
                            sources,
                        },
                    );
                }
            }
            // An entity no supply link feeds keeps what the shock put on it.
            for (entity, effect) in &effects {
                if !self.supply.contains_key(entity) {
                    next.insert(entity.clone(), effect.clone());
                }
            }
            let moved = next.len() != effects.len()
                || next.iter().any(|(entity, effect)| {
                    effects
                        .get(entity)
                        .is_none_or(|old| (old.change - effect.change).abs() > floor)
                });
            if round > max_hops {
                truncated = moved;
                break;
            }
            if let Some((entity, effect)) = next.iter().find(|(_, e)| e.change < -1.0) {
                return Err(below_all(entity, effect.change));
            }
            effects = next;
            if !moved {
                break;
            }
        }
        Ok(TwinResponse {
            origin: origin.to_string(),
            change,
            effects,
            truncated,
        })
    }
}
