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
///
/// Read from the entry's `secret_mounts` block alone: a configuration file
/// under `config_files` names its `_PATH` variable with the same field, and
/// counted alongside the secrets it would read as a mount with no secret.
fn catalogue_secret_mounts(body: &str) -> Vec<(String, String)> {
    let body = block_under(body, "secret_mounts = {");
    assert!(
        !body.is_empty(),
        "the catalogue entry has no `secret_mounts` block; the entry shape has changed"
    );
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
fn a_service_that_failed_its_first_revision_is_repaired_rather_than_left_unfixable() {
    // The deadlock this closes, found by the migration's first successful
    // create phase. A Cloud Run service whose first revision is refused —
    // there, an egress sidecar whose image was not yet attested — exists as
    // an object and does not serve. Terraform marks it tainted; a tainted
    // resource is planned destroy-then-create; and the destroy is refused:
    //
    //   Error: cannot destroy service without setting
    //   deletion_protection=false and running `terraform apply`
    //
    // So the environment holds a broken service Terraform can neither repair
    // nor remove, which is the GKE cluster of August in a new place. The
    // answer is the same one: untaint on evidence, so the apply updates in
    // place. This test exists mostly to stop the *other* answer, which is to
    // make deletion_protection a variable — the test above refuses that, and
    // a future reader hitting this error will reach for it first.
    let infra = read(".github/workflows/infra.yml");
    let steps = job_steps(&infra);

    let position = |needle: &str| {
        steps
            .iter()
            .position(|step| step.contains(needle))
            .unwrap_or_else(|| panic!("infra.yml has no step containing {needle}"))
    };
    // By step name: the comment block explaining this sits *above* the step,
    // and job_steps attaches leading comments to the step before, so a search
    // for "untaint" finds the plan step and every assertion below then reads
    // the wrong block. That is how the first draft of this test failed.
    let repair = position("name: repair a tainted service");
    let apply = position("apply -input=false -auto-approve");

    // Ordering: an untaint after the apply is an untaint that changed nothing
    // about the apply that just failed.
    assert!(
        repair < apply,
        "the repair runs at step {repair} and the apply at {apply}, so the \
         apply still plans a replacement it cannot perform"
    );

    let step = &steps[repair];

    // It only ever runs on an apply. A plan that mutated state would be a
    // read-only action that is not one.
    assert!(
        step.contains("inputs.action == 'up'"),
        "the repair is not gated on the apply, so `plan` mutates state"
    );

    // Evidence, not assumption: the taint is cleared only for a service Cloud
    // Run confirms it still has. Without the describe this is "untaint
    // everything", which would hide a service that genuinely needs recreating.
    assert!(
        step.contains("gcloud run services describe"),
        "the repair untaints without asking Cloud Run whether the service is \
         there, so a service that must be recreated silently is not"
    );
    let describe = step
        .find("gcloud run services describe")
        .expect("checked above");
    let untaint = step
        .find("terraform -chdir=infrastructure/terraform untaint")
        .expect("the repair untaints");
    assert!(
        describe < untaint,
        "the untaint precedes the read that is supposed to justify it"
    );

    // And the deletion guard itself is untouched — this is the whole point of
    // taking this route rather than the other one.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        sets(&module, "deletion_protection", "true"),
        "the repair path was taken and the guard was weakened anyway"
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
    // workflow's own authentication. The target is the execution nodes,
    // the one thing that bills while idle.
    assert!(
        infra.contains("-target=module.execution_node"),
        "infra.yml's down is no longer targeted at the execution nodes"
    );
    let text = without_comments(&infra);
    let destroys: Vec<String> = text
        .lines()
        .filter(|line| line.contains("terraform -chdir=infrastructure/terraform destroy"))
        .map(|line| line.trim().to_string())
        .collect();
    assert!(
        !destroys.is_empty(),
        "infra.yml no longer destroys anything"
    );
    for destroy in &destroys {
        let after = text.split(destroy.as_str()).nth(1).unwrap_or_default();
        let next_lines: String = after.lines().take(2).collect::<Vec<_>>().join("\n");
        assert!(
            next_lines.contains("-target=module.execution_node"),
            "infra.yml runs an untargeted destroy: `{destroy}` is not followed by a target"
        );
    }
}

#[test]
fn no_variable_validation_reads_through_the_null_it_is_guarding_against() {
    // `a == null || a.field` does not protect `a.field`. Terraform evaluates
    // both sides of `||`, so a null value there fails the whole plan with
    // "Attempt to get attribute from null value" — which is how the first
    // plan of the Cloud Run migration ended, on
    // `var.market_data_connector == null || startswith(var.market_data_connector.base_url, ...)`
    // with the variable at its own default of null. `can()` and `try()` swallow
    // that error, which is why the neighbouring validations survived; the
    // conditional operator is the one that genuinely does not evaluate the
    // branch it did not take.
    let mut unguarded = Vec::new();
    let mut guarded_forms = 0usize;
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
            // The variable being guarded, as the condition names it.
            let Some(name) = left.split("var.").nth(1).map(str::trim) else {
                continue;
            };
            let dereference = format!("var.{name}.");
            if !right.contains(&dereference) {
                continue;
            }
            guarded_forms += 1;
            if !right.contains("can(") && !right.contains("try(") && !right.contains('?') {
                unguarded.push(format!("{}: {line}", path.display()));
            }
        }
    }

    // Premise: the scan found conditions of this shape at all, so an empty
    // result means they are guarded rather than that the walk reads nothing.
    assert!(
        guarded_forms >= 3,
        "only {guarded_forms} null-guarded validations were seen; the scan is \
         not reading the conditions it is meant to judge"
    );
    assert!(
        unguarded.is_empty(),
        "a validation dereferences the value it is checking for null, which \
         Terraform evaluates anyway and fails the plan on:\n{}",
        unguarded.join("\n")
    );
}

#[test]
fn an_environment_can_be_brought_up_before_anything_has_been_deployed_to_it() {
    // The deadlock this closes. `catalogue.tf` refuses to create a Cloud Run
    // service without a digest for it, `image_digests` defaults to empty, and
    // the digests live in `images.tfvars` — which the bootstrap, the script
    // that creates the services and the only path prod has, never passed. So
    // the apply stopped at "No digest is recorded for qip-api, ...", the
    // services were never created, and deploy.yml — which only ever *moves* a
    // service — had nothing to move. Neither end could go first.
    let bootstrap = read("scripts/bootstrap-deploy.sh");

    // Premise: the script still applies, and the precondition it tripped on
    // is still there to trip on.
    assert!(
        bootstrap.contains("terraform -chdir=\"${TF_DIR}\" apply"),
        "the bootstrap no longer applies, so this test guards nothing"
    );
    let catalogue = read("infrastructure/terraform/catalogue.tf");
    assert!(
        catalogue.contains("No digest is recorded for"),
        "catalogue.tf no longer refuses a workload with no digest; the \
         deadlock this guards is gone and so is its premise"
    );

    assert!(
        bootstrap.contains("images.tfvars"),
        "the bootstrap passes no images.tfvars, so its apply cannot create a \
         Cloud Run service and the environment it bootstraps has none"
    );
    assert!(
        bootstrap.contains("tf_var_files+=(-var-file=\"${IMAGES_TFVARS}\")"),
        "the bootstrap knows the file but does not pass it to terraform"
    );

    // And deploy.yml says which of the two runs first, rather than leaving a
    // raw `Service could not be found` in the log with no fix beside it.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("Terraform creates the Cloud Run services"),
        "deploy.yml does not say that a missing service is Terraform's to make"
    );
    assert!(
        deploy.contains("action=up"),
        "deploy.yml names no way to create the service it could not find"
    );
}

