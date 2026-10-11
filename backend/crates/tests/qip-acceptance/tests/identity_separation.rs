//! Which identity may do what, read from the committed Terraform.
//!
//! Two separations of blueprint §15 that no single module can see, because
//! each is a property of every IAM grant in the tree at once:
//!
//! * R12-SEC-9139 — compromise of a trading service must not yield
//!   deployment-signing authority. No workload identity may hold a role that
//!   signs, attests or admits an image, or that could grant itself one.
//! * R12-SEC-9138 — compromise of an engineering agent must not yield
//!   authority to move capital. The identities development runs as (the
//!   pipeline, the infrastructure account, the build pools and the agent
//!   identity) hold no grant on a secret, so none of them can read the venue
//!   credential or the capital envelope key a workload signs with.
//!
//! Like `security_controls.rs`, these read configuration rather than a
//! deployed project: they prove the grant is not declared, not that nobody
//! added one by hand. The org policies refusing service-account keys
//! (`main.tf`) are what make "declared" and "held" the same for these
//! accounts.
#![allow(clippy::panic_in_result_fn)]

/// Every committed `.tf` file under `infrastructure/terraform`, comments
/// removed so a comment naming a role is not mistaken for a grant of it.
fn terraform_tree() -> Vec<(String, String)> {
    let root = qip_acceptance::repository_root();
    qip_acceptance::files_with_extension("infrastructure/terraform", "tf")
        .into_iter()
        .filter(|path| {
            !path
                .components()
                .any(|part| part.as_os_str() == ".terraform")
        })
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
            let text = text
                .lines()
                .map(|line| line.split('#').next().unwrap_or("").trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            (relative, text)
        })
        .collect()
}

