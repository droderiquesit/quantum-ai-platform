//! A typed knowledge graph and the axioms that close it (R12-REASON-3054).
//!
//! The symbolic stage could type a subject against a registry of names, and
//! judge one hypothesis's status move, and nothing else: no reasoning step
//! could say that a future is a dated contract, that a dated contract has an
//! expiry, or that a hypothesis about a future is therefore about something
//! that stops existing. An [`Ontology`] is that vocabulary as data — classes
//! in an is-a hierarchy, relations with a domain, a range, an optional
//! inverse and an optional transitivity — and [`Ontology::infer`] closes a set
//! of asserted facts under four axioms:
//!
//! 1. **is-a transitivity**: an instance of a class is an instance of every
//!    class above it;
//! 2. **domain/range typing**: a relation's subject must be an instance of its
//!    domain and its object of its range;
//! 3. **inverse relations**: `(a r b)` entails `(b inverse(r) a)`;
//! 4. **transitive relations**: `(a r b)` and `(b r c)` entail `(a r c)`.
//!
//! **Typing refuses rather than infers.** OWL would conclude from a domain
//! that the subject *is* of that class; here a relation whose ends are not
//! already typed to its domain and range is reported as a violation, because
//! a fact that silently retypes an instrument is a caller bug that survives
//! (the repository's second principle).
//!
//! **Every inferred fact carries its derivation**, naming the axiom and the
//! facts it was derived from, so a reader can rebuild the chain from the
//! record alone. Storage is `BTreeMap`/`BTreeSet` throughout so the closure,
//! and therefore the journaled record, is byte-identical across runs.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The upper bound on closure rounds. Each round adds at least one fact or
/// stops, and the facts are bounded by `entities² × relations`, so this is a
/// guard against a defect, not a tuning knob.
pub const MAX_ROUNDS: usize = 256;

/// A declared relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationSpec {
    /// The class every subject must already be an instance of.
    pub domain: String,
    /// The class every object must already be an instance of.
    pub range: String,
    /// The relation `(b inverse a)` entailed by every `(a this b)`.
    pub inverse: Option<String>,
    /// Whether `(a r b)` and `(b r c)` entail `(a r c)`.
    pub transitive: bool,
}

/// The vocabulary: classes, their parents, and relations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ontology {
    pub name: String,
    pub version: u32,
    /// Each class and its direct parents. A root has an empty set.
    pub classes: BTreeMap<String, BTreeSet<String>>,
    pub relations: BTreeMap<String, RelationSpec>,
}

/// An asserted or inferred fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Fact {
    /// `entity` is an instance of `class`.
    InstanceOf { entity: String, class: String },
    /// `subject relation object`.
    Related {
        subject: String,
        relation: String,
        object: String,
    },
}

impl Fact {
    pub fn instance(entity: &str, class: &str) -> Self {
        Self::InstanceOf {
            entity: entity.to_string(),
            class: class.to_string(),
        }
    }

    pub fn related(subject: &str, relation: &str, object: &str) -> Self {
        Self::Related {
            subject: subject.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        }
    }
}

/// Why a fact holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Derivation {
    /// It was given.
    Asserted,
    /// Axiom 1: `entity` is an instance of `child`, and `child` is-a `class`.
    IsA { from: Fact, parent_of: String },
    /// Axiom 3: the inverse of `from`.
    Inverse { from: Fact },
    /// Axiom 4: `left` then `right` under a transitive relation.
    Transitive { left: Fact, right: Fact },
}

/// A relation fact whose ends are not typed to the relation's declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TypingViolation {
    pub fact: Fact,
    /// `"domain"` or `"range"`.
    pub end: String,
    pub entity: String,
    pub expected: String,
}

