//! Checks that what the documentation claims matches what the code does.
//!
//! Documentation that has drifted from the code is worse than none, because
//! someone will believe it. These tests check the specific, checkable claims —
//! counts, defaults, names, thresholds — rather than trying to verify prose.
//!
//! Each test fails in one of two ways, and both are useful: the code changed
//! and the documentation did not, or the documentation was wrong to begin with.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

// --- reading a document without being fooled by it ---------------------------

/// Does `text` name `token` on its own, rather than inside a longer word?
///
/// Two assertions in this file were carried by a bare `contains` and each was
/// satisfied by something other than the thing it was written to check:
/// `contains("serde")` is true of a document naming only `serde_json`, and
/// `contains("18 ")` is true of one that says `118 `. They are the third and
/// fourth of their kind here — `5028b82` fixed
/// `contains("autonomous_live")`, which is true of
/// `limited_autonomous_live`, in this file, and the same defect was still
/// live in `infrastructure.rs` until it was found alongside these two. A value
/// that is a substring of its neighbour survives the mutation that deletes it,
/// and `.claude/rules/architecture/01-testing-strategy.md` records that class
/// as having already cost this repository a test.
///
/// A token is named when the characters on either side of it are not part of
/// the same word. That admits every form these documents actually use — ``
/// `serde` ``, "eighteen-agent", "18 governed" — while refusing `serde_json`
/// and `118`. Underscore counts as part of a word on purpose: `serde_json` is
/// one identifier and a document naming it has not named `serde`.
fn names_token(text: &str, token: &str) -> bool {
    token_positions(text, token).next().is_some()
}

/// Where `text` names `token` as a word of its own, by byte offset.
///
/// The positions rather than a yes/no, so that a caller needing to look at what
/// follows an occurrence — `states_agent_count` below does — asks the same
/// question about word boundaries as `names_token` does, rather than a second
/// question that could drift from it.
fn token_positions<'a>(text: &'a str, token: &'a str) -> impl Iterator<Item = usize> + 'a {
    assert!(
        !token.is_empty(),
        "the empty token is named by every text; this is a caller bug, not a documentation failure"
    );
    fn is_boundary(character: Option<char>) -> bool {
        character.is_none_or(|character| !character.is_alphanumeric() && character != '_')
    }
    text.match_indices(token).filter_map(move |(at, _)| {
        (is_boundary(text[..at].chars().next_back())
            && is_boundary(text[at + token.len()..].chars().next()))
        .then_some(at)
    })
}

/// The small numbers as the words a person writes them with.
///
/// The documents state counts and thresholds in words at least as often as in
/// numerals — "the eighteen-agent investment organisation", "Third consecutive
/// breach", "Three consecutive observations" — and a test that can only match a
/// numeral either misses those or, worse, matches something else that happens
/// to be a digit. Both happened here.
const CARDINALS: [&str; 21] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
];

/// The same numbers as the ordinals a runbook writes a threshold with.
const ORDINALS: [&str; 8] = [
    "zeroth", "first", "second", "third", "fourth", "fifth", "sixth", "seventh",
];

/// `n` as the word a person writes it with.
///
/// Panics outside the table rather than returning a default. A silent empty
/// string would be named by every document, which is how a check that cannot
/// express its subject turns into a check that passes on everything.
fn cardinal(n: usize) -> &'static str {
    CARDINALS.get(n).copied().unwrap_or_else(|| {
        panic!(
            "no word for {n} in this test's table; extend CARDINALS rather than dropping to a \
             numeral, because the documents write this count as a word"
        )
    })
}

/// `n` as the ordinal a runbook writes a threshold with. Panics outside the
/// table, for the same reason `cardinal` does.
fn ordinal(n: usize) -> &'static str {
    ORDINALS.get(n).copied().unwrap_or_else(|| {
        panic!(
            "no ordinal for {n} in this test's table; extend ORDINALS rather than dropping to a \
             numeral, because the runbook writes this threshold as a word"
        )
    })
}

/// The number a word names, or `None` if it names none.
///
/// Both spellings, because a runbook writes the same threshold as "Third" in a
/// table and "Three" in the prose beneath it, and the point of reading them
/// separately is that they can disagree with each other and with the code.
fn number_named_by(word: &str) -> Option<usize> {
    let word = word.trim().to_lowercase();
    CARDINALS
        .iter()
        .position(|candidate| *candidate == word)
        .or_else(|| ORDINALS.iter().position(|candidate| *candidate == word))
}

// --- claims about the loop --------------------------------------------------

#[test]
fn the_readme_names_the_stages_the_code_actually_has() {
    let readme = read("README.md");
    let stages = qip_kernel::cycle::Stage::all();
    assert_eq!(stages.len(), 8, "the loop has eight stages");

    for stage in &stages {
        let upper = stage.as_str().to_uppercase();
        assert!(
            readme.contains(&upper),
            "the README does not mention the {upper} stage"
        );
    }

    // And the diagram is in the right order.
    let arrow_diagram = readme
        .lines()
        .find(|line| line.contains("SENSE") && line.contains("LEARN"))
        .expect("the README diagrams the loop");
    let mut cursor = 0usize;
    for stage in &stages {
        let upper = stage.as_str().to_uppercase();
        let position = arrow_diagram[cursor..]
            .find(&upper)
            .unwrap_or_else(|| panic!("{upper} is out of order in the diagram"));
        cursor += position + upper.len();
    }
}

#[test]
fn the_architecture_document_lists_a_crate_for_every_stage() {
    let architecture = read("docs/architecture/README.md");
    for stage in qip_kernel::cycle::Stage::all() {
        assert!(
            architecture.contains(&stage.as_str().to_uppercase()),
            "the architecture document does not cover {}",
            stage.as_str()
        );
    }
}

// --- claims about safety ----------------------------------------------------

#[test]
fn the_documented_default_autonomy_level_is_the_actual_default() {
    // The single most consequential claim in the documentation.
    let level = qip_risk_engine::autonomy::AutonomyLevel::DEFAULT;
    assert_eq!(
        level,
        qip_risk_engine::autonomy::AutonomyLevel::PaperTrading
    );

    for document in ["README.md", "docs/adr/0003-paper-trading-by-default.md"] {
        let content = read(document);
        assert!(
            content.contains("paper trading") || content.contains("PaperTrading"),
            "{document} does not state the default"
        );
    }
}

/// The list of levels the `autonomy_ceiling` variable will admit, as its own
/// region of `variables.tf`.
///
/// Scoping matters as much as the delimiter does. `variables.tf` names these
/// levels in two validations — one listing the six spellings the variable
/// admits, and one refusing the three that reach a real venue — so a
/// whole-file search reports a level as *accepted* when what it actually
/// found was the clause forbidding it. The refusal is somebody else's test
/// (`no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue`,
/// in `infrastructure.rs`); this region is what the variable lets through.
///
/// Every extraction step panics rather than degrading to a default. Each
/// assertion built on this asks whether a name is *present*, so a region that
/// silently widened back to the whole file would answer yes to everything —
/// which is the failure being removed, reintroduced through the back door.
fn admitted_ceiling_levels(variables: &str) -> &str {
    let block = variables
        .split_once("variable \"autonomy_ceiling\" {")
        .unwrap_or_else(|| {
            panic!("infrastructure/terraform/variables.tf no longer declares an autonomy_ceiling variable")
        })
        .1;
    // The variable block ends at the first brace in column zero; the nested
    // `validation` blocks close indented.
    let block = block.split_once("\n}").map_or(block, |(head, _)| head);
    // `condition = contains([` is the admitting validation. The refusing one
    // spells it `condition = !contains([`, which this does not match.
    let list = block
        .split_once("condition = contains([")
        .unwrap_or_else(|| {
            panic!(
                "the autonomy_ceiling variable no longer validates against a list of admitted \
                 levels, so nothing here can say which levels it accepts"
            )
        })
        .1;
    list.split_once("], var.autonomy_ceiling)")
        .unwrap_or_else(|| {
            panic!("the admitted-levels list does not close on var.autonomy_ceiling")
        })
        .0
}