#[test]
fn the_workflow_grants_itself_the_reads_before_terraform_refreshes_with_them() {
    // Terraform refreshes existing state before it applies anything, so a
    // role this same run would grant declaratively is already too late for
    // the refresh that needs it. Seven roles have been added to this loop one
    // failed apply at a time; the last was found by a teardown that issued a
    // GKE node pool delete, had it accepted, and then could not poll the
    // operation it was handed:
    //
    //   Error waiting for deleting GKE NodePool: googleapi: Error 403:
    //   Required "container.operations.get" permission(s)
    //
    // The delete had happened. The apply reported failure anyway and stopped
    // with the runtime ADR 0024 retires half torn down.
    let infra = read(".github/workflows/infra.yml");
    let steps = job_steps(&infra);

    let position = |needle: &str| {
        steps
            .iter()
            .position(|step| step.contains(needle))
            .unwrap_or_else(|| panic!("infra.yml has no step containing {needle}"))
    };

    let grant = position("for role in roles/");
    let init = position("terraform init");

    // Premise: the loop still grants, and grants to the account that plans.
    let step = &steps[grant];
    assert!(
        step.contains("gcloud projects add-iam-policy-binding")
            && step.contains("steps.identity.outputs.account"),
        "the step no longer grants the planning account anything, so what \
         this test asserts about its roles guards nothing"
    );

    // Ordering is the whole point: after init, every one of these is too late.
    assert!(
        grant < init,
        "the self-grant runs at step {grant} and terraform init at {init}, so \
         the refresh that needs these roles happens before they are held"
    );

    for role in [
        "roles/cloudkms.publicKeyViewer",
        "roles/binaryauthorization.attestorsAdmin",
        "roles/run.admin",
        "roles/dns.admin",
        "roles/iap.admin",
        "roles/identityplatform.admin",
        // The one the halted teardown needed, in both halves: polling the
        // node pool delete GKE had already accepted, and then issuing the
        // cluster delete after this same teardown destroyed the declarative
        // grant that carried it.
        "roles/container.clusterAdmin",
    ] {
        assert!(
            step.contains(&format!("{role} ")) || step.contains(&format!("{role};")),
            "the self-grant loop no longer carries {role}, so the read it \
             permits fails on the refresh ahead of the apply"
        );
    }

    // And nothing in this loop may reach past what it needs. The teardown
    // manages clusters and node pools; container.admin adds full access to
    // the Kubernetes API objects inside a cluster, which no step here
    // touches. The two names differ by one segment, so this matches the
    // delimited token — `contains("roles/container.admin")` would also be
    // true of a hypothetical `roles/container.adminViewer`.
    for delimiter in [' ', ';', '\n'] {
        assert!(
            !step.contains(&format!("roles/container.admin{delimiter}")),
            "the self-grant loop takes container.admin, which carries the \
             inside of a cluster as well as the cluster"
        );
    }
}

#[test]
fn the_retired_backup_plan_is_forgotten_rather_than_deleted_with_its_backups() {
    // The first apply of the Cloud Run runtime planned the GKE-era backup
    // plan for destruction and the API refused:
    //
    //   Error 400: Resource '"...backupPlans/qip-dev-journal"' has nested
    //   resources. If the API supports cascading delete, set 'force' to true.
    //
    // The nested resources are backups of the journal. `force = true` is the
    // available answer and it deletes the evidence to unblock a migration.
    let module = read("infrastructure/terraform/modules/backup/main.tf");

    // Premise: the module is still the journal's backup mechanism, so this is
    // a test about how it retires the old one rather than about a stub.
    assert!(
        module.contains("resource \"google_compute_resource_policy\" \"journal_snapshots\""),
        "modules/backup no longer declares the snapshot schedule, so there is \
         no mechanism here for the removed blocks to be retiring in favour of"
    );
    assert!(
        !module.contains("resource \"google_gke_backup_backup_plan\""),
        "modules/backup declares a GKE backup plan again, which ADR 0024 \
         retired with the cluster"
    );

    // Forgotten, with the backups left where they are.
    let forgotten = module
        .split("removed {")
        .skip(1)
        .filter(|block| block.contains("destroy = false"))
        .collect::<Vec<_>>();
    assert!(
        forgotten
            .iter()
            .any(|block| block.contains("from = google_gke_backup_backup_plan.journal")),
        "the backup plan is not forgotten with destroy = false, so the apply \
         either halts on it again or deletes it"
    );

    // Nothing anywhere may reach for the cascading delete.
    for path in files_with_extension("infrastructure", "tf") {
        // `terraform fmt` aligns the `=` of a block's arguments, so the
        // literal spacing here is whatever its neighbours make it. This scan
        // lost a mutation to exactly that before it was written this way.
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !text.contains("force = true"),
            "{} sets force = true, which is how a backup plan is deleted \
             together with the backups under it",
            path.display()
        );
    }
}

