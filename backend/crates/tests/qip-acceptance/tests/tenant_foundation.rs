//! The tenant foundation (ADR 0118): `infrastructure/foundation`, the root the
//! owner applies once, and `scripts/bootstrap-foundation.sh`, the one command
//! that applies it.
//!
//! The Terraform half is planned by `infrastructure/foundation/tests/`; what
//! lives here is what a plan cannot see — agreement with the platform root,
//! what the committed files must never hold, and how the script behaves before
//! it touches anything.

use qip_acceptance::{read, repository_root};

const FOUNDATION_VARIABLES: &str = "infrastructure/foundation/variables.tf";
const FOUNDATION_TFVARS: &str = "infrastructure/foundation/terraform.tfvars";
const SERVICES_MODULE: &str = "infrastructure/terraform/modules/services/main.tf";
const SCRIPT: &str = "scripts/bootstrap-foundation.sh";

/// The quoted `*.googleapis.com` names between a line opening `opening` and
/// the first line after it that closes the block with `closing`.
fn apis_in_block(text: &str, opening: &str, closing: &str) -> Vec<String> {
    let mut apis = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !inside {
            inside = trimmed.starts_with(opening);
            continue;
        }
        if trimmed == closing {
            break;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('"')
            && let Some((name, _)) = rest.split_once('"')
            && name.ends_with(".googleapis.com")
        {
            apis.push(name.to_string());
        }
    }
    apis.sort();
    apis
}

/// The foundation enables, in every environment project, exactly the APIs the
/// platform root's `modules/services` enables unconditionally.
///
/// Two lists of one fact. `bootstrap-deploy.sh` runs before the platform
/// root's first apply and needs IAM, STS and Storage on in a project the
/// foundation just created; an API the root later starts needing
/// unconditionally and the foundation does not enable is a first apply that
/// stops on `SERVICE_DISABLED` partway through, which is the failure
/// `modules/services` was written to end. An API here and not there is one the
/// platform never asked for, enabled in every project for nobody.
#[test]
fn the_foundation_enables_exactly_the_platform_roots_unconditional_apis() {
    let platform = apis_in_block(&read(SERVICES_MODULE), "always = {", "}");
    let foundation = apis_in_block(
        &read(FOUNDATION_VARIABLES),
        "variable \"platform_apis\"",
        "}",
    );
    // Premise: both parses found a list, so equality is not two empties.
    assert!(
        platform.len() >= 10 && foundation.len() >= 10,
        "the parse found {} platform APIs and {} foundation APIs; one of the two blocks moved \
         and this test would compare nothing",
        platform.len(),
        foundation.len()
    );
    assert_eq!(
        foundation, platform,
        "{FOUNDATION_VARIABLES}'s platform_apis and {SERVICES_MODULE}'s `always` set disagree"
    );
}

/// The committed foundation inputs hold no account identifier, and prod is
/// declared and off.
///
/// ORG_ID, BILLING_ACCOUNT_ID and the prefix are runtime inputs: an
/// organisation id or a billing account id in the tree is an account
/// identifier published with every clone (00-enterprise-governance.md). And a
/// prod project created by default would be the one environment every
/// workflow refuses, holding a billing link nobody uses.
#[test]
fn the_foundation_commits_no_account_identifier_and_keeps_prod_off() {
    let tfvars = read(FOUNDATION_TFVARS);
    let live: Vec<&str> = tfvars
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    // Premise: the file has assignments, so "no forbidden key" is not about
    // an empty file.
    assert!(
        live.iter().any(|line| line.starts_with("environments")),
        "{FOUNDATION_TFVARS} no longer declares `environments`"
    );
    for key in ["org_id", "billing_account_id", "prefix"] {
        assert!(
            !live
                .iter()
                .any(|line| line.split('=').next().map(str::trim) == Some(key)),
            "{FOUNDATION_TFVARS} commits `{key}`, a runtime input that must never be in the tree"
        );
    }

    // A billing account id has a fixed shape; none may appear in the root's
    // configuration or its committed values. The test harness's invented one
    // lives under `tests/` and is excluded by walking only the root's files.
    for file in [
        "infrastructure/foundation/main.tf",
        "infrastructure/foundation/variables.tf",
        "infrastructure/foundation/outputs.tf",
        "infrastructure/foundation/versions.tf",
        FOUNDATION_TFVARS,
    ] {
        let text = read(file);
        let bytes = text.as_bytes();
        let shaped = (0..bytes.len().saturating_sub(19)).any(|start| {
            let window = &bytes[start..start + 20];
            window.iter().enumerate().all(|(index, byte)| match index {
                6 | 13 => *byte == b'-',
                _ => byte.is_ascii_digit() || (b'A'..=b'F').contains(byte),
            })
        });
        assert!(!shaped, "{file} contains a billing-account-shaped id");
    }

    let prod = live
        .iter()
        .find(|line| line.starts_with("prod "))
        .unwrap_or_else(|| panic!("{FOUNDATION_TFVARS} does not declare prod"));
    let collapsed: String = prod.split_whitespace().collect();
    assert!(
        collapsed.contains("enabled=false"),
        "{FOUNDATION_TFVARS} turns prod on: {prod}"
    );
    let dev = live
        .iter()
        .find(|line| line.starts_with("dev "))
        .unwrap_or_else(|| panic!("{FOUNDATION_TFVARS} does not declare dev"));
    assert!(
        dev.split_whitespace()
            .collect::<String>()
            .contains("enabled=true"),
        "{FOUNDATION_TFVARS} turns dev off, so the owner's one command creates no project: {dev}"
    );
}