#[test]
fn the_documented_autonomy_levels_are_the_ones_the_code_declares() {
    let levels = qip_risk_engine::autonomy::AutonomyLevel::all();
    let variables = read("infrastructure/terraform/variables.tf");
    let operations = read("docs/operations/enabling-live-trading.md");

    // Matched on the quoted, comma-terminated entry rather than the bare name.
    // This is not hypothetical caution: `"limited_autonomous_live"` contains
    // `autonomous_live`, so deleting the `autonomous_live` entry from the list
    // outright left the earlier `variables.contains(level.as_str())` green —
    // verified by doing it. That entry is one of the three live rungs layer one
    // of the paper-trading boundary exists to stop at plan time, and it is the
    // same class of defect `.claude/rules/architecture/01-testing-strategy.md`
    // records as having already happened in this repository once, on a value
    // that was a substring of its neighbour.
    //
    // The discipline is `paper_boundary.rs`'s, where
    // `the_delimited_check_on_the_refusal_would_reject_a_message_naming_the_neighbouring_rung`
    // states it as a test of its own.
    let admitted = admitted_ceiling_levels(&variables);
    for level in &levels {
        assert!(
            admitted.contains(&format!("\"{}\",", level.as_str())),
            "the Terraform ceiling variable does not accept {}",
            level.as_str()
        );
    }

    // The other direction, and an equality rather than a floor. A seventh
    // spelling in Terraform is a ceiling an operator can set that the code has
    // never heard of, and no per-level loop can see one.
    let mut admitted_names: Vec<&str> = admitted.split('"').skip(1).step_by(2).collect();
    admitted_names.sort_unstable();
    let mut declared: Vec<&str> = levels.iter().map(|level| level.as_str()).collect();
    declared.sort_unstable();
    assert_eq!(
        admitted_names, declared,
        "the levels the Terraform ceiling variable admits are not the levels the code declares"
    );

    // The runbook names the level it tells an operator to set. A bare match is
    // honest for this one: `supervised_live` is a substring of no other rung.
    assert!(
        operations.contains("supervised_live"),
        "the runbook does not name the level it tells an operator to set"
    );
}

#[test]
fn the_readme_lists_a_control_for_every_control_the_code_enforces() {
    let readme = read("README.md");
    for claim in [
        // 1: paper trading by default.
        "AutonomyLevel::DEFAULT",
        // 2: two operators.
        "second approver",
        // 3: no self-escalation.
        "change_autonomy_level",
        // 4: separation of duties.
        "AgentManifest::validate",
        // 5: numeric provenance.
        "observed",
        // 6: the asymmetric kill switch.
        "kill switch",
    ] {
        assert!(
            readme.contains(claim),
            "the README's safety section does not mention {claim}"
        );
    }
}

#[test]
fn no_agent_holds_the_capability_the_readme_says_none_holds() {
    // The README claims no agent of any role may hold `change_autonomy_level`.
    // The roster is the check.
    let roster = qip_investment_agents::manifests::roster(qip_core::Timestamp::from_secs(0));

    // Premise one: there is an organisation to check. Every assertion below is
    // an `is_empty()`, and an empty roster satisfies all of them — proved by
    // deleting all eighteen agents from `manifests::roster`, at which point
    // this test went on passing while the README's central safety claim was
    // being verified against nobody.
    assert!(
        !roster.is_empty(),
        "the roster holds no agents, so \"no agent holds this capability\" is true of nothing and \
         this test constrains nothing"
    );

    // Premise two: `holding` discriminates. A `holding` that returned an empty
    // vector whatever it was asked would satisfy the assertions below exactly
    // as a roster that genuinely withholds the capability does, and nothing
    // here could tell the two apart. `SubmitOrder` is the sharpest available
    // probe: separation of duties means exactly one agent on this roster may
    // hold it, so an answer of one agent proves `holding` both finds and
    // filters.
    let submitters = roster.holding(qip_agents::capability::Capability::SubmitOrder);
    assert_eq!(
        submitters.len(),
        1,
        "`holding` does not report exactly the one agent allowed to submit an order, so its empty \
         answers below say nothing about the roster"
    );
    assert_eq!(
        submitters[0].id,
        qip_investment_agents::manifests::ids::EXECUTION,
        "the agent holding submit_order is not the execution trader"
    );

    // Premise three: the README still makes the claim this is checking. A
    // README that dropped control 3 would leave the assertions below true and
    // pointless.
    let readme = read("README.md");
    assert!(
        readme.contains("`change_autonomy_level`"),
        "the README no longer names change_autonomy_level, so this test is checking a promise \
         nobody makes"
    );
    assert!(
        readme.contains("No agent, of any role, may hold"),
        "the README no longer claims that no agent may hold the capability; this test proves the \
         roster withholds something the documentation has stopped promising"
    );

    assert!(
        roster
            .holding(qip_agents::capability::Capability::ChangeAutonomyLevel)
            .is_empty(),
        "an agent holds change_autonomy_level, which the README says is impossible"
    );
    assert!(
        roster
            .holding(qip_agents::capability::Capability::OverrideRiskLimit)
            .is_empty(),
        "an agent holds override_risk_limit; a risk limit an agent can raise is not a limit"
    );
}

/// Does `text` state `count` as the number of agents, adjacent to the noun?
///
/// Adjacency is the whole of it. The old check asked only whether the numeral
/// appeared anywhere in the document, with `|| content.contains("eighteen")`
/// beside it — and that literal, being hardcoded rather than derived from the
/// roster, carried `README.md` on its own. Growing the roster to nineteen left
/// the README's assertion passing on the word "eighteen"; only the *other*
/// document failed. Verified by adding a nineteenth agent.
///
/// The window is a stated limit rather than a discovered one: a document that
/// separates the count from the noun by more than a short phrase reads as not
/// stating it, and fails loudly.
fn states_agent_count(text: &str, count: usize) -> bool {
    const WINDOW: usize = 32;
    [count.to_string(), cardinal(count).to_string()]
        .iter()
        .any(|form| {
            token_positions(text, form).any(|at| {
                text[at + form.len()..]
                    .chars()
                    .take(WINDOW)
                    .collect::<String>()
                    .contains("agent")
            })
        })
}