#[test]
fn the_infrastructure_workflow_reports_no_resource_count_it_did_not_read() {
    // `terraform state list | wc -l` printed `0` for a backend it could not
    // read exactly as it did for an environment holding nothing, so a broken
    // bootstrap reported "0 resources in dev state" as a fact about the
    // cloud. And `always()` ran it after the prod refusal, before terraform
    // was installed, burying the real failure under its own. A step that
    // reports a number nobody computed is the failure mode this repository
    // exists to refuse.
    let infra = read(".github/workflows/infra.yml");
    let steps = job_steps(&infra);
    let count = steps
        .iter()
        .find(|step| step.contains("what exists now"))
        .expect("infra.yml has a step that says what exists");

    // Premise: the step still counts state, so what follows is about how.
    assert!(
        count.contains("terraform -chdir=infrastructure/terraform state list"),
        "the step no longer lists state, so this test guards nothing"
    );
    assert!(
        !count.contains("state list | wc -l"),
        "the step still pipes the listing straight into wc, which counts a \
         failure as zero"
    );
    assert!(
        count.contains("set -euo pipefail"),
        "the step does not stop on a listing that failed"
    );
    assert!(
        count.contains("steps.init.outcome == 'success'"),
        "the step runs whether or not init succeeded, so it can fail before \
         terraform exists and hide the failure that mattered"
    );
    // And the id that condition names is on the init step, or the condition
    // is never true and the step never runs at all.
    assert!(
        steps
            .iter()
            .any(|step| step.contains("terraform init") && step.contains("id: init")),
        "no step carries `id: init`, so the condition above can never hold"
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
// A `removed` block says an instance leaves Terraform's management. It is not
// a declaration, and a scan looking for declarations must not read it as one.
fn without_removed_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut depth: Option<usize> = None;
    for line in text.lines() {
        match depth {
            None => {
                if line.trim_start().starts_with("removed ") && line.contains('{') {
                    depth = Some(line.matches('{').count() - line.matches('}').count());
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
            Some(open) => {
                let next = open + line.matches('{').count() - line.matches('}').count();
                depth = if next == 0 { None } else { Some(next) };
            }
        }
    }
    out
}

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

// --- the workloads and the binaries they run --------------------------------
//
// The bug these exist to catch is already in the repository's history: a
// NetworkPolicy governing a pod that no Deployment created. A rule for a
// workload that does not exist is not harmless — it is a reviewer reading the
// configuration and concluding the deep brain is deployed and constrained,
// when it is neither.
//
// So the correspondence is checked in both directions. A test that only asked
// "does every binary have a workload" would have passed on that namespace
// without noticing, because the missing half was the workload.

/// Binaries in the workspace that are deliberately not deployed as a workload.
///
/// Kept as a list with a reason attached rather than as a filter in the test,
/// because "why is this one exempt" is the question the next person will have
/// and a predicate cannot answer it.
const NOT_A_WORKLOAD: &[(&str, &str)] = &[(
    // `qip-cli` builds a binary called `qip`.
    "qip",
    "an operator's tool, run by a person against a deployment rather than \
     scheduled in one",
)];

/// Binaries the workspace builds that the pipeline deliberately builds no image
/// for.
///
/// The sibling of `NOT_A_WORKLOAD`, and deliberately a second list rather than
/// a reuse of the first: "nothing schedules it" and "nothing builds it" are
/// different decisions, and a crate could sensibly be one without the other —
/// an operator tool distributed as an image and run as a Cloud Run job would
/// be in the matrix and absent from the catalogue.
///
/// The reasons are the short form. The decision is
/// `docs/adr/0010-what-gets-deployed.md`, and
/// `every_deployment_exclusion_is_recorded_as_a_decision` is what keeps the two
/// from drifting.
const NOT_IN_THE_IMAGE_MATRIX: &[(&str, &str, &str)] = &[(
    "qip-cli",
    "qip",
    "an operator's tool, run by a person against a deployment rather than \
     scheduled in one",
)];

/// Workloads whose binary is not in the workspace yet.
///
/// This list has to shrink to nothing, and
/// `every_pending_workload_is_still_actually_pending` is what makes it: that
/// test fails the moment the crate lands, so the exemption is removed by
/// whoever lands it rather than surviving as a permanent hole in the check
/// above.
const AWAITING_ITS_CRATE: &[(&str, &str)] = &[];

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

/// The binary the execution node's unit runs, from its `ExecStart`.
fn node_binary() -> String {
    // The startup script writes two units: the egress proxy's, whose
    // ExecStart is the vendored Envoy, and the node's. Only the second runs
    // a binary this workspace builds, so the walk starts at its
    // Description line rather than at the first ExecStart in the file —
    // which is the proxy's, and would make every test here ask whether
    // `envoy` is a crate.
    let startup = read(NODE_STARTUP);
    let unit = startup
        .split("Description=qip execution node")
        .nth(1)
        .expect("the startup script writes a unit described as the execution node");
    unit.lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart=/usr/local/bin/"))
        .map(|rest| rest.split_whitespace().next().unwrap_or(rest).to_string())
        .expect("the node's unit names the binary it runs")
}

/// Every binary something deploys: the catalogue's services and the node.
fn deployed_binaries() -> Vec<String> {
    let mut deployed: Vec<String> = catalogue_workloads()
        .iter()
        .map(|(_, body)| catalogue_field(body, "binary"))
        .collect();
    deployed.push(node_binary());
    deployed.sort();
    deployed.dedup();
    deployed
}

/// The lines nested under `key`, by indentation.
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

#[test]
fn every_deployable_binary_in_the_workspace_has_a_workload() {
    let deployed = deployed_binaries();
    for binary in workspace_binaries() {
        if NOT_A_WORKLOAD.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            deployed.contains(&binary),
            "{binary} is a binary this workspace builds and nothing deploys it. \
             Either give it a catalogue entry or add it to NOT_A_WORKLOAD with \
             the reason it is not one."
        );
    }
}

#[test]
fn every_workload_runs_a_binary_that_exists() {
    // The other direction, and the one the repository actually got wrong.
    let binaries = workspace_binaries();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            binaries.contains(&binary),
            "a workload runs {binary}, which no crate in this workspace builds. \
             A catalogue entry for a binary that does not exist reads to a \
             reviewer as a component that is deployed and constrained."
        );
    }
}

#[test]
fn every_pending_workload_is_still_actually_pending() {
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
fn every_cloud_run_service_is_probed_on_a_path_its_binary_serves() {
    // A probe pointed at an endpoint that does not exist looks like coverage
    // and is not: Cloud Run would never route to the service, and the failure
    // reads as an image that will not start. Each catalogue entry names the
    // path, and the binary's own source has to serve it.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains("startup_probe {") && module.contains("liveness_probe {"),
        "the Cloud Run module no longer probes the workload"
    );
    assert_eq!(
        module.matches("path = var.health_path").count(),
        2,
        "the two probes do not both poll the catalogue's health path"
    );

    let mut probed = 0usize;
    for (name, body) in catalogue_workloads() {
        let binary = catalogue_field(&body, "binary");
        let path = catalogue_field(&body, "health_path");
        assert!(
            path.starts_with('/'),
            "{name}'s health path {path} is not a path"
        );
        let sources: String =
            files_with_extension(&format!("backend/crates/apps/{binary}/src"), "rs")
                .iter()
                .filter_map(|source| std::fs::read_to_string(source).ok())
                .collect();
        assert!(
            sources.contains(&format!("\"{path}\"")),
            "{name} is probed on {path} and {binary} serves no such path; Cloud Run would \
             never route to it and the failure would read as an image that will not start"
        );
        probed += 1;
    }
    assert_eq!(
        probed, 3,
        "the premise failed: not every workload was checked"
    );
}

#[test]
fn the_image_runs_as_a_non_root_user_on_an_empty_filesystem() {
    // The container properties the Kubernetes manifests used to carry as a
    // security context are properties of the image now, because Cloud Run
    // runs what the image says. A statically linked binary on scratch, as a
    // fixed non-root uid: no shell, no package manager, no libc to reach, so a
    // container an attacker reaches is a container they can do very little
    // with.
    let dockerfile = read("infrastructure/docker/Dockerfile");
    let stages: Vec<&str> = dockerfile
        .lines()
        .filter(|line| line.starts_with("FROM "))
        .collect();
    assert!(
        stages.last().is_some_and(|last| *last == "FROM scratch"),
        "the image's final stage is not scratch: {stages:?}"
    );
    assert!(
        dockerfile
            .lines()
            .any(|line| line.trim() == "USER 10001:10001"),
        "the image does not fix a non-root user, so Cloud Run runs it as root"
    );
    assert!(
        dockerfile.contains("--locked"),
        "the image is built without --locked, so it can resolve a dependency graph the tests never ran against"
    );
    // And the one third-party image is the distroless one, for the same
    // reason.
    let vendored = read("infrastructure/egress/vendored-images.txt");
    assert!(
        vendored.lines().any(|line| !line.starts_with('#')
            && line.contains("vendor/envoy")
            && line.contains("distroless")),
        "the vendored Envoy is not the distroless image; a shell on the process that terminates TLS is a shell"
    );
}

#[test]
fn every_cloud_run_container_declares_a_cpu_and_a_memory_limit() {
    // A memory limit without a CPU limit lets a busy instance starve its
    // neighbours; a CPU limit without a memory limit lets a leak take down the
    // instance. Both, on every container the module renders: the service, the
    // job, the proxy sidecar and the metrics collector.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    let limits: Vec<String> = module
        .split("limits = {")
        .skip(1)
        .map(|rest| rest.split('}').next().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        limits.len(),
        4,
        "{} limits blocks were read; the service, the job, the proxy sidecar and the metrics collector each carry one",
        limits.len()
    );
    for block in &limits {
        assert!(
            block.contains("cpu"),
            "a container has no CPU limit: {block}"
        );
        assert!(
            block.contains("memory"),
            "a container has no memory limit: {block}"
        );
    }
    // And the catalogue sets both for every workload rather than taking a
    // default sized for something else.
    for (name, body) in catalogue_workloads() {
        let cpu = catalogue_field(&body, "cpu");
        let memory = catalogue_field(&body, "memory");
        assert!(
            ["0.25", "0.5", "1", "2", "4", "8"].contains(&cpu.as_str()),
            "{name} asks for {cpu} CPU, which Cloud Run does not accept"
        );
        assert!(
            memory.ends_with("Mi") || memory.ends_with("Gi"),
            "{name} asks for {memory} memory, which Cloud Run does not accept"
        );
    }
}

