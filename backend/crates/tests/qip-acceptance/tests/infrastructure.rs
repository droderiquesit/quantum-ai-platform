//! Structural checks on the infrastructure configuration.
//!
//! `terraform validate` catches a configuration that will not parse or whose
//! references do not resolve. These catch a configuration that parses
//! perfectly and would deploy something unsafe — a service open to the
//! internet, a node with an external address, a secret in an environment.
//!
//! They are string checks on HCL and YAML rather than a parse, which is a
//! trade: they cannot understand the configuration, so they can be fooled by
//! someone rewriting it. What they can do is fail when a security property is
//! deleted, which is the change that actually happens.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

/// Whether a configuration sets a boolean setting to a value.
///
/// Compares with whitespace collapsed, so a `terraform fmt` that realigns the
/// equals signs does not break a security check. A test that fails on
/// formatting is a test people learn to edit rather than read.
fn sets(content: &str, setting: &str, value: &str) -> bool {
    content.lines().any(|line| {
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        collapsed == format!("{setting} = {value}")
    })
}

/// A configuration with its comments removed.
///
/// A comment mentioning a dangerous value is not the same as setting one, and
/// a check that cannot tell the difference makes it impossible to document
/// why the value is refused.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

// --- secrets ----------------------------------------------------------------

#[test]
fn no_secret_value_appears_in_the_terraform() {
    // The rule the secrets module exists to enforce: a state file that leaks
    // contains the shape of the deployment and none of its credentials.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = std::fs::read_to_string(&path).expect("readable");
        assert!(
            !content.contains("google_secret_manager_secret_version"),
            "{} creates a secret version, which puts its value in state",
            path.display()
        );
        assert!(
            !content.contains("secret_data"),
            "{} sets secret data in Terraform",
            path.display()
        );
    }
}

/// A line's whitespace collapsed to single spaces.
///
/// So a `terraform fmt` that realigns an equals sign does not change what
/// these evaluators read. A check that fails on formatting is a check people
/// learn to edit rather than read.
fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The one right-hand side of `key = ...` in a Terraform file, comments gone.
///
/// Exactly one spelling: a second assignment of the same key in different
/// words is the shape this whole area of the configuration is being kept out
/// of. The live-capability question was answered in three spellings and two
/// of them were backwards, so a divergent duplicate is a finding rather than
/// an inconvenience.
fn sole_assignment(path: &str, key: &str) -> String {
    let text = without_comments(&read(path));
    let matches: Vec<String> = text
        .lines()
        .filter_map(|line| {
            collapsed(line)
                .strip_prefix(&format!("{key} = "))
                .map(str::to_string)
        })
        .collect();
    // More than one module may consume the local — the secrets module and
    // every execution node both take `venue_credential_readable` — and that
    // is one spelling written twice, not two spellings. Two *different*
    // right-hand sides is the finding.
    let mut spellings = matches.clone();
    spellings.dedup();
    assert_eq!(
        spellings.len(),
        1,
        "{path} assigns `{key}` in {} different spellings: {spellings:?}; expected exactly one",
        spellings.len()
    );
    matches.into_iter().next().unwrap_or_default()
}

/// The `value` expression of a named output.
fn output_value(name: &str) -> String {
    let text = without_comments(&read("infrastructure/terraform/outputs.tf"));
    let start = text
        .find(&format!("output \"{name}\" {{"))
        .unwrap_or_else(|| panic!("outputs.tf declares no output named `{name}`"));
    text[start..]
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with('}'))
        .find_map(|line| collapsed(line).strip_prefix("value = ").map(str::to_string))
        .unwrap_or_else(|| panic!("the `{name}` output has no value expression"))
}

/// A boolean expression from the root module, evaluated at one autonomy
/// ceiling.
///
/// Evaluating the predicate rather than matching its text is the whole point.
/// The previous version of the venue-credential test asserted the literal
/// source line `venue_credential_readable = var.autonomy_ceiling !=
/// "paper_trading"`, and that predicate was inverted: with the three live
/// rungs refused at plan time, `!= "paper_trading"` is true for exactly
/// `observation` and `advisory` and false for every ceiling that could use a
/// venue credential. A test that pins source text certifies whatever is
/// written there, including the bug it was written to prevent — it passed for
/// as long as the defect existed and would have failed on the fix.
///
/// Deliberately a small evaluator over the forms this question has been asked
/// in rather than a general HCL one: an unrecognised form panics naming
/// itself, so a rewrite makes this check fail loudly instead of quietly
/// stopping.
fn evaluate_at(expression: &str, ceiling: &str) -> bool {
    // The single definition every consumer now indirects through. Resolved
    // rather than trusted, so the tests below are asserting about the value
    // the configuration actually computes and not about a name.
    if expression == "local.ceiling_reaches_a_venue" {
        let definition = sole_assignment(
            "infrastructure/terraform/main.tf",
            "ceiling_reaches_a_venue",
        );
        assert_ne!(
            definition, expression,
            "`ceiling_reaches_a_venue` is defined as itself"
        );
        return evaluate_at(&definition, ceiling);
    }
    if let Some(rest) = expression.strip_prefix("contains([") {
        let (list, tail) = rest
            .split_once("],")
            .unwrap_or_else(|| panic!("`{expression}` is a contains() whose list does not close"));
        assert_eq!(
            tail.trim(),
            "var.autonomy_ceiling)",
            "`{expression}` tests membership of something other than the ceiling"
        );
        return list
            .split(',')
            .map(|entry| entry.trim().trim_matches('"'))
            .any(|entry| entry == ceiling);
    }
    if let Some(rest) = expression.strip_prefix("var.autonomy_ceiling ") {
        if let Some(value) = rest.strip_prefix("== ") {
            return value.trim().trim_matches('"') == ceiling;
        }
        if let Some(value) = rest.strip_prefix("!= ") {
            return value.trim().trim_matches('"') != ceiling;
        }
    }
    panic!(
        "`{expression}` is a form this check cannot evaluate. Teach it the new \
         form — deleting the check leaves the venue credential's IAM grant and \
         the live-capability indicators with nothing asserting which ceilings \
         they appear at."
    )
}

/// A string-valued expression from the root module, evaluated at one ceiling.
///
/// Two forms, and the second is here because it is the one that was wrong: a
/// ternary over two string literals is a shape whose arms can be swapped by a
/// one-character edit that still reads as correct. Evaluating it means the
/// mutation is caught by a rung-by-rung assertion rather than by a panic about
/// an unfamiliar syntax.
fn evaluate_string_at(expression: &str, ceiling: &str) -> String {
    if let Some(inner) = expression
        .strip_prefix("tostring(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return evaluate_at(inner.trim(), ceiling).to_string();
    }
    if let Some((condition, arms)) = expression.split_once(" ? ") {
        let (taken, otherwise) = arms.split_once(" : ").unwrap_or_else(|| {
            panic!("`{expression}` is a conditional with no alternative branch")
        });
        let chosen = if evaluate_at(condition.trim(), ceiling) {
            taken
        } else {
            otherwise
        };
        return chosen.trim().trim_matches('"').to_string();
    }
    panic!("`{expression}` is a string form this check cannot evaluate")
}

/// Whether the root module's `venue_credential_readable` is true at a ceiling.
fn venue_credential_readable_at(ceiling: &str) -> bool {
    evaluate_at(
        &sole_assignment(
            "infrastructure/terraform/main.tf",
            "venue_credential_readable",
        ),
        ceiling,
    )
}

/// Whether the `live_capable` output is true at a ceiling.
fn live_capable_output_at(ceiling: &str) -> bool {
    evaluate_at(&output_value("live_capable"), ceiling)
}

/// What the `live_capable` resource label reads at a ceiling.
///
/// A string, and it stays a string: the consumer is a `gcloud ... --filter`
/// expression comparing against `false`, not a Terraform boolean.
fn live_capable_label_at(ceiling: &str) -> String {
    evaluate_string_at(
        &sole_assignment("infrastructure/terraform/main.tf", "live_capable"),
        ceiling,
    )
}

/// The rungs a plan can carry, and the rungs it cannot.
///
/// Taken from the code's own ladder rather than a literal list, so a seventh
/// rung is one these tests start covering rather than one they silently miss.
fn reachable_and_live_rungs() -> (
    Vec<qip_risk_engine::autonomy::AutonomyLevel>,
    Vec<qip_risk_engine::autonomy::AutonomyLevel>,
) {
    let ladder = qip_risk_engine::autonomy::AutonomyLevel::all();
    let (live, reachable): (Vec<_>, Vec<_>) = ladder.into_iter().partition(|level| level.is_live());
    assert_eq!(
        (live.len(), reachable.len()),
        (3, 3),
        "the ladder's live rungs changed; these tests' premise needs rewriting"
    );
    (reachable, live)
}

#[test]
fn the_venue_credential_is_unreadable_where_live_trading_is_impossible() {
    // The infrastructure half of the live-trading control. The application
    // refuses a live order below a live autonomy level; this makes the
    // credential unreadable in an environment that could not use it anyway.
    //
    // Asserted per rung, because the failure this exists to catch is a
    // predicate that is the right shape and the wrong way round.
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    assert!(
        secrets.contains("count = var.venue_credential_readable ? 1 : 0"),
        "the venue credential's IAM binding must be conditional"
    );

    // The premise: which rungs a plan can actually carry. `variables.tf`
    // refuses the three live ones, so the reachable set is the ladder minus
    // them.
    let (reachable, live) = reachable_and_live_rungs();

    // No ceiling a plan can carry creates the grant. `observation` is the one
    // that mattered: an operator hardening dev to it got a plan that *added*
    // secretAccessor on the venue credential for the fast brain, because the
    // predicate was `!= "paper_trading"`. Lowering autonomy handed out the
    // credential.
    for level in &reachable {
        assert!(
            !venue_credential_readable_at(level.as_str()),
            "at the `{}` ceiling the venue credential's IAM grant is created. \
             No order can reach a venue at that rung, and a plan cannot carry \
             any other kind, so this grant should not exist in any applyable \
             configuration.",
            level.as_str()
        );
    }

    // And the predicate is keyed to live capability rather than merely switched
    // off. A bare `false` would satisfy every assertion above while recording
    // the answer and losing the question, and the next reader would delete the
    // variable along with the reason the resource exists. None of these three
    // ceilings can reach a plan — `variables.tf` refuses all of them, which is
    // exactly why the grant is unreachable — so this asserts what the
    // expression *means*, not that anything is permitted to trade live.
    for level in &live {
        assert!(
            venue_credential_readable_at(level.as_str()),
            "the venue credential's grant is not conditioned on the ceiling \
             being able to use it: `{}` is a live rung and the predicate is \
             false for it, so the predicate no longer says why the resource is \
             guarded at all",
            level.as_str()
        );
    }
}

#[test]
fn no_ceiling_a_plan_can_carry_is_reported_as_live_capable() {
    // `live_capable` is a safety indicator, and an inverted one is worse than
    // an absent one. The output is what an operator reads to answer "could
    // this cluster trade"; the label is what a fleet-wide query reads to
    // answer it across every project. Both were `!= "paper_trading"` in one
    // spelling or another, which is true for exactly the two rungs *below*
    // paper trading — so setting an environment to `observation`, the safest
    // rung and the one `variables.tf`'s error message invites an operator to
    // choose, reported the environment as able to reach a real venue.
    //
    // Asserted per rung rather than against source text, because the failure
    // this exists to catch is a predicate of the right shape and the wrong way
    // round, and a text match certifies that shape either way.
    let (reachable, live) = reachable_and_live_rungs();

    for level in &reachable {
        assert!(
            !live_capable_output_at(level.as_str()),
            "the `live_capable` output is true at the `{}` ceiling. No order \
             can reach a venue at that rung and a plan cannot carry any other \
             kind, so the output an operator reads says this cluster can trade \
             when nothing it can apply ever could.",
            level.as_str()
        );
        assert_eq!(
            live_capable_label_at(level.as_str()),
            "false",
            "every resource is labelled `live_capable = true` at the `{}` \
             ceiling, so a query asking which clusters can trade returns one \
             that cannot",
            level.as_str()
        );
    }

    // And both say the property rather than merely being switched off. A bare
    // `false` would satisfy everything above while recording the answer and
    // losing the question. None of these three ceilings can reach a plan —
    // `variables.tf` refuses all of them — so this asserts what the expression
    // *means*, not that anything is permitted to trade live.
    for level in &live {
        assert!(
            live_capable_output_at(level.as_str()),
            "the `live_capable` output is false at `{}`, a live rung, so it no \
             longer reports live capability at all",
            level.as_str()
        );
        assert_eq!(
            live_capable_label_at(level.as_str()),
            "true",
            "the `live_capable` label is not `true` at `{}`, a live rung, so \
             the label no longer answers the question it is named for",
            level.as_str()
        );
    }
}

#[test]
fn the_label_the_output_and_the_credential_predicate_agree_at_every_rung() {
    // Three expressions answered "could this reach a venue" in three
    // spellings, and that is how the inversion spread: the credential
    // predicate was corrected and the label and the output were not, because
    // nothing connected them. They now indirect through one local, and this is
    // what makes a future divergence a failing test rather than a discovery.
    //
    // Every rung, including the three no plan can carry — a divergence that
    // only appears at a ceiling `variables.tf` refuses is still a
    // configuration whose three answers disagree, and still the thing a reader
    // deleting that refusal would inherit.
    let (reachable, live) = reachable_and_live_rungs();

    for level in reachable.iter().chain(live.iter()) {
        let ceiling = level.as_str();
        let output = live_capable_output_at(ceiling);
        let label = live_capable_label_at(ceiling);
        let credential = venue_credential_readable_at(ceiling);
        assert_eq!(
            (output, label.as_str()),
            (credential, if credential { "true" } else { "false" }),
            "at the `{ceiling}` ceiling the configuration gives three answers \
             to one question: output={output}, label={label}, \
             venue_credential_readable={credential}"
        );
    }
}

// --- the runtime: Cloud Run, the execution node, the trust zones ------------
//
// ADR 0024 retired the GKE runtime and provisioned the blueprint's in code:
// every warm binary a Cloud Run service from `catalogue.tf`, the execution
// node a Compute Engine machine from `modules/execution-node`, both attached
// to the trust zones of `modules/trust-zones`. Every property the Kubernetes
// manifests used to carry — no root, no token, no route to the internet, a
// credential only as a file — is asserted here against the Terraform that
// now carries it. A property that held on the cluster and is not re-asserted
// on Cloud Run is a property that was lost in the move and reads as kept.

const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const CLOUD_RUN_MODULE: &str = "infrastructure/terraform/modules/cloudrun/main.tf";
const CLOUD_RUN_VARIABLES: &str = "infrastructure/terraform/modules/cloudrun/variables.tf";
const NODE_MODULE: &str = "infrastructure/terraform/modules/execution-node/main.tf";
const NODE_STARTUP: &str =
    "infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl";
const TRUST_ZONES_MODULE: &str = "infrastructure/terraform/modules/trust-zones/main.tf";