#[test]
fn the_documented_agent_count_matches_the_roster() {
    let roster = qip_investment_agents::manifests::roster(qip_core::Timestamp::from_secs(0));
    let count = roster.len();
    // The premise. A roster of nobody is documented by any document at all.
    assert!(
        count > 0,
        "the roster holds no agents, so there is no count for a document to state"
    );

    // Both forms, and both derived from the roster. The documents disagree
    // about which to use — the README writes "the eighteen-agent investment
    // organisation" and the architecture document writes "18 governed agents"
    // — so accepting either is honest. Accepting a *hardcoded* word was not:
    // it made one of the two assertions independent of the number it was
    // checking.
    for document in ["README.md", "docs/architecture/README.md"] {
        let content = read(document).to_lowercase();
        assert!(
            states_agent_count(&content, count),
            "{document} states neither {count} nor \"{}\" as its agent count, and the roster holds \
             {count}",
            cardinal(count)
        );
    }
}

#[test]
fn the_delimited_check_would_reject_a_count_inside_a_longer_number_or_a_dependency_inside_a_longer_name()
 {
    // Written out rather than left to a manual mutation, because these are the
    // mutations a bare `contains` survives and the documents cannot be edited
    // into the shapes that would prove it. Both premises assert that the bare
    // form *does* match, so a delimiter that stopped discriminating fails here
    // rather than going unnoticed in the tests that depend on it.
    assert!(
        "118 governed agents".contains("18 "),
        "the premise of this test is that the bare substring matches a longer number; if it no \
         longer does, the check below discriminates nothing"
    );
    assert!(
        !names_token("118 governed agents", "18"),
        "the delimited check accepted a count inside a longer number"
    );
    assert!(
        "the platform depends on `serde_json`".contains("serde"),
        "the premise of this test is that the bare substring matches the longer dependency name; \
         if it no longer does, the check below discriminates nothing"
    );
    assert!(
        !names_token("the platform depends on `serde_json`", "serde"),
        "the delimited check accepted a dependency named only inside a longer one"
    );

    // And the other direction, which matters just as much: a delimiter that
    // refused everything would satisfy every assertion above and turn every
    // check built on it into a permanent failure that somebody eventually
    // deletes. These are the exact forms the two documents use.
    assert!(
        names_token("the eighteen-agent investment organisation", "eighteen"),
        "the delimited check refuses a count the README actually writes"
    );
    assert!(
        names_token("18 governed agents with", "18"),
        "the delimited check refuses a count the architecture document actually writes"
    );
    assert!(
        names_token("depends on `serde` and `serde_json`", "serde"),
        "the delimited check refuses a dependency the README actually names"
    );
}

// --- claims about dependencies ----------------------------------------------

#[test]
fn the_documented_dependencies_are_the_ones_in_the_manifest() {
    // The README and ADR 0002 both claim serde and serde_json and nothing
    // else. The workspace manifest is the authority.
    let manifest = read("backend/Cargo.toml");
    let dependencies: Vec<&str> = manifest
        .split("[workspace.dependencies]")
        .nth(1)
        .expect("the workspace declares its dependencies")
        .lines()
        .take_while(|line| !line.trim().starts_with('['))
        .filter_map(|line| line.split('=').next())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('#') && !name.starts_with("qip-"))
        .collect();

    assert_eq!(
        dependencies,
        vec!["serde", "serde_json"],
        "the third-party dependencies have changed; the README and ADR 0002 need updating"
    );

    // Named as tokens of their own, and iterated from the manifest rather than
    // written out again. Both halves matter and both were wrong.
    //
    // `contains("serde")` is satisfied by a document naming only `serde_json`,
    // so the two assertions this replaces were one assertion wearing two hats:
    // an ADR that dropped standalone `serde` and kept `serde_json` passed both.
    // Verified by editing ADR 0002 to say exactly that, at which point the test
    // stayed green — and `serde` is the dependency, not `serde_json`; a
    // document that has stopped naming it has stopped documenting the policy
    // `./scripts/check-dependencies.sh` enforces.
    //
    // Deriving the loop from `dependencies` is the other half: a hardcoded pair
    // says nothing about a third dependency, and the equality above would then
    // be the only thing standing between a new crate and two documents that
    // never mention it.
    for document in ["README.md", "docs/adr/0002-two-dependencies.md"] {
        let content = read(document);
        for dependency in &dependencies {
            assert!(
                names_token(&content, dependency),
                "{document} does not name {dependency} as a token of its own; naming it only \
                 inside a longer dependency name is not documenting it"
            );
        }
    }
}

#[test]
fn the_documented_lockfile_size_is_current() {
    // ADR 0002 claims the lockfile is small enough to read. Checking the claim
    // rather than the sentiment.
    let lockfile = read("backend/Cargo.lock");
    let third_party = lockfile
        .lines()
        .filter(|line| line.starts_with("name = "))
        .filter(|line| !line.contains("\"qip-"))
        .count();

    let adr = read("docs/adr/0002-two-dependencies.md");
    assert!(
        adr.contains(&format!("{third_party} packages")),
        "ADR 0002 claims a lockfile size that is no longer {third_party}. \
         The count is written as a numeral on purpose: a document saying \
         \"eleven\" would survive the count becoming twelve."
    );
}

// --- claims about the API ---------------------------------------------------

#[test]
fn every_documented_endpoint_exists() {
    let operations = files_with_extension("docs/operations", "md");
    let mut mentioned: Vec<String> = Vec::new();
    for path in &operations {
        let content = std::fs::read_to_string(path).expect("readable");
        for line in content.lines() {
            for token in line.split_whitespace() {
                let token = token.trim_matches(|c: char| !c.is_ascii_graphic());
                if let Some(index) = token.find("/api/v1") {
                    mentioned.push(token[index..].trim_end_matches(['`', '"', ')']).to_string());
                }
            }
        }
    }
    assert!(!mentioned.is_empty(), "the runbooks reference no endpoints");

    for path in mentioned {
        let suffix = path.trim_start_matches("/api/v1");
        if suffix.is_empty() {
            continue;
        }
        assert!(
            qip_api::ROUTES.iter().any(|route| route.pattern == suffix),
            "a runbook references {path}, which is not a route"
        );
    }
}

#[test]
fn the_documented_role_names_are_the_ones_the_code_defines() {
    let readme = read("docs/operations/README.md");
    for role in [
        qip_api::Role::Monitor,
        qip_api::Role::Viewer,
        qip_api::Role::Operator,
    ] {
        let variable = format!("QIP_TOKEN_{}", role.as_str().to_uppercase());
        assert!(
            readme.contains(&variable) || read("README.md").contains(&variable),
            "{variable} is not documented"
        );
    }
}

// --- claims about the escalation policy -------------------------------------

/// The threshold a line of the runbook states, read as the number it names.
///
/// `locator` selects the line and the cell or clause the number opens. The
/// number is read *out of the prose* and compared with the policy, rather than
/// the policy being formatted into a `contains` — because the runbook writes
/// the threshold in words, and searching it for a numeral finds something else.
///
/// Every step panics rather than returning a default. A `None` absorbed into a
/// `unwrap_or(0)` would compare cleanly against nothing and pass whenever the
/// runbook stopped stating a threshold at all.
fn threshold_stated_by(runbook: &str, locator: &str, cell: usize) -> usize {
    let line = runbook
        .lines()
        .find(|line| line.contains(locator))
        .unwrap_or_else(|| {
            panic!(
                "docs/operations/limit-breach.md no longer contains a line saying {locator:?}, so \
                 nothing here can read the threshold it used to state"
            )
        });
    let clause = line
        .split('|')
        .nth(cell)
        .unwrap_or_else(|| panic!("the line saying {locator:?} has no cell {cell}: {line:?}"));
    let word = clause.split_whitespace().next().unwrap_or_else(|| {
        panic!("the clause stating the threshold in {locator:?} is empty: {line:?}")
    });
    number_named_by(word).unwrap_or_else(|| {
        panic!(
            "the runbook opens its {locator:?} statement with {word:?}, which names no number this \
             test can read; state the threshold as a word or extend the table in this file"
        )
    })
}

