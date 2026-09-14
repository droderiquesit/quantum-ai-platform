//! What only a plan can prove, and what must be true for the plans to exist.
//!
//! `infrastructure.rs` reads the Terraform as text. That is the right tool for
//! "a service must not be `INGRESS_TRAFFIC_ALL`", and it is the wrong tool for
//! "this validation refuses a bad value and admits a good one" — because a
//! validation can read perfectly and be unable to run.
//!
//! That is not hypothetical here. `modules/network`'s `console_egress_cidr`
//! carried the guard `x == null || tonumber(split("/", x)[1]) <= 26`, which
//! looks like a null-guarded prefix check. Terraform evaluates both operands
//! of `||`, `split` refuses a null argument, and the plan died on a provider
//! error before any `error_message` was reached. The variable is null by
//! default and null in test, stage and prod, so `terraform plan` was
//! impossible in three of the four environments — a gate that refused every
//! good value while never once reporting the bad one it was written for.
//!
//! It survived because `infrastructure.rs` asserted the rule "both fires and
//! admits" by re-implementing the arithmetic in Rust and checking the mirror.
//! A mirror of an expression is not the expression. The only thing that finds
//! this class is a plan, and the plans live in `*.tftest.hcl` files run by
//! `terraform test` against a mocked provider — no credential, no project,
//! nothing created.
//!
//! This suite holds three things those plans cannot hold themselves: that no
//! other validation has the same shape, that each harness proves both halves
//! rather than only the refusal, and the handful of properties a `plan` run
//! cannot assert because the value is unknown until apply.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

const TRUST_ZONES: &str = "infrastructure/terraform/modules/trust-zones/main.tf";
const PUBLIC_EDGE: &str = "infrastructure/terraform/modules/public-edge/main.tf";

/// A configuration with its comments removed.
///
/// A comment quoting a dangerous expression is not the same as writing one,
/// and a check that cannot tell the difference makes it impossible to record
/// why an expression is refused — which is most of what the files below are.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `*.tftest.hcl` under `infrastructure/`, as its repository-relative
/// path and its text.
fn plan_harnesses() -> Vec<(String, String)> {
    let root = repository_root();
    let mut found: Vec<(String, String)> = files_with_extension("infrastructure", "hcl")
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".tftest.hcl"))
        })
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(&path).expect("readable");
            (relative, text)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn no_variable_validation_hands_the_null_it_is_guarding_against_to_a_function() {
    // The sibling of `no_variable_validation_reads_through_the_null_it_is
    // _guarding_against` in `infrastructure.rs`, and the half it does not see.
    //
    // That test looks for `var.x == null || … var.x.field …` — reading an
    // attribute off the null. There is a second way to read through a null and
    // it is the one that actually shipped: pass the value to a function.
    // `split("/", var.x)`, `length(var.x)`, `tonumber(var.x)` and
    // `regexall(p, var.x)` all refuse a null argument, and the refusal is a
    // provider-level error the validation block cannot catch and cannot
    // report. `can()` and `try()` swallow it; nothing else does.
    //
    // `modules/network`'s console_egress_cidr was in exactly this shape and
    // made `terraform plan` impossible in three of the four environments. The
    // older test passed on it throughout, because the expression dereferences
    // no attribute.
    let mut unguarded: Vec<String> = Vec::new();
    let mut inspected = 0usize;

    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("condition") {
                continue;
            }
            let Some((left, right)) = line.split_once("== null") else {
                continue;
            };
            // The variable the guard names, as the condition spells it.
            let Some(name) = left.split("var.").nth(1).map(str::trim) else {
                continue;
            };
            // Does the right-hand side hand that same variable to a function?
            // `var.x)` or `var.x,` or `var.x]` — a use that is not an
            // attribute access, which the older test already covers.
            let used_whole = [")", ",", "]", " "].iter().any(|suffix| {
                right
                    .split(&format!("var.{name}"))
                    .skip(1)
                    .any(|rest| rest.starts_with(suffix))
            });
            if !used_whole {
                continue;
            }
            inspected += 1;
            if !right.contains("can(") && !right.contains("try(") {
                unguarded.push(format!("{}: {line}", path.display()));
            }
        }
    }

    // Premise first. An empty result has to mean "they are all guarded", not
    // "the walk read nothing" — which is how a check like this rots into
    // decoration after somebody reformats the file it reads.
    assert!(
        inspected >= 3,
        "only {inspected} null-guarded validations passing the whole value to a function were \
         seen. The scan is no longer reading the conditions it judges, so its silence means \
         nothing"
    );

    assert!(
        unguarded.is_empty(),
        "a validation passes the value it is checking for null to a function. Terraform \
         evaluates both operands of `||`, the function refuses the null, and the plan dies on \
         an error the validation cannot report — so the gate refuses every good value and \
         never reports a bad one. Wrap the call in `try(…, false)` or `can(…)`:\n{}",
        unguarded.join("\n")
    );
}