/// The catalogue's workload entries, as `(name, body)`, comments stripped.
///
/// An entry opens at four spaces of indent with `name = {` and closes at a
/// line that is exactly `    }`. Brittle on purpose: a reformatted catalogue
/// makes this return nothing, and every caller asserts it found the three
/// workloads, so the failure is loud rather than a check that quietly stops
/// checking.
fn catalogue_workloads() -> Vec<(String, String)> {
    let text = without_comments(&read(CATALOGUE));
    let start = text
        .find("cloud_run_catalogue = {")
        .expect("catalogue.tf declares local.cloud_run_catalogue");
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text[start..].lines().skip(1) {
        if line == "  }" {
            break;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 4 && line.trim_end().ends_with("= {") {
            let name = line.trim().trim_end_matches("= {").trim().to_string();
            current = Some((name, Vec::new()));
            continue;
        }
        if line == "    }" {
            if let Some((name, body)) = current.take() {
                entries.push((name, body.join("\n")));
            }
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["api", "fastbrain", "deepbrain"],
        "the catalogue parsed to something other than the three workloads ADR \
         0010 records; the entry shape has changed and every check reading it \
         is reading the wrong thing"
    );
    entries
}

/// The value of a scalar field at the top level of a catalogue entry.
fn catalogue_field(body: &str, field: &str) -> String {
    body.lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            let indent = line.len() - line.trim_start().len();
            (indent == 6 && key.trim() == field).then(|| value.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| panic!("the catalogue entry has no `{field}` field:\n{body}"))
}

/// The `QIP_` variables a catalogue entry sets in its `env`, by name.
///
/// Every `KEY = value` line whose key is a `QIP_` name, wherever it sits in
/// the entry — the fast brain's `env` is a `merge` of two maps and a walk that
/// only read the first would miss the connector.
fn catalogue_env(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (key.starts_with("QIP_")
                && key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// The secrets a catalogue entry mounts, by the name the root creates them
/// under, and the `_FILE` variable each is exposed as.
fn catalogue_secret_mounts(body: &str) -> Vec<(String, String)> {
    let names: Vec<String> = body
        .lines()
        .filter_map(|line| {
            let rest = line.split("secret_ids[\"").nth(1)?;
            rest.split('"').next().map(str::to_string)
        })
        .collect();
    let variables: Vec<String> = body
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "env_file_variable").then(|| value.trim().trim_matches('"').to_string())
        })
        .collect();
    assert_eq!(
        names.len(),
        variables.len(),
        "a secret mount names a secret without a _FILE variable or the reverse: {names:?} against {variables:?}"
    );
    names.into_iter().zip(variables).collect()
}

/// Every `resource "<type>" "<name>"` in a Terraform file, as name and body.
fn terraform_resources(text: &str, resource_type: &str) -> Vec<(String, String)> {
    let marker = format!("resource \"{resource_type}\" \"");
    text.split(&marker)
        .skip(1)
        .map(|rest| {
            let (name, body) = rest.split_once('"').unwrap_or((rest, ""));
            let body = body.split("\nresource ").next().unwrap_or(body);
            (name.to_string(), body.to_string())
        })
        .collect()
}

/// The keys of a `name = {` map declared in a tfvars or Terraform file: the
/// quoted or bare identifiers opening an entry one level inside it.
fn map_keys(text: &str, map: &str) -> Vec<String> {
    let block = block_under(text, &format!("{map} = {{"));
    block
        .lines()
        .filter_map(|line| {
            let indent = line.len() - line.trim_start().len();
            let (key, rest) = line.split_once('=')?;
            (indent == 2 && rest.trim() == "{").then(|| key.trim().trim_matches('"').to_string())
        })
        .collect()
}

#[test]
fn every_cloud_run_service_is_internal_and_mounts_secrets_as_files_never_as_environment() {
    // The two properties the Kubernetes manifests carried as a private
    // cluster and a CSI volume, re-asserted on the substrate that replaced
    // them. A service with `INGRESS_TRAFFIC_ALL` answers the internet at its
    // own URL, so the load balancer and its identity check become a route
    // rather than the route; a secret in the environment is a secret in
    // /proc/<pid>/environ, in every child process and in every crash dump.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    let variables = without_comments(&read(CLOUD_RUN_VARIABLES));

    // Ingress. No input produces the open setting, and the catalogue asks
    // for the closed one of the two that remain.
    // Read from the code and not the variable's description, which names
    // the open setting precisely to say why it is absent.
    assert!(
        !module.contains("INGRESS_TRAFFIC_ALL"),
        "the Cloud Run module can produce INGRESS_TRAFFIC_ALL, which answers the \
         internet at the service's own URL"
    );
    assert!(
        variables.contains("contains([\"internal\", \"public-edge\"], var.ingress_posture)"),
        "the ingress posture admits a value other than internal or public-edge"
    );
    assert!(
        module.contains("INGRESS_TRAFFIC_INTERNAL_ONLY"),
        "the Cloud Run module no longer names the internal-only ingress"
    );
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        sets(&catalogue, "ingress_posture", "\"internal\""),
        "catalogue.tf does not place every service behind the internal posture"
    );

    // Secrets as files. The module mounts a `secret` volume per entry and the
    // environment carries the path; nothing reads a secret into a value.
    assert!(
        module.contains("secret {") && module.contains("volume_mounts {"),
        "the Cloud Run module no longer mounts secrets as volumes"
    );
    for forbidden in ["value_source", "secret_key_ref"] {
        assert!(
            !module.contains(forbidden),
            "the Cloud Run module reads a secret into an environment value through `{forbidden}`"
        );
    }
    assert!(
        variables.contains("(TOKEN|SECRET|CREDENTIAL|PASSWORD|PRIVATE_KEY|_KEY)$"),
        "the module's `env` validation no longer refuses a credential-shaped variable name"
    );
    assert!(
        variables.contains("^QIP_[A-Z0-9_]*_FILE$"),
        "the module's `secret_mounts` validation no longer requires the _FILE form qip_core::secret reads"
    );

    // And the catalogue: every token reaches a workload as a mounted file,
    // never as an environment value, and no environment value looks like a
    // credential.
    let mut mounts = 0usize;
    for (name, body) in catalogue_workloads() {
        for (variable, value) in catalogue_env(&body) {
            assert!(
                !variable.ends_with("_TOKEN")
                    && !variable.contains("_TOKEN_")
                    && !variable.ends_with("_KEY")
                    && !variable.ends_with("_SECRET"),
                "{name} sets {variable} in its environment; a credential reaches a \
                 workload as a mounted file through secret_mounts"
            );
            assert!(
                !looks_like_a_credential(value.trim_matches('"')),
                "{name} sets {variable} to what looks like a literal credential"
            );
        }
        for (secret, variable) in catalogue_secret_mounts(&body) {
            assert!(
                variable.ends_with("_FILE"),
                "{name} mounts {secret} as {variable}, which is not the _FILE form the binary reads"
            );
            mounts += 1;
        }
    }
    assert!(
        mounts >= 8,
        "only {mounts} secret mounts were read across the catalogue; the API alone mounts six"
    );
}

#[test]
fn the_execution_node_has_no_external_address_and_no_container_runtime() {
    // Blueprint §41.4, the two lines the module can hold: a machine with no
    // external address cannot be reached from the internet, which is a
    // stronger statement than any firewall rule; and an image with a
    // container runtime brings a scheduler, a network namespace and daemons
    // that will preempt an isolated core at the worst moment.
    let module = without_comments(&read(NODE_MODULE));
    assert!(
        !module.contains("access_config"),
        "the execution node's template carries an access_config, which is an external address"
    );
    assert!(
        module.contains("network_interface {"),
        "the execution node has no network interface at all; this check is reading the wrong module"
    );
    for (setting, why) in [
        (
            "enable_secure_boot = true",
            "without it the boot chain is unverified",
        ),
        (
            "enable_vtpm = true",
            "without it there is nothing to attest the boot against",
        ),
        (
            "enable_integrity_monitoring = true",
            "without it a modified boot goes unreported",
        ),
        (
            "enable-oslogin = \"TRUE\"",
            "the set of people who may open a shell is defined by IAM and nothing else",
        ),
        (
            "serial-port-enable = \"FALSE\"",
            "the serial port is the one door that bypasses OS Login",
        ),
        (
            "on_host_maintenance = \"TERMINATE\"",
            "a live migration's pause is invisible to everything except a workload measured in microseconds",
        ),
    ] {
        assert!(
            sets(
                &module,
                setting.split(" = ").next().unwrap_or(setting),
                setting.split(" = ").nth(1).unwrap_or("")
            ),
            "the execution node is missing `{setting}`: {why}"
        );
    }

    // The runtime check is the startup script's, and it refuses rather than
    // logs: a node that came up with a runtime does not start.
    let startup = read(NODE_STARTUP);
    assert!(
        startup.contains("for runtime in docker containerd podman crio runc; do"),
        "the startup script no longer checks for a container runtime"
    );
    let check = startup
        .split("for runtime in docker containerd podman crio runc; do")
        .nth(1)
        .and_then(|rest| rest.split("done").next())
        .unwrap_or_default();
    assert!(
        check.contains("fail \""),
        "the startup script finds a container runtime and continues; the check has to refuse"
    );

    // And the machine shape is the blueprint's, refused rather than defaulted.
    let variables = read("infrastructure/terraform/modules/execution-node/variables.tf");
    for shape in [
        "c3-highcpu-8",
        "c3-highcpu-22",
        "c3d-highcpu-8",
        "c3d-highcpu-16",
    ] {
        assert!(
            variables.contains(shape),
            "the permitted machine types no longer include {shape}"
        );
    }
    assert!(
        !without_comments(&variables).contains("n2-standard")
            && !without_comments(&variables).contains("e2-standard"),
        "the execution node admits a general-purpose shape with no Titanium offload"
    );
    assert!(
        variables.contains("!can(regex(\"/family/\", var.boot_image))"),
        "the boot image may be named through a family, which is a moving pointer"
    );
}

#[test]
fn an_execution_node_may_reach_its_venues_and_the_central_plane_and_nothing_else() {
    // The most security-relevant rules in the configuration. A node holds the
    // whole hot path and decides without asking anyone; these rules are the
    // only thing between a compromised node and an arbitrary outbound
    // connection, and shadow mode is the difference between a node that has
    // a venue route and one that structurally cannot.
    let module = without_comments(&read(NODE_MODULE));
    let rules = terraform_resources(&module, "google_compute_firewall");
    assert!(
        rules.len() >= 5,
        "only {} firewall rules were read out of the execution-node module; the walk is not reaching them",
        rules.len()
    );

    let mut egress_allows: Vec<String> = Vec::new();
    let mut denies = 0usize;
    for (name, body) in &rules {
        let egress = body.contains("direction = \"EGRESS\"");
        if body.contains("deny {") {
            denies += 1;
            if egress {
                assert!(
                    body.contains("priority  = 65000") && body.contains("[\"0.0.0.0/0\"]"),
                    "the deny-all egress rule `{name}` is not at priority 65000 over the whole internet"
                );
            }
            continue;
        }
        assert!(
            body.contains("allow {"),
            "rule `{name}` neither allows nor denies"
        );
        if egress {
            egress_allows.push(name.clone());
        }
    }
    assert!(
        denies >= 2,
        "the node is not denied by default in both directions"
    );

    // Exactly these, and no proxy rule: the proxy is on loopback and there is
    // no address to permit.
    let mut expected = vec![
        "central_plane".to_string(),
        "google_apis".to_string(),
        "venue".to_string(),
    ];
    expected.sort();
    egress_allows.sort();
    assert_eq!(
        egress_allows, expected,
        "the execution node may egress through {egress_allows:?}. Anything beyond Google APIs, \
         the central plane and its own venues is a route out of the machine that holds the hot \
         path."
    );

    // Shadow mode is structural: the venue rule does not exist until it is
    // turned off, and turning it off is an edit in main.tf, not a tfvars
    // value.
    let (_, venue) = rules
        .iter()
        .find(|(name, _)| name == "venue")
        .expect("the venue rule exists");
    assert!(
        venue.contains("for_each = var.shadow_mode ? {} : var.venues"),
        "the venue egress rule is not gated on shadow mode; a node nobody has observed has a route to a venue"
    );
    let root = without_comments(&read("infrastructure/terraform/main.tf"));
    let node_block = root
        .split("module \"execution_node\" {")
        .nth(1)
        .and_then(|rest| rest.split("\nmodule ").next())
        .expect("the root instantiates the execution node");
    assert!(
        sets(node_block, "shadow_mode", "true"),
        "the root does not pass shadow_mode = true as a literal, so a tfvars value could let a node out of shadow mode"
    );
    assert!(
        node_block.contains("venue_credential_readable = local.ceiling_reaches_a_venue"),
        "the node's venue-credential predicate is spelt differently from the one root local"
    );
}

#[test]
fn the_execution_nodes_are_one_module_rather_than_nine_copies() {
    // Nine copies of a firewall rule is nine places for one of them to be
    // wrong, and the wrong one is the one nobody reads.
    let root = read("infrastructure/terraform/main.tf");
    assert!(
        root.contains("source   = \"./modules/execution-node\""),
        "there is no execution-node module"
    );
    assert!(
        root.contains("for_each = var.execution_nodes"),
        "the nodes are not instantiated from a variable"
    );

    // Every environment declares the map — empty, today, because a node
    // needs a venue — and the runbook carries the nine locations.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            tfvars.contains("execution_nodes = {"),
            "{environment} declares no execution_nodes map"
        );
    }
    let runbook = read("docs/operations/deploying-an-edge-cell.md");
    for cell in [
        "dallas-1",
        "chicago-1",
        "newyork-1",
        "london-1",
        "frankfurt-1",
        "singapore-1",
        "tokyo-1",
        "saopaulo-1",
        "dubai-1",
    ] {
        assert!(
            runbook.contains(cell),
            "the runbook does not name the {cell} cell"
        );
    }

    // The module's own precondition is what makes an empty map the honest
    // default rather than a broken one.
    let module = read(NODE_MODULE);
    assert!(
        module.contains("condition     = length(var.venues) > 0"),
        "the node no longer refuses a plan with no venue, so a node could be created that boots, fails and restarts for ever"
    );
}

#[test]
fn no_service_account_key_exists_anywhere_in_the_terraform() {
    // Workload Identity Federation only. A downloaded key would survive the
    // machine or the revision it was made for, and the machine is the only
    // thing the identity is for.
    let mut scanned = 0usize;
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        scanned += content.lines().count();
        assert!(
            !content.contains("google_service_account_key"),
            "{} creates a service-account key",
            path.display()
        );
    }
    assert!(scanned > 1000, "only {scanned} lines were scanned");
}

#[test]
fn every_service_account_terraform_creates_runs_something_or_signs_something() {
    // Two identities existed with nothing attached to them once. An unused
    // service account is not merely tidy-up: it is a set of permissions
    // nobody is watching, and the first sign that it is being used is that
    // something has used it. The set of accounts is therefore pinned, and an
    // account added anywhere has to say here what runs as it.
    let mut created: Vec<(String, String)> = Vec::new();
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        let module = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("?")
            .to_string();
        for (name, _) in terraform_resources(&content, "google_service_account") {
            created.push((module.clone(), name));
        }
    }
    created.sort();
    let mut expected = vec![
        // Every catalogue workload, one account each.
        ("cloudrun".to_string(), "workload".to_string()),
        // Every execution node, one account each.
        ("execution-node".to_string(), "node".to_string()),
        // The pipeline that builds, signs and deploys.
        ("cicd".to_string(), "ci".to_string()),
        // The account infra.yml plans and applies as.
        ("cicd".to_string(), "infra".to_string()),
        // The portal, deployed by scripts/deploy-frontends.sh.
        ("secrets".to_string(), "console".to_string()),
    ];
    expected.sort();
    assert_eq!(
        created, expected,
        "the set of service accounts Terraform creates has changed. Each entry above names \
         what runs as it; a new account is added here with what runs as it, or it is an \
         identity with nothing attached."
    );
}