#[test]
fn the_runbook_describes_the_escalation_the_monitor_actually_performs() {
    let runbook = read("docs/operations/limit-breach.md");
    let policy = qip_risk_engine::monitor::MonitorPolicy::default();

    // The threshold, read from the two places the runbook states it and
    // compared with the monitor's own value.
    //
    // This used to be `runbook.contains(&format!("{}", breaches_before_halt))`,
    // and it passed on a markdown ordered-list marker. Every digit in that
    // runbook is a list marker — `grep -on '[0-9]\+'` gives `5:1 7:1 9:2 19:3
    // 22:4` — so `contains("3")` found list item 3 in the "Do this" section,
    // and the threshold could have been anything. Verified by changing
    // `breaches_before_halt` to 4, at which point the check found list item 4
    // and stayed green while the runbook still said "Third consecutive breach".
    //
    // Reading the number out of the prose rather than formatting the policy
    // into a search is what makes the failure legible: the message names what
    // the runbook says and what the monitor does, so a reader knows which of
    // the two is wrong.
    let in_the_table = threshold_stated_by(&runbook, "consecutive breach |", 1);
    assert_eq!(
        in_the_table,
        policy.breaches_before_halt,
        "the runbook's escalation table says a scope halts on the {} consecutive breach and the \
         monitor halts on the {}",
        ordinal(in_the_table),
        ordinal(policy.breaches_before_halt)
    );
    let in_the_prose = threshold_stated_by(&runbook, "consecutive observations", 0);
    assert_eq!(
        in_the_prose,
        policy.breaches_before_halt,
        "the runbook's prose says {} consecutive observations are the threshold and the monitor \
         halts on {}",
        cardinal(in_the_prose),
        cardinal(policy.breaches_before_halt)
    );

    assert!(
        runbook.contains("Reduce-only"),
        "the runbook does not describe the reduce-only state"
    );
    // And the claim that reduce-only still permits reductions.
    assert!(
        qip_risk_engine::monitor::MonitorAction::ReduceOnly {
            breaches: Vec::new()
        }
        .permits_reduction(),
        "the runbook says reduce-only permits reductions and the code disagrees"
    );
}

#[test]
fn the_runbook_states_the_credential_freshness_the_code_requires() {
    // Fifteen minutes, in both places.
    let runbook = read("docs/operations/enabling-live-trading.md");
    assert!(
        runbook.contains("fifteen minutes"),
        "the runbook does not state the credential freshness requirement"
    );

    // And the code enforces it: a credential from an hour ago is refused.
    let now = qip_core::Timestamp::from_secs(1_000_000);
    let operator = qip_risk_engine::autonomy::OperatorIdentity::verified("a", "token", now)
        .with_second_approver("b");
    let mut controller = qip_risk_engine::autonomy::AutonomyController::with_live_ceiling(
        qip_risk_engine::autonomy::AutonomyLevel::SupervisedLive,
    );
    let much_later = now.saturating_add(qip_core::Duration::from_hours(1));
    assert!(
        controller
            .request_change(
                qip_risk_engine::autonomy::AutonomyLevel::SupervisedLive,
                &operator,
                "enabling live trading for the pilot",
                much_later,
            )
            .is_err(),
        "the code does not enforce the freshness the runbook promises"
    );
}

#[test]
fn the_runbook_states_the_freshness_and_the_record_that_lifting_a_halt_requires() {
    // The same rule on the other control, and the rule that makes it
    // reviewable afterwards. Both are asserted against the code rather than
    // against a second copy of the prose, because prose is what drifts.
    let runbook = read("docs/operations/kill-switch.md");
    assert!(
        runbook.contains("15 minutes"),
        "the kill-switch runbook does not state the credential freshness"
    );
    assert!(
        runbook.contains("Every lift is recorded"),
        "the kill-switch runbook does not say that lifting a halt is recorded"
    );

    let now = qip_core::Timestamp::from_secs(1_000_000);
    let mut switch = qip_risk_engine::autonomy::KillSwitch::new();
    switch.trip_global(now, "test", "a halt to lift");

    let stale = qip_risk_engine::autonomy::OperatorIdentity::verified("a", "token", now);
    let much_later = now.saturating_add(qip_core::Duration::from_hours(1));
    assert!(
        switch.clear_global(&stale, much_later).is_err(),
        "the code does not enforce the freshness the runbook promises"
    );

    let fresh = qip_risk_engine::autonomy::OperatorIdentity::verified("a", "token", much_later);
    switch
        .clear_global(&fresh, much_later)
        .expect("a fresh credential lifts the halt");
    let recorded = switch.clearances();
    assert_eq!(recorded.len(), 1, "the lift was not recorded");
    assert_eq!(recorded[0].operator, "a");
    assert_eq!(recorded[0].cleared.reason, "a halt to lift");
}

#[test]
fn the_reconciliation_runbook_names_a_field_the_api_actually_returns() {
    // The runbook tells an operator at three in the morning to read the breaks
    // off two endpoints. A runbook that names a field the API does not return
    // is worse than no runbook, because it costs the reader the time to find
    // out.
    let runbook = read("docs/operations/reconciliation-break.md");
    assert!(
        runbook.contains("reconciliation_breaks"),
        "the runbook does not name the field to look at"
    );

    let routes = read("backend/crates/apps/qip-api/src/routes.rs");
    for endpoint in ["fn health(", "fn orders("] {
        let body = routes
            .split(endpoint)
            .nth(1)
            .unwrap_or_else(|| panic!("{endpoint} exists"));
        let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
        assert!(
            body.contains("reconciliation_breaks"),
            "{endpoint} does not return the field the runbook names"
        );
    }
}

// --- the documentation's own hygiene ----------------------------------------