#[test]
fn every_plan_harness_proves_a_refusal_and_an_admission() {
    // The infrastructure rules: "a real plan proving the gate fires on a bad
    // value **and admits a good one**. The second half is what distinguishes a
    // working gate from one that refuses everything."
    //
    // A harness with only `expect_failures` runs passes while the module
    // refuses every input, and a module that refuses every input stops a
    // deployment for a reason nobody can find. A harness with only asserting
    // runs never proves the gate is there at all.
    let harnesses = plan_harnesses();

    assert!(
        harnesses.len() >= 4,
        "only {} plan harness(es) were found under infrastructure/. The zone model, the \
         console's egress range, the public edge and the autonomy ceiling each have one; a \
         walk that finds fewer is reading the wrong tree",
        harnesses.len()
    );

    for (path, text) in &harnesses {
        let body = without_comments(text);
        let refusals = body.matches("expect_failures").count();
        let admissions = body.matches("assert {").count();
        assert!(
            refusals > 0,
            "{path} runs plans and expects no failure. A harness that only admits proves the \
             configuration parses, not that any gate in it fires"
        );
        assert!(
            admissions > 0,
            "{path} expects {refusals} failure(s) and asserts nothing about a good value. A \
             gate proven only to refuse may refuse everything, which is the failure the \
             infrastructure rules name by name"
        );
        // Every run is a plan. An `apply` here would be a mocked apply rather
        // than a real one, but the rule in this repository is that an agent
        // shows a plan and a person applies, and a harness is not the place to
        // start blurring which verb is being run.
        assert!(
            !body.contains("command = apply"),
            "{path} runs `command = apply`. Every run in this repository's harnesses is a plan"
        );
        // A harness that reached a real project would be a plan against
        // infrastructure nobody approved — and, today, against a project that
        // has been torn down.
        assert!(
            body.contains("mock_provider"),
            "{path} does not mock its provider, so it would need a credential and would reach \
             a real project"
        );
    }
}

#[test]
fn the_four_gates_this_platform_cannot_read_from_text_each_have_a_plan() {
    // Named individually rather than counted, because the count above says
    // only that four files exist. These are the four gates whose correctness
    // is not visible in the HCL: the paper-trading ceiling, the zone model's
    // preconditions, the prefix check that could not run, and the refusal that
    // keeps a client off a trading zone.
    for (harness, what) in [
        (
            "infrastructure/terraform/tests/paper-boundary.tftest.hcl",
            "the first of the three paper-trading layers",
        ),
        (
            "infrastructure/terraform/modules/trust-zones/tests/zone-model.tftest.hcl",
            "the §46.1 zone model's preconditions",
        ),
        (
            "infrastructure/terraform/modules/network/tests/console-egress.tftest.hcl",
            "the console egress prefix check that could not run",
        ),
        (
            "infrastructure/terraform/modules/public-edge/tests/public-edge.tftest.hcl",
            "the refusal that keeps a client off a trading zone",
        ),
    ] {
        assert!(
            repository_root().join(harness).exists(),
            "{harness} is gone, so {what} is asserted only as text again"
        );
    }

    // The ceiling harness names all three live rungs, one run each. One run
    // naming the widest would pass with two of the three admitted:
    // `contains("autonomous_live")` is true of `"limited_autonomous_live"`,
    // which is the substring trap this repository has already been caught by.
    let ceiling = read("infrastructure/terraform/tests/paper-boundary.tftest.hcl");
    for rung in [
        "\"supervised_live\"",
        "\"limited_autonomous_live\"",
        "\"autonomous_live\"",
    ] {
        assert!(
            ceiling.contains(rung),
            "the ceiling harness does not plan {rung}. A live rung with no plan behind it is a \
             refusal nobody has watched fire"
        );
    }
}