#[test]
fn no_workload_runs_as_the_projects_default_compute_identity() {
    // The default compute service account is shared by everything in the
    // project that does not name one; a grant given to it for one workload
    // is a grant given to all of them.
    for path in [CLOUD_RUN_MODULE, NODE_MODULE] {
        let module = without_comments(&read(path));
        assert!(
            !module.contains("compute@developer.gserviceaccount.com"),
            "{path} names the default compute identity"
        );
    }
    let cloud_run = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        cloud_run
            .matches("service_account = google_service_account.workload.email")
            .count()
            >= 2,
        "the Cloud Run service or job does not run as the account the module creates for it"
    );
    let node = without_comments(&read(NODE_MODULE));
    assert!(
        node.contains("email = google_service_account.node.email"),
        "the execution node's template does not run as the account the module creates for it"
    );
}

#[test]
fn every_secret_a_workload_mounts_is_created_by_terraform_and_granted_to_it() {
    // The chain this pins: the catalogue names a secret to mount; the secrets
    // module creates that secret; the Cloud Run module grants the workload's
    // identity read on exactly what it mounts. Each link lived in a different
    // file on GKE and nothing held them together, which is how the platform
    // shipped with the API's tokens named in a Secret that nothing created.
    let root = without_comments(&read("infrastructure/terraform/main.tf"));
    let created: Vec<String> = block_under(&root, "secret_names = [")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect();
    assert!(
        created.len() >= 8,
        "only {created:?} secrets were read out of the root; the list has been reshaped"
    );

    let mut mounted = 0usize;
    for (name, body) in catalogue_workloads() {
        let mounts = catalogue_secret_mounts(&body);
        assert!(
            !mounts.is_empty(),
            "{name} mounts no secret at all; every workload needs the envelope key"
        );
        for (secret, _) in mounts {
            assert!(
                created.contains(&secret),
                "{name} mounts {secret} and the secrets module is never told to create it"
            );
            mounted += 1;
        }
        // Every central workload signs or verifies capital envelopes against
        // the one key.
        assert!(
            body.contains("secret_ids[\"qip-capital-envelope-key\"]"),
            "{name} does not mount the capital-envelope key"
        );
    }
    assert!(mounted >= 8, "only {mounted} mounts were checked");

    // And the grant follows the mount, in the module, with no wider list.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    let (_, grant) = terraform_resources(&module, "google_secret_manager_secret_iam_member")
        .into_iter()
        .find(|(name, _)| name == "mounted")
        .expect("the Cloud Run module grants read on mounted secrets");
    assert!(
        grant.contains("for_each = var.secret_mounts")
            && grant.contains("roles/secretmanager.secretAccessor"),
        "the Cloud Run module's secret grant is not keyed on exactly the mounts"
    );
}

#[test]
fn every_deployable_has_its_own_cloud_run_identity() {
    // A compromised component has only its own permissions, which is the
    // entire argument for not sharing one.
    let binaries: Vec<String> = catalogue_workloads()
        .iter()
        .map(|(_, body)| catalogue_field(body, "binary"))
        .collect();
    assert_eq!(
        binaries,
        vec!["qip-api", "qip-fastbrain", "qip-deepbrain"],
        "the catalogue no longer runs the three deployables ADR 0010 records"
    );
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains("account_id   = \"qip-${var.name}-${var.environment}\""),
        "the Cloud Run module no longer creates one account per workload"
    );
}

#[test]
fn a_cloud_run_service_cannot_be_deleted_by_a_plan_nobody_read() {
    // A service deleted by a plan nobody read is an outage with a Terraform
    // commit for a cause. Not a variable: the GKE runtime's deletion flag was
    // one, so a tfvars edit could turn it off, and the only environments
    // that needed it off were the ones infra.yml tears down — which is now
    // the execution node alone, because a service that scales to zero costs
    // nothing standing.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        sets(&module, "deletion_protection", "true"),
        "the Cloud Run service no longer refuses deletion"
    );
    let variables = without_comments(&read(CLOUD_RUN_VARIABLES));
    assert!(
        !variables.contains("deletion_protection"),
        "deletion protection has become an input, so a tfvars value can turn it off"
    );
}

#[test]
fn every_key_rotates_and_the_ones_that_hold_data_cannot_be_destroyed() {
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    assert_eq!(
        secrets.matches("rotation_period").count(),
        2,
        "the secrets key and the secrets themselves rotate; the GKE node-encryption key left with the cluster"
    );
    assert_eq!(
        secrets.matches("prevent_destroy = true").count(),
        1,
        "destroying the key that encrypts every secret destroys every secret"
    );
    for (path, why) in [
        (
            "infrastructure/terraform/modules/evidence/main.tf",
            "destroying the evidence key deletes evidence without touching an object",
        ),
        (
            "infrastructure/terraform/modules/backup/main.tf",
            "destroying the snapshot key deletes every backup without touching a snapshot",
        ),
        (
            "infrastructure/terraform/modules/binaryauthorization/main.tf",
            "destroying the attestor key makes every attestation ever made unverifiable",
        ),
    ] {
        assert!(
            read(path).contains("prevent_destroy = true"),
            "{path}: {why}"
        );
    }
}

#[test]
fn the_venue_credential_is_bound_to_the_fast_brain_and_only_where_the_ceiling_permits() {
    // The one workload that could ever hold the venue credential, named by
    // the root from the catalogue, and the grant still conditional on the
    // ceiling. `the_venue_credential_is_unreadable_where_live_trading_is_impossible`
    // evaluates the predicate rung by rung; this pins which identity it
    // lands on when it ever does.
    let root = without_comments(&read("infrastructure/terraform/main.tf"));
    assert!(
        root.contains(
            "venue_credential_reader = module.cloud_run[\"fastbrain\"].service_account_email"
        ),
        "the venue credential's reader is not the fast brain's Cloud Run identity"
    );
    let secrets = without_comments(&read("infrastructure/terraform/modules/secrets/main.tf"));
    let (_, grant) = terraform_resources(&secrets, "google_secret_manager_secret_iam_member")
        .into_iter()
        .find(|(name, _)| name == "venue_credential")
        .expect("the secrets module holds the venue-credential grant");
    assert!(
        grant.contains("count = var.venue_credential_readable ? 1 : 0"),
        "the venue-credential grant is no longer conditional"
    );
    assert!(
        grant.contains("member    = \"serviceAccount:${var.venue_credential_reader}\""),
        "the venue-credential grant lands on something other than the named reader"
    );
    // And nothing else in the secrets module creates an identity to grant it
    // to: the workload accounts left with the cluster.
    assert!(
        terraform_resources(&secrets, "google_service_account")
            .iter()
            .all(|(name, _)| name == "console"),
        "the secrets module creates a workload identity again; every workload's account is the Cloud Run module's"
    );
}

#[test]
fn every_cloud_run_workload_is_placed_in_a_declared_trust_zone_and_carries_its_tag() {
    // A workload with no zone has no subnet, no tag and no rule — and reads,
    // in the console, as a service in a VPC. The catalogue names a zone per
    // workload, every environment declares that zone, and the module puts the
    // zone's tag on the interface so the zone's rules see the instance.
    let thirteen: Vec<String> = block_under(&read(TRUST_ZONES_MODULE), "zone_names = [")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect();
    assert_eq!(
        thirteen.len(),
        13,
        "the trust-zone module no longer names thirteen zones: {thirteen:?}"
    );

    let placed: Vec<(String, String)> = catalogue_workloads()
        .iter()
        .map(|(name, body)| (name.clone(), catalogue_field(body, "trust_zone")))
        .collect();
    for (name, zone) in &placed {
        assert!(
            thirteen.contains(zone),
            "{name} is placed in `{zone}`, which is not one of the thirteen zones"
        );
        assert_ne!(
            zone, "execution",
            "{name} is a Cloud Run service placed in the execution zone, which is the node's"
        );
    }
    let mut zones: Vec<&String> = placed.iter().map(|(_, zone)| zone).collect();
    zones.sort();
    zones.dedup();
    assert_eq!(
        zones.len(),
        placed.len(),
        "two catalogue workloads share a trust zone; each has its own identity and its own boundary"
    );

    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        let declared = map_keys(&tfvars, "trust_zones");
        assert!(
            declared.len() >= 3,
            "{environment} declares {declared:?}; the zone map has been reshaped or emptied"
        );
        for (name, zone) in &placed {
            assert!(
                declared.contains(zone),
                "{environment} does not declare the `{zone}` zone that {name} is placed in, so \
                 that workload has no subnet there"
            );
        }
    }

    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("network_tags   = compact([lookup(module.trust_zones.zone_network_tags, each.value.trust_zone, \"\")])"),
        "the catalogue no longer puts the zone's tag on the workload's interface, so the zone's rules never see it"
    );
    assert!(
        catalogue.contains(
            "egress_subnet  = lookup(module.trust_zones.zone_subnets, each.value.trust_zone, null)"
        ),
        "the catalogue no longer attaches a workload to its zone's subnet"
    );
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains("tags = var.network_tags"),
        "the Cloud Run module drops the network tags on the floor"
    );
}

#[test]
fn the_trust_zones_deny_by_default_and_only_optimisation_may_reach_ibm() {
    // Blueprint §46.1, the two halves a network can hold: default deny in
    // both directions per zone, and an external allowlist whose IBM entry can
    // be declared under exactly one zone.
    let module = without_comments(&read(TRUST_ZONES_MODULE));
    for direction in ["deny_egress", "deny_ingress"] {
        let (_, body) = terraform_resources(&module, "google_compute_firewall")
            .into_iter()
            .find(|(name, _)| name == direction)
            .unwrap_or_else(|| panic!("the trust-zone module has no {direction} rule"));
        assert!(
            body.contains("for_each = var.zones")
                && body.contains("priority  = 65000")
                && body.contains("deny {"),
            "{direction} is not a per-zone deny at priority 65000"
        );
    }

    let purposes = block_under(&module, "sanctioned_egress_purposes = {");
    assert!(
        purposes.lines().count() >= 5,
        "the sanctioned egress purposes have been reshaped: {purposes}"
    );
    let ibm: Vec<&str> = purposes
        .lines()
        .filter(|line| line.contains("\"ibm-quantum\""))
        .collect();
    assert_eq!(
        ibm.len(),
        1,
        "ibm-quantum appears {} times in the sanctioned purposes; once, under optimisation, is the whole control",
        ibm.len()
    );
    assert!(
        ibm[0].trim().starts_with("\"optimisation\""),
        "ibm-quantum is sanctioned for `{}`, not for optimisation",
        ibm[0].trim()
    );
    assert!(
        module.contains("contains(lookup(local.sanctioned_egress_purposes, each.value.zone, []), each.value.purpose)"),
        "the external-egress rule no longer refuses a purpose its zone does not hold"
    );
}

#[test]
fn no_firewall_allow_rule_permits_the_whole_internet() {
    // A `0.0.0.0/0` in an allow rule undoes every other rule in the module,
    // and it is one line that looks like the others. Only a deny may name it.
    let mut allows = 0usize;
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for (name, body) in terraform_resources(&content, "google_compute_firewall") {
            if !body.contains("allow {") {
                continue;
            }
            allows += 1;
            for whole in ["\"0.0.0.0/0\"", "\"::/0\""] {
                assert!(
                    !body.contains(whole),
                    "{}: the allow rule `{name}` permits {whole}",
                    path.display()
                );
            }
        }
    }
    assert!(
        allows >= 6,
        "only {allows} allow rules were read; the walk is not reaching the modules"
    );
}

#[test]
fn the_fast_brain_cannot_reach_anything_that_could_serve_a_language_model() {
    // ADR 0008, consequence 3: nothing on the hot path consults a model. The
    // binary refuses to start if an agent it hosts holds `call_language_model`;
    // this is the deployment saying the same thing four more ways.
    let workloads = catalogue_workloads();
    let (_, fastbrain) = workloads
        .iter()
        .find(|(name, _)| name == "fastbrain")
        .expect("the catalogue deploys the fast brain");

    // 1. No egress proxy. Port 9102 on it is a route to Vertex, and the proxy
    //    is the only way off the instance.
    assert_eq!(
        catalogue_field(fastbrain, "egress_proxy"),
        "false",
        "the fast brain carries the egress proxy, which has a listener that reaches a model API"
    );
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("condition     = !local.cloud_run_catalogue.fastbrain.egress_proxy"),
        "nothing at plan time refuses giving the fast brain the proxy; the entry is a value somebody edits"
    );

    // 2. Its zone may reach nothing outside the VPC: no sanctioned purpose,
    //    so no allowlist entry can be declared for it by any spelling.
    let zone = catalogue_field(fastbrain, "trust_zone");
    let purposes = block_under(
        &without_comments(&read(TRUST_ZONES_MODULE)),
        "sanctioned_egress_purposes = {",
    );
    assert!(
        !purposes
            .lines()
            .any(|line| line.trim().starts_with(&format!("\"{zone}\""))),
        "the fast brain's zone `{zone}` may hold an external-egress entry, which is a route to something that could serve a model"
    );

    // 3. It carries nothing that could authenticate to one: the envelope key
    //    and no other secret, and no variable naming a provider or endpoint.
    let mounts = catalogue_secret_mounts(fastbrain);
    assert_eq!(
        mounts
            .iter()
            .map(|(secret, _)| secret.as_str())
            .collect::<Vec<_>>(),
        vec!["qip-capital-envelope-key"],
        "the fast brain mounts a secret other than the envelope key"
    );
    let lowered = fastbrain.to_lowercase();
    for token in [
        "openai",
        "anthropic",
        "vertex",
        "aiplatform",
        "model_endpoint",
        "llm",
    ] {
        assert!(
            !lowered.contains(token),
            "the fast brain's catalogue entry mentions {token}"
        );
    }

    // 4. And the honest limit of all of the above is written down rather than
    //    left for somebody to discover: the restricted VIP is one range for
    //    every Google API, Vertex AI included, so the network cannot finish
    //    the job on its own.
    let gaps = read("docs/operations/external-dependencies.md");
    assert!(
        gaps.contains("VPC Service Controls"),
        "the gap document does not name the control that would actually close the fast brain's egress to a model API"
    );
}

// --- the environments -------------------------------------------------------

#[test]
fn every_environment_ships_with_a_paper_trading_ceiling() {
    // The one line in the repository that decides whether the platform can
    // reach a real venue. Production included: raising it is a separate,
    // reviewed change.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        assert!(
            tfvars.contains(r#"autonomy_ceiling = "paper_trading""#),
            "{environment} does not default to paper trading"
        );
    }
}

#[test]
fn no_environment_authorises_the_whole_internet() {
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            !tfvars.contains("0.0.0.0/0"),
            "{environment} authorises the whole internet"
        );
    }
}