/// The text between the braces of the block that opens at or after `from`.
fn braced(text: &str, from: usize) -> Option<&str> {
    let open = from + text[from..].find('{')?;
    let mut depth = 0usize;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every IAM grant resource in the tree, as (file, kind, name, body).
///
/// A grant is any resource whose kind ends in `_iam_member` or
/// `_iam_binding`; the body is the text between its own braces, so a
/// neighbouring resource's role cannot satisfy or fail a check on this one.
fn grants() -> Vec<(String, String, String, String)> {
    let mut found = Vec::new();
    for (file, text) in terraform_tree() {
        for (at, _) in text.match_indices("resource \"") {
            let after = at + "resource \"".len();
            let Some(kind) = text[after..].split('"').next() else {
                continue;
            };
            if !(kind.ends_with("_iam_member") || kind.ends_with("_iam_binding")) {
                continue;
            }
            let rest = &text[after + kind.len() + 1..];
            let Some(name) = rest
                .trim_start()
                .strip_prefix('"')
                .and_then(|r| r.split('"').next())
            else {
                continue;
            };
            let Some(body) = braced(&text, after) else {
                continue;
            };
            found.push((
                file.clone(),
                kind.to_string(),
                name.to_string(),
                body.to_string(),
            ));
        }
    }
    found
}

/// The principal expressions a grant body names: the right-hand side of a
/// `member =` line, or every element of a `members = [...]` list.
fn principals(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in body.match_indices("member") {
        let rest = &body[at + "member".len()..];
        let (rest, list) = match rest.strip_prefix('s') {
            Some(after) => (after, true),
            None => (rest, false),
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        if list {
            let value = value.trim_start();
            let end = value.find(']').unwrap_or(value.len());
            found.extend(
                value[..end]
                    .trim_start_matches('[')
                    .split(',')
                    .map(|element| element.trim().to_string())
                    .filter(|element| !element.is_empty()),
            );
        } else {
            found.push(value.lines().next().unwrap_or("").trim().to_string());
        }
    }
    found
}

/// Roles that sign or attest an image, admit one, or can grant themselves
/// either. `projectIamAdmin`, `owner` and `editor` are here because an
/// identity that can rewrite the project's IAM holds every role in this list
/// one API call later.
const DEPLOYMENT_SIGNING_ROLES: &[&str] = &[
    "\"roles/cloudkms.signer\"",
    "\"roles/cloudkms.signerVerifier\"",
    "\"roles/cloudkms.cryptoOperator\"",
    "\"roles/cloudkms.admin\"",
    "\"roles/binaryauthorization.attestorsAdmin\"",
    "\"roles/binaryauthorization.attestorsEditor\"",
    "\"roles/binaryauthorization.policyAdmin\"",
    "\"roles/binaryauthorization.policyEditor\"",
    "\"roles/containeranalysis.notes.attacher\"",
    "\"roles/containeranalysis.occurrences.editor\"",
    "\"roles/containeranalysis.admin\"",
    "\"roles/iam.serviceAccountTokenCreator\"",
    "\"roles/iam.serviceAccountKeyAdmin\"",
    "\"roles/iam.securityAdmin\"",
    "\"roles/resourcemanager.projectIamAdmin\"",
    "\"roles/owner\"",
    "\"roles/editor\"",
];

/// The identities development and delivery run as. Every one of them is
/// impersonated through workload identity federation or runs a build; none
/// serves a request or holds a position.
const PIPELINE_IDENTITIES: &[&str] = &[
    "var.ci_service_account",
    "google_service_account.ci.email",
    "google_service_account.infra.email",
    "var.infra_service_account",
    "google_service_account.build.email",
    "google_service_account.builder.email",
    "google_service_account.agent_identity.email",
    "agent_identity_email",
];

/// The workload identities a trading or serving process runs as.
const WORKLOAD_IDENTITIES: &[&str] = &[
    "google_service_account.workload.email",
    "google_service_account.node.email",
    "google_service_account.ledgerd.email",
    "google_service_account.fabricd",
    "google_service_account.console",
    "google_service_account.landing",
    "var.venue_credential_reader",
    "var.training_service_account",
    "console_service_account_email",
];

fn names_a_pipeline_identity(principal: &str) -> bool {
    PIPELINE_IDENTITIES
        .iter()
        .any(|identity| principal.contains(identity))
}

#[test]
fn no_workload_identity_holds_a_role_that_signs_attests_or_admits_a_deployment() {
    // R12-SEC-9139. The attestor key signs the attestation Binary
    // Authorization admits an image on. A trading service that could sign
    // with it could put its own build on every Cloud Run service and the
    // execution node, so the property is that only the pipeline's
    // identities appear on any grant carrying a signing role.
    let all = grants();
    let signing: Vec<_> = all
        .iter()
        .filter(|(_, _, _, body)| {
            DEPLOYMENT_SIGNING_ROLES
                .iter()
                .any(|role| body.contains(role))
        })
        .collect();

    // Premise: the grant that actually signs exists and is seen, so an empty
    // `signing` list cannot pass this by finding nothing to check.
    assert!(
        signing.iter().any(|(file, _, name, body)| {
            file.ends_with("modules/binaryauthorization/main.tf")
                && name == "ci_signs"
                && body.contains("\"roles/cloudkms.signerVerifier\"")
        }),
        "premise: the attestor key's signer grant (binaryauthorization ci_signs) was not \
         found, so this test is reading the wrong tree"
    );

    for (file, kind, name, body) in &signing {
        let named = principals(body);
        assert!(
            !named.is_empty(),
            "{file}: {kind}.{name} carries a signing role but names no member this test can \
             read; a grant whose principal cannot be checked is not proven safe"
        );
        for principal in named {
            assert!(
                !WORKLOAD_IDENTITIES
                    .iter()
                    .any(|identity| principal.contains(identity)),
                "{file}: {kind}.{name} grants a deployment-signing role to workload identity \
                 {principal}; a compromised trading service could then sign and deploy its \
                 own image"
            );
            assert!(
                names_a_pipeline_identity(&principal),
                "{file}: {kind}.{name} grants a deployment-signing role to {principal}, which \
                 is not one of the pipeline identities; only the pipeline signs"
            );
        }
    }
}

#[test]
fn only_this_repositorys_github_tokens_can_act_as_the_signing_identities() {
    // R12-SEC-9139, the other door. A workload that can impersonate the
    // pipeline's account holds whatever that account holds; the grant that
    // would let it is `workloadIdentityUser`, `serviceAccountUser` or
    // `serviceAccountTokenCreator` on the pipeline account itself.
    let impersonation: Vec<_> = grants()
        .into_iter()
        .filter(|(_, kind, _, body)| {
            kind == "google_service_account_iam_member"
                && (body.contains("google_service_account.ci.name")
                    || body.contains("google_service_account.infra.name"))
        })
        .collect();
    assert!(
        impersonation.len() >= 2,
        "premise: the pipeline and infrastructure accounts' impersonation grants were not \
         both found ({} seen)",
        impersonation.len()
    );
    for (file, _, name, body) in impersonation {
        for principal in principals(&body) {
            assert!(
                principal.starts_with("\"principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.repository/"),
                "{file}: {name} lets {principal} act as a pipeline account; only this \
                 repository's federated GitHub tokens may"
            );
        }
    }
}

#[test]
fn no_development_or_agent_identity_holds_a_grant_on_any_secret() {
    // R12-SEC-9138. Capital moves on two secrets: the venue credential an
    // order is placed with and the capital envelope key the execution node
    // verifies a grant with. Both are read only by the workload that uses
    // them. An engineering agent runs as one of the pipeline identities, so
    // the property is that none of those identities appears on any secret
    // grant at all — not only on those two, because a secret added later is
    // a secret nobody remembered to list here.
    // Terraform addresses and principals, never secret values; a `secret_`
    // name made CodeQL's name heuristic treat the messages below as leaks.
    let manager_grants: Vec<_> = grants()
        .into_iter()
        .filter(|(_, kind, _, _)| kind.starts_with("google_secret_manager_secret_iam"))
        .collect();

    for (expected_file, expected_name) in [
        ("modules/execution-node/main.tf", "capital_envelope_key"),
        ("modules/execution-node/main.tf", "venue_credential"),
        ("modules/secrets/main.tf", "venue_credential"),
    ] {
        assert!(
            manager_grants
                .iter()
                .any(|(file, _, name, _)| file.ends_with(expected_file) && name == expected_name),
            "premise: the capital-moving secret grant {expected_file} {expected_name} was not \
             found, so its reader cannot be checked"
        );
    }

    for (file, kind, name, body) in &manager_grants {
        for principal in principals(body) {
            assert!(
                !names_a_pipeline_identity(&principal),
                "{file}: {kind}.{name} lets development identity {principal} read a secret; \
                 a compromised engineering agent would then hold whatever that secret moves"
            );
        }
    }
}

#[test]
fn the_agent_identity_and_build_identities_hold_no_project_role_over_secrets_or_keys() {
    // R12-SEC-9138, the project-level half. A project-wide
    // `secretmanager.secretAccessor` reaches every secret without naming one,
    // so the per-secret test above cannot see it. The infrastructure account
    // is the one exception and is stated rather than hidden: it holds
    // `secretmanager.admin` and `cloudkms.admin` because it creates those
    // resources, it is reachable only from `infra.yml` on manual dispatch,
    // and `modules/cicd/main.tf` records that cost. Every other development
    // identity — the deploy pipeline, the build pools and the agent identity
    // — must hold neither.
    let project_grants: Vec<_> = grants()
        .into_iter()
        .filter(|(_, kind, _, _)| kind == "google_project_iam_member")
        .collect();
    assert!(
        project_grants
            .iter()
            .any(|(_, _, name, body)| name == "deploy"
                && body.contains("google_service_account.ci.email")),
        "premise: the deploy pipeline's project grant was not found"
    );
    for (file, _, name, body) in &project_grants {
        let named = principals(body);
        let development = named.iter().any(|principal| {
            names_a_pipeline_identity(principal)
                && !principal.contains("google_service_account.infra.email")
                && !principal.contains("var.infra_service_account")
        });
        if !development {
            continue;
        }
        for forbidden in [
            "\"roles/secretmanager.",
            "\"roles/cloudkms.admin\"",
            "\"roles/cloudkms.cryptoKeyDecrypter\"",
            "\"roles/cloudkms.cryptoKeyEncrypterDecrypter\"",
            "\"roles/owner\"",
            "\"roles/editor\"",
        ] {
            assert!(
                !body.contains(forbidden),
                "{file}: {name} grants a development identity ({named:?}) a project-wide \
                 {forbidden} role, which reaches every secret or key without naming one"
            );
        }
    }
}

/// The body of `variable "<name>"` in comment-free Terraform text.
fn variable_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("variable \"{name}\""))?;
    braced(text, at)
}

