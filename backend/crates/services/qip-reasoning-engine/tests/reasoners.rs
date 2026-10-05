//! The symbolic reasoners beside the rule engine: lifecycle admissibility
//! (REASON-011), event ordering (REASON-009), Bayesian networks (REASON-012),
//! constraint satisfaction (REASON-015) and its QUBO formulation (REASON-017).
//!
//! Each property is checked against a reference written here, in the test,
//! that shares no code with the reasoner it judges. A reasoner checked only
//! against itself agrees with itself however wrong it is.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

use qip_core::time::Timestamp;
use qip_numerics::anneal::solve_exact;
use qip_reasoning_engine::constraint::{
    Answer, Constraint, Form, MAX_ASSIGNMENTS, Problem, Variable,
};
use qip_reasoning_engine::lifecycle::Lifecycle;
use qip_reasoning_engine::network::{Network, Node, TOLERANCE};
use qip_reasoning_engine::rules::{Cond, Op, Value};
use qip_reasoning_engine::temporal::{Event, Order, Timeline};

/// Deterministic generator so a failing case is reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
    fn unit(&mut self) -> f64 {
        (self.next(1_000_000) as f64 + 1.0) / 1_000_001.0
    }
}

// ---- REASON-011 -----------------------------------------------------------

/// The fixture's transition relation, written a second time as a plain table.
/// The reference walk below reads this and nothing of the reasoner's.
const CONTRACT_MOVES: &[(&str, &str)] = &[
    ("draft", "signed"),
    ("draft", "cancelled"),
    ("signed", "active"),
    ("signed", "cancelled"),
    ("active", "suspended"),
    ("suspended", "active"),
    ("active", "settled"),
];

fn contract_lifecycle() -> Lifecycle {
    Lifecycle {
        name: "contract".into(),
        version: 3,
        initial: BTreeSet::from(["draft".to_string()]),
        transitions: CONTRACT_MOVES
            .iter()
            .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
            .collect(),
    }
}

/// `(index, from, to)` of the first move the table does not allow.
fn reference_break(sequence: &[&str]) -> Option<(usize, Option<String>, String)> {
    if sequence[0] != "draft" {
        return Some((0, None, sequence[0].to_string()));
    }
    for i in 1..sequence.len() {
        if !CONTRACT_MOVES.contains(&(sequence[i - 1], sequence[i])) {
            return Some((
                i,
                Some(sequence[i - 1].to_string()),
                sequence[i].to_string(),
            ));
        }
    }
    None
}

#[test]
fn the_fabric_admits_exactly_the_sequences_a_lifecycle_allows_and_names_the_first_break() {
    // The failure this prevents: a checker that says "inadmissible" without
    // saying where, or that names a later move than the one that broke the
    // sequence, sends the reader to the wrong transition.
    let lifecycle = contract_lifecycle();
    // "void" is in no transition: an undeclared state must break, not pass.
    let states = [
        "draft",
        "signed",
        "active",
        "suspended",
        "settled",
        "cancelled",
        "void",
    ];
    let mut g = Lcg(11);
    let (mut admitted, mut refused, mut refused_late) = (0, 0, 0);
    for _ in 0..4000 {
        let length = 1 + g.next(6) as usize;
        // Bias towards the opening state so admissible sequences occur.
        let sequence: Vec<&str> = (0..length)
            .map(|i| {
                if i == 0 && g.next(3) > 0 {
                    "draft"
                } else {
                    states[g.next(states.len() as u64) as usize]
                }
            })
            .collect();
        let verdict = lifecycle.judge(&sequence).unwrap();
        assert_eq!(
            (verdict.lifecycle.as_str(), verdict.version),
            ("contract", 3)
        );
        let found = verdict.first_break.clone().map(|b| (b.index, b.from, b.to));
        assert_eq!(found, reference_break(&sequence), "sequence {sequence:?}");
        match &verdict.first_break {
            None => {
                assert!(verdict.is_admissible());
                admitted += 1;
            }
            Some(broke) => {
                assert!(!verdict.is_admissible());
                refused += 1;
                // It is the *first* break: everything before it is admissible.
                if broke.index > 0 {
                    assert!(
                        lifecycle
                            .judge(&sequence[..broke.index])
                            .unwrap()
                            .is_admissible()
                    );
                    refused_late += 1;
                }
            }
        }
    }
    // The premise: the generator produced both verdicts, and breaks that were
    // not simply the opening state.
    assert!(
        admitted > 100 && refused > 100 && refused_late > 100,
        "{admitted} {refused} {refused_late}"
    );

    // A sequence with two bad moves names the earlier one.
    let twice = lifecycle
        .judge(&["draft", "active", "draft"])
        .unwrap()
        .first_break
        .unwrap();
    assert_eq!(
        (twice.index, twice.from.as_deref(), twice.to.as_str()),
        (1, Some("draft"), "active")
    );

    // Nothing observed is refused, not reported clean.
    let nothing: [&str; 0] = [];
    let error = lifecycle.judge(&nothing).unwrap_err();
    assert!(
        error.message().contains("no states were supplied"),
        "{error:?}"
    );
}