#[test]
fn every_internal_link_resolves() {
    // A broken link in a runbook is found by the person who needed it.
    let root = repository_root();
    let mut checked = 0usize;
    for path in files_with_extension("docs", "md")
        .into_iter()
        .chain(files_with_extension("ops", "md"))
        .chain([root.join("README.md")])
    {
        let content = std::fs::read_to_string(&path).expect("readable");
        let directory = path.parent().expect("a file has a parent");
        for line in content.lines() {
            let mut rest = line;
            while let Some(open) = rest.find("](") {
                let after = &rest[open + 2..];
                let Some(close) = after.find(')') else { break };
                let target = &after[..close];
                rest = &after[close..];

                if target.starts_with("http") || target.starts_with('#') {
                    continue;
                }
                let target = target.split('#').next().unwrap_or(target);
                let resolved = directory.join(target);
                assert!(
                    resolved.exists(),
                    "{} links to {target}, which does not exist",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5, "only {checked} internal links were checked");
}

/// Assert that a walk of the documentation tree actually found the tree.
///
/// `files_with_extension` returns an empty vector for a directory it cannot
/// read — `read_dir` fails, the loop `continue`s, and the caller gets `[]` with
/// no error. Every test below iterates such a walk and asserts a property of
/// each file, so a renamed directory turns all of them into loops over nothing
/// that pass forever. Verified by making the walk look one level below where
/// the documents are, at which point all three went green with 51 decision
/// records and 113 documents unexamined.
///
/// Anchored on a named file rather than on a count, deliberately. A count has
/// to be lowered by hand whenever a document is withdrawn, and lowering a
/// number to obtain a pass is the move this repository forbids; a named anchor
/// tracks the tree on its own and fails only when the walk stops working.
fn assert_walk_found(paths: &[std::path::PathBuf], anchor: &str, walked: &str) {
    assert!(
        paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == anchor)),
        "the walk of {walked} did not find {anchor}, so it is examining {} files and the property \
         asserted of each of them is being asserted of nothing",
        paths.len()
    );
}

#[test]
fn every_decision_record_states_what_it_costs() {
    // A decision with no stated cost has not been thought about.
    let records = files_with_extension("docs/adr", "md");
    assert_walk_found(&records, "0002-two-dependencies.md", "docs/adr");

    let mut examined = 0usize;
    for path in records {
        if path.file_name().is_some_and(|name| name == "README.md") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable");
        assert!(
            content.contains("## What it costs"),
            "{} does not state what the decision costs",
            path.display()
        );
        assert!(
            content.contains("## What would make this wrong"),
            "{} states no condition under which it should be revisited",
            path.display()
        );
        examined += 1;
    }
    // The second premise. The anchor proves the walk saw the directory; this
    // proves the filter above did not then discard everything it saw.
    assert!(
        examined > 1,
        "only {examined} decision records were examined, and there are more than that"
    );
}

#[test]
fn every_decision_record_is_listed_in_the_index() {
    let index = read("docs/adr/README.md");
    let records = files_with_extension("docs/adr", "md");
    assert_walk_found(&records, "0002-two-dependencies.md", "docs/adr");

    let mut examined = 0usize;
    for path in records {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "README.md" {
            continue;
        }
        assert!(
            index.contains(name),
            "{name} is not listed in the decision index"
        );
        examined += 1;
    }
    assert!(
        examined > 1,
        "only {examined} decision records were checked against the index, and there are more than \
         that"
    );
}

#[test]
fn no_document_promises_something_the_platform_does_not_do() {
    // A narrow check for the specific overclaims that matter: anything
    // presenting the platform as production-ready or as having demonstrated
    // quantum advantage.
    //
    // `ops` does not exist in this tree and `files_with_extension` returns
    // nothing for it without complaining. That is left in place rather than
    // removed — the directory is named in `every_internal_link_resolves` too —
    // but it is the reason the premise below is anchored on `docs`, which does
    // exist and holds every document this is about.
    let documents: Vec<std::path::PathBuf> = files_with_extension("docs", "md")
        .into_iter()
        .chain(files_with_extension("ops", "md"))
        .chain([repository_root().join("README.md")])
        .collect();
    assert_walk_found(&documents, "0002-two-dependencies.md", "docs and ops");

    let mut examined = 0usize;
    for path in documents {
        let content = std::fs::read_to_string(&path).expect("readable");
        let lowered = content.to_lowercase();
        for claim in [
            "production ready",
            "production-ready",
            "quantum advantage over",
            "guaranteed profit",
            "battle tested",
            "battle-tested",
        ] {
            assert!(
                !lowered.contains(claim),
                "{} claims \"{claim}\", which the platform has not demonstrated",
                path.display()
            );
        }
        examined += 1;
    }
    // The README alone would satisfy the loop above, and does when the walk of
    // `docs` finds nothing: the chained path is a literal and exists whatever
    // the walk returns.
    assert!(
        examined > 1,
        "only {examined} documents were read, which is the README and nothing else"
    );
}

// --- the final system report ------------------------------------------------

#[test]
fn the_delivery_status_scores_every_numbered_blueprint_section() {
    // This replaced `the_final_report_states_a_verdict_for_every_layer…` on
    // 2026-09-07, when nineteen status documents became one. The property is
    // the same and stronger: a status document that quietly drops the section
    // with the worst verdict reads better and says less, so every numbered
    // section of the blueprint must carry a row.
    //
    // The blueprint is the spine precisely because this repository cannot
    // redefine it — the previous registers each invented their own grouping,
    // which is how they drifted apart.
    let blueprint = qip_acceptance::read("docs/architecture/algorik-blueprint-v10.1-source.md");
    let status = qip_acceptance::read("docs/DELIVERY-STATUS.md");

    let sections: Vec<String> = blueprint
        .lines()
        .filter_map(|line| {
            let number = line.split_whitespace().next()?;
            let rest = line[number.len()..].trim_start();
            // A section number is `5` or `5.1`, never `5.` — a trailing dot
            // is a prose list marker, and admitting those found 57 phantom
            // "sections" the first time this ran.
            let numeric = !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit() || c == '.')
                && number.chars().next().is_some_and(|c| c.is_ascii_digit())
                && !number.ends_with('.');
            let titled = rest.chars().next().is_some_and(char::is_uppercase);
            (numeric && titled).then(|| number.to_string())
        })
        .collect();

    // The premise: the spine really was read. A blueprint that failed to load
    // would make the loop below vacuous and this test would pass on nothing.
    assert!(
        sections.len() > 150,
        "only {} numbered sections were found in the blueprint, so this test is \
         not reading the specification it was written for",
        sections.len()
    );

    let missing: Vec<&String> = sections
        .iter()
        .filter(|section| !status.contains(&format!("| {section} |")))
        .collect();
    assert!(
        missing.is_empty(),
        "the delivery status has no row for {} blueprint section(s): {missing:?}",
        missing.len()
    );
}

#[test]
fn the_delivery_status_counts_the_crates_that_are_actually_there() {
    // The evidence table is measured, and a measured number goes stale. This
    // is what notices — it is cheap to recount and expensive to be wrong
    // about, since the count is the first thing a reader checks.
    let report = qip_acceptance::read("docs/DELIVERY-STATUS.md");
    let manifests = qip_acceptance::files_with_extension("backend/crates", "toml")
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .count();
    assert!(
        report.contains(&format!("| Crates | {manifests} |")),
        "the delivery status does not say there are {manifests} crates, and there are"
    );
}

