//! Rule packs: versioned verdicts, declared invariants, determinism, and an
//! independent checker (REASON-001, -013, -014, -027, -035).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::process::Command;

use qip_reasoning_engine::rules::{Cond, Invariant, Op, Rule, RulePack, Value, Verdict};
use serde_json::Value as Json;

fn cmp(fact: &str, op: Op, value: Value) -> Cond {
    Cond::Cmp {
        fact: fact.into(),
        op,
        value,
    }
}
fn rule(id: &str, when: Cond, fact: &str, value: Value) -> Rule {
    Rule {
        id: id.into(),
        when,
        fact: fact.into(),
        value,
    }
}
fn facts(pairs: &[(&str, Value)]) -> Vec<(String, Value)> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
}
fn pack(name: &str, rules: Vec<Rule>, invariants: Vec<Invariant>) -> RulePack {
    RulePack {
        name: name.into(),
        version: 7,
        rules,
        invariants,
    }
}
fn fired(v: &Verdict) -> Vec<&str> {
    v.trace.iter().map(|s| s.rule.as_str()).collect()
}

/// Deterministic generator so a failing case is reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// A pack with a deliberately buggy rule (any KYC-passed applicant is eligible)
/// so the invariant has something to catch.
fn guarded_pack() -> RulePack {
    pack(
        "onboarding",
        vec![rule(
            "eligible-if-kyc",
            cmp("kyc", Op::Eq, Value::Bool(true)),
            "eligible",
            Value::Bool(true),
        )],
        vec![
            Invariant {
                id: "no-minor-eligible".into(),
                forbidden: Cond::And(vec![
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                    cmp("age", Op::Lt, Value::Int(18)),
                ]),
            },
            Invariant {
                id: "no-negative-balance-posting".into(),
                forbidden: Cond::And(vec![
                    Cond::Present { fact: "eligible".into() },
                    cmp("balance", Op::Lt, Value::Int(0)),
                ]),
            },
        ],
    )
}

fn random_inputs(g: &mut Lcg) -> Vec<(String, Value)> {
    facts(&[
        ("age", Value::Int(g.next(40) as i64)),
        ("kyc", Value::Bool(g.next(2) == 1)),
        ("balance", Value::Int(g.next(5) as i64 - 1)),
    ])
}

#[test]
fn each_named_use_yields_a_verdict_citing_the_pack_version_and_the_rules_that_fired() {
    // eligibility
    let p = pack(
        "eligibility",
        vec![rule(
            "adult-with-kyc",
            Cond::And(vec![
                cmp("age", Op::Ge, Value::Int(18)),
                cmp("kyc", Op::Eq, Value::Bool(true)),
            ]),
            "eligible",
            Value::Bool(true),
        )],
        vec![],
    );
    let v = p
        .evaluate(&facts(&[("age", Value::Int(30)), ("kyc", Value::Bool(true))]))
        .unwrap();
    assert_eq!(
        (v.version, v.pack.as_str(), fired(&v)),
        (7, "eligibility", vec!["adult-with-kyc"])
    );
    assert_eq!(v.facts["eligible"], Value::Bool(true));
    let none = p
        .evaluate(&facts(&[("age", Value::Int(12)), ("kyc", Value::Bool(true))]))
        .unwrap();
    assert!(fired(&none).is_empty() && !none.facts.contains_key("eligible"));

    // accounting invariant: a posting against an unbalanced book is refused.
    let acct = pack(
        "ledger",
        vec![rule(
            "post-when-open",
            cmp("open", Op::Eq, Value::Bool(true)),
            "posting_allowed",
            Value::Bool(true),
        )],
        vec![Invariant {
            id: "trial-balance".into(),
            forbidden: Cond::And(vec![
                cmp("posting_allowed", Op::Eq, Value::Bool(true)),
                cmp("difference", Op::Ne, Value::Int(0)),
            ]),
        }],
    );
    let ok = acct
        .evaluate(&facts(&[("open", Value::Bool(true)), ("difference", Value::Int(0))]))
        .unwrap();
    assert_eq!(fired(&ok), vec!["post-when-open"]);
    let err = acct
        .evaluate(&facts(&[("open", Value::Bool(true)), ("difference", Value::Int(5))]))
        .unwrap_err();
    assert!(err.message().contains("'trial-balance'"), "{}", err.message());

    // market structure: halt wins, chained through a second rule.
    let mkt = pack(
        "structure",
        vec![
            rule("halt", cmp("halted", Op::Eq, Value::Bool(true)), "may_trade", Value::Bool(false)),
            rule(
                "open-session",
                Cond::And(vec![
                    cmp("session", Op::Eq, Value::Text("open".into())),
                    Cond::Not(Box::new(cmp("halted", Op::Eq, Value::Bool(true)))),
                ]),
                "may_trade",
                Value::Bool(true),
            ),
        ],
        vec![],
    );
    let v = mkt
        .evaluate(&facts(&[
            ("session", Value::Text("open".into())),
            ("halted", Value::Bool(true)),
        ]))
        .unwrap();
    assert_eq!((fired(&v), &v.facts["may_trade"]), (vec!["halt"], &Value::Bool(false)));

    // contract term: a notional above the threshold needs approval, chained
    // into a second conclusion.
    let con = pack(
        "contract",
        vec![
            rule("large", cmp("notional", Op::Gt, Value::Int(1_000_000)), "needs_approval", Value::Bool(true)),
            rule(
                "route",
                cmp("needs_approval", Op::Eq, Value::Bool(true)),
                "route",
                Value::Text("desk-head".into()),
            ),
        ],
        vec![],
    );
    let v = con.evaluate(&facts(&[("notional", Value::Int(2_000_000))])).unwrap();
    assert_eq!(fired(&v), vec!["large", "route"]);
    assert_eq!(v.facts["route"], Value::Text("desk-head".into()));
}