// ---- REASON-009 -----------------------------------------------------------

const END: Timestamp = Timestamp::from_secs(1_000);

fn strict_partial_order(timeline: &Timeline, ids: &[String], as_of: Timestamp) {
    for a in ids {
        // Irreflexive: nothing is before itself.
        assert!(matches!(
            timeline.order(a, a, as_of),
            Order::Same | Order::Undecided
        ));
        for b in ids {
            let ab = timeline.order(a, b, as_of);
            let ba = timeline.order(b, a, as_of);
            // Asymmetric: before one way is after the other, never both.
            assert_eq!(
                ab == Order::Before,
                ba == Order::After,
                "{a} {b} {ab:?} {ba:?}"
            );
            for c in ids {
                if ab == Order::Before && timeline.order(b, c, as_of) == Order::Before {
                    assert_eq!(
                        timeline.order(a, c, as_of),
                        Order::Before,
                        "{a} < {b} < {c}"
                    );
                }
            }
        }
    }
}

#[test]
fn event_ordering_is_a_strict_partial_order_that_never_puts_an_effect_before_its_cause_and_reads_only_what_was_knowable()
 {
    // The failures this prevents: a causal claim stored against time, which
    // makes "before" contradict itself; and an as-of query that reads an
    // event or a link nobody could have known yet.
    let mut g = Lcg(29);
    let (mut accepted, mut refused, mut concurrent, mut undecided, mut tie_broken) =
        (0, 0, 0, 0, 0);
    for _ in 0..80 {
        let count = 3 + g.next(5) as usize;
        let events: Vec<Event> = (0..count)
            .map(|i| Event {
                id: format!("e{i}"),
                // A narrow range, so equal timestamps are common.
                occurred_at: Timestamp::from_secs(g.next(4) as i64),
                knowable_at: Timestamp::from_secs(g.next(6) as i64),
            })
            .collect();
        let ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        let mut timeline = Timeline::new();
        for event in &events {
            timeline.record(event.clone()).unwrap();
        }
        let mut links: Vec<(String, String, Timestamp)> = Vec::new();
        for _ in 0..(2 * count) {
            let cause = &ids[g.next(count as u64) as usize];
            let effect = &ids[g.next(count as u64) as usize];
            let knowable_at = Timestamp::from_secs(g.next(6) as i64);
            match timeline.link(cause, effect, knowable_at) {
                Ok(()) => {
                    accepted += 1;
                    links.push((cause.clone(), effect.clone(), knowable_at));
                }
                Err(error) => {
                    refused += 1;
                    assert!(
                        error.message().contains("cannot be its effect"),
                        "{error:?}"
                    );
                    // Refused for the stated reason and no other.
                    assert!(
                        cause == effect || timeline.order(effect, cause, END) == Order::Before,
                        "{cause} -> {effect} was refused without the effect preceding the cause"
                    );
                }
            }
        }

        // No effect is ordered before its cause, and every held link orders
        // its two ends.
        for (cause, effect, _) in &links {
            assert_eq!(timeline.order(cause, effect, END), Order::Before);
            let same_instant = events.iter().find(|e| e.id == *cause).unwrap().occurred_at
                == events.iter().find(|e| e.id == *effect).unwrap().occurred_at;
            tie_broken += usize::from(same_instant);
        }
        strict_partial_order(&timeline, &ids, END);

        // As of each instant, the answers are exactly those of a timeline
        // that was only ever told what was knowable by then.
        for t in 0..6 {
            let as_of = Timestamp::from_secs(t);
            let known: BTreeSet<&String> = events
                .iter()
                .filter(|e| e.knowable_at <= as_of)
                .map(|e| &e.id)
                .collect();
            let mut then = Timeline::new();
            for event in events.iter().filter(|e| known.contains(&e.id)) {
                then.record(event.clone()).unwrap();
            }
            for (cause, effect, knowable_at) in &links {
                if *knowable_at <= as_of && known.contains(cause) && known.contains(effect) {
                    then.link(cause, effect, *knowable_at).unwrap();
                }
            }
            for a in &ids {
                for b in &ids {
                    let answer = timeline.order(a, b, as_of);
                    if known.contains(a) && known.contains(b) {
                        assert_eq!(answer, then.order(a, b, END), "{a} {b} as of {t}");
                        concurrent += usize::from(answer == Order::Concurrent);
                    } else {
                        assert_eq!(answer, Order::Undecided, "{a} {b} as of {t}");
                        undecided += 1;
                    }
                }
            }
            strict_partial_order(&timeline, &ids, as_of);
        }
    }
    // The premise: every arm above was exercised.
    assert!(
        accepted > 50 && refused > 50 && concurrent > 50 && undecided > 50 && tie_broken > 20,
        "{accepted} {refused} {concurrent} {undecided} {tie_broken}"
    );

    // An event never recorded and one not yet knowable read the same.
    let mut timeline = Timeline::new();
    for (id, knowable) in [("early", 0), ("late", 9)] {
        timeline
            .record(Event {
                id: id.into(),
                occurred_at: Timestamp::from_secs(1),
                knowable_at: Timestamp::from_secs(knowable),
            })
            .unwrap();
    }
    let as_of = Timestamp::from_secs(5);
    assert_eq!(timeline.order("early", "late", as_of), Order::Undecided);
    assert_eq!(
        timeline.order("early", "never-recorded", as_of),
        Order::Undecided
    );
}