/// The catalogue workloads whose binary opens the hash-chained event log and
/// runs the cycle on its own clock, with the value each sets for a scaling
/// field.
///
/// Read from the binaries rather than from a list here: the day a fourth
/// binary opens the archive and loops on an interval, this finds it, and a
/// list would not. Two facts make a workload one of these — `main.rs` opens
/// the `ChainArchive`, and `config.rs` declares a `DEFAULT_CYCLE_INTERVAL` —
/// because the API opens the archive too and cycles only when asked, which
/// is a workload that may scale.
fn workloads_that_run_the_cycle_over_the_journal(field: &str) -> Vec<(String, String)> {
    catalogue_workloads()
        .into_iter()
        .filter(|(_, body)| {
            let binary = catalogue_field(body, "binary");
            let main = read(&format!("backend/crates/apps/{binary}/src/main.rs"));
            let config =
                repository_root().join(format!("backend/crates/apps/{binary}/src/config.rs"));
            let config = std::fs::read_to_string(config).unwrap_or_default();
            main.contains("ChainArchive::open(") && config.contains("DEFAULT_CYCLE_INTERVAL")
        })
        .map(|(name, body)| {
            let value = catalogue_field(&body, field);
            (name, value)
        })
        .collect()
}

#[test]
fn every_workload_that_runs_the_cycle_over_the_journal_is_pinned_to_one_warm_instance() {
    // The module's defaults are a floor of zero and a ceiling of four, sized
    // for a request-serving workload. The two brains are not that: each runs
    // the cycle on its own clock and appends to one hash-chained log. A
    // second instance is a second writer, and two writers of one chain
    // produce the fork the chain exists to detect — a double-run cycle
    // reported as corruption rather than tolerated as redundancy. And a zero
    // floor is a loop that stops: nothing requests these services, so the
    // instance Cloud Run retires for want of a request is never started
    // again. Both bounds belong in the catalogue, where a diff shows them,
    // not in a default sized for something else.
    let ceilings = workloads_that_run_the_cycle_over_the_journal("max_instances");
    assert!(
        !ceilings.is_empty(),
        "no catalogue workload opens the event log and runs a cycle on its own \
         clock; the binaries have been reshaped and every bound below is vacuous"
    );
    for (name, ceiling) in &ceilings {
        assert_eq!(
            ceiling, "1",
            "{name} runs the cycle over the event log and may scale to {ceiling} \
             instances; two would each run the cycle and fork the chain"
        );
    }
    for (name, floor) in workloads_that_run_the_cycle_over_the_journal("min_instances") {
        assert_eq!(
            floor, "1",
            "{name} runs the cycle on its own clock with a floor of {floor}; \
             nothing requests it, so an instance retired for idleness is a \
             cycle that never runs again"
        );
    }
    for (name, why) in workloads_that_run_the_cycle_over_the_journal("always_on_justification") {
        assert!(
            why.len() > 40,
            "{name} keeps a warm instance with no written reason; the module \
             refuses that at plan time, and a reviewer should be able to read \
             why without the plan"
        );
    }

    // The API is the workload that may scale, and it still does: pinning it
    // too would be the opposite mistake, a ceiling copied from the service
    // next door.
    let (_, body) = catalogue_workloads()
        .into_iter()
        .find(|(name, _)| name == "api")
        .expect("the catalogue has an api entry");
    let api_ceiling: u32 = catalogue_field(&body, "max_instances")
        .parse()
        .expect("the API's max_instances is a number");
    assert!(
        api_ceiling > 1,
        "the API has been pinned to one instance, which it does not need"
    );

    // And the values reach the module rather than sitting in the entry: the
    // catalogue passes each one through and the module applies it to the
    // service's scaling block.
    let catalogue = without_comments(&read(CATALOGUE));
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    for field in ["min_instances", "max_instances", "always_on_justification"] {
        assert!(
            sets(&catalogue, field, &format!("each.value.{field}")),
            "the catalogue no longer passes {field} to the module, so the entry's value is decoration"
        );
    }
    assert!(
        sets(&module, "max_instance_count", "var.max_instances")
            && sets(&module, "min_instance_count", "var.min_instances"),
        "the module no longer applies its instance bounds to the service"
    );
}

#[test]
fn the_metrics_collector_runs_only_under_a_digest_pinned_image_and_nothing_claims_a_scrape() {
    // The Cloud Run services emit and nothing scrapes them (NOT-SCRAPED.md).
    // The collector that closes that is a third-party image, and a
    // third-party image on this platform is admitted only as the bytes the
    // attestor signed. So the sidecar is keyed on one thing — a digest — and
    // the failure this test prevents has two shapes: a collector declared
    // under a tag, which Binary Authorization would refuse at admission and
    // an operator would read as a broken deploy; and a collector declared
    // by default, which would put a sidecar nobody vendored on every
    // service the day the module was applied.
    let variables = without_comments(&read(CLOUD_RUN_VARIABLES));
    let block = variables
        .split("variable \"collector_image_digest\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("modules/cloudrun declares collector_image_digest");
    assert!(
        sets(block, "default", "null"),
        "collector_image_digest has a default other than null, so a collector is declared on a workload nobody named one for"
    );
    assert!(
        block.contains(
            "var.collector_image_digest == null || can(regex(\"^[a-z0-9][a-z0-9._/-]*[a-z0-9]@sha256:[a-f0-9]{64}$\", var.collector_image_digest))"
        ),
        "collector_image_digest no longer refuses anything but null or a full repository@sha256 digest"
    );

    // The module renders the sidecar from the digest and from nothing else,
    // and reports only that it declared one.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        sets(
            &module,
            "has_metrics_collector",
            "var.collector_image_digest != null"
        ),
        "the collector is keyed on something other than the digest being set"
    );
    let sidecar = module
        .split("for_each = local.has_metrics_collector ? [var.collector_image_digest] : []")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("the Cloud Run module renders the collector from the digest");
    // Premise: this really is the collector's block.
    assert!(
        sets(sidecar, "image", "containers.value") && sidecar.contains("depends_on = [var.name]"),
        "the collector block has been reshaped; every absence below is vacuous"
    );
    for (marker, why) in [
        (
            "secret",
            "a mounted secret is a credential the collector has no use for",
        ),
        (
            "env {",
            "an environment value on the collector is one more thing in /proc/<pid>/environ",
        ),
        (
            "service_account",
            "an identity of its own would be a second principal for one service",
        ),
    ] {
        assert!(
            !sidecar.contains(marker),
            "the metrics collector carries `{marker}`: {why}"
        );
    }
    let outputs = without_comments(&read(
        "infrastructure/terraform/modules/cloudrun/outputs.tf",
    ));
    let collected = outputs
        .split("output \"metrics_collected\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("modules/cloudrun outputs metrics_collected");
    assert!(
        sets(collected, "value", "local.has_metrics_collector"),
        "metrics_collected answers something other than whether a collector was declared"
    );

    // What it scrapes is written down: the workload's own port, the path the
    // brains serve, and a bounded interval — not the sidecar's built-in
    // default, which is a target and a cadence that live in an image.
    let config = module
        .split("collector_config = <<-EOT")
        .nth(1)
        .and_then(|rest| rest.split("\n  EOT").next())
        .expect("the module writes a RunMonitoring document");
    for line in [
        "kind: RunMonitoring",
        "port: ${var.container_port}",
        "path: /metrics",
        "interval: 30s",
        "timeout: 10s",
    ] {
        assert!(
            config.contains(line),
            "the collector's RunMonitoring document no longer says `{line}`"
        );
    }

    // The root names the digest bare and the catalogue composes it with the
    // registry prefix, so the upstream repository cannot reach a plan; and
    // the two brains carry it while the API, whose /metrics needs a token,
    // does not.
    let root = without_comments(&read("infrastructure/terraform/variables.tf"));
    let root_block = root
        .split("variable \"metrics_collector_image_digest\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("the root declares metrics_collector_image_digest");
    assert!(
        sets(root_block, "default", "null")
            && root_block.contains(
                "can(regex(\"^sha256:[a-f0-9]{64}$\", var.metrics_collector_image_digest))"
            ),
        "the root's collector digest is not null-by-default and refused unless it is a bare sha256 digest"
    );
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("\"${module.registry.image_prefix}/vendor/cloud-run-gmp-sidecar@${var.metrics_collector_image_digest}\""),
        "the catalogue composes the collector image from somewhere other than the environment's own registry"
    );
    let mut collecting = Vec::new();
    for (name, body) in catalogue_workloads() {
        let wants = catalogue_field(&body, "metrics_collector");
        assert!(
            wants == "true" || wants == "false",
            "{name}'s metrics_collector is `{wants}`, not a boolean"
        );
        if wants == "true" {
            collecting.push(name);
        }
    }
    assert_eq!(
        collecting,
        vec!["fastbrain", "deepbrain"],
        "the collector is attached to something other than the two brains; the API's /metrics is behind Role::Monitor and answers a tokenless scrape 401"
    );

    // And no environment names a digest, because none has been reviewed,
    // mirrored and attested. The tfvars comment says how one would be.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        assert!(
            !tfvars.lines().any(|line| line
                .trim_start()
                .starts_with("metrics_collector_image_digest")),
            "{environment} names a collector digest that vendored-images.txt does not carry"
        );
    }
}