#[test]
fn the_autonomy_ceiling_variable_accepts_only_the_declared_levels() {
    // The six levels the application declares, and no seventh that would be
    // silently ignored.
    let variables = read("infrastructure/terraform/variables.tf");
    for level in [
        "observation",
        "advisory",
        "paper_trading",
        "supervised_live",
        "limited_autonomous_live",
        "autonomous_live",
    ] {
        assert!(
            variables.contains(level),
            "the ceiling variable does not accept {level}"
        );
    }
    // And the same six the code knows about, so the two cannot drift.
    let levels = qip_risk_engine::autonomy::AutonomyLevel::all();
    assert_eq!(levels.len(), 6);
    for level in levels {
        assert!(
            variables.contains(level.as_str()),
            "{} is a level in the code that the infrastructure does not accept",
            level.as_str()
        );
    }
}

#[test]
fn no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue() {
    // `every_environment_ships_with_a_paper_trading_ceiling` asserts what the
    // four committed files say today. This asserts what the *fifth* one, and
    // any edit to the four, will be allowed to say — which is the part that
    // survives somebody adding an environment.
    //
    // The refusal lives in Terraform because it is the earliest place a live
    // value can be stopped: before the apply writes it into the `qip-config`
    // ConfigMap that every workload reads its ceiling from. It is not the only
    // place, and is not meant to be — Terraform cannot see a `kubectl edit
    // configmap`, so the composition roots refuse the same values at start-up.
    // This layer catches the reviewed, committed mistake; that one catches the
    // unreviewed live edit.
    let variables = read("infrastructure/terraform/variables.tf");

    // The premise: there is a `validation` block that names the negation. A
    // test that only searched for the three level names would pass on the
    // *other* validation, the one that lists all six as permitted spellings.
    assert!(
        variables.contains("condition = !contains(["),
        "no validation refuses anything; the ceiling variable admits every \
         level it can spell"
    );

    // Each live rung named in that refusal. `AutonomyLevel::is_live` is the
    // authority on which those are, so a seventh level added to the ladder as
    // live is a level this test starts requiring — rather than one it silently
    // stops covering, which a hardcoded list of three would do.
    let live: Vec<_> = qip_risk_engine::autonomy::AutonomyLevel::all()
        .into_iter()
        .filter(|level| level.is_live())
        .collect();
    assert_eq!(
        live.len(),
        3,
        "the ladder's live rungs changed; this test's premise needs rewriting"
    );
    let refusal = variables
        .split("condition = !contains([")
        .nth(1)
        .unwrap_or_default();
    // Terminated on the argument that closes the call, not on `])`: the list
    // ends `], var.autonomy_ceiling)`, so a `])` delimiter finds nothing here
    // and swallows the rest of the file — including the *other* validation,
    // which names all six levels and would satisfy every assertion below.
    let refusal = refusal
        .split("], var.autonomy_ceiling)")
        .next()
        .unwrap_or_default();
    for level in live {
        // The quoted, comma-terminated token, not the bare name.
        // `limited_autonomous_live` contains `autonomous_live` as a substring,
        // so a bare `contains` reports the most permissive rung on the list
        // when only the middle one is — which is how the first version of this
        // test passed a mutation that deleted `autonomous_live` outright.
        let entry = format!("\"{}\",", level.as_str());
        assert!(
            refusal.contains(&entry),
            "{} reaches a real venue and the ceiling variable does not refuse it",
            level.as_str()
        );
    }

    // And the refusal stops at the live rungs. A validation that also refused
    // `paper_trading` would pass every assertion above and make the platform
    // unapplyable, which is a different failure and a worse one to debug.
    for permitted in ["observation", "advisory", "paper_trading"] {
        assert!(
            !refusal.contains(&format!("\"{permitted}\",")),
            "the ceiling variable refuses {permitted}, at which no order reaches a venue"
        );
    }
}

#[test]
fn every_attestation_command_the_pipeline_runs_has_a_grant_that_permits_it() {
    // Three permission failures in a row taught the shape of this bug: a role
    // named for the neighbouring half of the same product. `cloudkms.admin`
    // manages keys and excludes the crypto operations. `binaryauthorization.
    // policyAdmin` governs the policy and not the attestor resource. And
    // `containeranalysis.notes.attacher` grants `attachOccurrence` but not
    // `listOccurrences` — a verb apart on the same note, which cost a run that
    // had already built and pushed four images before it asked.
    //
    // Each of those was found by a real apply rather than by reading, so this
    // test works the other way round: from the gcloud commands the pipeline
    // actually runs to the grant that permits each one. A command added to the
    // workflow without its grant fails here rather than in a deployment.
    let workflow = read(".github/workflows/deploy.yml");
    let binauthz = read("infrastructure/terraform/modules/binaryauthorization/main.tf");

    // The premise: the workflow really does run these, so the mapping below is
    // about live commands rather than ones that were removed years ago.
    for command in [
        "gcloud container binauthz attestations list",
        "gcloud beta container binauthz attestations sign-and-create",
    ] {
        assert!(
            workflow.contains(command),
            "the workflow no longer runs `{command}`; this test's premise needs rewriting"
        );
    }

    // What each command asks of Google, and the role that answers it. Every
    // one of these is a grant to the CI account — the identity the pipeline
    // authenticates as — not to the infrastructure account, which never runs
    // the images job.
    for (need, role) in [
        // `attestations list` filters a note's occurrences by artifact url.
        (
            "list the occurrences already on the note",
            "roles/containeranalysis.notes.occurrences.viewer",
        ),
        // `sign-and-create` writes one, which is two acts: attach to the note,
        // and create the occurrence the note points at.
        (
            "attach an occurrence to the note",
            "roles/containeranalysis.notes.attacher",
        ),
        (
            "create the occurrence",
            "roles/containeranalysis.occurrences.editor",
        ),
        // It reads the attestor before signing for it,
        (
            "read the attestor being signed for",
            "roles/binaryauthorization.attestorsViewer",
        ),
        // and signs with the KMS key version the attestor names.
        (
            "sign with the attestor key",
            "roles/cloudkms.signerVerifier",
        ),
    ] {
        let granted = binauthz
            .split("resource \"")
            .any(|block| block.contains(role) && block.contains("var.ci_service_account"));
        assert!(
            granted,
            "nothing grants the pipeline account the role that lets it {need} \
             ({role}); the images job will build and push before it discovers this"
        );
    }
}

// --- Kubernetes -------------------------------------------------------------

#[test]
fn the_namespace_denies_all_traffic_by_default() {
    let namespace = read("infrastructure/kubernetes/base/namespace.yaml");
    assert!(
        namespace.contains("name: default-deny"),
        "without a default deny, every flow is permitted"
    );
    assert!(
        namespace
            .lines()
            .any(|line| line.trim() == "pod-security.kubernetes.io/enforce: restricted")
    );
}

#[test]
fn no_container_runs_as_root_or_can_escalate() {
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        if !is_workload(&content) {
            continue;
        }
        for (setting, why) in [
            (
                "runAsNonRoot: true",
                "a container running as root is a node compromise away",
            ),
            (
                "allowPrivilegeEscalation: false",
                "without it a setuid binary inside the container escalates",
            ),
            (
                "readOnlyRootFilesystem: true",
                "a writable root filesystem lets an attacker persist",
            ),
            (
                "drop: [\"ALL\"]",
                "a container needs none of the default capabilities",
            ),
            (
                "seccompProfile",
                "without one the container can make any syscall",
            ),
        ] {
            assert!(
                content.contains(setting),
                "{} is missing {setting}: {why}",
                path.display()
            );
        }
    }
}

#[test]
fn every_container_has_both_a_cpu_and_a_memory_limit() {
    // A memory limit without a CPU limit lets a busy pod starve its
    // neighbours; a CPU limit without a memory limit lets a leak take down the
    // node.
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        if !is_workload(&content) {
            continue;
        }
        assert!(
            content.contains("limits:"),
            "{} has no limits",
            path.display()
        );
        let after_limits = content.split("limits:").nth(1).unwrap_or("");
        assert!(
            after_limits.contains("cpu:"),
            "{} has no CPU limit",
            path.display()
        );
        assert!(
            after_limits.contains("memory:"),
            "{} has no memory limit",
            path.display()
        );
    }
}

#[test]
fn no_credential_appears_in_a_kubernetes_manifest() {
    // Every credential comes from a secret reference, never from a literal.
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("value:") {
                continue;
            }
            let value = line.trim_start_matches("value:").trim().trim_matches('"');
            assert!(
                !looks_like_a_credential(value),
                "{} has what looks like a literal credential: {line}",
                path.display()
            );
        }
        // And the tokens specifically come from the secret store, as files
        // projected by the CSI driver. `secretKeyRef` was the earlier shape
        // and is refused now: it reads a synced Kubernetes Secret, which puts
        // the plaintext in etcd and does not exist on a fresh cluster until
        // after the first pod has already failed.
        if content.contains("QIP_TOKEN_") {
            assert!(
                content.contains("secrets-store-gke.csi.k8s.io"),
                "{} sets a token without projecting it from the secret store",
                path.display()
            );
            assert!(
                !content.contains("secretKeyRef"),
                "{} reads a credential through a synced Kubernetes Secret; \
                 project it as a file through the CSI driver instead",
                path.display()
            );
        }
    }
}

/// Whether a manifest value looks like a credential rather than configuration.
///
/// Deliberately narrow: a check that flags every long string produces a wall of
/// false positives, and a wall of false positives is a check people stop
/// reading.
fn looks_like_a_credential(value: &str) -> bool {
    value.len() >= 24
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_uppercase())
}

#[test]
fn the_autonomy_ceiling_comes_from_a_named_resource_rather_than_a_command_line() {
    // Changing what the platform is permitted to do should appear in a diff
    // and in an audit log.
    let api = read("infrastructure/kubernetes/base/api.yaml");
    assert!(
        api.contains("configMapKeyRef"),
        "the ceiling must come from a config map"
    );
    let config = read("infrastructure/kubernetes/base/config.yaml");
    assert!(
        config.contains(r#"autonomy_ceiling: "paper_trading""#),
        "the shipped config map must be paper trading"
    );
}

#[test]
fn the_api_pod_mounts_no_service_account_token() {
    // The workload authenticates through workload identity; a mounted token is
    // a credential nothing needs.
    let api = read("infrastructure/kubernetes/base/api.yaml");
    assert!(
        api.lines()
            .any(|line| line.trim() == "automountServiceAccountToken: false")
    );
}

// --- CI ---------------------------------------------------------------------

#[test]
fn the_pipeline_gates_on_everything_it_claims_to() {
    let workflow = read(".github/workflows/ci.yml");
    for gate in [
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets",
        "cargo test --workspace",
        "cargo build --workspace --release --locked",
        "check-dependencies.sh",
        "cargo audit",
        "cargo cyclonedx",
        "terraform",
        "check-secrets.sh",
    ] {
        assert!(workflow.contains(gate), "the pipeline does not run {gate}");
    }
}

#[test]
fn the_pipeline_fails_on_warnings() {
    // A warning nobody has to fix is a warning everybody stops reading.
    let workflow = read(".github/workflows/ci.yml");
    assert!(workflow.contains(r#"RUSTFLAGS: "-D warnings""#));
}

#[test]
fn the_pipeline_starts_with_the_least_permission_it_can() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(
        workflow.contains("contents: read"),
        "the default token must be read-only"
    );
}

#[test]
fn the_dependency_policy_is_enforced_rather_than_documented() {
    let script = read("scripts/check-dependencies.sh");
    assert!(script.contains("set -euo pipefail"));
    assert!(script.contains("readonly PERMITTED"));
    // And it actually reads the lockfile rather than the manifests, so a
    // transitive dependency cannot slip past.
    assert!(script.contains("Cargo.lock"));
}

// --- the workloads and the binaries they run --------------------------------
//
// The bug these exist to catch is already in the repository's history: an
// `allow-deepbrain-egress` NetworkPolicy governing a pod that no Deployment
// creates. A rule for a workload that does not exist is not harmless — it is a
// reviewer reading the namespace and concluding the deep brain is deployed and
// constrained, when it is neither.
//
// So the correspondence is checked in both directions. A test that only asked
// "does every binary have a Deployment" would have passed on that namespace
// without noticing, because the missing half was the Deployment.

/// Binaries in the workspace that are deliberately not deployed as a workload.
///
/// Kept as a list with a reason attached rather than as a filter in the test,
/// because "why is this one exempt" is the question the next person will have
/// and a predicate cannot answer it.
const NOT_A_WORKLOAD: &[(&str, &str)] = &[(
    // `qip-cli` builds a binary called `qip`.
    "qip",
    "an operator's tool, run by a person against a cluster rather than \
     scheduled in one",
)];

/// Binaries the workspace builds that the pipeline deliberately builds no image
/// for.
///
/// The sibling of `NOT_A_WORKLOAD`, and deliberately a second list rather than
/// a reuse of the first: "nothing schedules it" and "nothing builds it" are
/// different decisions, and a crate could sensibly be one without the other —
/// an operator tool distributed as an image and run as a `Job` would be in the
/// matrix and absent from the manifests.
///
/// Each entry carries the crate it comes from as well as the binary, because
/// the two differ exactly where this matters: `qip-cli` builds `qip`, and a
/// reader searching the decision record for "qip" finds every line in it.
///
/// The reasons are the short form. The decision is
/// `docs/adr/0010-what-gets-deployed.md`, and
/// `every_deployment_exclusion_is_recorded_as_a_decision` is what keeps the two
/// from drifting.
const NOT_IN_THE_IMAGE_MATRIX: &[(&str, &str, &str)] = &[(
    "qip-cli",
    "qip",
    "an operator's tool, run by a person against a cluster rather than \
     scheduled in one",
)];

/// Deployments whose binary is not in the workspace yet.
///
/// This list has to shrink to nothing, and
/// `every_pending_workload_is_still_actually_pending` is what makes it: that
/// test fails the moment the crate lands, so the exemption is removed by
/// whoever lands it rather than surviving as a permanent hole in the check
/// above.
const AWAITING_ITS_CRATE: &[(&str, &str)] = &[];

/// Workloads whose binary has no serving loop, so no probe can be written.
///
/// Same discipline: a probe pointed at an endpoint that does not exist looks
/// like coverage and is not, and an exemption that cannot expire is a
/// permanent one.
const DOES_NOT_SERVE_YET: &[(&str, &str)] = &[
    // Empty, and the list stays here rather than being deleted with its last
    // entry: the discipline it encodes — an exemption states its reason and
    // expires when the reason does — is what the next unserving binary needs,
    // and re-deriving it under deadline is how a permanent hole gets opened.
    //
    // `qip-fastbrain` was the first to leave. `qip-deepbrain` was the last: it
    // now runs a bounded research loop behind its roster validation and serves
    // `/health` and `/ready`, so its Deployment carries real probes and the
    // exemption would be describing a binary that no longer exists.
];

/// The binaries the workspace actually builds, by binary name.
///
/// Read from the manifests rather than from a list here, because a list here
/// would be a second copy of the truth and the whole point of this section is
/// that the two copies drift.
fn workspace_binaries() -> Vec<String> {
    let mut found = Vec::new();
    for path in files_with_extension("backend/crates", "toml") {
        if path.file_name().is_none_or(|name| name != "Cargo.toml") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable");
        let Some(directory) = path.parent() else {
            continue;
        };
        // A `[[bin]]` section names the binary explicitly; a `src/main.rs`
        // with no section produces one named after the package. Both count,
        // because both are something a Deployment could run.
        let declared: Vec<String> = content
            .split("[[bin]]")
            .skip(1)
            .filter_map(|section| {
                section
                    .lines()
                    .find(|line| line.trim_start().starts_with("name"))
                    .and_then(|line| line.split('"').nth(1))
                    .map(str::to_string)
            })
            .collect();
        if !declared.is_empty() {
            found.extend(declared);
            continue;
        }
        if directory.join("src/main.rs").exists() {
            let name = content
                .lines()
                .find(|line| line.trim_start().starts_with("name"))
                .and_then(|line| line.split('"').nth(1))
                .expect("a package declares a name");
            found.push(name.to_string());
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.len() >= 4,
        "only {found:?} were found; the manifest walk is not reaching the apps"
    );
    found
}

/// Every YAML document in every manifest, paired with the file it came from.
fn manifest_documents() -> Vec<(std::path::PathBuf, String)> {
    let mut documents = Vec::new();
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for document in content.split("\n---\n") {
            documents.push((path.clone(), document.to_string()));
        }
    }
    assert!(
        documents.len() > 10,
        "only {} documents were found; the manifest walk is not reaching them",
        documents.len()
    );
    documents
}

/// Documents of one kind.
/// Documents whose **own** kind is `kind`.
///
/// The match is anchored at column zero rather than trimmed, because `kind:`
/// appears nested inside several Kubernetes objects and a trimmed match reads
/// those as the document's own kind. A `HorizontalPodAutoscaler` names its
/// target in a `scaleTargetRef` containing `kind: Deployment`, so the trimmed
/// version pulled every HPA into the Deployment set and then panicked on it
/// having no container — which made a correctly written HPA fail four tests
/// that have nothing to do with autoscaling.
///
/// The workaround at the time was to write `scaleTargetRef` as an inline flow
/// mapping so the line never appeared on its own. That is a coupling between a
/// manifest's formatting and a test's parser, and the kind of thing that holds
/// until somebody reformats a file for good reasons.
fn documents_of_kind(kind: &str) -> Vec<(std::path::PathBuf, String)> {
    manifest_documents()
        .into_iter()
        .filter(|(_, document)| document.lines().any(|line| line == format!("kind: {kind}")))
        .collect()
}

/// The value of the first `key:` in a document, at any indentation.
fn first_value(document: &str, key: &str) -> Option<String> {
    document.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(&format!("{key}:"))
            .map(|value| value.trim().trim_matches('"').to_string())
    })
}