#[test]
fn every_workforce_access_list_admits_only_managed_cloud_identity_users_and_groups() {
    // R12-SEC-10002. People reach the console and the GitOps controllers
    // through IAP, and the list of who may is a variable. A `domain:` entry
    // admits every account that domain will ever mint and a
    // `serviceAccount:` entry is a workload wearing a person's door; the
    // validation each list carries refuses both, and `allUsers`, before a
    // plan exists. The module harness (`modules/iap-run/tests`) proves it
    // fires and admits; this proves no access list ships without it.
    let tree = terraform_tree();
    let lists = [
        (
            "infrastructure/terraform/modules/iap-run/variables.tf",
            "iap_members",
        ),
        (
            "infrastructure/terraform/modules/gitops-gateway/variables.tf",
            "iap_members",
        ),
        (
            "infrastructure/terraform/variables.tf",
            "gitops_iap_members",
        ),
    ];
    for (path, name) in lists {
        let text = &tree
            .iter()
            .find(|(file, _)| file == path)
            .unwrap_or_else(|| panic!("premise: {path} is not in the tree"))
            .1;
        let body = variable_body(text, name)
            .unwrap_or_else(|| panic!("premise: {path} declares no variable {name}"));
        let compact: String = body.split_whitespace().collect();
        assert!(
            compact.contains(&format!(
                "formemberinvar.{name}:can(regex(\"^(user|group):[^@:[:space:]]+@[^@:[:space:]]+$\",member))"
            )),
            "{path}: {name} does not require every member to be a Cloud Identity user: or \
             group: principal, so a domain-wide or service-account grant would plan"
        );
    }

    // And nothing committed grants a human surface to everyone by literal.
    for (file, _, name, body) in grants() {
        for principal in principals(&body) {
            for open in ["\"allUsers\"", "\"allAuthenticatedUsers\"", "\"domain:"] {
                assert!(
                    !principal.starts_with(open),
                    "{file}: {name} grants {principal} by literal; workforce access is a \
                     managed user or group"
                );
            }
        }
    }
}