const EDGE_TELEMETRY: &str = "backend/crates/edge/qip-edge/src/telemetry.rs";
const OBSERVABILITY_MODULE: &str = "infrastructure/terraform/modules/observability/main.tf";

/// The `source` labels `CellMetrics::halt` writes `qip_edge_halted` under,
/// read from the one place they are literals.
///
/// Brittle on purpose: the walk stops at the next `pub fn`, so a halt method
/// reshaped to take its sources some other way returns nothing, and the
/// caller's premise check fails loudly rather than the policy text being
/// compared against an empty list.
fn edge_halt_sources() -> Vec<String> {
    let telemetry = read(EDGE_TELEMETRY);
    let body = telemetry
        .split("pub fn halt(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn ").next())
        .expect("CellMetrics has a `halt` method");
    body.split("self.with(\"source\", \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

#[test]
fn the_edge_halt_alert_names_every_halt_discipline_the_cell_records() {
    // The policy fires on `qip_edge_halted{source} > 0` and its text tells
    // the person paged what each `source` means. The cell gained a third
    // discipline — the polled flag on the node's own filesystem — and the
    // text went on naming two, so an operator woken by `source="polled"`
    // was reading a runbook that said no such source existed. This binds the
    // text to the literals the cell writes, so the next discipline cannot
    // land without its sentence.
    let sources = edge_halt_sources();
    // Premise: the walk found the disciplines the cell is known to have,
    // including the one that was missing from the text.
    for known in ["kill_switch", "policy", "polled"] {
        assert!(
            sources.iter().any(|source| source == known),
            "CellMetrics::halt no longer writes source=\"{known}\"; found {sources:?}. The walk \
             has stopped reading the literals and every check below is vacuous"
        );
    }

    let module = read(OBSERVABILITY_MODULE);
    let policies = terraform_resources(&module, "google_monitoring_alert_policy");
    let (_, halted) = policies
        .iter()
        .find(|(name, _)| name == "edge_halted")
        .expect("the observability module declares the edge_halted alert policy");
    assert!(
        halted.contains("max by (cell, source) (qip_edge_halted) > 0"),
        "the edge_halted policy no longer groups on `source`; a text naming the sources \
         would describe a label the alert does not carry"
    );
    let documentation = halted
        .split("content   = <<-EOT")
        .nth(1)
        .and_then(|rest| rest.split("EOT").next())
        .expect("the edge_halted policy carries documentation");
    for source in &sources {
        assert!(
            documentation.contains(&format!("`{source}`")),
            "the edge_halted policy's documentation does not name `{source}`, which \
             CellMetrics::halt writes as a source; an operator paged on it has no sentence \
             saying what stopped the node"
        );
    }
}

#[test]
fn every_workload_that_reads_the_universe_is_given_the_committed_catalogue_as_a_mounted_file() {
    // The three central roots assemble the desk from an instrument universe
    // read at `QIP_UNIVERSE_PATH`. Two failures this prevents. A workload
    // that reads the variable and is given no file starts on whatever the
    // root falls back to, healthy, with nothing anywhere saying which
    // instruments it is trading. And a file fetched from somewhere other
    // than the commit — an object somebody uploaded, a template rendered at
    // apply — is a universe no reviewer read: the plan must carry the
    // committed bytes, by `file()`, and name them by hash.
    let catalogue = without_comments(&read(CATALOGUE));
    let source = catalogue
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "universe_catalogue").then(|| value.trim().to_string())
        })
        .expect("catalogue.tf declares local.universe_catalogue");
    let relative = source
        .strip_prefix("file(\"${path.module}/../../")
        .and_then(|rest| rest.strip_suffix("\")"))
        .unwrap_or_else(|| {
            panic!(
                "the universe is read as `{source}`, not with file() from a repository path; \
                 a plan cannot prove a fetched object is the reviewed one"
            )
        });
    assert!(
        relative.starts_with("data/") && relative.ends_with(".json"),
        "the universe is read from {relative}, which is not a JSON file under data/ (ADR 0016)"
    );
    // The file's presence is enforced by `file()` at plan time, which refuses
    // a path that does not exist; what is asserted here is that, when it is
    // present, it is the JSON the roots parse and not an empty placeholder.
    let committed = repository_root().join(relative);
    if committed.exists() {
        let text = read(relative);
        assert!(
            !text.trim().is_empty(),
            "{relative} is empty; an empty universe mounted at the path the roots read is a \
             desk with no instruments and nothing wrong reported"
        );
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|error| panic!("{relative} is not JSON: {error}"));
    }

    // Every workload: given the committed bytes, under the name the module
    // mounts, pointed at by the variable the roots read — and never by a
    // path written as a literal in `env` beside it.
    let workloads = catalogue_workloads();
    for (name, body) in &workloads {
        let files = block_under(body, "config_files = {");
        assert!(
            !files.is_empty(),
            "{name} mounts no config_files; a root that reads QIP_UNIVERSE_PATH is given no universe"
        );
        assert!(
            sets(&files, "content", "local.universe_catalogue"),
            "{name}'s universe is not the committed file read once in local.universe_catalogue"
        );
        assert!(
            sets(&files, "file_name", "\"universe.json\""),
            "{name} mounts the universe under a name other than universe.json, so the roots' \
             default path /etc/qip/universe.json reads nothing"
        );
        assert!(
            sets(&files, "env_file_variable", "\"QIP_UNIVERSE_PATH\""),
            "{name} does not point QIP_UNIVERSE_PATH at the mounted universe"
        );
        assert!(
            catalogue_env(body)
                .iter()
                .all(|(variable, _)| variable != "QIP_UNIVERSE_PATH"),
            "{name} sets QIP_UNIVERSE_PATH as a literal in env as well; the module writes the \
             path, and a path written twice is a path that will be written two ways"
        );
    }
    assert!(
        catalogue.contains("config_files  = each.value.config_files"),
        "catalogue.tf no longer hands each entry's config_files to modules/cloudrun"
    );

    // The module: the path the variable carries is
    // /etc/qip/<hash>/<file_name>, the object's content is the input's
    // content under that same hash-named directory, and the volume is a
    // read-only mount of the bucket, on both kinds. The hash is in the path
    // rather than in the mount because the GA provider has no `mount_options`
    // to select a directory with; either way the variable names exactly the
    // bytes the plan carried, which is the property.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        sets(&module, "config_root", "\"/etc/qip\"")
            && module.contains(
                "key => \"${local.config_root}/${local.config_prefix}/${file.file_name}\""
            )
            && module.contains("file.env_file_variable => local.config_files[key]"),
        "the module no longer writes /etc/qip/<hash>/<file_name> into the _PATH variable"
    );
    let objects = terraform_resources(&module, "google_storage_bucket_object");
    let (_, object) = objects
        .iter()
        .find(|(name, _)| name == "config_files")
        .expect("the module publishes config_files as bucket objects");
    assert!(
        sets(object, "content", "each.value.content")
            && sets(
                object,
                "name",
                "\"${local.config_prefix}/${each.value.file_name}\""
            ),
        "the config object's content or hash-named path is no longer the input's"
    );
    assert!(
        module.contains("sha256(file.content)"),
        "the module no longer hashes each file's content, so the object's name says nothing about its bytes"
    );
    // Line-based and whitespace-collapsed, because `terraform fmt` aligns
    // the mount's `name` with its `mount_path` and a substring search for
    // `name = "config-files"` would find the volume and miss the mount.
    let lines: Vec<&str> = module.lines().collect();
    let mut volumes = 0usize;
    let mut mounts = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if line.split_whitespace().collect::<Vec<_>>().join(" ") != "name = \"config-files\"" {
            continue;
        }
        // The lines that follow, up to the block's own closing brace.
        let piece = lines[index + 1..]
            .iter()
            .take_while(|line| line.trim() != "}")
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        let block = piece.as_str();
        if block.contains("gcs {") {
            // Read-only, from the workload's own bucket. The hash-named
            // directory used to be selected by `only-dir`, which the GA
            // provider has no argument for — it refused the first plan of
            // this runtime — so the whole bucket mounts and the hash is
            // carried in the path the environment names instead. The
            // guarantee is the same and it is asserted below, on the path.
            assert!(
                sets(block, "read_only", "true")
                    && !block.contains("mount_options")
                    && block.contains("google_storage_bucket.config_files[0].name"),
                "a config-files volume is not a plain read-only mount of the workload's own \
                 bucket:\n{block}"
            );
            volumes += 1;
        } else {
            let preceding = lines[index.saturating_sub(4)..index].join("\n");
            assert!(
                sets(block, "mount_path", "volume_mounts.value")
                    && preceding.contains("[local.config_root]"),
                "a config-files mount is somewhere other than config_root:\n{preceding}\n{block}"
            );
            mounts += 1;
        }
    }
    assert_eq!(
        (volumes, mounts),
        (2, 2),
        "expected a config-files volume and mount on both the service and the job"
    );
    // The grant is the narrow one, on the workload's own bucket.
    let grants = terraform_resources(&module, "google_storage_bucket_iam_member");
    let (_, grant) = grants
        .iter()
        .find(|(name, _)| name == "config_files")
        .expect("the module grants the workload its config bucket");
    assert!(
        sets(grant, "role", "\"roles/storage.objectViewer\""),
        "the config bucket grant is wider than objectViewer"
    );

    // Not a secret, and refused if shaped like one; and a change in the
    // committed bytes is visible in the plan as a hash.
    let variables = without_comments(&read(CLOUD_RUN_VARIABLES));
    assert!(
        variables.contains("^QIP_[A-Z0-9_]*_PATH$"),
        "config_files no longer refuses a _FILE variable name, so a catalogue could be read as a credential"
    );
    assert!(
        sets(&catalogue, "value", "sha256(local.universe_catalogue)"),
        "the root no longer outputs the universe's hash"
    );

    // And the readers: every deployable binary whose source names the
    // variable is one of the workloads above, each of which was just shown
    // to mount the file. A binary that is deployed by no workload is argued
    // in NOT_A_WORKLOAD and is not a Cloud Run reader.
    let given: Vec<String> = workloads
        .iter()
        .map(|(_, body)| catalogue_field(body, "binary"))
        .collect();
    for path in files_with_extension("backend/crates/apps", "rs") {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains("QIP_UNIVERSE_PATH") {
            continue;
        }
        let crate_name = path
            .strip_prefix(repository_root().join("backend/crates/apps"))
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .expect("an apps source file sits under its crate");
        assert!(
            given.contains(&crate_name)
                || NOT_A_WORKLOAD
                    .iter()
                    .any(|(binary, _)| *binary == crate_name),
            "{crate_name} reads QIP_UNIVERSE_PATH and is deployed by a workload that is not \
             given the universe"
        );
    }
}