/// The lines nested under `key`, by indentation.
///
/// Splitting on a key and then on a fixed indent looks simpler and is wrong:
/// the remainder begins with the newline the split left behind, so the first
/// element is empty and the check quietly passes on nothing.
fn block_under(text: &str, key: &str) -> String {
    let mut lines = text.lines();
    let opening = lines.find(|line| line.trim() == key);
    let Some(opening) = opening else {
        return String::new();
    };
    let indent = opening.len() - opening.trim_start().len();
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() > indent)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a manifest declares a workload that runs containers.
///
/// A `StatefulSet` is a `Deployment` that keeps its volumes, and every rule in
/// this file — no root, no escalation, limits, a probe, a pinned image — is
/// about the container rather than about which controller manages it. Keying
/// these checks on `kind: Deployment` alone is how a workload converted to a
/// `StatefulSet` would quietly stop being checked.
///
/// Anchored at column zero, exactly like `documents_of_kind` and for exactly
/// its reason: `kind:` appears nested inside other objects' target
/// references, and the substring version of this helper pulled
/// `autoscaling.yaml` — three VerticalPodAutoscalers whose `targetRef` each
/// name `kind: Deployment` — into the workload set and demanded its
/// nonexistent containers carry probes and drop capabilities. The parser
/// above had already been fixed; this helper was the copy the fix missed.
fn is_workload(content: &str) -> bool {
    WORKLOAD_KINDS
        .iter()
        .any(|kind| content.lines().any(|line| line == format!("kind: {kind}")))
}

/// The kinds of workload the manifests may declare.
const WORKLOAD_KINDS: [&str; 2] = ["Deployment", "StatefulSet"];

/// Every workload document in the manifests, of either kind.
///
/// The single place that knows which controllers run containers, so a third
/// kind is one edit rather than nine.
fn workload_documents() -> Vec<(std::path::PathBuf, String)> {
    WORKLOAD_KINDS
        .iter()
        .flat_map(|kind| documents_of_kind(kind))
        .collect()
}

/// The container blocks inside a workload document.
///
/// Split on the fixed indentation these manifests use. That is brittle on
/// purpose: a reindented manifest makes this return nothing, and every caller
/// asserts it found at least one container, so the failure is loud rather than
/// a check that quietly stops checking.
fn containers(document: &str) -> Vec<String> {
    let Some(start) = document.find("\n      containers:\n") else {
        return Vec::new();
    };
    let body = &document[start..];
    let body = body.split("\n      volumes:").next().unwrap_or(body);
    body.split("\n        - name: ")
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// The binary a container runs, taken from its image reference.
///
/// The image is the honest link between a Deployment and a binary: a container
/// name is a label somebody chose, and an image is the thing that will actually
/// execute.
fn container_binary(container: &str) -> String {
    let image = container
        .lines()
        .find_map(|line| line.trim().strip_prefix("image:"))
        .unwrap_or_else(|| panic!("a container declares an image:\n{container}"))
        .trim();
    let without_tag = image.split(':').next().unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

/// Every binary a Deployment in the manifests runs.
fn deployed_binaries() -> Vec<String> {
    let mut deployed: Vec<String> = WORKLOAD_KINDS
        .iter()
        .flat_map(|kind| documents_of_kind(kind))
        .collect::<Vec<_>>()
        .iter()
        .flat_map(|(path, document)| {
            let found = containers(document);
            assert!(
                !found.is_empty(),
                "{} has a workload with no container the split could find; \
                 the manifest's indentation has changed and this check has \
                 stopped checking",
                path.display()
            );
            found
        })
        .map(|container| container_binary(&container))
        .collect();
    deployed.sort();
    deployed.dedup();
    deployed
}

#[test]
fn every_deployable_binary_in_the_workspace_has_a_deployment() {
    let deployed = deployed_binaries();
    for binary in workspace_binaries() {
        if NOT_A_WORKLOAD.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            deployed.contains(&binary),
            "{binary} is a binary this workspace builds and nothing deploys it. \
             Either give it a Deployment or add it to NOT_A_WORKLOAD with the \
             reason it is not one."
        );
    }
}

#[test]
fn every_deployment_runs_a_binary_that_exists() {
    // The other direction, and the one the repository actually got wrong.
    let binaries = workspace_binaries();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            binaries.contains(&binary),
            "a Deployment runs {binary}, which no crate in this workspace \
             builds. A manifest for a binary that does not exist reads to a \
             reviewer as a component that is deployed and constrained."
        );
    }
}

#[test]
fn every_pending_workload_is_still_actually_pending() {
    // What keeps AWAITING_ITS_CRATE from becoming permanent. When the crate
    // lands this fails, and the person who landed it deletes the entry — at
    // which point `every_deployment_runs_a_binary_that_exists` starts covering
    // it for real.
    let binaries = workspace_binaries();
    for (binary, reason) in AWAITING_ITS_CRATE {
        assert!(
            !binaries.contains(&(*binary).to_string()),
            "{binary} now exists in the workspace, so the exemption \"{reason}\" \
             is stale. Delete it from AWAITING_ITS_CRATE."
        );
    }
}

#[test]
fn every_workload_that_cannot_be_probed_says_why_and_stops_being_exempt_when_it_can() {
    // A Deployment with no liveness probe is normally a mistake. Here three of
    // them are deliberate, because the binaries do not serve yet, and a probe
    // written against an endpoint that does not exist is worse than none: it
    // looks like coverage.
    for (path, document) in workload_documents() {
        let workload = first_value(&document, "name").expect("a Deployment is named");
        let binary = containers(&document)
            .first()
            .map(|container| container_binary(container))
            .unwrap_or_else(|| panic!("{} has a Deployment with no container", path.display()));

        let exempt = DOES_NOT_SERVE_YET.iter().any(|(name, _)| *name == binary);
        let probed = document.contains("livenessProbe") && document.contains("readinessProbe");

        if exempt {
            assert!(
                !probed,
                "{binary} carries a probe and is still listed in \
                 DOES_NOT_SERVE_YET. If it serves now, delete the entry."
            );
            assert!(
                document.contains("No liveness or readiness probe"),
                "{} does not say why {workload} has no probe",
                path.display()
            );
        } else {
            assert!(
                probed,
                "{workload} has no liveness and readiness probe, and is not \
                 listed in DOES_NOT_SERVE_YET with a reason"
            );
        }
    }
}

// --- identity ---------------------------------------------------------------

#[test]
fn every_workload_has_its_own_service_account_and_mounts_no_token() {
    // Sharing an account would undo the entire argument for having several,
    // and a mounted token is a credential nothing here needs: every workload
    // authenticates to Google through workload identity and none of them talks
    // to the Kubernetes API.
    let mut seen: Vec<String> = Vec::new();
    for (path, document) in workload_documents() {
        let workload = first_value(&document, "name").expect("a Deployment is named");
        let account = first_value(&document, "serviceAccountName").unwrap_or_else(|| {
            panic!("{workload} runs under the namespace's default service account")
        });
        assert!(
            !seen.contains(&account),
            "{workload} shares the service account {account} with another workload"
        );
        seen.push(account);

        assert!(
            document
                .lines()
                .any(|line| line.trim() == "automountServiceAccountToken: false"),
            "{} mounts a service-account token for {workload}",
            path.display()
        );
    }
    assert!(
        seen.len() >= 4,
        "only {} workloads were checked",
        seen.len()
    );
}

// --- what a container may do ------------------------------------------------

#[test]
fn every_container_has_a_cpu_and_a_memory_request_as_well_as_a_limit() {
    // The existing check reads the first `limits:` block in a file. This one
    // reads every container, and checks requests too: a container with limits
    // and no requests is scheduled as if it were free, which is how a node ends
    // up with more promised to it than it has.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        let found = containers(&document);
        assert!(
            !found.is_empty(),
            "{} has a Deployment whose containers could not be read",
            path.display()
        );
        for container in found {
            let name = container.lines().next().unwrap_or("?").trim();
            let resources = block_under(&container, "resources:");
            assert!(
                !resources.is_empty(),
                "{name} in {} has no resources",
                path.display()
            );
            for section in ["requests:", "limits:"] {
                let block = block_under(&resources, section);
                assert!(!block.is_empty(), "{name} has no {section}");
                assert!(block.contains("cpu:"), "{name} has no cpu in {section}");
                assert!(
                    block.contains("memory:"),
                    "{name} has no memory in {section}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} containers were checked");
}

#[test]
fn no_container_in_any_workload_runs_as_root_or_can_escalate() {
    // The existing check asks whether the settings appear anywhere in a file.
    // This one asks per container, so a second container added to a Deployment
    // that already has the settings once cannot arrive without them.
    for (path, document) in workload_documents() {
        assert!(
            document.contains("runAsNonRoot: true"),
            "{} does not set runAsNonRoot",
            path.display()
        );
        assert!(
            !document.contains("runAsUser: 0"),
            "{} runs a container as uid 0",
            path.display()
        );
        for container in containers(&document) {
            let name = container.lines().next().unwrap_or("?").trim();
            for (setting, why) in [
                (
                    "allowPrivilegeEscalation: false",
                    "a setuid binary inside the container escalates without it",
                ),
                (
                    "readOnlyRootFilesystem: true",
                    "a writable root filesystem lets an attacker persist",
                ),
                (
                    "drop: [\"ALL\"]",
                    "a container needs none of the default capabilities",
                ),
            ] {
                assert!(
                    container.contains(setting),
                    "the {name} container in {} is missing {setting}: {why}",
                    path.display()
                );
            }
        }
    }
}

// --- what a workload may reach ----------------------------------------------

/// The `app` label a NetworkPolicy selects, and which directions it governs.
fn policy_targets() -> Vec<(String, bool, bool)> {
    documents_of_kind("NetworkPolicy")
        .iter()
        .filter_map(|(_, document)| {
            let selector = document
                .split("podSelector:")
                .nth(1)
                .and_then(|rest| rest.split("policyTypes:").next())
                .unwrap_or("");
            let app = selector
                .lines()
                .find_map(|line| line.trim().strip_prefix("app:"))
                .map(|value| value.trim().to_string())?;
            let types = block_under(document, "policyTypes:");
            Some((app, types.contains("- Ingress"), types.contains("- Egress")))
        })
        .collect()
}

#[test]
fn every_workload_is_covered_by_both_an_ingress_and_an_egress_policy() {
    // The default is deny, so a workload with no matching policy cannot be
    // reached and cannot reach anything — and it fails by hanging rather than
    // by erroring, which is the failure that takes longest to diagnose. A
    // missing egress rule for the API is exactly what was wrong here.
    let policies = policy_targets();
    let mut checked = 0usize;
    for (_, document) in workload_documents() {
        let app = document
            .split("labels:")
            .nth(1)
            .and_then(|rest| {
                rest.lines()
                    .find_map(|line| line.trim().strip_prefix("app:"))
            })
            .map(|value| value.trim().to_string())
            .expect("a Deployment carries an app label");

        assert!(
            policies
                .iter()
                .any(|(target, ingress, _)| *target == app && *ingress),
            "{app} is covered by no ingress policy, so nothing can reach it and \
             the connection hangs rather than failing"
        );
        assert!(
            policies
                .iter()
                .any(|(target, _, egress)| *target == app && *egress),
            "{app} is covered by no egress policy, so it can reach nothing — \
             not Secret Manager, not the API, not telemetry"
        );
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} workloads were checked");

    // And the default is still deny, so all of the above means something.
    let namespace = read("infrastructure/kubernetes/base/namespace.yaml");
    assert!(namespace.contains("name: default-deny"));
    let deny = namespace
        .split("name: default-deny")
        .nth(1)
        .expect("the default-deny policy exists")
        .split("\n---")
        .next()
        .expect("the document ends");
    assert!(
        deny.contains("podSelector: {}"),
        "the default deny does not select every pod"
    );
    assert!(
        deny.contains("- Ingress") && deny.contains("- Egress"),
        "the default deny does not cover both directions"
    );
}

#[test]
fn no_network_policy_permits_the_whole_internet() {
    // A `0.0.0.0/0` in an egress rule undoes every other rule in the file, and
    // it is one line that looks like the others.
    for (path, document) in documents_of_kind("NetworkPolicy") {
        let content = without_comments(&document);
        assert!(
            !content.contains("0.0.0.0/0"),
            "{} permits the whole internet",
            path.display()
        );
        assert!(
            !content.contains("::/0"),
            "{} permits the whole internet over IPv6",
            path.display()
        );
    }
}

/// The `to:` destinations a named egress policy permits: `ipBlock` CIDRs and
/// the `app` labels of the pods it names.
///
/// Searched across every manifest rather than in one file. A cell's policies
/// live with the cell because they name that cell's venues, and a check that
/// only looked in namespace.yaml would stop finding them the moment they moved.
fn egress_destinations(policy: &str) -> (Vec<String>, Vec<String>) {
    let document = manifest_documents()
        .into_iter()
        .find(|(_, document)| {
            document
                .lines()
                .any(|line| line.trim() == format!("name: {policy}"))
        })
        .map(|(_, document)| document)
        .unwrap_or_else(|| panic!("{policy} exists"));
    let egress = document
        .split("  egress:")
        .nth(1)
        .unwrap_or_else(|| panic!("{policy} has an egress section"))
        .to_string();
    let egress = without_comments(&egress);

    let cidrs = egress
        .lines()
        .filter_map(|line| line.trim().strip_prefix("cidr:"))
        .map(|value| value.trim().to_string())
        .collect();
    let apps = egress
        .lines()
        .filter_map(|line| line.trim().strip_prefix("app:"))
        .map(|value| value.trim().to_string())
        .collect();
    (cidrs, apps)
}

// --- the evidence store -----------------------------------------------------

#[test]
fn the_evidence_bucket_is_versioned_and_retained() {
    // `EvidenceStore` is write-once by construction: no delete, no overwrite,
    // a second write of different bytes refused by digest. That guarantee is
    // worth exactly as much as the bucket underneath it.
    let evidence = read("infrastructure/terraform/modules/evidence/main.tf");
    assert!(
        evidence.contains("versioning {"),
        "an overwrite that somehow happens would replace the original"
    );
    assert!(
        evidence.contains("retention_policy {"),
        "without a retention policy a delete is refused by an IAM binding \
         somebody can change rather than by the storage service"
    );
    assert!(
        sets(&evidence, "uniform_bucket_level_access", "true"),
        "a per-object ACL could grant what the bucket policy refuses"
    );
    assert!(
        sets(&evidence, "public_access_prevention", "\"enforced\""),
        "the evidence store must not be reachable without authentication"
    );
    assert!(
        sets(&evidence, "force_destroy", "false"),
        "force_destroy empties the bucket before deleting it, which is a delete \
         with extra steps"
    );
    assert!(
        evidence.contains("default_kms_key_name"),
        "the bucket is not encrypted with a key we control"
    );

    // And the restrictive defaults: the retention policy is locked, which is
    // irreversible and is the point.
    let variables = read("infrastructure/terraform/modules/evidence/variables.tf");
    assert!(
        sets(&variables, "default", "true"),
        "the retention policy does not default to locked"
    );
}

#[test]
fn no_workload_identity_can_delete_from_the_evidence_bucket() {
    // An append-only store whose writer holds a delete permission is not
    // append-only; it is a store nobody has deleted from yet. Each of the roles
    // below carries `storage.objects.delete`.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for role in [
            "roles/storage.objectAdmin",
            "roles/storage.objectUser",
            "roles/storage.admin",
            "roles/storage.legacyBucketOwner",
            "roles/owner",
            "roles/editor",
        ] {
            assert!(
                !content.contains(role),
                "{} grants {role}, which can delete an object from the evidence \
                 store",
                path.display()
            );
        }
    }

    // And what is granted is the narrow one.
    let evidence = read("infrastructure/terraform/modules/evidence/main.tf");
    assert!(
        evidence.contains("roles/storage.objectCreator"),
        "the evidence writers hold something other than object creation"
    );
}

