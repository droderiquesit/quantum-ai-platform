//! Versioned rule packs with declared invariants (REASON-001, -013, -014).
//!
//! A [`RulePack`] is plain data: a name, a version, rules, and the invariants
//! every conclusion must preserve. [`RulePack::evaluate`] forward-chains over
//! facts and returns a [`Verdict`] that cites the pack version and the rules
//! that fired, in order.
//!
//! **Determinism is structural.** Facts are folded into a `BTreeMap` before
//! anything reads them, rules are visited in id order, and nothing here takes
//! a clock, a random source or a model — so the facts' arrival order cannot
//! reach the verdict. A conclusion that would contradict an existing fact is
//! refused rather than overwritten, because last-writer-wins would make the
//! verdict depend on rule order.
//!
//! **Invariants are checked at every step, before the step is committed**, and
//! the refusal names the invariant and the rule that would have broken it. An
//! invariant checked only on the final state could be violated by a
//! conclusion that a later rule happened to mask.
//!
//! Integers only: quantities are minor units, so no float rounding can make
//! two evaluations of the same pack disagree.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A fact's value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(String),
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A declared formal condition over facts. Absent facts make a comparison
/// false, never an error: a rule about a fact nobody supplied does not fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cond {
    Cmp { fact: String, op: Op, value: Value },
    Present { fact: String },
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
}

impl Cond {
    fn holds(&self, facts: &BTreeMap<String, Value>) -> bool {
        match self {
            Cond::Present { fact } => facts.contains_key(fact),
            Cond::Cmp { fact, op, value } => facts.get(fact).is_some_and(|have| {
                // Values of different kinds compare unequal and unordered.
                if std::mem::discriminant(have) != std::mem::discriminant(value) {
                    return matches!(op, Op::Ne);
                }
                match op {
                    Op::Eq => have == value,
                    Op::Ne => have != value,
                    Op::Lt => have < value,
                    Op::Le => have <= value,
                    Op::Gt => have > value,
                    Op::Ge => have >= value,
                }
            }),
            Cond::And(all) => all.iter().all(|c| c.holds(facts)),
            Cond::Or(any) => any.iter().any(|c| c.holds(facts)),
            Cond::Not(inner) => !inner.holds(facts),
        }
    }
}

/// One rule: when `when` holds, conclude `fact = value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub when: Cond,
    pub fact: String,
    pub value: Value,
}

/// A declared invariant: the state `forbidden` describes must never be
/// reachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invariant {
    pub id: String,
    pub forbidden: Cond,
}

/// A fired rule and what it concluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStep {
    pub rule: String,
    pub fact: String,
    pub value: Value,
}

/// The outcome of one evaluation. Serialises byte-identically for equal
/// inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub pack: String,
    pub version: u32,
    /// Rules that fired, in firing order.
    pub trace: Vec<TraceStep>,
    /// Every fact after evaluation: the inputs plus what was derived.
    pub facts: BTreeMap<String, Value>,
}

/// A versioned set of rules and the invariants they must preserve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulePack {
    pub name: String,
    pub version: u32,
    pub rules: Vec<Rule>,
    pub invariants: Vec<Invariant>,
}

impl RulePack {
    /// The id of the first invariant the state violates, in id order.
    fn violated(&self, facts: &BTreeMap<String, Value>) -> Option<&Invariant> {
        self.invariants
            .iter()
            .filter(|i| i.forbidden.holds(facts))
            .min_by(|a, b| a.id.cmp(&b.id))
    }

    /// Forward-chain `inputs` to a fixpoint.
    ///
    /// Refuses a pack with duplicate rule ids, two different inputs for one
    /// fact, a rule that contradicts an existing fact, and any step that would
    /// violate an invariant — including an input state that already does.
    pub fn evaluate(&self, inputs: &[(String, Value)]) -> Result<Verdict> {
        let mut seen = BTreeSet::new();
        for r in &self.rules {
            if !seen.insert(&r.id) {
                return Err(Error::invalid(format!(
                    "rule pack '{}' declares rule '{}' twice; give each rule a unique id",
                    self.name, r.id
                )));
            }
        }
        let mut facts = BTreeMap::new();
        for (k, v) in inputs {
            if let Some(prev) = facts.insert(k.clone(), v.clone())
                && prev != *v
            {
                return Err(Error::invalid(format!(
                    "fact '{k}' was supplied with two different values; supply one"
                )));
            }
        }
        if let Some(inv) = self.violated(&facts) {
            return Err(Error::denied(format!(
                "the supplied facts already violate invariant '{}'; correct the facts",
                inv.id
            )));
        }
        let mut order: Vec<&Rule> = self.rules.iter().collect();
        order.sort_by(|a, b| a.id.cmp(&b.id));
        let mut trace = Vec::new();
        let mut fired = BTreeSet::new();
        // Each rule fires at most once, so the loop ends within rules.len()+1 sweeps.
        loop {
            let mut progressed = false;
            for rule in &order {
                if fired.contains(&rule.id) || !rule.when.holds(&facts) {
                    continue;
                }
                match facts.get(&rule.fact) {
                    Some(have) if *have != rule.value => {
                        return Err(Error::denied(format!(
                            "rule '{}' would set '{}' against an established value; \
                             the pack contradicts itself or the facts",
                            rule.id, rule.fact
                        )));
                    }
                    _ => {}
                }
                let mut next = facts.clone();
                next.insert(rule.fact.clone(), rule.value.clone());
                if let Some(inv) = self.violated(&next) {
                    return Err(Error::denied(format!(
                        "rule '{}' would violate invariant '{}'; the step is refused",
                        rule.id, inv.id
                    )));
                }
                facts = next;
                fired.insert(rule.id.clone());
                trace.push(TraceStep {
                    rule: rule.id.clone(),
                    fact: rule.fact.clone(),
                    value: rule.value.clone(),
                });
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        Ok(Verdict {
            pack: self.name.clone(),
            version: self.version,
            trace,
            facts,
        })
    }

    /// Re-evaluate and refuse a verdict that differs from what the pack
    /// produces: a tampered or stale trace is not accepted as support.
    pub fn verify(&self, inputs: &[(String, Value)], claimed: &Verdict) -> Result<()> {
        if self.evaluate(inputs)? == *claimed {
            Ok(())
        } else {
            Err(Error::denied(
                "the claimed verdict differs from re-evaluation of the pack; discard it",
            ))
        }
    }
}