/// No Kubernetes manifest exists for a credential to appear in; the property
/// this test owned moved to the Cloud Run catalogue and is asserted there.
///
/// The name is kept because `security.rs` and the threat model cite it by name
/// as the test that scans deployed configuration for an inline credential, and
/// a renamed test is one those citations stop finding. What it now asserts is
/// the truth on this runtime: there is no manifest, and the catalogue that
/// replaced it carries no literal credential and projects every token as a
/// mounted file.
#[test]
fn no_credential_appears_in_a_kubernetes_manifest() {
    for retired in ["infrastructure/kubernetes", "infrastructure/helm"] {
        assert!(
            !repository_root().join(retired).exists(),
            "{retired} exists again; the Kubernetes runtime was retired under ADR 0024 and a \
             manifest directory nothing applies reads as the running system"
        );
    }
    let catalogue = without_comments(&read(CATALOGUE));
    let mut values = 0usize;
    for line in catalogue.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().starts_with("QIP_") {
            continue;
        }
        values += 1;
        let value = value.trim().trim_matches('"');
        assert!(
            !looks_like_a_credential(value),
            "catalogue.tf has what looks like a literal credential: {line}"
        );
    }
    assert!(values >= 8, "only {values} environment values were scanned");
    // Every token is a mounted file, never a value. `QIP_TOKEN_` appears in
    // the catalogue only as the `_FILE` variable of a secret mount.
    for line in catalogue.lines().filter(|line| line.contains("QIP_TOKEN_")) {
        assert!(
            line.contains("env_file_variable") && line.contains("_FILE"),
            "catalogue.tf carries a token outside a secret mount: {line}"
        );
    }
}

/// Whether a value looks like a credential rather than configuration.
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
fn the_autonomy_ceiling_comes_from_the_one_root_variable_rather_than_a_literal() {
    // Changing what the platform is permitted to do should appear in a diff
    // and in an audit log, in one place. Every catalogue workload takes the
    // ceiling from `var.autonomy_ceiling` — the variable whose validation
    // refuses the three live rungs at plan time — and never from a string.
    let mut checked = 0usize;
    for (name, body) in catalogue_workloads() {
        let setting = body
            .lines()
            .find(|line| line.trim_start().starts_with("QIP_AUTONOMY_CEILING"))
            .unwrap_or_else(|| panic!("{name} does not set QIP_AUTONOMY_CEILING"));
        let (_, value) = setting.split_once('=').expect("an assignment");
        assert_eq!(
            value.trim(),
            "var.autonomy_ceiling",
            "{name} takes its ceiling from `{}` rather than from the one root variable",
            value.trim()
        );
        checked += 1;
    }
    assert_eq!(checked, 3, "not every workload was checked");
}

