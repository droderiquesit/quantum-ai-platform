//! Typed drivers for synthetic worlds (R12-TICK-3097; ADR 0151 decision 5).
//!
//! [`crate::futures::FutureTree::grow`] varies shock size, propagation and
//! sign per driver, and a [`crate::futures::WorldDriver`] says nothing about
//! *what kind* of thing it is. A world that varies policy, weather, supply
//! or default assumptions therefore had no way to declare it, and nothing
//! could tell a branch a feed could ever have produced from one that is
//! pure hypothesis.
//!
//! [`DriverKind`] is that declaration, and [`SyntheticWorlds`] grows a tree
//! from kinded drivers and remembers each branch's kind. Only [`DriverKind::Price`]
//! and [`DriverKind::OwnAction`] have a producer in this repository; the other
//! four are hypotheses no feed measures, so [`SyntheticWorlds::feed_backed`]
//! — the only accessor meant for anything that reasons about the real
//! market — never returns a branch of those kinds.
//!
//! Kept beside `futures.rs` rather than inside it so `WorldDriver`'s literal
//! construction sites, in several crates, are untouched.

use crate::futures::{Branch, FutureTree, TreeConfig, TwinState, WorldDriver};
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What a driver is a hypothesis about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    Policy,
    Weather,
    Default,
    Supply,
    OwnAction,
    Price,
}

impl DriverKind {
    /// Every kind, in a fixed order.
    pub const ALL: [Self; 6] = [
        Self::Policy,
        Self::Weather,
        Self::Default,
        Self::Supply,
        Self::OwnAction,
        Self::Price,
    ];

    /// Whether anything in this repository produces data of this kind
    /// (ADR 0151 decision 5: only `OwnAction` and `Price` do).
    pub const fn has_feed_producer(self) -> bool {
        matches!(self, Self::OwnAction | Self::Price)
    }
}

/// A driver with its declared kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KindedDriver {
    pub kind: DriverKind,
    pub driver: WorldDriver,
}

/// A tree grown from kinded drivers, remembering what each branch is about.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticWorlds {
    tree: FutureTree,
    kind_by_driver: BTreeMap<String, DriverKind>,
}

impl SyntheticWorlds {
    /// Grow the tree. Refuses a duplicate driver id (the id names its
    /// branches, so two drivers would share a kind they may not have), an id
    /// containing `:` (the branch id's separator, so the owner could not be
    /// read back), and a driver that reaches no instrument the twin knows —
    /// [`FutureTree::grow`] skips such a driver silently, and a declared kind
    /// that grew nothing would read as explored.
    pub fn grow(state: &TwinState, drivers: &[KindedDriver], config: TreeConfig) -> Result<Self> {
        let mut kind_by_driver = BTreeMap::new();
        for kinded in drivers {
            let id = &kinded.driver.id;
            if id.contains(':') {
                return Err(Error::invalid(format!(
                    "driver id {id:?} contains ':', the separator of its branch ids; rename it"
                )));
            }
            if kind_by_driver.insert(id.clone(), kinded.kind).is_some() {
                return Err(Error::invalid(format!(
                    "driver id {id:?} is declared twice; its branches would share one kind"
                )));
            }
        }
        let plain: Vec<WorldDriver> = drivers.iter().map(|k| k.driver.clone()).collect();
        let tree = FutureTree::grow(state, &plain, config)?;
        let worlds = Self {
            tree,
            kind_by_driver,
        };
        for kinded in drivers {
            if worlds
                .branches_of_driver(&kinded.driver.id)
                .next()
                .is_none()
            {
                return Err(Error::invalid(format!(
                    "driver {} of kind {:?} reaches no instrument the twin was fitted on, so it \
                     grew no world; name an instrument the twin knows",
                    kinded.driver.id, kinded.kind
                )));
            }
        }
        Ok(worlds)
    }

    /// The tree itself.
    pub const fn tree(&self) -> &FutureTree {
        &self.tree
    }

    fn branches_of_driver<'a>(&'a self, driver: &'a str) -> impl Iterator<Item = &'a Branch> {
        self.tree
            .branches
            .values()
            .filter(move |b| b.id.split(':').next() == Some(driver))
    }

    /// The kind a branch is about; `None` for the baseline and for a branch
    /// split from reality, which belong to no driver.
    pub fn kind_of(&self, branch_id: &str) -> Option<DriverKind> {
        let owner = branch_id.split(':').next()?;
        self.kind_by_driver.get(owner).copied()
    }

    /// Every branch of one kind, in id order.
    pub fn branches_of(&self, kind: DriverKind) -> Vec<&Branch> {
        self.tree
            .branches
            .values()
            .filter(|b| self.kind_of(&b.id) == Some(kind))
            .collect()
    }

    /// The kinds that grew at least one branch.
    pub fn kinds_grown(&self) -> BTreeSet<DriverKind> {
        self.tree
            .branches
            .keys()
            .filter_map(|id| self.kind_of(id))
            .collect()
    }

    /// Branches whose kind a feed could have produced. The only accessor
    /// meant for anything that reasons about the real market: a policy,
    /// weather, default or supply branch is a hypothesis nothing measures and
    /// is never returned here. The baseline, which assumes nothing, is.
    pub fn feed_backed(&self) -> Vec<&Branch> {
        self.tree
            .branches
            .values()
            .filter(|b| {
                self.kind_of(&b.id)
                    .is_none_or(DriverKind::has_feed_producer)
            })
            .collect()
    }
}