/// A document that says a type does not exist must be right about that.
///
/// The scored architecture documents make existence claims — "there is no
/// `Ledger` type", and once "no `Intent` type exists anywhere" — and those are
/// the sentences a reader trusts instead of grepping. They are also the
/// sentences that rot silently: the `Intent` claim was false for four commits
/// on the same branch that built the type, inside the document scored against
/// the same blueprint, and nothing anywhere could notice.
///
/// So every such claim is checked against the tree. This is deliberately a
/// narrow pattern rather than prose analysis: it catches the exact class that
/// has already happened once, and it fails loudly when a claim outlives its
/// subject.
#[test]
fn no_scored_document_denies_the_existence_of_a_type_the_workspace_defines() {
    let documents = [
        // Was three scored architecture documents until 2026-09-07; all three
        // were among the nineteen consolidated into the single status file,
        // which is what this now reads.
        "docs/DELIVERY-STATUS.md",
    ];

    // Every source file once, so the claims below are checked against the
    // whole workspace rather than a guessed subset — but comment lines are
    // dropped first. Without that the scan finds its own explanatory comment
    // below, which quotes `pub struct Ledger` to describe the prefix rule, and
    // reports the still-true statement that no `Ledger` type exists as a
    // falsehood. A test whose own prose is part of its input measures itself.
    let mut sources = String::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let file = std::fs::read_to_string(&path).unwrap_or_default();
        for line in file.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            sources.push_str(line);
            sources.push('\n');
        }
    }

    /// Does the workspace declare a type by exactly this name?
    fn declares(sources: &str, name: &str) -> bool {
        ["pub struct ", "pub enum ", "pub type ", "pub trait "]
            .iter()
            .any(|keyword| {
                let needle = format!("{keyword}{name}");
                sources.match_indices(&needle).any(|(at, _)| {
                    // Reject a prefix match: `pub struct Ledger` must not be
                    // satisfied by `pub struct LedgerEntry`.
                    sources[at + needle.len()..]
                        .chars()
                        .next()
                        .is_none_or(|next| !next.is_alphanumeric() && next != '_')
                })
            })
    }

    let mut wrong = Vec::new();
    for document in documents {
        let text = qip_acceptance::read(document);
        for (index, line) in text.lines().enumerate() {
            // Walk the backtick-delimited spans of the *original* line, so the
            // name keeps its casing. An earlier version searched a lowercased
            // copy and then looked the name up again by substring, which found
            // the first match rather than this one — on a row beginning
            // "strategy intent" it recovered the lowercase word and asked
            // whether `pub struct intent` existed, so the claim it was written
            // to catch sailed through.
            let bytes: Vec<char> = line.chars().collect();
            let ticks: Vec<usize> = bytes
                .iter()
                .enumerate()
                .filter(|(_, c)| **c == '`')
                .map(|(at, _)| at)
                .collect();
            for pair in ticks.chunks_exact(2) {
                let (open, close) = (pair[0], pair[1]);
                let name: String = bytes[open + 1..close].iter().collect();
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // The claim shape: "no `X` type", allowing the emphasis these
                // documents use around either part.
                let before: String = bytes[..open].iter().collect::<String>().to_lowercase();
                let trimmed = before.trim_end();
                // Ends with the word "no", whatever emphasis precedes it —
                // `**No \`Intent\`` is the shape these documents actually use,
                // and trimming trailing asterisks does nothing for it because
                // the asterisks come *before* the word.
                let Some(head) = trimmed.strip_suffix("no") else {
                    continue;
                };
                if head
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    continue;
                }
                let after: String = bytes[close + 1..].iter().collect::<String>().to_lowercase();
                if !after
                    .trim_start()
                    .trim_start_matches('*')
                    .starts_with("type")
                {
                    continue;
                }
                if declares(&sources, &name) {
                    wrong.push(format!("{document}:{}: `{name}`", index + 1));
                }
            }
        }
    }

    // The vacuity guard. It was `claims > 0` until 2026-09-07, which was right
    // while three scored architecture documents reliably carried such claims —
    // and became backwards the moment those nineteen documents were replaced by
    // one that denies nothing. "Some document must always deny a type" is not a
    // property worth holding; a corpus with zero denials is the goal state, not
    // a broken reader.
    //
    // So the guard proves the *machinery* instead: the sources were really read,
    // and `declares` distinguishes a type the workspace defines from one it does
    // not. If either stops being true the test fails, which is what the old
    // guard was for; what it no longer does is demand a live falsehood exist.
    assert!(
        sources.len() > 100_000,
        "the workspace sources did not load, so this test is scanning nothing: \
         {} bytes",
        sources.len()
    );
    assert!(
        declares(&sources, "Platform"),
        "`declares` cannot find `pub struct Platform`, so the detector is broken \
         and every claim below would read as honest"
    );
    assert!(
        !declares(&sources, "NoSuchTypeExistsAnywhere"),
        "`declares` reports a type the workspace does not define, so it would \
         never flag a denial"
    );
    assert!(
        wrong.is_empty(),
        "these documents deny the existence of a type the workspace defines: \
         {wrong:?}"
    );
}

// --- a document that quotes a command as its evidence ------------------------
//
// The test above reads three architecture documents and matches one phrasing,
// roughly ``no `X` type``. On 2026-09-07 a completeness audit found six
// contradictions by hand, and **not one of them was in either the documents it
// reads or the shape it matches**. They were in the scored *plan* documents,
// and the worst of them read, verbatim:
//
//     | 9 Self-model and exploration | Value of information measured |
//     | Nothing (`grep -rln SelfModel` empty) | MISSING, deliberately |
//
// That command returns eight files. The cell carried its own disproof, the same
// document said the opposite two hundred lines above, and no gate in this
// repository could have fired on it — which is why it survived long enough for
// a person to find it. An understatement of this kind is not the harmless
// direction: it sends an engineer to build a second copy of something that
// already works, beside the first.
//
// So the shape is read *and the command is run*. A document that quotes a
// command as proof is held to what the command prints; anything less would be
// this file believing a citation for the same reason the reader it protects
// would.

/// The documents whose commands are re-run against the tree.
///
/// The plan documents rather than the architecture ones because that is where
/// the register lives — every finding of the 2026-09-07 audit was in one of
/// these two. Adding a third is fine; the premise below asserts each named
/// document is present and substantial, so a rename fails loudly rather than
/// silently shrinking the corpus to nothing.
/// Widened from two to five on 2026-09-07. The three added here each carried
/// exactly the defect this gate exists to catch, found by the audit that
/// prompted the gate and corrected in the same commit that widened the list:
/// `integration-truth-pass.md` said ``grep -rn TransferGate`` returned nothing
/// while it named ten files, `wave-7-backlog.md` called a passkey search
/// "empty, confirmed" against four, and `algorik-instruction-precedence.md`
/// said ``grep -rni algorik`` returned nothing against three. A corpus of two
/// would have gone on missing all three, which is the shape of the gate that
/// preceded this one — real, passing, and scoped away from where the
/// overstatements were.
const SCORED_PLAN_DOCUMENTS: [&str; 2] = [
    "docs/DELIVERY-STATUS.md",
    "docs/plan/algorik-instruction-precedence.md",
];

/// A cell claiming that a quoted search command comes back empty.
#[derive(Debug)]
struct EmptinessClaim {
    line: usize,
    command: String,
}

/// The word-boundary rule, applied to the prose around a quoted command.
///
/// `names_token` and not `contains`, throughout, and for the reason this file
/// already carries three scars about: `contains("none")` is true of "nonetheless"
/// and `contains("but")` of "attribute", and a qualifier missed is a claim
/// evaluated that the document never made.
fn names_any(text: &str, tokens: &[&str]) -> bool {
    tokens.iter().any(|token| names_token(text, token))
}