/// The script never creates a key, never auto-approves, never pushes, and
/// applies only the plan it showed.
///
/// Read on its executable lines: comments and `echo`s may name what it does
/// not do, and the instructions it prints for the owner name `git push`.
#[test]
fn the_foundation_script_never_mints_a_key_never_auto_approves_and_applies_only_the_plan_it_showed()
{
    let source = read(SCRIPT);
    let executable: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("echo"))
        .collect();
    // Premise: there is a script here at all.
    assert!(
        executable.len() > 50,
        "{SCRIPT} has {} executable lines",
        executable.len()
    );
    for forbidden in ["keys create", "auto-approve", "git push", "git commit"] {
        let hits: Vec<&&str> = executable
            .iter()
            .filter(|line| line.contains(forbidden))
            .collect();
        assert!(hits.is_empty(), "{SCRIPT} runs `{forbidden}`: {hits:?}");
    }
    let applies: Vec<&&str> = executable
        .iter()
        .filter(|line| line.split_whitespace().any(|word| word == "apply"))
        .collect();
    assert_eq!(
        applies,
        vec![&"tf apply -input=false \"${plan_file}\""],
        "{SCRIPT} applies something other than the saved plan it showed"
    );
    let confirm = source
        .find("confirm \"Apply exactly this plan?\"")
        .unwrap_or_else(|| panic!("{SCRIPT} applies without asking"));
    let apply = source
        .find("tf apply -input=false \"${plan_file}\"")
        .expect("the apply line was found above");
    let plan = source
        .find("tf plan -input=false")
        .unwrap_or_else(|| panic!("{SCRIPT} never shows a plan"));
    assert!(
        plan < confirm && confirm < apply,
        "{SCRIPT} must plan, then ask, then apply — in that order"
    );
}

fn on_path(tool: &str) -> std::path::PathBuf {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .filter(|directory| !directory.is_empty())
        .map(|directory| std::path::Path::new(directory).join(tool))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("`{tool}` is not on PATH, and this test drives a shell script"))
}

/// The script refuses malformed arguments before it looks for a single tool,
/// and admits well-formed ones as far as the tool check — driven, not read.
///
/// The child's PATH holds only what the script uses before that check
/// (`dirname`, `sed`, `head`, `bash`), so `missing: gcloud …` is the
/// deterministic end of every admitted case and this test cannot reach a
/// cloud API on a machine that has `gcloud` installed. The shapes are the
/// foundation root's own validations; a script wider than them would create a
/// seed project and a bucket before Terraform refused the same prefix.
#[test]
fn the_foundation_script_refuses_malformed_arguments_before_any_tool_and_admits_well_formed_ones() {
    let bash = on_path("bash");
    let workspace = std::env::temp_dir().join(format!("qip-foundation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    let bin = workspace.join("bin");
    std::fs::create_dir_all(&bin).expect("the sandboxed bin is creatable");
    for tool in ["dirname", "sed", "head", "bash"] {
        std::os::unix::fs::symlink(on_path(tool), bin.join(tool))
            .unwrap_or_else(|error| panic!("cannot place {tool} in the sandboxed PATH: {error}"));
    }
    let sandboxed = bin.display().to_string();
    let root = repository_root();
    let script = root.join(SCRIPT);
    let run = |arguments: &[&str]| {
        let output = std::process::Command::new(&bash)
            .arg(&script)
            .args(arguments)
            .current_dir(&root)
            .env_clear()
            .env("PATH", &sandboxed)
            .env("HOME", &sandboxed)
            .output()
            .unwrap_or_else(|error| panic!("{SCRIPT} could not be run: {error}"));
        let mut printed = String::from_utf8_lossy(&output.stdout).to_string();
        printed.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.code().unwrap_or(-1), printed)
    };

    const BILLING: &str = "0A1B2C-3D4E5F-6A7B8C";
    let refused: [(&[&str], &str); 7] = [
        (&[], "no arguments"),
        (&["none", BILLING, "qip-"], "a prefix ending in a hyphen"),
        (
            &["none", BILLING, "abcdefghijklmnopqrstuvwxy"],
            "a 25-character prefix: <prefix>-stage is 31",
        ),
        (&["none", BILLING, "1qip"], "a prefix starting with a digit"),
        (
            &["none", "0a1b2c-3d4e5f-6a7b8c", "algorik-qip"],
            "a lower-case billing account",
        ),
        (
            &[
                "none",
                "billingAccounts/0A1B2C-3D4E5F-6A7B8C",
                "algorik-qip",
            ],
            "a prefixed billing account",
        ),
        (
            &["organizations/123", BILLING, "algorik-qip"],
            "a prefixed organisation id",
        ),
    ];
    for (arguments, why) in refused {
        let (code, printed) = run(arguments);
        assert!(
            code == 64 || code == 65,
            "{SCRIPT} did not refuse {why} as a usage or data error; it exited {code}: {printed}"
        );
        assert!(
            !printed.contains("missing:"),
            "{SCRIPT} reached the tool check with {why}; the refusal ran after work, not before: \
             {printed}"
        );
    }

    // Admission, both modes, against the committed tree. A guard that refused
    // everything would pass every case above.
    for org in ["none", "123456789012"] {
        let (code, printed) = run(&[org, BILLING, "algorik-qip"]);
        assert_eq!(
            code, 69,
            "{SCRIPT} did not stop at the tool check for well-formed arguments (org {org}); a \
             guard that refuses everything is not a guard. It printed: {printed}"
        );
        assert!(
            printed.contains("missing:") && printed.contains("gcloud"),
            "{SCRIPT} admitted org {org} and did not report the missing tools: {printed}"
        );
    }

    let _ = std::fs::remove_dir_all(&workspace);
}