// ---- REASON-012 -----------------------------------------------------------

fn node(name: &str, states: &[&str], parents: &[&str], table: Vec<Vec<f64>>) -> Node {
    Node {
        name: name.into(),
        states: states.iter().map(|s| (*s).to_string()).collect(),
        parents: parents.iter().map(|s| (*s).to_string()).collect(),
        table,
    }
}

fn evidence(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn distribution(g: &mut Lcg, states: usize) -> Vec<f64> {
    let raw: Vec<f64> = (0..states).map(|_| g.unit()).collect();
    let total: f64 = raw.iter().sum();
    raw.iter().map(|p| p / total).collect()
}

const YES_NO: &[&str] = &["true", "false"];

fn sprinkler() -> Network {
    Network::new(vec![
        node("rain", YES_NO, &[], vec![vec![0.2, 0.8]]),
        node(
            "sprinkler",
            YES_NO,
            &["rain"],
            vec![vec![0.01, 0.99], vec![0.4, 0.6]],
        ),
        node(
            "wet",
            YES_NO,
            &["sprinkler", "rain"],
            vec![
                vec![0.99, 0.01],
                vec![0.9, 0.1],
                vec![0.8, 0.2],
                vec![0.0, 1.0],
            ],
        ),
    ])
    .unwrap()
}

#[test]
fn posteriors_are_probabilities_that_sum_to_one_and_favourable_evidence_never_lowers_a_hypothesis()
{
    // The failure this prevents: competing hypotheses updated one at a time,
    // whose probabilities need not sum to one and where nothing says what the
    // third lost when the first gained.

    // A hand-computed posterior first, so the generated checks below are not
    // the only thing standing between the evaluator and a wrong number.
    // P(wet, rain) = 0.2 * (0.01*0.99 + 0.99*0.8) = 0.16038
    // P(wet, dry)  = 0.8 * (0.4*0.9  + 0.6*0.0)  = 0.288
    let rain = sprinkler()
        .posterior("rain", &evidence(&[("wet", "true")]))
        .unwrap();
    assert!((rain["true"] - 0.16038 / 0.44838).abs() < 1e-12, "{rain:?}");
    // With no evidence the posterior is the prior.
    let prior = sprinkler().posterior("rain", &BTreeMap::new()).unwrap();
    assert!((prior["true"] - 0.2).abs() < 1e-12, "{prior:?}");

    // An uncertain rule: a downgraded issuer defaults with probability 0.3,
    // any other with 0.02. P(default) = 0.1*0.3 + 0.9*0.02 = 0.048.
    let rule = Network::new(vec![
        node("downgraded", YES_NO, &[], vec![vec![0.1, 0.9]]),
        Node::uncertain_rule("defaults", "downgraded", 0.3, 0.02),
    ])
    .unwrap();
    let defaults = rule.posterior("defaults", &BTreeMap::new()).unwrap();
    assert!((defaults["true"] - 0.048).abs() < 1e-12, "{defaults:?}");
    let downgraded = rule
        .posterior("downgraded", &evidence(&[("defaults", "true")]))
        .unwrap();
    assert!(
        (downgraded["true"] - 0.03 / 0.048).abs() < 1e-12,
        "{downgraded:?}"
    );

    // Generated networks: one hypothesis node and evidence nodes below it,
    // some of which also depend on the previous evidence node.
    let mut g = Lcg(43);
    let (mut raised, mut lowered, mut wide) = (0, 0, 0);
    for round in 0..400 {
        let hypotheses = 2 + g.next(3) as usize;
        let h_states: Vec<String> = (0..hypotheses).map(|i| format!("h{i}")).collect();
        let h_refs: Vec<&str> = h_states.iter().map(String::as_str).collect();
        // Even rounds are a star (each evidence node depends on the
        // hypothesis alone), which is where a likelihood ratio is a number
        // readable straight off the table.
        let star = round % 2 == 0;
        let children = 1 + g.next(4) as usize;
        let mut nodes = vec![node(
            "h",
            &h_refs,
            &[],
            vec![distribution(&mut g, hypotheses)],
        )];
        let mut child_states: Vec<usize> = Vec::new();
        for c in 0..children {
            let states = 2 + g.next(2) as usize;
            let names: Vec<String> = (0..states).map(|i| format!("s{i}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let chained = !star && c > 0;
            let previous = format!("e{}", c.saturating_sub(1));
            let parents: Vec<&str> = if chained {
                vec!["h", &previous]
            } else {
                vec!["h"]
            };
            let rows = hypotheses * if chained { child_states[c - 1] } else { 1 };
            let table = (0..rows).map(|_| distribution(&mut g, states)).collect();
            nodes.push(node(&format!("e{c}"), &refs, &parents, table));
            child_states.push(states);
        }
        let tables: Vec<Vec<Vec<f64>>> = nodes.iter().map(|n| n.table.clone()).collect();
        let network = Network::new(nodes).unwrap();

        // Observe a random subset of the evidence nodes, one at a time.
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        let mut before = network.posterior("h", &seen).unwrap();
        for c in 0..children {
            if g.next(4) == 0 {
                continue;
            }
            let state = g.next(child_states[c] as u64) as usize;
            seen.insert(format!("e{c}"), format!("s{state}"));
            let after = network.posterior("h", &seen).unwrap();

            // Every posterior is a probability, and over the exhaustive set
            // of hypotheses they sum to one within the stated tolerance.
            assert_eq!(after.len(), hypotheses);
            assert!(after.values().all(|p| (0.0..=1.0).contains(p)), "{after:?}");
            assert!(
                (after.values().sum::<f64>() - 1.0).abs() <= TOLERANCE,
                "{after:?}"
            );
            wide += usize::from(hypotheses > 2);

            if star && hypotheses == 2 {
                // The likelihood ratio for h0, read from the table alone.
                let ratio = tables[c + 1][0][state] / tables[c + 1][1][state];
                if ratio > 1.0 {
                    assert!(
                        after["h0"] >= before["h0"],
                        "ratio {ratio}: {before:?} -> {after:?}"
                    );
                    raised += usize::from(after["h0"] > before["h0"] + 1e-9);
                } else {
                    lowered += usize::from(after["h0"] < before["h0"] - 1e-9);
                }
            }
            before = after;
        }
    }
    // The premise: evidence moved the hypothesis both ways, and sets wider
    // than a pair were summed.
    assert!(
        raised > 50 && lowered > 50 && wide > 100,
        "{raised} {lowered} {wide}"
    );

    // A row that is not a distribution is refused, not renormalised.
    let error = Network::new(vec![node("x", YES_NO, &[], vec![vec![0.5, 0.4]])]).unwrap_err();
    assert!(error.message().contains("not a distribution"), "{error:?}");
    // Evidence the model says cannot happen has no posterior.
    let impossible = evidence(&[("sprinkler", "false"), ("rain", "false"), ("wet", "true")]);
    let error = sprinkler().posterior("rain", &impossible).unwrap_err();
    assert!(error.message().contains("probability zero"), "{error:?}");
}

// ---- REASON-015 -----------------------------------------------------------

/// A constraint's meaning, written a second time over raw integers. The
/// brute-force reference evaluates these and nothing of the solver's.
type Meaning = Box<dyn Fn(&[i64]) -> bool>;

fn compare(left: i64, op: Op, right: i64) -> bool {
    match op {
        Op::Eq => left == right,
        Op::Ne => left != right,
        Op::Lt => left < right,
        Op::Le => left <= right,
        Op::Gt => left > right,
        Op::Ge => left >= right,
    }
}

const OPS: [Op; 6] = [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge];

fn random_constraint(g: &mut Lcg, id: usize, variables: usize) -> (Constraint, Meaning) {
    let op = OPS[g.next(6) as usize];
    let a = g.next(variables as u64) as usize;
    let b = g.next(variables as u64) as usize;
    let bound = g.next(5) as i64 - 1;
    let id = format!("c{id}");
    if g.next(3) == 0 {
        // Either of two comparisons against a constant: a clause.
        let other = OPS[g.next(6) as usize];
        let form = Form::Holds(Cond::Or(vec![
            Cond::Cmp {
                fact: format!("v{a}"),
                op,
                value: Value::Int(bound),
            },
            Cond::Cmp {
                fact: format!("v{b}"),
                op: other,
                value: Value::Int(1),
            },
        ]));
        let meaning: Meaning =
            Box::new(move |x| compare(x[a], op, bound) || compare(x[b], other, 1));
        (Constraint { id, form }, meaning)
    } else {
        let (p, q) = (g.next(3) as i64 - 1, g.next(3) as i64);
        let form = Form::Linear {
            terms: vec![(p, format!("v{a}")), (q, format!("v{b}"))],
            op,
            bound,
        };
        let meaning: Meaning = Box::new(move |x| compare(p * x[a] + q * x[b], op, bound));
        (Constraint { id, form }, meaning)
    }
}

/// Whether any assignment over `0..domain` satisfies every listed meaning.
fn brute_force(variables: usize, domain: i64, meanings: &[&Meaning]) -> bool {
    let mut x = vec![0i64; variables];
    loop {
        if meanings.iter().all(|m| m(&x)) {
            return true;
        }
        let mut digit = 0;
        loop {
            if digit == variables {
                return false;
            }
            x[digit] += 1;
            if x[digit] < domain {
                break;
            }
            x[digit] = 0;
            digit += 1;
        }
    }
}

fn clause(id: &str, literals: &[(&str, bool)]) -> Constraint {
    Constraint {
        id: id.into(),
        form: Form::Holds(Cond::Or(
            literals
                .iter()
                .map(|(variable, value)| Cond::Cmp {
                    fact: (*variable).to_string(),
                    op: Op::Eq,
                    value: Value::Bool(*value),
                })
                .collect(),
        )),
    }
}

fn booleans(names: &[&str]) -> Vec<Variable> {
    names
        .iter()
        .map(|name| Variable {
            name: (*name).to_string(),
            domain: vec![Value::Bool(false), Value::Bool(true)],
        })
        .collect()
}

#[test]
fn a_constraint_problem_yields_a_satisfying_assignment_or_a_conflicting_subset_that_is_itself_unsatisfiable()
 {
    // The failure this prevents: "no solution" with nothing to act on, or a
    // conflict set that blames constraints which could in fact all be met.
    let mut g = Lcg(71);
    let (mut satisfied, mut unsatisfiable, mut narrowed) = (0, 0, 0);
    for _ in 0..600 {
        let variables = 2 + g.next(3) as usize;
        let domain = 2 + g.next(3) as i64;
        let count = 1 + g.next(6) as usize;
        let (constraints, meanings): (Vec<Constraint>, Vec<Meaning>) = (0..count)
            .map(|i| random_constraint(&mut g, i, variables))
            .unzip();
        let problem = Problem {
            variables: (0..variables)
                .map(|i| Variable {
                    name: format!("v{i}"),
                    domain: (0..domain).map(Value::Int).collect(),
                })
                .collect(),
            constraints,
        };
        let every: Vec<&Meaning> = meanings.iter().collect();
        match problem.solve().unwrap() {
            Answer::Satisfied(assignment) => {
                satisfied += 1;
                let x: Vec<i64> = (0..variables)
                    .map(|i| match assignment[&format!("v{i}")] {
                        Value::Int(value) => value,
                        ref other => panic!("v{i} was assigned {other:?}"),
                    })
                    .collect();
                assert!(x.iter().all(|value| (0..domain).contains(value)));
                // The returned assignment satisfies every constraint.
                assert!(every.iter().all(|m| m(&x)), "{problem:?} -> {x:?}");
            }
            Answer::Unsatisfiable { conflict } => {
                unsatisfiable += 1;
                // Confirmed by brute force that shares nothing with the solver.
                assert!(!brute_force(variables, domain, &every), "{problem:?}");
                let position = |id: &String| id[1..].parse::<usize>().unwrap();
                let core: Vec<&Meaning> =
                    conflict.iter().map(|id| &meanings[position(id)]).collect();
                assert!(!core.is_empty());
                // The reported subset is itself unsatisfiable …
                assert!(
                    !brute_force(variables, domain, &core),
                    "{conflict:?} of {problem:?}"
                );
                // … and minimal: drop any one member and it can be met.
                for skip in 0..core.len() {
                    let rest: Vec<&Meaning> = core
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != skip)
                        .map(|(_, m)| *m)
                        .collect();
                    assert!(
                        brute_force(variables, domain, &rest),
                        "{conflict:?} is not minimal"
                    );
                }
                narrowed += usize::from(conflict.len() < count);
            }
        }
    }
    // The premise: both verdicts occurred, and the conflict was often a strict
    // subset — otherwise "the whole problem" would pass for a conflict.
    assert!(
        satisfied > 100 && unsatisfiable > 100 && narrowed > 50,
        "{satisfied} {unsatisfiable} {narrowed}"
    );

    // The SAT formulation. Four clauses over x and y that cannot all hold,
    // and a fifth over z that has nothing to do with it.
    let mut sat = Problem {
        variables: booleans(&["x", "y", "z"]),
        constraints: vec![
            clause("z-holds", &[("z", true)]),
            clause("x-or-y", &[("x", true), ("y", true)]),
            clause("x-or-not-y", &[("x", true), ("y", false)]),
            clause("not-x-or-y", &[("x", false), ("y", true)]),
            clause("not-x-or-not-y", &[("x", false), ("y", false)]),
        ],
    };
    assert_eq!(
        sat.solve().unwrap(),
        Answer::Unsatisfiable {
            conflict: vec![
                "not-x-or-not-y".to_string(),
                "not-x-or-y".to_string(),
                "x-or-not-y".to_string(),
                "x-or-y".to_string(),
            ]
        }
    );
    // Remove one clause of the conflict and the rest has exactly one model.
    sat.constraints.pop();
    let model = BTreeMap::from([
        ("x".to_string(), Value::Bool(true)),
        ("y".to_string(), Value::Bool(true)),
        ("z".to_string(), Value::Bool(true)),
    ]);
    assert_eq!(sat.solve().unwrap(), Answer::Satisfied(model));

    // A constraint over a variable nobody declared is refused, not "unsat".
    sat.constraints.push(clause("stray", &[("w", true)]));
    let error = sat.solve().unwrap_err();
    assert!(
        error.message().contains("not a declared variable"),
        "{error:?}"
    );
    // A search space past the bound is refused rather than left to run.
    let names: Vec<String> = (0..21).map(|i| format!("b{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let huge = Problem {
        variables: booleans(&refs),
        constraints: vec![],
    };
    const { assert!(1u64 << 21 > MAX_ASSIGNMENTS) };
    let error = huge.solve().unwrap_err();
    assert!(
        error.message().contains("candidate assignments"),
        "{error:?}"
    );
}

// ---- REASON-017 -----------------------------------------------------------

fn zero_or_one(count: usize) -> Vec<Variable> {
    (0..count)
        .map(|i| Variable {
            name: format!("x{i}"),
            domain: vec![Value::Int(0), Value::Int(1)],
        })
        .collect()
}

fn weighted(weights: &[i64]) -> Vec<(i64, String)> {
    weights
        .iter()
        .enumerate()
        .map(|(i, weight)| (*weight, format!("x{i}")))
        .collect()
}

#[test]
fn a_constraint_problem_as_a_qubo_decodes_to_the_classical_optimum_of_the_problem_as_posed() {
    // The failure this prevents: an encoding checked only against an
    // enumeration of itself. That comparison passes for a QUBO whose minimum
    // breaks a constraint, or solves a different problem, because both sides
    // read the same wrong matrix. The baseline here is the problem as posed.
    let mut g = Lcg(101);
    let (mut solved, mut unsatisfiable, mut penalised) = (0, 0, 0);
    for _ in 0..600 {
        let count = 3 + g.next(5) as usize;
        let rows: Vec<(Vec<i64>, i64)> = (0..1 + g.next(2))
            .map(|_| {
                (
                    (0..count).map(|_| g.next(3) as i64).collect(),
                    g.next(4) as i64,
                )
            })
            .collect();
        let cost: Vec<i64> = (0..count).map(|_| g.next(7) as i64 - 3).collect();
        let problem = Problem {
            variables: zero_or_one(count),
            constraints: rows
                .iter()
                .enumerate()
                .map(|(row, (weights, bound))| Constraint {
                    id: format!("eq{row}"),
                    form: Form::Linear {
                        terms: weighted(weights),
                        op: Op::Eq,
                        bound: *bound,
                    },
                })
                .collect(),
        };
        let objective = weighted(&cost);

        // The reference: every assignment, judged on raw integers.
        let dot =
            |weights: &[i64], x: &[i64]| -> i64 { weights.iter().zip(x).map(|(w, v)| w * v).sum() };
        let satisfies = |x: &[i64]| {
            rows.iter()
                .all(|(weights, bound)| dot(weights, x) == *bound)
        };
        let (mut best, mut unconstrained) = (None::<i64>, i64::MAX);
        for mask in 0..(1u32 << count) {
            let x: Vec<i64> = (0..count).map(|i| i64::from((mask >> i) & 1)).collect();
            let value = dot(&cost, &x);
            unconstrained = unconstrained.min(value);
            if satisfies(&x) {
                best = Some(best.map_or(value, |held| held.min(value)));
            }
        }

        // The classical baseline, on the problem as posed.
        let classical = problem.minimise(&objective).unwrap();
        assert_eq!(classical.as_ref().map(|o| o.objective), best, "{problem:?}");

        // The QUBO's brute-forced minimum, decoded.
        let qubo = problem.to_qubo(&objective).unwrap();
        let minimum = solve_exact(&qubo).unwrap();
        let x: Vec<i64> = minimum.assignment.iter().map(|b| i64::from(*b)).collect();
        match (best, problem.decode(&objective, &minimum.assignment)) {
            (Some(best), Ok(decoded)) => {
                // It satisfies the original constraints, at the optimum.
                assert!(satisfies(&x), "{problem:?} decoded to {x:?}");
                assert_eq!(decoded.objective, best, "{problem:?} decoded to {x:?}");
                // On a feasible assignment the energy is the objective.
                assert!((minimum.energy - best as f64).abs() < 1e-9, "{minimum:?}");
                solved += 1;
                // Cases where ignoring the constraints would have done better:
                // the ones in which the penalty is what holds the answer.
                penalised += usize::from(unconstrained < best);
            }
            (None, Err(error)) => {
                // No solution exists, and the QUBO's minimum is not passed
                // off as one.
                assert!(!satisfies(&x));
                assert!(error.message().contains("violates constraint"), "{error:?}");
                unsatisfiable += 1;
            }
            (best, decoded) => panic!("optimum {best:?} but decoded {decoded:?}: {problem:?}"),
        }
    }
    // The premise: both outcomes occurred, and the penalty was often the only
    // thing between the QUBO's minimum and an infeasible answer.
    assert!(
        solved > 100 && unsatisfiable > 100 && penalised > 100,
        "{solved} {unsatisfiable} {penalised}"
    );

    // What does not admit a QUBO is refused by name, not approximated.
    let wide = Problem {
        variables: vec![Variable {
            name: "x0".into(),
            domain: vec![Value::Int(0), Value::Int(1), Value::Int(2)],
        }],
        constraints: vec![],
    };
    let error = wide.to_qubo(&weighted(&[1])).unwrap_err();
    assert!(
        error.message().contains("does not take exactly 0 and 1"),
        "{error:?}"
    );
    let bounded = Problem {
        variables: zero_or_one(2),
        constraints: vec![Constraint {
            id: "at-most-one".into(),
            form: Form::Linear {
                terms: weighted(&[1, 1]),
                op: Op::Le,
                bound: 1,
            },
        }],
    };
    let error = bounded.to_qubo(&weighted(&[1, 1])).unwrap_err();
    assert!(
        error
            .message()
            .contains("'at-most-one' is not a linear equality"),
        "{error:?}"
    );
    // The classical search still answers it: pick neither, at no cost.
    assert_eq!(
        bounded
            .minimise(&weighted(&[1, 1]))
            .unwrap()
            .unwrap()
            .objective,
        0
    );
}