/// Every claim on one line that a quoted `grep` finds nothing.
///
/// Four things disqualify a candidate, and each of them exists because a real
/// line in these documents would otherwise be read as a claim it does not make:
///
/// * **Reported speech.** The corrections of 2026-09-07 quote the old cell
///   inside double quotes — `read "Nothing (\`grep -rln SelfModel\` empty)" and
///   its own command disproves it` — so a command inside a quoted region is the
///   document describing a claim, not making one. This is a bypass in
///   principle: a false claim written inside quotation marks is not checked.
///   It is accepted because the alternative fails on every document that
///   corrects itself, and a gate that fires on the fix is a gate that gets
///   deleted.
/// * **A qualifier.** ``finds nothing but "preserved"`` is a claim about what
///   the matches *are*, not that there are none, and running it finds the
///   match the sentence already names.
/// * **An anchor.** "is still empty at `296e187`" is a claim about a commit,
///   and this test reads the working tree. Detected by the window being cut
///   short by a backtick directly after a preposition, which is the shape of
///   "at `<commit>`", "on `<date>`", "in `<file>`".
/// * **No emptiness word at all** within the cell, which is the ordinary case:
///   these documents quote greps far more often as evidence that something *is*
///   there.
fn emptiness_claims(text: &str) -> Vec<EmptinessClaim> {
    const EMPTINESS: [&str; 3] = ["empty", "nothing", "none"];
    const QUALIFIERS: [&str; 6] = ["but", "except", "only", "other", "besides", "unless"];
    const ANCHORS: [&str; 9] = [
        "at", "in", "on", "since", "under", "before", "after", "as", "against",
    ];
    const WINDOW: usize = 80;

    let mut claims = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let characters: Vec<char> = line.chars().collect();
        let ticks: Vec<usize> = characters
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == '`')
            .map(|(at, _)| at)
            .collect();
        for pair in ticks.chunks_exact(2) {
            let (open, close) = (pair[0], pair[1]);
            let command: String = characters[open + 1..close].iter().collect();
            let command = command.trim().to_string();
            if command != "grep" && !command.starts_with("grep ") {
                continue;
            }
            // Reported speech: an odd number of quotation marks before the
            // command means it opens inside a quoted region.
            if characters[..open].iter().filter(|c| **c == '"').count() % 2 == 1 {
                continue;
            }
            // The window ends where the cell, the next quoted span, or the
            // reader's patience does, whichever comes first.
            let mut window = String::new();
            let mut cut_by_backtick = false;
            for character in characters[close + 1..].iter().take(WINDOW) {
                if *character == '|' {
                    break;
                }
                if *character == '`' {
                    cut_by_backtick = true;
                    break;
                }
                window.push(*character);
            }
            let window = window.to_lowercase();
            if !names_any(&window, &EMPTINESS) || names_any(&window, &QUALIFIERS) {
                continue;
            }
            let anchored = cut_by_backtick
                && window
                    .split_whitespace()
                    .next_back()
                    .is_some_and(|last| ANCHORS.contains(&last));
            if anchored {
                continue;
            }
            claims.push(EmptinessClaim {
                line: index + 1,
                command,
            });
        }
    }
    claims
}

/// A quoted command this test is prepared to run itself, or the reason it is not.
enum Search {
    Runnable {
        pattern: String,
        insensitive: bool,
        roots: Vec<String>,
    },
    Unreadable,
}

/// Read a quoted `grep` conservatively, refusing anything it does not fully
/// understand.
///
/// **The command is never handed to a shell**, and that is the whole design.
/// Text in a repository document is data, not an instruction — a document is
/// exactly the surface `.claude/rules/01-security-and-safety.md` says may not
/// redirect what an agent or a process does — so `grep -rn foo x | rm -rf y`
/// must be *unreadable*, not obeyed. The parser therefore admits one shape: a
/// short-flag `grep`, one literal pattern of word characters, and paths that
/// exist. A regex, an alternation, a pipe, a `--include`, a `git show` in front
/// of it: all refused, and refusing means "not evaluated" rather than "passed".
///
/// The cost is stated rather than hidden: claims quoting a command this cannot
/// read are counted and not checked, and the test asserts that some claim
/// *was* checked so the readable set can never quietly become empty.
fn parse_grep(command: &str) -> Search {
    if command.contains(['|', ';', '&', '$', '>', '<', '(', ')', '\\', '*', '"', '\'']) {
        return Search::Unreadable;
    }
    let mut tokens = command.split_whitespace();
    if tokens.next() != Some("grep") {
        return Search::Unreadable;
    }
    let mut recursive = false;
    let mut insensitive = false;
    let mut pattern = None;
    let mut roots = Vec::new();
    for token in tokens {
        if let Some(flags) = token.strip_prefix('-') {
            if pattern.is_some() || flags.is_empty() || flags.starts_with('-') {
                // A flag after the pattern, or a long option: not a shape this
                // understands well enough to run.
                return Search::Unreadable;
            }
            for flag in flags.chars() {
                match flag {
                    'r' | 'R' => recursive = true,
                    'i' => insensitive = true,
                    // Flags that change the report but not what matches.
                    'l' | 'n' | 'c' | 'h' | 'o' => {}
                    _ => return Search::Unreadable,
                }
            }
            continue;
        }
        if pattern.is_none() {
            if !token.chars().all(|c| c.is_alphanumeric() || c == '_') {
                // Anything that could be a regex is refused: this test matches
                // literally, and a literal reading of `struct Belief\b` finds
                // nothing where grep finds plenty.
                return Search::Unreadable;
            }
            pattern = Some(token.to_string());
            continue;
        }
        roots.push(token.to_string());
    }
    let Some(pattern) = pattern else {
        return Search::Unreadable;
    };
    if roots.is_empty() {
        // A recursive grep written with no path — the shape the self-model row
        // used — means the workspace, which is the root the correcting commit
        // searched. Without `-r` a pathless grep reads standard input and this
        // test would be inventing the claim's subject.
        if !recursive {
            return Search::Unreadable;
        }
        roots.push("backend/crates".to_string());
    }
    for root in &roots {
        let path = repository_root().join(root);
        if !path.exists() {
            return Search::Unreadable;
        }
        if path.is_dir() && !recursive {
            return Search::Unreadable;
        }
    }
    Search::Runnable {
        pattern,
        insensitive,
        roots,
    }
}

/// The files a readable `grep` would name.
///
/// Substring matching here, deliberately and in the one place it is right:
/// `grep` matches substrings, so a delimited match would answer a question the
/// document did not ask and could report a cell empty that the command it
/// quotes fills.
///
/// This file excludes itself. A test that quotes a document's search term would
/// otherwise satisfy or falsify the document's claim with its own prose, which
/// is the self-measurement the test above had to strip comments to avoid.
fn files_matching(pattern: &str, insensitive: bool, roots: &[String]) -> Vec<String> {
    let repository = repository_root();
    let needle = if insensitive {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };
    let mut found = Vec::new();
    let mut stack: Vec<std::path::PathBuf> =
        roots.iter().map(|root| repository.join(root)).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == "target" || name == ".git" || name == "node_modules")
            {
                continue;
            }
            for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                stack.push(entry.path());
            }
            continue;
        }
        if path.ends_with("qip-acceptance/tests/documentation.rs") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let haystack = if insensitive {
            content.to_lowercase()
        } else {
            content
        };
        if haystack.contains(&needle) {
            let shown = path.strip_prefix(&repository).unwrap_or(path.as_path());
            found.push(shown.display().to_string());
        }
    }
    found.sort();
    found
}

/// What one document's emptiness claims are worth: how many were made, how many
/// this test could run, and which of them the tree contradicts.
fn audit_emptiness_claims(document: &str, text: &str) -> (usize, usize, Vec<String>) {
    let claims = emptiness_claims(text);
    let mut evaluated = 0usize;
    let mut contradicted = Vec::new();
    for claim in &claims {
        let Search::Runnable {
            pattern,
            insensitive,
            roots,
        } = parse_grep(&claim.command)
        else {
            continue;
        };
        evaluated += 1;
        let matches = files_matching(&pattern, insensitive, &roots);
        if !matches.is_empty() {
            contradicted.push(format!(
                "{document}:{}: the row quotes `{}` as finding nothing, and it names {} file(s), \
                 among them {:?}",
                claim.line,
                claim.command,
                matches.len(),
                matches.iter().take(3).collect::<Vec<_>>()
            ));
        }
    }
    (claims.len(), evaluated, contradicted)
}