#[test]
fn no_emitted_conclusion_violates_a_declared_invariant_and_every_refusal_names_one() {
    let p = guarded_pack();
    let mut g = Lcg(11);
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..500 {
        let inputs = random_inputs(&mut g);
        match p.evaluate(&inputs) {
            Ok(v) => {
                accepted += 1;
                let eligible = v.facts.get("eligible") == Some(&Value::Bool(true));
                let age = match v.facts["age"] {
                    Value::Int(a) => a,
                    _ => unreachable!(),
                };
                assert!(!(eligible && age < 18), "emitted a minor as eligible: {v:?}");
            }
            Err(e) => {
                refused += 1;
                let m = e.message();
                assert!(
                    p.invariants.iter().any(|i| m.contains(&format!("'{}'", i.id))),
                    "refusal names no invariant: {m}"
                );
            }
        }
    }
    // Premise: both outcomes occurred, so the loops above asserted something.
    assert!(accepted > 20 && refused > 20, "accepted {accepted}, refused {refused}");

    // Remove the minor invariant: the same generator now emits the violation,
    // proving the check was what stopped it.
    let mut weakened = p.clone();
    weakened.invariants.retain(|i| i.id != "no-minor-eligible");
    let mut g = Lcg(11);
    let mut emitted_violation = false;
    for _ in 0..500 {
        if let Ok(v) = weakened.evaluate(&random_inputs(&mut g))
            && v.facts.get("eligible") == Some(&Value::Bool(true))
            && matches!(v.facts["age"], Value::Int(a) if a < 18)
        {
            emitted_violation = true;
        }
    }
    assert!(emitted_violation);
}

#[test]
fn a_refused_step_names_the_rule_and_the_invariant_it_would_have_broken() {
    let err = guarded_pack()
        .evaluate(&facts(&[("age", Value::Int(10)), ("kyc", Value::Bool(true)), ("balance", Value::Int(1))]))
        .unwrap_err();
    assert!(err.message().contains("'eligible-if-kyc'") && err.message().contains("'no-minor-eligible'"));
}