// --- the registry -----------------------------------------------------------

#[test]
fn the_image_registry_is_not_world_readable_and_nothing_can_delete_from_it() {
    let registry = read("infrastructure/terraform/modules/registry/main.tf");
    assert!(
        sets(&registry, "immutable_tags", "true"),
        "without immutable tags, the image that was tested and the image that \
         was deployed can be different bytes under the same name"
    );
    assert!(
        registry.contains("roles/artifactregistry.writer"),
        "the pipeline cannot push"
    );
    assert!(
        registry.contains("roles/artifactregistry.reader"),
        "the cluster cannot pull"
    );
    assert!(
        !registry.contains("roles/artifactregistry.repoAdmin"),
        "repoAdmin can delete a version, and a pipeline that can delete a tag \
         can delete the evidence of what it shipped"
    );

    // World-readable, in either of the two ways an IAM binding can be.
    //
    // A `validation` block is skipped, and only a `validation` block. It is
    // the one construct in HCL that can name these principals without
    // granting anything — `console-ingress/variables.tf` names both in a
    // condition that *refuses* an operator list containing either, which is
    // the guarantee this test exists to protect rather than a breach of it.
    // Scanning the whole file for the bare token could not tell the two
    // apart, and read the refusal as the grant.
    //
    // Everything outside a validation block is still checked in full, so a
    // grant laundered through a `locals` value is caught exactly as before.
    let mut scanned = 0usize;
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        let mut validation_depth: usize = 0;
        for line in content.lines() {
            let opened = line.matches('{').count();
            let closed = line.matches('}').count();
            let entering =
                validation_depth == 0 && line.trim_start().starts_with("validation") && opened > 0;
            if entering || validation_depth > 0 {
                validation_depth += opened;
                validation_depth -= closed.min(validation_depth);
                continue;
            }
            scanned += 1;
            for member in ["allUsers", "allAuthenticatedUsers"] {
                assert!(
                    !line.contains(member),
                    "{} grants a role to {member}, which tells an attacker exactly \
                     what is running and lets them read it",
                    path.display()
                );
            }
        }
    }

    // The premise: lines were actually read. A path that silently found no
    // files would satisfy every assertion above by never running one.
    assert!(
        scanned > 1000,
        "only {scanned} lines of Terraform were scanned, so this test proved \
         nothing about the ones it did not read"
    );
}

// --- the deployment pipeline ------------------------------------------------

#[test]
fn nothing_deploys_that_has_not_passed_the_test_suite() {
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("workflows: [\"ci\"]"),
        "the deploy workflow is not triggered by the test suite"
    );
    assert!(
        deploy.contains("github.event.workflow_run.conclusion"),
        "the automatic path does not check that ci succeeded"
    );
    assert!(
        deploy.contains("actions/workflows/ci.yml/runs?head_sha="),
        "the manual path does not check that ci succeeded for the commit; \
         somebody pressing a button is not evidence"
    );
    // Both build and deploy hang off the gate.
    assert!(
        deploy.matches("needs: gate").count() >= 1 && deploy.contains("needs: images"),
        "a job can run without the gate"
    );
}

#[test]
fn no_workflow_depends_on_a_repository_variable() {
    // Repository variables were the one input to this pipeline that nothing
    // reviewed. They are written by the bootstrap's last step, which a failed
    // bootstrap never reaches — and once, a bootstrap that DID reach it ran
    // against Cloud Shell's `terraform` stub and captured several lines of apt
    // install instructions into the workload-identity variable. Non-empty, so
    // every check that only asked whether they were set waved it through, and
    // both workflows then failed on an audience nobody could explain.
    //
    // Both derive their identity from the environment's committed tfvars now,
    // where every value is reviewed like any other configuration and a broken
    // bootstrap cannot reach it. A `vars.` creeping back into either workflow
    // reintroduces the entire failure mode.
    for workflow_file in [
        ".github/workflows/infra.yml",
        ".github/workflows/deploy.yml",
    ] {
        let workflow = read(workflow_file);
        assert!(
            !workflow.contains("${{ vars."),
            "{workflow_file} reads a repository variable; derive the value from \
             the environment's tfvars instead"
        );
        // And the derivation is real: both fields it constructs from must be
        // read, or the assertion above is satisfied by a workflow that
        // authenticates with nothing at all.
        for field in ["project_id", "project_number"] {
            assert!(
                workflow.contains(field),
                "{workflow_file} no longer derives {field} from the tfvars"
            );
        }
    }
}

/// The value assigned to a top-level tfvars key, quotes stripped.
///
/// `first_value` reads `key: value`, which is YAML; a tfvars file is HCL and
/// writes `key = value`, so reading one with the other silently finds nothing
/// and every assertion downstream is skipped — which is exactly how this test
/// first "passed" nothing. Anchored at the start of the line because these
/// keys are top-level, and a nested `project_id` inside an `edge_cells` block
/// would be a different fact.
fn tfvars_value(tfvars: &str, key: &str) -> Option<String> {
    tfvars.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        let rest = rest.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string())
    })
}

#[test]
fn every_environment_names_a_project_of_its_own() {
    // Two environments sharing one project share one IAM boundary, one KMS
    // key ring and one Binary Authorization attestor, whatever their resource
    // name prefixes say. A compromise of the test pipeline's service account
    // would then sit inside production's project, and the blast radius that
    // was supposed to stop at a project boundary would not stop anywhere.
    //
    // The files said all four named one project, on the reasoning that the
    // `environment` prefix kept the names apart. That premise expired twice
    // over without the files noticing: `dev` moved to `algorik-dev`, and the
    // project the other three still named was deleted, so their recorded id
    // pointed at nothing while reading as entirely plausible. Both halves are
    // pinned here — an environment either names a project no other
    // environment names, or says out loud that it has none.
    const UNPROVISIONED: &str = "unprovisioned";

    let mut provisioned: Vec<(String, String)> = Vec::new();
    let mut checked = 0usize;
    const ENVIRONMENTS: [&str; 4] = ["dev", "test", "stage", "prod"];

    for environment in ENVIRONMENTS {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        let project = tfvars_value(&tfvars, "project_id")
            .unwrap_or_else(|| panic!("{environment} names no project_id at all"));
        checked += 1;
        if project == UNPROVISIONED {
            // An unprovisioned environment must also carry a number nothing
            // can authenticate with: a real number beside the marker is the
            // half-updated state this check exists to catch.
            let number = tfvars_value(&tfvars, "project_number")
                .unwrap_or_else(|| panic!("{environment} names no project_number"));
            assert_eq!(
                number, "0",
                "{environment} is marked unprovisioned but records project_number {number}; \
                 one of the two is stale"
            );
            continue;
        }
        if let Some((other, _)) = provisioned.iter().find(|(_, id)| id == &project) {
            panic!(
                "{environment} and {other} both deploy into project {project}. Separate \
                 environments must not share one IAM boundary, key ring and attestor; give \
                 {environment} its own project or mark it `{UNPROVISIONED}`."
            );
        }
        provisioned.push((environment.to_string(), project));
    }

    assert_eq!(
        checked,
        ENVIRONMENTS.len(),
        "not every environment was read; this check stopped checking"
    );
    // The premise: at least one environment is real, or the whole check is
    // satisfied by four markers and proves nothing about sharing.
    assert!(
        provisioned.iter().any(|(name, _)| name == "dev"),
        "dev names no provisioned project, so this check has nothing to compare"
    );
}

#[test]
fn the_teardown_writes_the_flag_to_state_before_it_reads_it() {
    // `down` is two commands, and the order is the whole point. GKE reads
    // `deletion_protection` from prior state during a destroy, never from
    // configuration, so an environment whose tfvars say false but whose state
    // still says true refuses its own teardown — which is exactly how the
    // first real teardown attempt failed, twice, once before the flag was a
    // variable at all and once after. The targeted apply is what writes false
    // into state; delete it and `down` breaks again, silently, and only for
    // environments that have not been applied since.
    let infra = read(".github/workflows/infra.yml");
    let down = block_under(&infra, "- name: down");

    let apply = down.find("terraform -chdir=infrastructure/terraform apply");
    let destroy = down.find("terraform -chdir=infrastructure/terraform destroy");

    let apply = apply.expect(
        "infra.yml's down no longer applies before destroying, so deletion_protection \
         never reaches state and the teardown refuses itself",
    );
    let destroy = destroy.expect("infra.yml's down no longer destroys anything");
    assert!(
        apply < destroy,
        "infra.yml's down destroys before it applies, so the destroy still reads \
         the old deletion_protection out of state"
    );
}

#[test]
fn the_infrastructure_workflow_cannot_touch_production() {
    // infra.yml holds the identity that can reshape an environment, which is
    // exactly why it must never offer prod. Two layers: prod absent from the
    // dispatch choices, and a step that refuses it even if the choices are
    // edited. Both checked, because either alone is one edit from gone.
    let infra = read(".github/workflows/infra.yml");
    let choices = block_under(&infra, "environment:");
    assert!(
        !choices.contains("- prod"),
        "infra.yml offers prod as a dispatch choice"
    );
    assert!(
        infra.contains("inputs.environment }}\" = \"prod\" ]"),
        "infra.yml no longer refuses prod in a step, so a fork that adds the \
         choice gets it"
    );
    // And the destructive action is targeted, never a full destroy: KMS keys
    // and the workload identity pool are soft-deleted by name, so a full
    // destroy/apply cycle collides with its own remains — including this
    // workflow's own authentication.
    assert!(
        infra.contains("-target=module.cluster"),
        "infra.yml's down is no longer targeted at the cluster"
    );
    assert!(
        !without_comments(&infra)
            .contains("destroy -input=false -auto-approve \\\n            -var-file"),
        "infra.yml runs an untargeted destroy"
    );
}

#[test]
fn production_is_never_deployed_automatically() {
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("TARGET_ENVIRONMENT: ${{ inputs.environment || 'dev' }}"),
        "the automatic path does not default to dev"
    );
    assert!(
        deploy.contains("refuse a production deployment that nobody dispatched"),
        "nothing refuses an automatic production deployment"
    );
    assert!(
        deploy.contains(
            "env.TARGET_ENVIRONMENT == 'prod' && github.event_name != 'workflow_dispatch'"
        ),
        "the refusal does not test what it claims to"
    );
    assert!(
        deploy.contains("environment: ${{ inputs.environment || 'dev' }}"),
        "the deploy job names no GitHub environment, so required reviewers have \
         nothing to attach to"
    );
}

#[test]
fn the_pipeline_authenticates_without_a_long_lived_key() {
    // A service-account key in a repository secret is a credential that never
    // expires, is copied by anyone who can read the secret, and leaves no trace
    // of which run used it.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("id-token: write"),
        "the pipeline cannot mint an OIDC token"
    );
    assert!(
        deploy.contains("workload_identity_provider:"),
        "the pipeline does not use workload identity federation"
    );
    assert!(
        !deploy.contains("credentials_json"),
        "the pipeline authenticates with a key"
    );
    assert!(
        !deploy.contains("service_account_key"),
        "the pipeline authenticates with a key"
    );

    // And the pool refuses every repository but this one. Without the
    // condition, any GitHub repository in the world can present a valid token.
    let cicd = read("infrastructure/terraform/modules/cicd/main.tf");
    assert!(
        cicd.contains("attribute_condition"),
        "the workload identity pool accepts a token from any repository"
    );
    assert!(
        cicd.contains("attribute.repository == '${var.github_repository}'"),
        "the pool's condition does not pin the repository"
    );
    let variables = read("infrastructure/terraform/modules/cicd/variables.tf");
    assert!(
        !variables.contains("default     = \""),
        "the repository has a default, and a default here is a repository \
         somebody else could be running"
    );

    // What is missing is written down rather than invented.
    let gaps = read("docs/operations/external-dependencies.md");
    assert!(
        gaps.contains("GCP_WORKLOAD_IDENTITY_PROVIDER"),
        "the variables the pipeline needs are not documented anywhere"
    );
}

/// The pool provider's attribute condition, as the CEL expression it is.
fn wif_attribute_condition() -> String {
    let cicd = without_comments(&read("infrastructure/terraform/modules/cicd/main.tf"));
    cicd.lines()
        .find_map(|line| line.trim().strip_prefix("attribute_condition ="))
        .map(|value| value.trim().trim_matches('"').to_string())
        .expect("the pool provider declares an attribute_condition")
}