#[test]
fn the_public_edge_attaches_every_backend_to_the_cloud_armor_policy() {
    // This cannot be asserted in the plan harness: `security_policy` holds the
    // policy's `id`, unknown until apply, and Terraform refuses a `plan` run
    // whose condition compares two unknowns rather than passing it vacuously.
    // So it is asserted on the configuration here, and the harness records
    // that the division exists.
    //
    // The failure it prevents is the one the observability rules name in
    // another domain: a control that reads as protection and is not. A Cloud
    // Armor policy attached to no backend shows in the console as a project
    // being protected while every request goes straight through.
    let module = without_comments(&read(PUBLIC_EDGE));
    assert!(
        module.contains("resource \"google_compute_security_policy\" \"edge\""),
        "the public edge declares no Cloud Armor policy"
    );
    assert!(
        module.contains("security_policy = google_compute_security_policy.edge[0].id"),
        "the public edge's backend service is not attached to its Cloud Armor policy; an \
         unattached policy protects nothing and reads in the console as protection"
    );

    // And the policy has to carry the two controls §40.14 names. A policy with
    // only its mandatory default `allow` rule is an empty policy that a
    // reviewer counting `google_compute_security_policy` resources would score
    // as present.
    assert!(
        module.contains("rate_limit_options"),
        "the Cloud Armor policy carries no rate limit; §40.14 requires one and a policy \
         without it is a default-allow rule with a name"
    );
    assert!(
        module.contains("origin.region_code"),
        "the Cloud Armor policy can express no geographic restriction; §40.14 names one"
    );
}

#[test]
fn the_public_edge_can_front_only_a_zone_a_client_may_reach() {
    // §40.5's load-bearing sentence: customer traffic and trading traffic never
    // share a load balancer, an identity, a credential or a route. The plan
    // harness proves the refusal fires for `execution`, `ledger` and
    // `treasury-write` and that `application-identity` is admitted. What it
    // cannot prove is that the module's list and §46.1's list are the same
    // list — two copies of a set drift, and the copy that drifts wider is the
    // one nobody notices.
    let edge = without_comments(&read(PUBLIC_EDGE));
    let zones = without_comments(&read(TRUST_ZONES));

    let extract = |text: &str, source: &str| -> Vec<String> {
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with("client_reachable_zones"))
            .unwrap_or_else(|| panic!("{source} declares client_reachable_zones"));
        line.split('[')
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .unwrap_or("")
            .split(',')
            .map(|entry| entry.trim().trim_matches('"').to_string())
            .filter(|entry| !entry.is_empty())
            .collect()
    };

    let edge_zones = extract(&edge, "modules/public-edge");
    let model_zones = extract(&zones, "modules/trust-zones");

    // Premise: the walk found real names, so an equality below is an equality
    // between two populated sets rather than between two empty ones.
    assert_eq!(
        edge_zones,
        vec![
            "public-edge".to_string(),
            "application-identity".to_string()
        ],
        "the public edge's client-reachable zones are {edge_zones:?}; §46.1 marks exactly \
         public-edge and application-identity"
    );
    assert_eq!(
        edge_zones, model_zones,
        "the public edge would front {edge_zones:?} and the zone model marks {model_zones:?} \
         client-reachable. Two copies of this set have drifted, and the wider one decides \
         whether a browser can be given a route to a trading zone"
    );

    // The refusal is a precondition — a plan that stops — rather than a
    // `count` that silently creates nothing. A backend that vanishes because
    // its zone was wrong is a surface that reads as published and serves 404.
    assert!(
        edge.contains("precondition"),
        "the zone refusal is no longer a precondition; a plan has to stop on it"
    );
}

#[test]
fn no_environment_opens_a_public_address() {
    // Every environment leaves `public_edge` at its default, so the module
    // plans to nothing in all four. The failure this prevents is a hostname
    // added to a tfvars as documentation — the module treats a hostname as the
    // switch for the entire edge, so a name written down to show the shape of
    // the thing would allocate a global address and a forwarding rule on 443.
    let mut checked = 0usize;
    for environment in ["dev", "test", "stage", "prod"] {
        let path = format!("infrastructure/environments/{environment}/terraform.tfvars");
        let tfvars = without_comments(&read(&path));
        checked += 1;
        assert!(
            !tfvars.contains("public_edge"),
            "{path} declares public_edge. Nothing is deployed to put behind an edge, and a \
             hostname there is a public address on the internet rather than a note about one. \
             If a customer surface is genuinely being published, this test is the place to \
             record that decision"
        );
    }
    assert_eq!(checked, 4, "only {checked} environments were read");

    // And the default is the off state rather than a range or a name, so the
    // absence above is absence rather than a value inherited from the root.
    let variables = without_comments(&read("infrastructure/terraform/variables.tf"));
    let declaration = variables
        .split("variable \"public_edge\"")
        .nth(1)
        .expect("the root declares public_edge")
        .split("\nvariable ")
        .next()
        .unwrap_or_default();
    assert!(
        declaration.contains("default  = {}") || declaration.contains("default = {}"),
        "public_edge's default is not the empty object; an edge created because a variable had \
         a default is a public address nobody decided to open"
    );
}