/// The closure of an asserted set, every fact with its derivation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inference {
    pub ontology: String,
    pub version: u32,
    /// Every fact that holds, asserted or inferred, with why. Serialised as a
    /// list because a map keyed by an enum is not a JSON object.
    pub facts: Vec<(Fact, Derivation)>,
    /// Axiom 2's findings, checked against the closed typing so an is-a
    /// inference satisfies a domain its asserted class does not name.
    pub violations: Vec<TypingViolation>,
}

impl Inference {
    pub fn holds(&self, fact: &Fact) -> bool {
        self.facts.iter().any(|(f, _)| f == fact)
    }

    pub fn derivation(&self, fact: &Fact) -> Option<&Derivation> {
        self.facts.iter().find(|(f, _)| f == fact).map(|(_, d)| d)
    }

    /// Every class `entity` is an instance of, closed under is-a.
    pub fn classes_of(&self, entity: &str) -> BTreeSet<String> {
        self.facts
            .iter()
            .filter_map(|(f, _)| match f {
                Fact::InstanceOf { entity: e, class } if e == entity => Some(class.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn is_consistent(&self) -> bool {
        self.violations.is_empty()
    }
}

impl Ontology {
    pub fn new(name: &str, version: u32) -> Self {
        Self {
            name: name.to_string(),
            version,
            ..Self::default()
        }
    }

    /// Declare `class` beneath `parents`. Refuses a parent not yet declared,
    /// which also makes an is-a cycle impossible to write: a class can only
    /// name classes that existed before it.
    pub fn class(mut self, class: &str, parents: &[&str]) -> Result<Self> {
        if self.classes.contains_key(class) {
            return Err(Error::invalid(format!(
                "ontology '{}' already declares class '{class}'; declare each class once",
                self.name
            )));
        }
        for parent in parents {
            if !self.classes.contains_key(*parent) {
                return Err(Error::invalid(format!(
                    "class '{class}' names parent '{parent}', which ontology '{}' does not \
                     declare; declare the parent first",
                    self.name
                )));
            }
        }
        self.classes.insert(
            class.to_string(),
            parents.iter().map(|p| p.to_string()).collect(),
        );
        Ok(self)
    }

    /// Declare a relation. Refuses an undeclared domain or range, and an
    /// inverse whose own declaration does not swap them back.
    pub fn relation(mut self, name: &str, spec: RelationSpec) -> Result<Self> {
        for (end, class) in [("domain", &spec.domain), ("range", &spec.range)] {
            if !self.classes.contains_key(class) {
                return Err(Error::invalid(format!(
                    "relation '{name}' has {end} '{class}', which ontology '{}' does not \
                     declare; declare the class first",
                    self.name
                )));
            }
        }
        if let Some(inverse) = &spec.inverse
            && let Some(declared) = self.relations.get(inverse)
            && (declared.domain != spec.range || declared.range != spec.domain)
        {
            return Err(Error::invalid(format!(
                "relation '{name}' names '{inverse}' as its inverse, but '{inverse}' runs \
                 {} -> {} rather than {} -> {}; an inverse swaps domain and range",
                declared.domain, declared.range, spec.range, spec.domain
            )));
        }
        if self.relations.contains_key(name) {
            return Err(Error::invalid(format!(
                "ontology '{}' already declares relation '{name}'; declare each relation once",
                self.name
            )));
        }
        self.relations.insert(name.to_string(), spec);
        Ok(self)
    }

    /// Close `asserted` under the four axioms.
    ///
    /// Refuses a fact naming an undeclared class or relation: reasoning over a
    /// word the vocabulary does not hold would produce conclusions nobody can
    /// check against a definition.
    pub fn infer(&self, asserted: &[Fact]) -> Result<Inference> {
        for (name, spec) in &self.relations {
            if let Some(inverse) = &spec.inverse
                && !self.relations.contains_key(inverse)
            {
                return Err(Error::invalid(format!(
                    "relation '{name}' names '{inverse}' as its inverse, which ontology '{}' \
                     does not declare; an undeclared inverse would be inferred with no domain \
                     or range to type it against, so declare it",
                    self.name
                )));
            }
        }
        let mut facts: BTreeMap<Fact, Derivation> = BTreeMap::new();
        for fact in asserted {
            match fact {
                Fact::InstanceOf { class, .. } if !self.classes.contains_key(class) => {
                    return Err(Error::invalid(format!(
                        "an asserted fact types an entity as '{class}', which ontology '{}' \
                         does not declare; declare the class or correct the fact",
                        self.name
                    )));
                }
                Fact::Related { relation, .. } if !self.relations.contains_key(relation) => {
                    return Err(Error::invalid(format!(
                        "an asserted fact uses relation '{relation}', which ontology '{}' does \
                         not declare; declare the relation or correct the fact",
                        self.name
                    )));
                }
                _ => {}
            }
            facts.entry(fact.clone()).or_insert(Derivation::Asserted);
        }
        for _ in 0..MAX_ROUNDS {
            let new = self.round(&facts);
            if new.is_empty() {
                let violations = self.violations(&facts);
                return Ok(Inference {
                    ontology: self.name.clone(),
                    version: self.version,
                    facts: facts.into_iter().collect(),
                    violations,
                });
            }
            for (fact, why) in new {
                facts.entry(fact).or_insert(why);
            }
        }
        Err(Error::numeric(format!(
            "ontology '{}' did not close within {MAX_ROUNDS} rounds; the closure is bounded, so \
             this is a defect in the reasoner rather than in the facts",
            self.name
        )))
    }

    /// One round of axioms 1, 3 and 4 over the facts so far: only facts not
    /// already held, each with the first derivation found in key order.
    fn round(&self, facts: &BTreeMap<Fact, Derivation>) -> BTreeMap<Fact, Derivation> {
        let mut new = BTreeMap::new();
        let mut offer = |fact: Fact, why: Derivation| {
            if !facts.contains_key(&fact) {
                new.entry(fact).or_insert(why);
            }
        };
        for fact in facts.keys() {
            match fact {
                Fact::InstanceOf { entity, class } => {
                    for parent in self.classes.get(class).into_iter().flatten() {
                        offer(
                            Fact::instance(entity, parent),
                            Derivation::IsA {
                                from: fact.clone(),
                                parent_of: class.clone(),
                            },
                        );
                    }
                }
                Fact::Related {
                    subject,
                    relation,
                    object,
                } => {
                    let Some(spec) = self.relations.get(relation) else {
                        continue;
                    };
                    if let Some(inverse) = &spec.inverse {
                        offer(
                            Fact::related(object, inverse, subject),
                            Derivation::Inverse { from: fact.clone() },
                        );
                    }
                    if spec.transitive {
                        for right in facts.keys() {
                            if let Fact::Related {
                                subject: s2,
                                relation: r2,
                                object: o2,
                            } = right
                                && r2 == relation
                                && s2 == object
                            {
                                offer(
                                    Fact::related(subject, relation, o2),
                                    Derivation::Transitive {
                                        left: fact.clone(),
                                        right: right.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        new
    }

    fn violations(&self, facts: &BTreeMap<Fact, Derivation>) -> Vec<TypingViolation> {
        let typed = |entity: &str, class: &str| facts.contains_key(&Fact::instance(entity, class));
        let mut found = BTreeSet::new();
        for fact in facts.keys() {
            let Fact::Related {
                subject,
                relation,
                object,
            } = fact
            else {
                continue;
            };
            let Some(spec) = self.relations.get(relation) else {
                continue;
            };
            for (end, entity, expected) in [
                ("domain", subject, &spec.domain),
                ("range", object, &spec.range),
            ] {
                if !typed(entity, expected) {
                    found.insert(TypingViolation {
                        fact: fact.clone(),
                        end: end.to_string(),
                        entity: entity.clone(),
                        expected: expected.clone(),
                    });
                }
            }
        }
        found.into_iter().collect()
    }
}