#[test]
fn no_scored_plan_document_calls_a_search_empty_that_is_not() {
    let mut claims = 0usize;
    let mut evaluated = 0usize;
    let mut contradicted = Vec::new();
    for document in SCORED_PLAN_DOCUMENTS {
        let text = read(document);
        // The premise, per document. `read` panics on a missing file, so what
        // is left to prove is that the file is the register and not a stub
        // someone left behind a rename.
        //
        // The floor was 200 until 2026-09-07 and was calibrated to the two
        // large registers this test started with. It refused
        // `wave-7-backlog.md` at 186 lines — a real document carrying a real
        // false claim, rejected for being short. A premise guard that excludes
        // the evidence it was written to protect is worse than none, because
        // it fails loudly and gets *narrowed* rather than fixed: the cheapest
        // way out is dropping the document, which restores the blind spot this
        // whole gate exists to remove. 100 still refuses a stub left behind a
        // rename, and the assertions below — claims found, claims evaluated —
        // are what actually prevent this passing on an empty corpus.
        assert!(
            text.lines().count() > 100,
            "{document} has only {} lines, which is not the scored register this test was written \
             to read",
            text.lines().count()
        );
        let (found, ran, wrong) = audit_emptiness_claims(document, &text);
        claims += found;
        evaluated += ran;
        contradicted.extend(wrong);
    }

    // The vacuity guards, and both are load-bearing. The first fails if the
    // documents stop writing the shape — which is how the test above could
    // have been left reading nothing. The second fails if every claim becomes
    // unreadable to the conservative parser, at which point this is a test that
    // recognises sentences and checks none of them.
    assert!(
        claims > 0,
        "no cell in the scored plan documents claims a quoted command comes back empty, so this \
         test is reading none of the sentences it was written for"
    );
    assert!(
        evaluated > 0,
        "{claims} emptiness claims were found and none was in a shape this test could run, so \
         nothing was checked against the tree"
    );

    assert!(
        contradicted.is_empty(),
        "a scored plan document quotes a command as evidence that something is absent, and the \
         command finds it: {contradicted:#?}"
    );
}

#[test]
fn the_emptiness_claim_reader_fires_on_the_cell_that_disproved_itself_and_not_on_an_honest_missing_row()
 {
    // Written out rather than left to the documents, for the reason the
    // delimiter test above is: the rows that would prove each branch have been
    // corrected, and a gate whose only proof is a text nobody may restore is a
    // gate nobody can re-verify. Every string here is verbatim from
    // `docs/plan/completion-plan.md` before and after `8812982`.

    // The cell that disproved itself, at the revision it was written. It is a
    // contradiction because the type is there — the assertion is on the
    // *finding*, not merely on the shape, so a reader that recognised the row
    // and then ran nothing would fail here.
    let historical = "| 9 Self-model and exploration | Value of information measured | Nothing \
         (`grep -rln SelfModel` empty) | MISSING, deliberately | Phase 8 gate |";
    let (found, ran, wrong) = audit_emptiness_claims("completion-plan.md", historical);
    assert_eq!(
        found, 1,
        "the reader did not recognise the row that has already been wrong once"
    );
    assert_eq!(ran, 1, "the row's command was recognised and then not run");
    assert_eq!(
        wrong.len(),
        1,
        "the row claims a search is empty, the workspace defines the type it searches for, and the \
         reader reported no contradiction"
    );
    assert!(
        wrong[0].contains("completion-plan.md:1:"),
        "the failure does not name the row it is about: {wrong:?}"
    );

    // The same shape about something genuinely absent must stay silent, or the
    // gate fires on every honest register entry and is removed within the week.
    let (_, ran_absent, absent) = audit_emptiness_claims(
        "synthetic",
        "| 9 | x | Nothing (`grep -rln ZzNoSuchIdentifierZz` empty) | MISSING | gate |",
    );
    assert_eq!(ran_absent, 1, "the honest row's command was not run either");
    assert!(
        absent.is_empty(),
        "the reader contradicted a row whose search really is empty: {absent:?}"
    );

    // A row may say a deliverable is missing. What it may not do is assert a
    // search comes back empty when it does not. None of these three is a claim
    // about a command, and a reader that counted them would fire on the whole
    // register.
    for honest in [
        "| 12 Wallet and treasury | Every holding reconciled | Nothing beyond internal placement; \
         refused by ADR 0021 | MISSING, bounded | Separate owner decision |",
        "| 19 Market creation | Per class, on evidence | Nothing | MISSING, and the blueprint says \
         last | Phases 7, 8, 14 |",
        "`grep -rln SelfModel backend/crates` returns eight files, among them the learning engine \
         and the platform |",
    ] {
        let (found, _, wrong) = audit_emptiness_claims("synthetic", honest);
        assert_eq!(
            found, 0,
            "the reader treated an honest row as a claim about a command: {honest}"
        );
        assert!(
            wrong.is_empty(),
            "and reported it as contradicted: {wrong:?}"
        );
    }

    // The four disqualifiers, each on the real line that motivated it. All are
    // claims the tree would contradict if they were evaluated, so a reader that
    // dropped any one of these rules turns this repository's own corrections
    // into failures.
    for excluded in [
        // Reported speech: the correction quoting the cell it replaced.
        "| 9 | x | **Corrected 2026-09-07: this cell read \"Nothing (`grep -rln SelfModel` empty)\" \
         and its own command disproves it** |",
        // A qualifier: a claim about what the matches are.
        "`grep -n -i SelfModel backend/crates` finds nothing but a doc comment |",
        // An anchor: a claim about a commit, not about the working tree.
        "`grep -rln SelfModel backend/crates` is still empty at `296e187` |",
        // No emptiness word: the ordinary citation, which is evidence that
        // something is present.
        "`grep -rln SelfModel backend/crates` names the composed self-model |",
    ] {
        let (found, _, wrong) = audit_emptiness_claims("synthetic", excluded);
        assert_eq!(
            found, 0,
            "the reader made a claim out of a line that makes none: {excluded}"
        );
        assert!(wrong.is_empty(), "and contradicted it: {wrong:?}");
    }

    // And the parser refuses what it cannot read rather than guessing. A shell
    // pipeline is the case that matters: document text is data, and a test that
    // executed it would be a repository document choosing what a process runs.
    for unreadable in [
        "grep -rn \"struct Belief\\b\" backend/crates",
        "git show HEAD:x | grep -n foo",
        "grep -rn Foo backend/crates --include=*.rs",
        "grep -c -i alpaca",
        "grep -rn Foo no/such/path",
    ] {
        assert!(
            matches!(parse_grep(unreadable), Search::Unreadable),
            "the parser claims it can run {unreadable}, which it cannot read safely"
        );
    }
    // The other direction, which matters just as much: a parser that refused
    // everything would satisfy every assertion above and check nothing.
    assert!(
        matches!(parse_grep("grep -rln SelfModel"), Search::Runnable { .. }),
        "the parser refuses the exact command the row that has already been wrong once quoted"
    );
}