fn demo_pack() -> RulePack {
    pack(
        "determinism",
        vec![
            rule("b", cmp("x", Op::Ge, Value::Int(1)), "y", Value::Int(2)),
            rule("a", cmp("x", Op::Ge, Value::Int(1)), "z", Value::Int(3)),
            rule("c", cmp("y", Op::Eq, Value::Int(2)), "w", Value::Bool(true)),
        ],
        vec![],
    )
}
fn demo_facts(rev: bool) -> Vec<(String, Value)> {
    let mut f = facts(&[("x", Value::Int(4)), ("q", Value::Text("t".into())), ("k", Value::Bool(false))]);
    if rev {
        f.reverse();
    }
    f
}
fn bytes(v: &Verdict) -> String {
    serde_json::to_string(v).unwrap()
}

/// Child half of the cross-process test: prints its verdict and exits. Without
/// the env var it is a no-op, so a normal run does not depend on it.
#[test]
fn child_emit_verdict() {
    if let Ok(order) = std::env::var("QIP_RULES_CHILD_ORDER") {
        let v = demo_pack().evaluate(&demo_facts(order == "rev")).unwrap();
        println!("VERDICT<<{}>>", bytes(&v));
    }
}

fn run_child(order: &str) -> String {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args(["--exact", "child_emit_verdict", "--nocapture", "--test-threads=1"])
        .env("QIP_RULES_CHILD_ORDER", order)
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let a = s.find("VERDICT<<").expect("child printed no verdict") + 9;
    let b = s.find(">>").unwrap();
    s[a..b].to_string()
}

#[test]
fn the_same_pack_and_facts_give_byte_identical_verdicts_across_processes_and_fact_orders() {
    let local_fwd = bytes(&demo_pack().evaluate(&demo_facts(false)).unwrap());
    // Premise: the input orders genuinely differ, and the trace has rules in it.
    assert_ne!(demo_facts(false), demo_facts(true));
    assert!(local_fwd.contains("\"rule\":\"a\""));
    let fwd = run_child("fwd");
    let rev = run_child("rev");
    assert_eq!(fwd, rev);
    assert_eq!(fwd, local_fwd);
    // The trace is id-ordered, not declaration-ordered: a before b before c.
    let v = demo_pack().evaluate(&demo_facts(true)).unwrap();
    assert_eq!(fired(&v), vec!["a", "b", "c"]);
}

#[test]
fn the_evaluation_path_makes_no_model_call() {
    // Structural: the module names no language-model, clock, random or I/O
    // facility. A regression that threaded one in would have to add the name.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/rules.rs")).unwrap();
    let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    for banned in ["qip_ai", "providers", "qip_transport", "SystemTime", "Instant", "std::net", "std::env"] {
        assert!(!code.contains(banned), "rules.rs names {banned}");
    }
}

// ---- A checker that shares no code with the reasoner: it reads only the
// serialised pack, inputs and trace as JSON. ----