#[test]
fn every_image_the_matrix_builds_has_a_workload_that_runs_it() {
    // An image nobody deploys is a build that costs money and ships nothing.
    // Worse, it reads to a reviewer as a component that is deployed: the
    // pipeline visibly builds and pushes it, and nothing says it is never run.
    let deployed = deployed_binaries();
    for binary in image_matrix() {
        assert!(
            deployed.contains(&binary),
            "the pipeline builds and pushes {binary} and no workload runs it. \
             Either write the catalogue entry or take it out of the matrix."
        );
    }
}

#[test]
fn the_pipeline_builds_an_image_for_every_workload_it_deploys() {
    // The reverse, and the one that fails a deployment rather than wasting a
    // build: a service whose image nothing pushes is a revision waiting for a
    // digest that will never exist.
    let matrix = image_matrix();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            matrix.contains(&binary),
            "{binary} has a workload and the pipeline builds no image for it"
        );
    }
}

#[test]
fn every_workload_pulls_from_the_repository_the_pipeline_pushes_to() {
    // Three places name the same repository and none of them can see the other
    // two: the workflow builds `<region>-docker.pkg.dev/<project>/qip-<env>`,
    // Terraform creates `qip-<env>`, and the catalogue reads the prefix from
    // the registry module. If any drifts, every pull in every environment
    // fails with a 404 that names neither of the two that disagree.
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

    // And the catalogue pins no registry of its own, and never a tag: the
    // image is the registry module's prefix, the binary, and a digest.
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("image_digest = \"${module.registry.image_prefix}/${each.value.binary}@${lookup(var.image_digests, each.value.binary, \"\")}\""),
        "the catalogue names an image some way other than registry prefix, binary and digest"
    );
    let variables = read(CLOUD_RUN_VARIABLES);
    assert!(
        variables.contains("@sha256:[a-f0-9]{64}$"),
        "the Cloud Run module no longer refuses an image that is not pinned by digest"
    );
}

/// Files outside this change's paths that still mention the retired stack,
/// with what each is. Each is re-proved to still exist and still mention it,
/// so the list expires entry by entry rather than excusing the next mention.
const STILL_MENTIONS_THE_RETIRED_STACK: &[(&str, &str)] = &[
    // Empty since the documentation sweep that followed ADR 0024: the four
    // scripts were deleted and the five runbooks rewritten, and every entry
    // expired the way the loop below demands. The list stays so the next
    // mention has somewhere to be declared rather than somewhere to hide.
];

#[test]
fn no_kubernetes_manifest_helm_chart_or_gitops_controller_remains() {
    // The failure this prevents: a directory of manifests a reviewer takes for
    // the running system, that nothing applies. It began as a guard on an
    // empty overlays directory, and on 2026-08-31 the same failure arrived at
    // a hundred times the size when Argo CD replaced kubectl and left the
    // sed-rendered manifests behind. ADR 0024 retired the whole runtime; this
    // is what keeps it retired.
    for retired in [
        "infrastructure/kubernetes",
        "infrastructure/helm",
        "infrastructure/gitops",
    ] {
        assert!(
            !repository_root().join(retired).exists(),
            "{retired} exists again. The Kubernetes runtime was retired under ADR 0024; a \
             manifest, chart or controller directory nothing applies reads as the running \
             system."
        );
    }

    // No manifest anywhere under infrastructure or the workflows: a YAML
    // document with a top-level `kind:` is a Kubernetes object whatever
    // directory it is in.
    let mut yaml_scanned = 0usize;
    for directory in ["infrastructure", ".github"] {
        for extension in ["yaml", "yml"] {
            for path in files_with_extension(directory, extension) {
                let content = std::fs::read_to_string(&path).expect("readable");
                yaml_scanned += 1;
                assert!(
                    !content
                        .lines()
                        .any(|line| line.starts_with("kind: ") || line.starts_with("apiVersion: ")),
                    "{} is a Kubernetes manifest",
                    path.display()
                );
            }
        }
    }
    assert!(
        yaml_scanned >= 5,
        "only {yaml_scanned} YAML files were scanned"
    );

    // No workflow runs the retired tooling. Comments are stripped, because a
    // workflow explaining why it no longer runs kubectl must be allowed to
    // say so.
    for path in files_with_extension(".github/workflows", "yml") {
        let commands: String = std::fs::read_to_string(&path)
            .expect("readable")
            .lines()
            .map(|line| line.split('#').next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for tool in ["kubectl ", "helm ", "argocd ", "kargo ", "kustomize "] {
            assert!(
                !commands.contains(tool),
                "{} runs `{tool}`, which has nothing to run against",
                path.display()
            );
        }
    }

    // No GKE resource in the Terraform.
    //
    // A `removed` block is exempt and has to be: it names the type precisely
    // because it is telling Terraform to stop managing an instance of it, and
    // forbidding the name would force the alternative — leaving the resource
    // in the plan to be destroyed, which for the journal backup plan means
    // deleting the backups. Only the declaring form is refused here.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_removed_blocks(&without_comments(
            &std::fs::read_to_string(&path).expect("readable"),
        ));
        for resource in [
            "google_container_cluster",
            "google_container_node_pool",
            "google_gke_backup_backup_plan",
            "svc.id.goog",
            "kubernetes_service_account",
        ] {
            assert!(
                !content.contains(resource),
                "{} declares {resource}; the cluster is gone and this is a resource for it",
                path.display()
            );
        }
    }

    // What still mentions the retired stack outside this change's paths, each
    // re-proved so the list shrinks rather than grows.
    for (path, what) in STILL_MENTIONS_THE_RETIRED_STACK {
        let content = std::fs::read_to_string(repository_root().join(path)).unwrap_or_else(|_| {
            panic!(
                "{path} no longer exists; delete its entry from STILL_MENTIONS_THE_RETIRED_STACK"
            )
        });
        let lowered = content.to_lowercase();
        assert!(
            [
                "argocd",
                "argo cd",
                "kargo",
                "keda",
                "helm",
                "kubernetes",
                "configmap",
                "node pool",
                "statefulset"
            ]
            .iter()
            .any(|token| lowered.contains(token)),
            "{path} no longer mentions the retired stack ({what}); delete its entry so the list expires"
        );
    }
}