/// Whether that condition would admit a token minted for a git ref.
///
/// Only the ref terms are evaluated; the repository term is asserted
/// separately, and a term naming neither is not this function's business. An
/// unrecognised ref term panics rather than being read as permissive, because
/// the failure this whole check exists to prevent is a condition that looks
/// like it binds something and does not.
fn wif_condition_admits(condition: &str, git_ref: &str) -> bool {
    let mut admitted = true;
    let mut ref_terms = 0usize;
    for term in condition.split("&&").map(str::trim) {
        if !term.contains("attribute.ref") {
            continue;
        }
        ref_terms += 1;
        if let Some(rest) = term.strip_prefix("attribute.ref.startsWith(") {
            admitted &= git_ref.starts_with(rest.trim_end_matches(')').trim_matches('\''));
        } else if let Some(rest) = term.strip_prefix("attribute.ref == ") {
            admitted &= git_ref == rest.trim_matches('\'');
        } else if let Some(rest) = term.strip_prefix("attribute.ref in [") {
            admitted &= rest
                .trim_end_matches(']')
                .split(',')
                .map(|entry| entry.trim().trim_matches('\''))
                .any(|entry| entry == git_ref);
        } else {
            panic!(
                "`{term}` is a ref term this check cannot evaluate. Teach it the \
                 new form rather than deleting the check."
            );
        }
    }
    assert!(
        ref_terms > 0,
        "the pool's attribute condition has no ref term at all: `{condition}`"
    );
    admitted
}

/// The branches deploy.yml's automatic path fires on, read from the workflow.
fn deploy_workflow_run_branches() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    let branches: Vec<String> = block_under(&deploy, "branches:")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(|value| value.trim().trim_matches('"').to_string())
        .collect();
    assert!(
        !branches.is_empty(),
        "deploy.yml's workflow_run names no branches; this test's premise needs \
         rewriting"
    );
    branches
}

#[test]
fn the_pool_admits_only_branches_of_this_repository_and_says_so_truthfully() {
    // The condition used to be `attribute.repository == '…'` and nothing else,
    // under a comment claiming "only on a branch the deployment is allowed
    // from". There was no ref term and `attribute_mapping` bound the ref to
    // nothing, so any ref of this repository — a pull request's merge ref, a
    // tag anyone who can push one controls — could exchange a token for an
    // account holding compute.admin, container.admin, projectIamAdmin,
    // cloudkms.admin and secretmanager.admin. A comment asserting a control
    // that does not exist is worse than no comment: it is the reason the next
    // reader does not check.
    let condition = wif_attribute_condition();
    assert!(
        condition.contains("attribute.repository == '${var.github_repository}'"),
        "the pool's condition no longer pins the repository: `{condition}`"
    );

    // The condition may only name attributes the provider maps. One that names
    // an unmapped attribute is rejected at apply time, so this is the half that
    // keeps the fix from being a broken apply.
    let cicd = read("infrastructure/terraform/modules/cicd/main.tf");
    assert!(
        sets(&cicd, "\"attribute.ref\"", "\"assertion.ref\""),
        "the condition constrains attribute.ref and attribute_mapping does not \
         map it; the provider refuses that at apply time"
    );

    // Every branch the automatic path fires on is still admitted — read from
    // deploy.yml so a rename there fails here rather than in GCP.
    for branch in deploy_workflow_run_branches() {
        let git_ref = format!("refs/heads/{branch}");
        assert!(
            wif_condition_admits(&condition, &git_ref),
            "the pool refuses `{git_ref}`, which deploy.yml's workflow_run \
             fires on; the pipeline cannot authenticate to GCP at all"
        );
    }

    // And so is a branch nothing has heard of. infra.yml is workflow_dispatch
    // only, dispatched against the branch carrying the change being applied, so
    // an allowlist of the two branches above would refuse every real infra run
    // with an audience error naming neither the branch nor the list.
    assert!(
        wif_condition_admits(&condition, "refs/heads/claude/some-working-branch"),
        "the pool admits only named branches, so infra.yml — dispatched against \
         whatever branch the change lives on — can never authenticate"
    );

    // What the ref term is actually for: the ref classes nothing here
    // authenticates from. A pull request runs in *this* repository's context,
    // so the repository claim does not distinguish it; the ref does.
    for refused in ["refs/pull/42/merge", "refs/tags/v1.4.0"] {
        assert!(
            !wif_condition_admits(&condition, refused),
            "the pool admits `{refused}`. Nothing in this repository \
             authenticates from that ref class, and the account it can \
             impersonate administers IAM, KMS and Secret Manager."
        );
    }
}

// --- workflow step outputs --------------------------------------------------
//
// `the_pipeline_authenticates_without_a_long_lived_key` asks whether the string
// `workload_identity_provider:` appears in deploy.yml. It does — and for a
// while it appeared in a job where the value interpolated into it was empty.
// deploy.yml's `gitops-update` read `steps.identity.outputs.provider` and
// `.account` from a `derive the identity from the tfvars` step that wrote only
// `project` and `region`, so the job's one GCP login was handed two empty
// strings and the digest resolution behind it could never run. A substring
// check cannot tell a populated value from an absent one; nothing else looked.
//
// GitHub does not error on this. An unwritten step output interpolates to the
// empty string, so the failure surfaces as whatever the action does with a
// blank argument — here, an authentication error naming neither the step that
// should have written the value nor the job that read it.

/// A line's leading-space count.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The jobs of a workflow, as `(name, body)`.
///
/// Hand-rolled rather than parsed: the workspace has two dependencies and
/// neither reads YAML (ADR 0002, ADR 0009). It relies on the shape these four
/// files actually have — `jobs:` at column zero, job names at indent two — and
/// on the premise assertions below failing loudly if that shape changes,
/// rather than on quietly finding nothing.
fn workflow_jobs(workflow: &str) -> Vec<(String, String)> {
    let mut jobs: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    let mut seen_jobs_key = false;

    for line in workflow.lines() {
        if !seen_jobs_key {
            seen_jobs_key = line.trim_end() == "jobs:";
            continue;
        }
        let trimmed = line.trim_start();
        // A column-zero key ends the jobs map. Comments there are not keys.
        if !trimmed.is_empty() && indent_of(line) == 0 && !trimmed.starts_with('#') {
            break;
        }
        let starts_a_job = indent_of(line) == 2
            && !trimmed.starts_with('#')
            && !trimmed.starts_with('-')
            && trimmed.ends_with(':');
        if starts_a_job {
            if let Some((name, body)) = current.take() {
                jobs.push((name, body.join("\n")));
            }
            current = Some((trimmed.trim_end_matches(':').to_string(), Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((name, body)) = current {
        jobs.push((name, body.join("\n")));
    }
    jobs
}

/// The steps of one job body, each as its own text.
fn job_steps(job: &str) -> Vec<String> {
    let mut steps: Vec<Vec<&str>> = Vec::new();
    let mut item_indent: Option<usize> = None;
    let mut in_steps = false;

    for line in job.lines() {
        if !in_steps {
            in_steps = line.trim_start() == "steps:";
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if let Some(last) = steps.last_mut() {
                last.push(line);
            }
            continue;
        }
        let ind = indent_of(line);
        match item_indent {
            None => {
                if trimmed.starts_with("- ") {
                    item_indent = Some(ind);
                    steps.push(vec![line]);
                }
            }
            Some(base) => {
                if ind < base {
                    break; // a key after the steps list; the list is over
                }
                if ind == base && trimmed.starts_with("- ") {
                    steps.push(vec![line]);
                } else if let Some(last) = steps.last_mut() {
                    last.push(line);
                }
            }
        }
    }
    steps.into_iter().map(|step| step.join("\n")).collect()
}

/// A step's `id`, if it declares one.
fn step_id(step: &str) -> Option<String> {
    step.lines().find_map(|line| {
        // `id-token: write` must not match, so the colon is part of the prefix.
        let value = line.trim_start().strip_prefix("id:")?;
        Some(value.trim().to_string())
    })
}

/// The output names a step writes with `echo "name=value" >> "$GITHUB_OUTPUT"`.
///
/// Anything written another way reads here as written by nobody, and the test
/// fails rather than passes — which is the safe direction: a new way of writing
/// an output makes this stop and be extended, instead of silently trusting it.
fn step_outputs(step: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in step.lines() {
        let Some(rest) = line.trim_start().strip_prefix("echo \"") else {
            continue;
        };
        let Some((name, _)) = rest.split_once('=') else {
            continue;
        };
        // `echo "deb [signed-by=..."` is not an output assignment.
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            names.insert(name.to_string());
        }
    }
    names
}

/// Every `steps.<id>.outputs.<name>` a piece of workflow text reads.
fn step_output_references(text: &str) -> std::collections::BTreeSet<(String, String)> {
    fn token(text: &str) -> String {
        text.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect()
    }

    let mut references = std::collections::BTreeSet::new();
    let mut rest = text;
    while let Some(position) = rest.find("steps.") {
        rest = &rest[position + "steps.".len()..];
        let id = token(rest);
        // Every character of `id` is ASCII, so this is a char boundary.
        let Some(tail) = rest[id.len()..].strip_prefix(".outputs.") else {
            continue;
        };
        let name = token(tail);
        if !id.is_empty() && !name.is_empty() {
            references.insert((id, name));
        }
    }
    references
}

#[test]
fn every_step_output_a_workflow_reads_is_one_that_job_writes() {
    const WORKFLOWS: [&str; 4] = [
        ".github/workflows/ci.yml",
        ".github/workflows/deploy.yml",
        ".github/workflows/infra.yml",
        ".github/workflows/vendor.yml",
    ];

    /// The outputs each `id`-bearing step of one job writes.
    type Written = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

    let mut references_checked = 0usize;
    let mut jobs_read = 0usize;

    for workflow_file in WORKFLOWS {
        let workflow = read(workflow_file);
        let jobs = workflow_jobs(&workflow);
        assert!(
            !jobs.is_empty(),
            "{workflow_file} parsed to no jobs at all; this check stopped checking"
        );
        jobs_read += jobs.len();

        for (job_name, body) in jobs {
            let mut written = Written::new();
            for step in job_steps(&body) {
                if let Some(id) = step_id(&step) {
                    written.entry(id).or_default().extend(step_outputs(&step));
                }
            }

            for (id, name) in step_output_references(&body) {
                references_checked += 1;
                let Some(outputs) = written.get(&id) else {
                    panic!(
                        "{workflow_file}: job `{job_name}` reads \
                         steps.{id}.outputs.{name}, but no step in that job \
                         declares `id: {id}`. An unwritten step output \
                         interpolates to the empty string rather than failing."
                    );
                };
                assert!(
                    outputs.contains(&name),
                    "{workflow_file}: job `{job_name}` reads \
                     steps.{id}.outputs.{name}, but that step writes only \
                     {outputs:?}. The value interpolates to the empty string \
                     and whatever consumes it fails without naming either."
                );
            }
        }
    }

    // The premise. Both halves have been empty in this repository's history:
    // a workflow whose jobs did not parse, and a job with no references, each
    // satisfying every assertion above while proving nothing.
    assert!(
        jobs_read >= 4,
        "only {jobs_read} jobs were read across four workflows"
    );
    assert!(
        references_checked >= 6,
        "only {references_checked} step-output references were checked; the \
         deploy pipeline alone reads more than that"
    );
}

// --- what deploys, and what deliberately does not ---------------------------
//
// Three lists have to agree: the binaries the workspace builds, the images the
// pipeline pushes, and the workloads the manifests declare. Every pair of them
// can disagree in both directions, and each of the six failures is quiet:
//
//   * a binary with no image — the crate ships nowhere, and the deploy that was
//     supposed to include it succeeds;
//   * an image with no manifest — a build that costs money and ships nothing,
//     and reads to a reviewer as a deployed component;
//   * a manifest with no image — a rollout waiting for a tag that will never
//     exist;
//   * a workload the rollout check never waits on — a pipeline that reports
//     success for a container that never started.
//
// None of these produces an error anywhere. They produce a deployment that is
// missing something, and a review in which everything present is correct.

/// The binaries the deploy workflow builds an image for.
///
/// Parsed out of the matrix rather than listed here, for the same reason
/// `workspace_binaries` is parsed out of the manifests: a list here would be a
/// third copy of the truth, and this whole section exists because copies drift.
fn image_matrix() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    let block = deploy
        .split("\n        binary:\n")
        .nth(1)
        .expect("the image job declares a matrix of binaries")
        .split("\n    steps:")
        .next()
        .expect("the matrix ends where the job's steps begin");
    let mut binaries: Vec<String> = block
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(str::to_string)
        .collect();
    binaries.sort();
    binaries.dedup();
    assert!(
        binaries.len() >= 4,
        "only {binaries:?} were parsed out of the image matrix; the workflow's \
         indentation has changed and this check has stopped checking"
    );
    binaries
}

/// The templates the chart carries but dev does not deploy.
///
/// The edge cell is the only one, and it is gated on a value rather than
/// skipped by a pipeline: bringing up a cell needs a cell id, a region and a
/// set of venue ranges, and it is a deliberate act with a runbook rather than
/// something an unattended sync does to a workload that trades.
fn templates_dev_does_not_deploy() -> Vec<String> {
    vec!["edge-cell.yaml".to_string()]
}

/// The workloads named in the record of who verifies a promotion.
///
/// Was read out of `deploy.yml`'s `kubectl rollout status` loop. That loop is
/// gone: the pipeline no longer touches the cluster, so there is no longer a
/// pipeline-side answer to read. The property it protected — that a
/// deployment producing a broken pod is noticed — outlived the mechanism, so
/// it is asserted against the place the gap is now recorded.
fn workloads_named_in_the_verification_record() -> Vec<String> {
    let record = read("docs/operations/gitops-exceptions.md");
    let section = record
        .split("## 4.")
        .nth(1)
        .expect("gitops-exceptions.md must record what verifies a promotion");
    ["qip-api", "qip-fastbrain", "qip-deepbrain"]
        .into_iter()
        .filter(|workload| section.contains(*workload))
        .map(str::to_string)
        .collect()
}

/// Every workload that actually gets deployed, by workload name.
///
/// Read out of the Helm chart, because the chart is what Argo CD applies.
/// It used to be read out of `deploy.yml`'s render step, which was correct
/// while the pipeline applied with kubectl and is not any more: the pipeline
/// now stops at the registry and commits digests, so a check that reads it
/// would be asking the wrong file what is deployed.
fn workloads_deployed_to_dev() -> Vec<String> {
    let excluded = templates_dev_does_not_deploy();
    let mut applied: Vec<String> = Vec::new();
    for path in files_with_extension("infrastructure/helm/qip/templates", "yaml") {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a template has a name")
            .to_string();
        if excluded.contains(&name) {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable");
        // A chart template is not parseable YAML — it carries Go template
        // actions — so the Deployment's name is matched textually. The names
        // in this chart are literals rather than expressions, which is what
        // makes that sound; a templated name would not match and would fail
        // the count assertion below rather than passing silently.
        let mut in_deployment = false;
        for line in content.lines() {
            if line.trim() == "kind: Deployment" {
                in_deployment = true;
            } else if in_deployment {
                if let Some(rest) = line.trim().strip_prefix("name: ") {
                    let candidate = rest.trim().trim_matches('"');
                    if candidate.starts_with("qip-") && !candidate.contains("{{") {
                        applied.push(candidate.to_string());
                        in_deployment = false;
                    }
                }
            }
        }
    }
    applied.sort();
    applied.dedup();
    assert!(
        applied.len() >= 3,
        "only {applied:?} were found in the chart; the template walk is not \
         reaching them and every check built on it is asserting nothing"
    );
    applied
}

#[test]
fn every_binary_the_workspace_builds_is_in_the_image_matrix_or_excluded_by_name() {
    // The direction nobody notices: a new deployable lands, the manifests are
    // written, and the matrix is not touched. The apply then references an
    // image tag that was never pushed.
    let matrix = image_matrix();
    for binary in workspace_binaries() {
        if NOT_IN_THE_IMAGE_MATRIX
            .iter()
            .any(|(_, name, _)| *name == binary)
        {
            continue;
        }
        assert!(
            matrix.contains(&binary),
            "{binary} is a binary this workspace builds and deploy.yml builds \
             no image for it. Either add it to the matrix, or add it to \
             NOT_IN_THE_IMAGE_MATRIX with the reason it is not deployed and \
             record that reason in docs/adr/0010-what-gets-deployed.md."
        );
    }
}

#[test]
fn nothing_is_excluded_from_the_image_matrix_and_built_by_it() {
    // What stops the exclusion list becoming a place things are left. An entry
    // that is also in the matrix is an exclusion somebody undid without
    // deleting, and an entry for a binary the workspace no longer builds is one
    // whose crate has gone.
    let matrix = image_matrix();
    let binaries = workspace_binaries();
    for (crate_name, binary, reason) in NOT_IN_THE_IMAGE_MATRIX {
        assert!(
            !matrix.contains(&(*binary).to_string()),
            "{binary} is excluded from the image matrix for the reason \
             \"{reason}\" and the matrix builds it anyway. Delete the entry."
        );
        assert!(
            binaries.contains(&(*binary).to_string()),
            "NOT_IN_THE_IMAGE_MATRIX excludes {binary}, which no crate in this \
             workspace builds. An exclusion for something that does not exist \
             hides the next thing that takes its name."
        );
        assert!(
            repository_root()
                .join(format!("backend/crates/apps/{crate_name}/Cargo.toml"))
                .exists(),
            "NOT_IN_THE_IMAGE_MATRIX names the crate {crate_name}, which is not \
             under crates/apps"
        );
    }
}

#[test]
fn qip_web_is_a_library_and_stops_being_exempt_the_moment_it_is_not() {
    // The one crate under crates/apps with no image, no manifest and no
    // service account — and the one whose absence most looks like an
    // oversight. It is a library qip-api links and renders from, so its pages
    // are already served by the qip-api image on qip-api's port, and there is
    // no second process to schedule.
    //
    // This is checked rather than left in a comment because the day it grows a
    // `main.rs` is the day four files need changing together: the matrix, a
    // manifest, the service-account map in Terraform, and the rollout check.
    // Without this the crate would simply build a binary nothing deploys.
    let manifest = read("backend/crates/apps/qip-web/Cargo.toml");
    assert!(
        !manifest.contains("[[bin]]"),
        "qip-web declares a binary, and the deployment configuration has no \
         entry for it anywhere: no image in deploy.yml, no manifest, no \
         service account in Terraform. Either give it all of them or take the \
         binary back out. See docs/adr/0010-what-gets-deployed.md."
    );
    assert!(
        !workspace_binaries().contains(&"qip-web".to_string()),
        "qip-web now produces a binary. See docs/adr/0010-what-gets-deployed.md \
         for the four places that have to change with it."
    );

    // And it is still linked by the thing that serves it. A library nothing
    // links is not a decision not to deploy it; it is dead code.
    let api = read("backend/crates/apps/qip-api/Cargo.toml");
    assert!(
        api.contains("qip-web.workspace = true"),
        "qip-api no longer links qip-web, so nothing renders its pages and \
         nothing serves them. Either something else does — and then that is \
         what deploys them — or the crate is unused."
    );
}

#[test]
fn every_image_the_matrix_builds_has_a_manifest_that_runs_it() {
    // An image nobody deploys is a build that costs money and ships nothing.
    // Worse, it reads to a reviewer as a component that is deployed: the
    // pipeline visibly builds and pushes it, and nothing says the cluster never
    // asks for it.
    let deployed = deployed_binaries();
    for binary in image_matrix() {
        assert!(
            deployed.contains(&binary),
            "the pipeline builds and pushes {binary} and no manifest runs it. \
             Either write the manifest or take it out of the matrix."
        );
    }
}

#[test]
fn the_pipeline_builds_an_image_for_every_workload_it_deploys() {
    // The reverse, and the one that fails a deployment rather than wasting a
    // build: a Deployment whose image nothing pushes is a rollout waiting for a
    // tag that will never exist.
    let matrix = image_matrix();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            matrix.contains(&binary),
            "{binary} has a Deployment and the pipeline builds no image for it"
        );
    }
}

#[test]
fn every_manifest_pulls_from_the_repository_the_pipeline_pushes_to() {
    // Three places name the same repository and none of them can see the other
    // two: the workflow builds `<region>-docker.pkg.dev/<project>/qip-<env>`,
    // Terraform creates `qip-<env>`, and every manifest writes `IMAGE_PREFIX`
    // for the workflow to substitute. If any drifts, every pull in every
    // environment fails with a 404 that names neither of the two that disagree.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains(
            "docker.pkg.dev/${{ steps.identity.outputs.project }}/qip-${TARGET_ENVIRONMENT}"
        ),
        "the pipeline no longer pushes to qip-<environment> in the project's \
         Artifact Registry"
    );
    let registry = read("infrastructure/terraform/modules/registry/main.tf");
    assert!(
        sets(&registry, "repository_id", r#""qip-${var.environment}""#),
        "the registry Terraform creates is not the one the pipeline pushes to"
    );

    // And no manifest pins a registry of its own. A hard-coded prefix survives
    // the pipeline's placeholder check — there is no placeholder left in it —
    // and pulls from wherever it says, in every environment.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        for container in containers(&document) {
            let image = container
                .lines()
                .find_map(|line| line.trim().strip_prefix("image:"))
                .map(str::trim)
                .unwrap_or_else(|| panic!("a container declares an image"));
            assert!(
                image.starts_with("IMAGE_PREFIX/"),
                "{} pulls {image}, which names a registry rather than deferring \
                 to the one the pipeline substitutes",
                path.display()
            );
            assert!(
                image.ends_with(":IMAGE_TAG"),
                "{} pulls {image}, which pins a tag rather than the commit \
                 being deployed",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} images were checked");
}

