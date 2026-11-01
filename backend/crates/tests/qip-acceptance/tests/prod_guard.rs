//! Production is reachable from no workflow trigger (R12 Decision 6).
//!
//! `.claude/rules/02-change-management.md` says `prod` is refused by both
//! `infra.yml` and the deploy gate. `deploy.yml` listed `prod` as a
//! `workflow_dispatch` choice and its refusal skipped `workflow_dispatch`, so
//! a human dispatch was gated only by the `prod` GitHub environment's
//! required reviewers: a repository setting no file in this tree can create
//! or read. The reading taken is the stricter one: the gate refuses prod on
//! every trigger, a dispatch included, and prod is not a dispatch choice in
//! any workflow. Lifting it is an ADR (0024 decision 3, 0036 decision 6).
//!
//! The tests read the workflows as text (no YAML parser is admitted here) and
//! drive the refusal scripts under bash, because a test that matches a
//! spelling pins the spelling and not the behaviour.

use qip_acceptance::{files_with_extension, read};

const WORKFLOWS: &str = ".github/workflows";

/// `(file name, text)` for every workflow.
fn workflows() -> Vec<(String, String)> {
    let found: Vec<(String, String)> = files_with_extension(WORKFLOWS, "yml")
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            (name.clone(), read(&format!("{WORKFLOWS}/{name}")))
        })
        .collect();
    // Premise: the walk saw the workflows this suite is about.
    for needed in ["deploy.yml", "infra.yml", "vendor.yml", "ci.yml"] {
        assert!(
            found.iter().any(|(name, _)| name == needed),
            "{WORKFLOWS}/{needed} was not found; the walk sees no workflows"
        );
    }
    found
}

/// The `on:` block: from the `on:` line to the next top-level key.
fn on_block(text: &str) -> String {
    let mut lines = text.lines().skip_while(|line| *line != "on:");
    let mut block = Vec::new();
    assert!(lines.next().is_some(), "no top-level `on:` key");
    for line in lines {
        if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#') {
            break;
        }
        block.push(line);
    }
    block.join("\n")
}

/// The trigger names: keys at two spaces of indentation in the `on:` block.
fn triggers(text: &str) -> Vec<String> {
    on_block(text)
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.trim().strip_suffix(':').map(str::to_string))
        .collect()
}

/// Every value listed under an `options:` key in the `on:` block.
fn dispatch_options(text: &str) -> Vec<String> {
    let block = on_block(text);
    let mut options = Vec::new();
    let mut inside: Option<usize> = None;
    for line in block.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "options:" {
            inside = Some(indent);
            continue;
        }
        if let Some(at) = inside {
            if let Some(item) = trimmed.strip_prefix("- ") {
                options.push(item.trim().trim_matches('"').to_string());
                continue;
            }
            if indent <= at {
                inside = None;
            }
        }
    }
    options
}

/// The text of the step whose `- name:` line is `name`, up to the next step.
fn step(text: &str, name: &str) -> String {
    let marker = format!("- name: {name}");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start() == marker)
        .unwrap_or_else(|| panic!("no step named {name:?}"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let mut end = lines.len();
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        let here = line.len() - line.trim_start().len();
        if !line.trim().is_empty() && here <= indent {
            end = offset;
            break;
        }
    }
    lines[start..end].join("\n")
}