#[test]
fn nothing_added_here_raises_the_autonomy_ceiling_anywhere() {
    // Everything above adds workloads, identities and network paths. None of
    // it is allowed to change the one line that decides whether the platform
    // can reach a real venue — and the venue credential's IAM binding must
    // stay absent wherever the ceiling is paper trading.
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

    // Every workload reads the ceiling from the one root variable, and no
    // live rung is spelt anywhere in the catalogue.
    let catalogue = without_comments(&read(CATALOGUE));
    // Matched as a trimmed line, not a substring: `terraform fmt` pads the
    // `=` to align with the entry's longest key, so a fixed-width match
    // counts a workload only when its neighbours happen to be short.
    let workloads = catalogue_workloads();
    assert_eq!(
        workloads.len(),
        3,
        "the catalogue no longer has three entries"
    );
    let takes_the_root_ceiling = catalogue
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("QIP_AUTONOMY_CEILING")
                && line.trim_end().ends_with("= var.autonomy_ceiling")
        })
        .count();
    assert_eq!(
        takes_the_root_ceiling,
        workloads.len(),
        "not every catalogue workload takes the ceiling from var.autonomy_ceiling"
    );
    for level in [
        "supervised_live",
        "limited_autonomous_live",
        "autonomous_live",
    ] {
        assert!(
            !catalogue.contains(level),
            "catalogue.tf names {level}, a live autonomy level"
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

// --- what deploys, and what deliberately does not ---------------------------
//
// Three lists have to agree: the binaries the workspace builds, the images the
// pipeline pushes, and the workloads the catalogue and the node declare. Every
// pair of them can disagree in both directions, and each of the six failures
// is quiet:
//
//   * a binary with no image — the crate ships nowhere, and the deploy that was
//     supposed to include it succeeds;
//   * an image with no workload — a build that costs money and ships nothing,
//     and reads to a reviewer as a deployed component;
//   * a workload with no image — a revision waiting for a digest that will
//     never exist;
//   * a workload the pipeline never proves serving — a pipeline that reports
//     success for a revision that never started.
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
fn the_teardown_stops_the_meter_and_touches_nothing_that_scales_to_zero() {
    // `down` exists to stop the hourly bill between sessions. The execution
    // nodes are the only thing here that bills while idle; a Cloud Run
    // service at zero instances costs nothing and its `deletion_protection`
    // refuses a destroy in any case. A `down` that reached the services would
    // fail on that refusal — or worse, succeed after somebody relaxed it —
    // so the target is pinned and nothing else is named.
    let infra = read(".github/workflows/infra.yml");
    let down = block_under(&infra, "- name: down");
    assert!(
        down.contains("terraform -chdir=infrastructure/terraform destroy"),
        "infra.yml's down no longer destroys anything"
    );
    assert!(
        down.contains("-target=module.execution_node"),
        "infra.yml's down is not targeted at the execution nodes"
    );
    for forbidden in [
        "module.cloud_run",
        "module.secrets",
        "module.cicd",
        "module.evidence",
        "module.egress_proxy",
    ] {
        assert!(
            !down.contains(forbidden),
            "infra.yml's down names {forbidden}, which either scales to zero or must never be torn down"
        );
    }
    // And `up` passes the digests the pipeline recorded, when the environment
    // has any, so an apply after a deploy does not plan a service at nothing.
    assert!(
        infra.contains("images.tfvars"),
        "infra.yml no longer passes images.tfvars, so every plan refuses on a missing digest"
    );
}

/// The workloads deploy.yml moves: read from the same catalogue the workflow
/// itself parses, by the same rule, so the two cannot disagree about which
/// services exist.
fn workloads_the_pipeline_moves() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    // The workflow reads catalogue.tf with these two patterns. If either
    // changes here, the awk in the workflow has changed shape and the
    // catalogue's own tests need re-reading.
    for pattern in [
        "/^    [a-z][a-z0-9-]* = \\{$/",
        "/^      binary[[:space:]]*=/",
    ] {
        assert!(
            deploy.contains(pattern),
            "deploy.yml no longer reads the catalogue with `{pattern}`; the services it moves are now decided somewhere this check cannot see"
        );
    }
    catalogue_workloads()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn a_promotion_names_who_verifies_it() {
    // An apply that returns is not a deployment that worked. The GitOps
    // cut-over lost the rollout wait, so a build that crash-looped on boot
    // produced a green pipeline, and docs/operations/gitops-exceptions.md
    // recorded the gap rather than closing it. `gcloud run services update`
    // blocks until the revision is Ready and routing and fails otherwise;
    // the describe afterwards proves the serving revision runs the digest
    // that was signed. Both halves are asserted, because the first alone is
    // satisfied by a traffic split that never moved.
    let deploy = read(".github/workflows/deploy.yml");
    let jobs = workflow_jobs(&deploy);
    let (_, body) = jobs
        .iter()
        .find(|(name, _)| name == "deploy")
        .expect("deploy.yml has a deploy job");
    let steps = job_steps(body);
    let rollout = steps
        .iter()
        .find(|step| step.contains("id: rollout"))
        .expect("the deploy job has a rollout step");

    // The premise: it moves every workload the catalogue deploys, and nothing
    // the catalogue does not know.
    let moved = workloads_the_pipeline_moves();
    assert_eq!(moved.len(), 3, "the catalogue parsed to {moved:?}");
    assert!(
        rollout.contains("service=\"qip-${TARGET_ENVIRONMENT}-${name}\""),
        "the rollout step no longer names services the way modules/cloudrun names them"
    );

    // The wait.
    assert!(
        rollout.contains("gcloud run services update \"$service\""),
        "the rollout step no longer moves the service with `gcloud run services update`, which is the step that waits for the revision"
    );
    assert!(
        rollout.contains("--image \"$image\""),
        "the rollout step moves the service to something other than the attested image"
    );
    // By digest: the image is assembled from the registry's own digest, never
    // from the tag the build pushed.
    assert!(
        rollout.contains("image=\"${prefix}/${binary}@${digest}\""),
        "the rollout step deploys by tag rather than by the digest the attestation names"
    );

    // Every global flag precedes `--container`, which opens a scope only
    // container-level flags may enter. This is not style: with `--quiet`
    // after it, gcloud refused the whole command — `unrecognized arguments:
    // --quiet` — and every deployment between the sidecars landing and the
    // fix failed there, run 33636602162 among them.
    let update = rollout
        .split("gcloud run services update")
        .nth(1)
        .expect("the rollout step runs `gcloud run services update`");
    let update = update.split("\n\n").next().unwrap_or(update);
    let container_at = update
        .find("--container")
        .expect("the update names the container it moves");
    for global in ["--quiet", "--project", "--region"] {
        let at = update
            .find(global)
            .unwrap_or_else(|| panic!("the update no longer passes {global}"));
        assert!(
            at < container_at,
            "the update passes {global}, a global flag, after --container; gcloud refuses that"
        );
    }

    // The proof. Read by name, never by position: every service carries the
    // egress sidecar and may carry the metrics collector, so an index picks
    // a sidecar as readily as the workload, and nothing documents the first
    // condition as Ready. And the revisions read are the ones traffic
    // routes to — `spec.template` is what was asked for, which is the half
    // a traffic split that never moved would also satisfy.
    // Comment lines go first: the step's own comment names both positional
    // selectors in order to say why they are wrong, and a check that read
    // prose would refuse the explanation along with the defect.
    let rollout_code: String = rollout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rollout_code.contains("containers[0]") && !rollout_code.contains("conditions[0]"),
        "the rollout step selects a container or a condition by position again"
    );
    assert!(
        rollout.contains("c.get(\"type\") == \"Ready\""),
        "the rollout step does not select the Ready condition by type"
    );
    assert!(
        rollout.contains("c.get(\"name\") == wanted"),
        "the rollout step does not select the workload's own container by name"
    );
    assert!(
        rollout.contains("status.get(\"traffic\", [])"),
        "the rollout step does not read the revisions traffic actually routes to"
    );
    assert!(
        rollout.contains("if [ \"$serving\" != \"$image\" ]; then") && rollout.contains("exit 1"),
        "the rollout step reads the serving revision back and does not fail when it is not the one asked for"
    );

    // And the record is written after the proof, not before it: a digest
    // recorded for a service that never served it is the GitOps values file
    // all over again.
    let proof = rollout.find("exit 1").expect("the proof refuses");
    let record = rollout
        .find(">> \"$images_file\"")
        .expect("the step records the digest");
    assert!(
        proof < record,
        "the rollout step records a digest before it has proven the service serves it"
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