#[test]
fn a_promotion_names_who_verifies_it() {
    // `kubectl apply` returns when the API server has accepted the objects, not
    // when the containers are running. Without this step a pipeline reports
    // success for an image that crashes on start-up, which is the failure the
    // whole deployment exists to catch.
    //
    // Both directions matter. A workload missing from the list is never
    // checked; a workload on the list that the pipeline did not apply makes
    // `rollout status` wait on a Deployment nobody created until it times out,
    // which fails the deployment for a reason that is not the real one.
    let waited = workloads_named_in_the_verification_record();
    let applied = workloads_deployed_to_dev();

    for workload in &applied {
        assert!(
            waited.contains(workload),
            "{workload} is deployed by the chart and is not named in the \
             verification record. The pipeline stopped waiting on a rollout \
             when the kubectl path was retired, so a {workload} that never \
             starts is now a deployment that reports success — and the one \
             thing that must not happen is that being true and unwritten."
        );
    }
    for workload in &waited {
        assert!(
            applied.contains(workload),
            "the verification record names {workload}, which the chart does \
             not deploy. A record of who watches a workload nobody deploys \
             reads as coverage and is not."
        );
    }
}

#[test]
fn nothing_reads_as_deployed_that_nothing_deploys() {
    // The failure this prevents: a directory of manifests a reviewer takes for
    // the running system, that nothing applies. It began as a guard on an
    // empty `infrastructure/kubernetes/overlays` — harmless while empty, a lie
    // the moment somebody put a manifest in it.
    //
    // On 2026-08-31 the same failure arrived at a hundred times the size.
    // deploy.yml stopped applying `infrastructure/kubernetes/base/*.yaml`
    // because Argo CD applies the Helm chart instead, which left that whole
    // directory in exactly the state this test exists to forbid. It is kept —
    // 22 checks read it as the description of the platform's Kubernetes shape
    // — so what it now needs is to say plainly that it is not what runs.
    //
    // The marker is required rather than assumed. A comment somebody may or
    // may not have written is not a guarantee; a test that fails without it
    // is.
    // Comments are stripped before the match, and that is not fussiness. The
    // first version of this check read the whole file and failed on the
    // comment in deploy.yml that explains why the apply was removed — a test
    // that forbids describing the thing it forbids. `#` opens a comment in
    // both YAML and the shell inside a `run:` block, so one rule covers both.
    let deploy = read(".github/workflows/deploy.yml");
    let commands: String = deploy
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !commands.contains("kubectl apply"),
        "deploy.yml applies to the cluster again. Argo CD applies the chart; \
         two unattended writers to one namespace undo each other on every \
         disagreement, and the divergence that made this a real incident was \
         invisible until the cluster behaved oddly."
    );

    let readme = read("infrastructure/kubernetes/base/README.md");
    assert!(
        readme.contains("infrastructure/helm/qip"),
        "infrastructure/kubernetes/base/README.md must name the chart that \
         replaced it, or a reader has no way to find what actually deploys"
    );

    let skipped = templates_dev_does_not_deploy();
    let runbooks: String = files_with_extension("docs/operations", "md")
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();

    let mut checked = 0usize;
    for path in files_with_extension("infrastructure/helm/qip/templates", "yaml") {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a manifest has a name")
            .to_string();
        assert!(
            path.parent()
                .is_some_and(|parent| parent.ends_with("infrastructure/helm/qip/templates")),
            "{} is a template outside the directory the Argo CD Application \
             points at. Nothing applies it.",
            path.display()
        );
        if skipped.contains(&name) {
            assert!(
                runbooks.contains(&name),
                "{name} is skipped by the deploy pipeline and named by no \
                 runbook in docs/operations, so nothing applies it and nobody \
                 is told to"
            );
        }
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} manifests were checked");
}

/// The environment variables a binary refuses to start without.
///
/// Read out of the source rather than listed here. `required("QIP_X", &mut
/// missing)` is the call that puts a variable on the list the binary exits on,
/// so matching it is matching the actual requirement rather than a second
/// statement of it.
fn variables_the_binary_refuses_to_start_without(source: &str) -> Vec<String> {
    // `required_secret(` is the credential variant of the same refusal, and a
    // split on `required(` alone does not match it — which is exactly how the
    // envelope key fell out of this walk when it moved to `qip_core::secret`
    // and took a `_FILE` alternative. Both sites put the variable on the list
    // the binary exits on, so both belong here.
    ["required(\"", "required_secret(\""]
        .iter()
        .flat_map(|call| {
            source
                .split(call)
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn every_variable_a_deployable_refuses_to_start_without_is_set_by_its_manifest() {
    // The bug this was written for: `edge-cell.yaml` set QIP_CELL_ID,
    // QIP_CELL_REGION and QIP_CAPITAL_ENVELOPE_KEY and not QIP_VENUES, which
    // `qip-edge-node` also requires. The container would have exited with a
    // configuration error and been restarted for ever.
    //
    // Nothing else would have caught it. The manifest is not applied by the
    // pipeline, so no rollout check runs against it; the runbook that does
    // apply it substituted the placeholders that were there.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        for container in containers(&document) {
            let binary = container_binary(&container);
            let source = format!("backend/crates/apps/{binary}/src/main.rs");
            if !repository_root().join(&source).exists() {
                continue;
            }
            for variable in variables_the_binary_refuses_to_start_without(&read(&source)) {
                // `- name: {variable}_FILE` also satisfies the requirement:
                // `qip_core::secret` accepts the file variant, and it is the
                // one the manifests use for anything projected by the CSI
                // driver. The plain match below matches it too, as a prefix —
                // stated here so a reader does not think the file variant
                // slips through unchecked.
                assert!(
                    container.contains(&format!("- name: {variable}")),
                    "{binary} refuses to start without {variable} and {} does \
                     not set it. The container exits with a configuration error \
                     and is restarted for ever, which looks like a crash loop \
                     rather than a missing value.",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 4,
        "only {checked} required variables were checked; the walk from a \
         container to the source of the binary it runs is finding nothing"
    );
}

#[test]
fn every_deployment_exclusion_is_recorded_as_a_decision() {
    // A reason in a `const` is read by whoever is already editing this file.
    // The person asking why the operator CLI is not in the cluster, or why
    // there are six application crates and four images, is not that person —
    // and a comment in a build matrix is not where they will look.
    let adr = read("docs/adr/0010-what-gets-deployed.md");

    for (crate_name, _, _) in NOT_IN_THE_IMAGE_MATRIX {
        assert!(
            adr.contains(crate_name),
            "{crate_name} is excluded from the image matrix and the decision \
             record does not mention it"
        );
    }
    for (binary, _) in NOT_A_WORKLOAD {
        assert!(
            adr.contains(binary),
            "{binary} is deployed by no workload and the decision record does \
             not mention it"
        );
    }
    assert!(
        adr.contains("qip-web"),
        "the decision record does not say why qip-web is neither built nor \
         deployed, which is the exclusion that most looks like an oversight"
    );

    // And the record lists what *is* deployed too. A record of only the
    // exceptions cannot be checked against the thing it describes.
    for binary in image_matrix() {
        assert!(
            adr.contains(&binary),
            "the decision record does not name {binary}, which the pipeline \
             builds and deploys"
        );
    }
}

// --- the safety property that outranks all of this --------------------------

#[test]
fn nothing_added_here_raises_the_autonomy_ceiling_anywhere() {
    // Everything above adds workloads, identities and network paths. None of it
    // is allowed to change the one line that decides whether the platform can
    // reach a real venue — and the venue credential's IAM binding must stay
    // absent wherever the ceiling is paper trading.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            tfvars.contains(r#"autonomy_ceiling = "paper_trading""#),
            "{environment} does not ship with a paper-trading ceiling"
        );
        for level in [
            "supervised_live",
            "limited_autonomous_live",
            "autonomous_live",
        ] {
            assert!(
                !tfvars.contains(level),
                "{environment} names {level}, which is above paper trading"
            );
        }
    }

    // The config map the workloads read is paper trading too, and every new
    // workload reads it from that config map rather than from its own default.
    let config = read("infrastructure/kubernetes/base/config.yaml");
    assert!(config.contains(r#"autonomy_ceiling: "paper_trading""#));
    for manifest in ["fastbrain.yaml", "deepbrain.yaml", "edge-cell.yaml"] {
        let content = read(&format!("infrastructure/kubernetes/base/{manifest}"));
        assert!(
            content.contains("key: autonomy_ceiling"),
            "{manifest} does not take the ceiling from the config map"
        );
        assert!(
            !content.contains("supervised_live"),
            "{manifest} names a live autonomy level"
        );
    }

    // And the binding is still conditional on the ceiling, with nothing new
    // granting the venue credential unconditionally.
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    let bindings = secrets.matches("qip-venue-credential").count();
    assert_eq!(
        bindings, 1,
        "the venue credential is named {bindings} times in the secrets module; \
         exactly one of them is the conditional IAM binding"
    );
}