fn jcmp(a: &Json, b: &Json) -> Option<Ordering> {
    let (ka, va) = a.as_object()?.iter().next()?;
    let (kb, vb) = b.as_object()?.iter().next()?;
    if ka != kb {
        return None;
    }
    match ka.as_str() {
        "Int" => va.as_i64()?.partial_cmp(&vb.as_i64()?),
        "Bool" => va.as_bool()?.partial_cmp(&vb.as_bool()?),
        _ => va.as_str()?.partial_cmp(vb.as_str()?),
    }
}
fn jholds(c: &Json, f: &BTreeMap<String, Json>) -> bool {
    let (tag, body) = c.as_object().unwrap().iter().next().unwrap();
    match tag.as_str() {
        "Present" => f.contains_key(body["fact"].as_str().unwrap()),
        "Cmp" => match f.get(body["fact"].as_str().unwrap()) {
            None => false,
            Some(have) => {
                let ord = jcmp(have, &body["value"]);
                match (body["op"].as_str().unwrap(), ord) {
                    ("Ne", None) => true,
                    (_, None) => false,
                    ("Eq", Some(o)) => o == Ordering::Equal,
                    ("Ne", Some(o)) => o != Ordering::Equal,
                    ("Lt", Some(o)) => o == Ordering::Less,
                    ("Le", Some(o)) => o != Ordering::Greater,
                    ("Gt", Some(o)) => o == Ordering::Greater,
                    ("Ge", Some(o)) => o != Ordering::Less,
                    _ => false,
                }
            }
        },
        "And" => body.as_array().unwrap().iter().all(|x| jholds(x, f)),
        "Or" => body.as_array().unwrap().iter().any(|x| jholds(x, f)),
        "Not" => !jholds(body, f),
        other => panic!("unknown condition {other}"),
    }
}
fn jviolated(pack: &Json, f: &BTreeMap<String, Json>) -> Vec<String> {
    pack["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| jholds(&i["forbidden"], f))
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}
fn jinputs(inputs: &[(String, Value)]) -> BTreeMap<String, Json> {
    inputs.iter().map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap())).collect()
}
/// Accepts a trace only if every step is a rule that held, none repeats, no
/// invariant breaks, and nothing that should have fired was left unfired.
fn check_trace(pack: &Json, inputs: &[(String, Value)], trace: &Json) -> bool {
    let rules = pack["rules"].as_array().unwrap();
    let mut f = jinputs(inputs);
    let mut used = Vec::new();
    for step in trace.as_array().unwrap() {
        let id = step["rule"].as_str().unwrap();
        let Some(r) = rules.iter().find(|r| r["id"] == id) else { return false };
        if used.contains(&id) || !jholds(&r["when"], &f) || r["fact"] != step["fact"] || r["value"] != step["value"] {
            return false;
        }
        used.push(id);
        f.insert(r["fact"].as_str().unwrap().to_string(), r["value"].clone());
        if !jviolated(pack, &f).is_empty() {
            return false;
        }
    }
    rules.iter().all(|r| used.contains(&r["id"].as_str().unwrap()) || !jholds(&r["when"], &f))
}

#[test]
fn an_independent_checker_agrees_with_every_constraint_verdict() {
    let constraints_only = RulePack { rules: vec![], ..guarded_pack() };
    let pj = serde_json::to_value(&constraints_only).unwrap();
    let mut g = Lcg(5);
    let (mut broke, mut held) = (0, 0);
    for _ in 0..500 {
        let mut inputs = random_inputs(&mut g);
        if g.next(2) == 1 {
            inputs.push(("eligible".into(), Value::Bool(g.next(2) == 1)));
        }
        let theirs = jviolated(&pj, &jinputs(&inputs));
        match constraints_only.evaluate(&inputs) {
            Ok(_) => {
                held += 1;
                assert!(theirs.is_empty(), "checker found {theirs:?} where the fabric accepted");
            }
            Err(e) => {
                broke += 1;
                assert!(
                    theirs.iter().any(|id| e.message().contains(&format!("'{id}'"))),
                    "fabric refused ({}) where checker found {theirs:?}",
                    e.message()
                );
            }
        }
    }
    assert!(broke > 20 && held > 20, "broke {broke}, held {held}");
}

#[test]
fn the_checker_accepts_untampered_traces_and_rejects_and_the_pack_refuses_any_one_step_mutated() {
    let p = demo_pack();
    let pj = serde_json::to_value(&p).unwrap();
    let inputs = demo_facts(false);
    let v = p.evaluate(&inputs).unwrap();
    assert!(v.trace.len() >= 3);
    let tj = serde_json::to_value(&v.trace).unwrap();
    assert!(check_trace(&pj, &inputs, &tj));
    assert!(p.verify(&inputs, &v).is_ok());
    for i in 0..v.trace.len() {
        let mut bad = v.clone();
        bad.trace[i].value = match &bad.trace[i].value {
            Value::Int(n) => Value::Int(n + 1),
            Value::Bool(b) => Value::Bool(!b),
            Value::Text(s) => Value::Text(format!("{s}x")),
        };
        assert!(!check_trace(&pj, &inputs, &serde_json::to_value(&bad.trace).unwrap()), "step {i}");
        assert!(p.verify(&inputs, &bad).is_err(), "step {i} accepted by verify");
    }
    // Dropping a step is also a mutation: the unfired rule is then detected.
    let mut short = v.trace.clone();
    short.pop();
    assert!(!check_trace(&pj, &inputs, &serde_json::to_value(&short).unwrap()));
}