/// The `run: |` script of a step, dedented, comments kept.
fn script(step_text: &str) -> String {
    let body = step_text
        .split_once("run: |\n")
        .unwrap_or_else(|| panic!("the step has no `run: |` block:\n{step_text}"))
        .1;
    let indent = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .unwrap_or_default();
    body.lines()
        .map(|line| {
            if line.len() >= indent {
                &line[indent..]
            } else {
                ""
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run a refusal script with `TARGET_ENVIRONMENT` set; exit status and output.
fn run(script: &str, environment: &str, tag: &str) -> (i32, String) {
    let path = std::env::temp_dir().join(format!(
        "qip-prod-guard-{}-{tag}-{}.sh",
        std::process::id(),
        environment.len()
    ));
    std::fs::write(&path, script)
        .unwrap_or_else(|error| panic!("the probe script is not writable: {error}"));
    let output = std::process::Command::new("bash")
        .arg(&path)
        .env("TARGET_ENVIRONMENT", environment)
        .output()
        .unwrap_or_else(|error| panic!("bash could not be run: {error}"));
    let _ = std::fs::remove_file(&path);
    let mut printed = String::from_utf8_lossy(&output.stdout).to_string();
    printed.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), printed)
}

#[test]
fn no_workflow_offers_prod_as_a_dispatch_choice() {
    // The first layer: nobody is offered the button. Every workflow, not a
    // list of the ones known today, so a new workflow that takes an
    // environment choice cannot add prod unseen.
    let all = workflows();
    let mut seen_choice_lists = 0usize;
    for (name, text) in &all {
        let options = dispatch_options(text);
        if !options.is_empty() {
            seen_choice_lists += 1;
        }
        assert!(
            !options.iter().any(|option| option == "prod"),
            "{name} offers `prod` as a dispatch choice ({options:?}); production is changed by a \
             person from the bootstrap script and by no workflow (ADR 0024 decision 3, ADR 0036 \
             decision 6)"
        );
    }
    // Premise: the parser found choice lists at all, and dev is in deploy's.
    assert!(
        seen_choice_lists >= 4,
        "only {seen_choice_lists} workflows show an `options:` list"
    );
    let deploy = all.iter().find(|(n, _)| n == "deploy.yml").map(|(_, t)| t);
    let options = dispatch_options(deploy.unwrap_or_else(|| panic!("deploy.yml is present")));
    assert!(
        options.iter().any(|option| option == "dev"),
        "deploy.yml's options parse as {options:?}; this test cannot see its choices"
    );
}

#[test]
fn only_the_triggers_that_exist_today_can_reach_an_environment() {
    // Every trigger path, enumerated. A new trigger on a workflow that takes
    // an environment is a new way to reach prod, and has to be read by a
    // person before it is admitted here.
    let all = workflows();
    let expected: &[(&str, &[&str])] = &[
        ("deploy.yml", &["workflow_run", "workflow_dispatch"]),
        ("infra.yml", &["workflow_dispatch"]),
        ("vendor.yml", &["push", "workflow_dispatch"]),
        ("image.yml", &["workflow_dispatch"]),
        ("fleet.yml", &["workflow_dispatch"]),
        ("model-pack-publish.yml", &["workflow_dispatch"]),
    ];
    for (file, wanted) in expected {
        let text = &all
            .iter()
            .find(|(name, _)| name == file)
            .unwrap_or_else(|| panic!("{file} is missing"))
            .1;
        let mut found = triggers(text);
        found.sort();
        let mut wanted: Vec<String> = wanted.iter().map(|t| (*t).to_string()).collect();
        wanted.sort();
        assert_eq!(
            found, wanted,
            "{file} is triggered by {found:?}; each added trigger is a path to an environment \
             that this suite has not read. Extend it only with a refusal of prod on that path."
        );
    }
    for (name, text) in &all {
        let found = triggers(text);
        assert!(
            !found.iter().any(|t| t == "repository_dispatch"),
            "{name} listens for repository_dispatch, which anyone holding a token can fire"
        );
        assert!(
            !found.iter().any(|t| t == "pull_request_target"),
            "{name} runs on pull_request_target with the base repository's secrets"
        );
        if found.iter().any(|t| t == "workflow_call") {
            assert!(
                !text.contains("inputs.environment") && !text.contains("TARGET_ENVIRONMENT"),
                "{name} is reusable and takes an environment; a caller can then pass prod"
            );
        }
        // A scheduled or pull-request run has no human to refuse, so it must
        // not take an environment at all.
        if found.iter().any(|t| t == "schedule" || t == "pull_request") {
            assert!(
                !text.contains("TARGET_ENVIRONMENT"),
                "{name} runs without a person and names a target environment"
            );
        }
    }
    // Premise for the filter above: the enumeration is not vacuous.
    assert!(
        all.iter()
            .any(|(_, t)| triggers(t).iter().any(|x| x == "schedule")),
        "no workflow parses as scheduled; `triggers` cannot see a schedule"
    );
}

#[test]
fn prod_is_refused_on_every_trigger_by_deploy_and_infra() {
    let deploy = read(".github/workflows/deploy.yml");
    let gate = step(&deploy, "refuse a production deployment on every trigger");
    // Structural: no condition, so no trigger is exempt. The old step carried
    // `if: ... github.event_name != 'workflow_dispatch'`.
    assert!(
        !gate.lines().any(|l| l.trim_start().starts_with("if:")),
        "deploy.yml's prod refusal is conditional; a condition is a trigger that skips it:\n{gate}"
    );
    assert!(
        !gate.contains("github.event_name") && !gate.contains("workflow_dispatch"),
        "deploy.yml's prod refusal names an event; it must not depend on one:\n{gate}"
    );
    // Wiring: the refusal is in `gate`, `images` needs `gate`, `deploy` needs
    // `images`, so a refusal stops the whole pipeline.
    let gate_job = deploy
        .split("\n  gate:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  simulation-gate:\n").next())
        .expect("deploy.yml has a `gate` job followed by a `simulation-gate` job");
    assert!(
        gate_job.contains("refuse a production deployment on every trigger"),
        "the prod refusal is not inside the `gate` job"
    );
    // R12-CICD-7116/7120: `images` waits on the named simulation and paper
    // gates as well, each of which itself needs `gate`. Dropping any of the
    // three from the list lets an image build past the claim it names.
    let images_job = deploy
        .split("\n  images:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  deploy:\n").next())
        .expect("deploy.yml has an `images` job followed by a `deploy` job");
    assert!(
        images_job.contains("    needs: [gate, simulation-gate, paper-gate]\n"),
        "`images` no longer needs `gate`, `simulation-gate` and `paper-gate`"
    );
    for named in ["simulation-gate", "paper-gate"] {
        let rest = deploy
            .split(&format!("\n  {named}:\n"))
            .nth(1)
            .unwrap_or_else(|| panic!("deploy.yml has no `{named}` job"));
        // Ends at the next line indented exactly two spaces (a sibling job or
        // a comment above one), so a neighbour's `needs:` is not read as this
        // job's own.
        let end = rest
            .match_indices("\n  ")
            .find(|(at, _)| rest[at + 3..].chars().next().is_some_and(|c| c != ' '))
            .map_or(rest.len(), |(at, _)| at);
        let job = &rest[..end];
        assert!(
            job.contains("    needs: gate\n"),
            "`{named}` no longer needs `gate`, so it can run on a commit the gate refused"
        );
        // R12-CICD-7120: the dependency proves nothing about what the job
        // runs. A paper-gate hollowed to `run: true` kept the needs line
        // and passed every test, so its command is pinned here too.
        if named == "paper-gate" {
            assert!(
                job.contains(
                    "        run: cargo test -p qip-acceptance --no-fail-fast --test paper_boundary \
                     --test arch_007_paper_boundary --test prod_guard\n"
                ),
                "`paper-gate` no longer runs the paper boundary and prod guard suites"
            );
        }
    }
    assert!(
        deploy.contains("    needs: images\n"),
        "`deploy` no longer needs `images`"
    );
    // The GitHub environment, with its reviewers, is kept: a third layer.
    assert!(
        deploy.contains("environment: ${{ inputs.environment || 'dev' }}"),
        "the deploy job lost its GitHub environment"
    );

    // Behaviour: driven, with the half that proves it is a gate not a wall.
    let refusal = script(&gate);
    let (code, printed) = run(&refusal, "prod", "deploy");
    assert_ne!(code, 0, "deploy.yml's refusal exited 0 for prod: {printed}");
    assert!(
        printed.contains("prod"),
        "the refusal does not name prod: {printed}"
    );
    for admitted in ["dev", "test", "stage"] {
        let (code, printed) = run(&refusal, admitted, "deploy");
        assert_eq!(
            code, 0,
            "deploy.yml's refusal blocked `{admitted}`: {printed}"
        );
    }

    // infra.yml: refusal unconditional, driven the same way.
    let infra = read(".github/workflows/infra.yml");
    let infra_gate = step(&infra, "refuse what this workflow must never do");
    assert!(
        !infra_gate
            .lines()
            .any(|l| l.trim_start().starts_with("if:")),
        "infra.yml's prod refusal is conditional:\n{infra_gate}"
    );
    assert!(infra_gate.contains("ENVIRONMENT: ${{ inputs.environment }}"));
    // vendor.yml mirrors into an environment's registry: same refusal.
    let vendor = read(".github/workflows/vendor.yml");
    let vendor_gate = step(&vendor, "refuse production");
    assert!(
        !vendor_gate
            .lines()
            .any(|l| l.trim_start().starts_with("if:")),
        "vendor.yml's prod refusal is conditional:\n{vendor_gate}"
    );
    let vendor_script = script(&vendor_gate);
    let (code, printed) = run(&vendor_script, "prod", "vendor");
    assert_ne!(code, 0, "vendor.yml's refusal exited 0 for prod: {printed}");
    let (code, printed) = run(&vendor_script, "dev", "vendor");
    assert_eq!(code, 0, "vendor.yml's refusal blocked dev: {printed}");
}
