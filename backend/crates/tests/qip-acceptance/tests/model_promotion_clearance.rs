//! R12-MODEL-4021 (blueprint §10.1): a model reaches production only on a
//! promotion clearance its adversarial perturbation suite was measured for,
//! or on the log's record of a promotion that spent one.
//!
//! `qip_ai::registry::PromotionClearance` and `JournaledAdmission` have
//! private fields and no `Deserialize`, `Default` or `Clone`. The registry
//! moves a card into production only through `promote_artifact` (which spends
//! a clearance), `readmit_on_journaled_admission` (which spends an admission) and the rollback and
//! reactivation of bytes it admitted; `promote` and `get_mut` are private and
//! `register` refuses an unadmitted production card. Rust cannot restrict a
//! public constructor to one other crate, so this scan is what makes each
//! proof mean what its name says: a clearance is minted only where the
//! kernel has just run every promotion test, the attack included; an
//! admission only where the kernel has just found the promotion on its log;
//! and the provider a clearance names is one of the providers production
//! code can serve through. It follows the pattern ADR 0430 set for
//! `qip_risk::gate::DeterministicGate`.
//!
//! Matching is by delimited token over every occurrence, on production text
//! with comments, string literals and `#[cfg(test)]` items removed, so a
//! name quoted in prose or in an error message neither satisfies nor trips a
//! check, and a second call site is counted rather than hidden behind the
//! first. Tests (`tests/` directories and `#[cfg(test)]` items) may mint
//! either proof: a test is not a path a model reaches production by.

/// The one production source that may mint a clearance.
const CLEARANCE_MINTER: &str = "backend/crates/runtime/qip-kernel/src/central/models.rs";
/// The one production source that may mint an admission from the log.
const ADMISSION_MINTER: &str = "backend/crates/runtime/qip-kernel/src/model_serving.rs";
/// Where both proof types, their constructors and the demands live.
const REGISTRY: &str = "backend/crates/libs/qip-ai/src/registry.rs";
/// The production sources that may implement `ModelProvider`: the interface's
/// own `NoProvider` and the in-tree provider the two central roots serve by.
const PROVIDERS: [&str; 2] = [
    "backend/crates/libs/qip-ai/src/serving.rs",
    "backend/crates/services/qip-training/src/serve.rs",
];
const DEEP_BRAIN: &str = "backend/crates/apps/qip-deepbrain/src/learning.rs";

fn relative(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    display
        .rsplit_once("backend/crates/")
        .map_or(display.clone(), |(_, tail)| {
            format!("backend/crates/{tail}")
        })
}

/// Production text with trailing `//` comments and the contents of string
/// and character literals blanked, so only code is left to match.
fn code(source: &str) -> String {
    let production = qip_acceptance::production_text(source);
    let mut out = String::with_capacity(production.len());
    for line in production.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut previous = '\0';
        for character in line.chars() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                    out.push('"');
                }
                previous = character;
                continue;
            }
            if character == '/' && previous == '/' {
                out.pop();
                break;
            }
            if character == '"' {
                in_string = true;
            }
            out.push(character);
            previous = character;
        }
        out.push('\n');
    }
    out
}

/// Every offset where `token` begins as a whole identifier.
fn token_offsets(text: &str, token: &str) -> Vec<usize> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(token)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + token.len()..].chars().next();
            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        })
        .map(|(at, _)| at)
        .collect()
}

/// Calls of `name` — the token followed by `(` — that are not its definition.
fn calls(text: &str, name: &str) -> usize {
    token_offsets(text, name)
        .into_iter()
        .filter(|at| text[at + name.len()..].trim_start().starts_with('('))
        .filter(|at| !text[..*at].trim_end().ends_with("fn"))
        .count()
}

/// The production sources of the workspace, read once.
fn production_sources() -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let relative = relative(&path);
        if !relative.contains("/src/") {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
        sources.push((relative, code(&source)));
    }
    assert!(
        sources.len() > 100,
        "premise: the scan read the workspace's production sources, and it read {}",
        sources.len()
    );
    sources
}

/// Where `name` is called in production code, by file, with the count.
fn call_sites(sources: &[(String, String)], name: &str) -> Vec<(String, usize)> {
    sources
        .iter()
        .filter_map(|(path, text)| {
            let count = calls(text, name);
            (count > 0).then(|| (path.clone(), count))
        })
        .collect()
}

/// The body of the one `fn name(` in `text`, through its balanced braces.
fn function_body<'a>(text: &'a str, name: &str) -> &'a str {
    let definitions: Vec<usize> = token_offsets(text, name)
        .into_iter()
        .filter(|at| text[..*at].trim_end().ends_with("fn"))
        .collect();
    assert_eq!(
        definitions.len(),
        1,
        "premise: exactly one `fn {name}` is defined in the file scanned"
    );
    let start = definitions[0];
    let open = start
        + text[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`fn {name}` has no body"));
    let mut depth = 0usize;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`fn {name}` never closes")
}

/// Mutations: call `PromotionClearance::minted_by_measured_promotion_tests`
/// in `qip-kernel/src/model_serving.rs` — that file is named as an offender;
/// call `JournaledAdmission::minted_from_the_promotion_log` in the deep
/// brain's `learning.rs` — the same; add a second mint in `central/models.rs`
/// — the count fires.
#[test]
fn each_promotion_proof_is_minted_at_one_production_site_and_nowhere_else() {
    let sources = production_sources();
    for (name, minter) in [
        ("minted_by_measured_promotion_tests", CLEARANCE_MINTER),
        ("minted_from_the_promotion_log", ADMISSION_MINTER),
    ] {
        let sites = call_sites(&sources, name);
        let offenders: Vec<&(String, usize)> =
            sites.iter().filter(|(path, _)| path != minter).collect();
        assert!(
            offenders.is_empty(),
            "`{name}` is called in production code outside {minter}: {offenders:?}. Each \
             promotion proof is the measurement or the log record itself; obtain one through \
             the kernel (R12-MODEL-4021)"
        );
        assert_eq!(
            sites,
            vec![(minter.to_string(), 1)],
            "`{name}` must be called exactly once in production, in {minter}; with none, \
             nothing could ever be promoted or restored"
        );
    }
}

/// The reader's provider finding: a clearance names the provider whose
/// arithmetic it measured and `promote_model_from` refuses any other name,
/// which holds only while no production code can implement a provider that
/// borrows a served provider's name. So production implementations of
/// `ModelProvider` are confined to the two files that hold them today.
///
/// Mutation: add `impl ModelProvider for` a lenient type in
/// `qip-kernel/src/model_serving.rs` — that file is named as an offender.
#[test]
fn production_code_serves_models_through_the_known_providers_alone() {
    let sources = production_sources();
    let mut implementors = Vec::new();
    for (path, text) in &sources {
        for at in token_offsets(text, "ModelProvider") {
            let after = text[at + "ModelProvider".len()..].trim_start();
            let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
            let before = &text[line_start..at];
            if after.starts_with("for ") && before.trim_start().starts_with("impl") {
                implementors.push(path.clone());
            }
        }
    }
    assert!(
        PROVIDERS
            .iter()
            .all(|provider| implementors.iter().any(|path| path == provider)),
        "premise: the scan found both known providers, and it found {implementors:?}"
    );
    let offenders: Vec<&String> = implementors
        .iter()
        .filter(|path| !PROVIDERS.contains(&path.as_str()))
        .collect();
    assert!(
        offenders.is_empty(),
        "production code implements `ModelProvider` outside {PROVIDERS:?}: {offenders:?}; a \
         provider whose name a clearance can carry is a provider every clearance check trusts"
    );
}

/// The readers' first two findings, as regressions: `ModelRegistry::promote`
/// and `get_mut` were public, so any holder of a registry could set a card's
/// stage. Both are private now; a `pub` on either is the door reopened. And
/// the registry's restart door is `readmit_on_journaled_admission`, called in
/// production only by
/// the kernel's log-backed readmission.
///
/// Mutations: make `fn promote` or `fn get_mut` `pub` again — the visibility
/// assertion fires; call `.readmit_on_journaled_admission(` in the deep
/// brain — it is named.
#[test]
fn no_public_registry_door_moves_a_card_into_production() {
    let registry = code(&qip_acceptance::read(REGISTRY));
    for name in ["promote", "get_mut"] {
        let definitions: Vec<usize> = token_offsets(&registry, name)
            .into_iter()
            .filter(|at| registry[..*at].trim_end().ends_with("fn"))
            .collect();
        assert_eq!(
            definitions.len(),
            1,
            "premise: the registry defines `fn {name}` once"
        );
        let line_start = registry[..definitions[0]].rfind('\n').map_or(0, |n| n + 1);
        let declaration = registry[line_start..definitions[0]].trim();
        assert_eq!(
            declaration, "fn",
            "`ModelRegistry::{name}` is declared `{declaration} {name}`: a visible `{name}` \
             lets any holder of a registry put a card into production with no clearance"
        );
    }
    let sources = production_sources();
    let readmitters: Vec<(String, usize)> = call_sites(&sources, "readmit_on_journaled_admission")
        .into_iter()
        .filter(|(path, _)| path != REGISTRY)
        .collect();
    assert_eq!(
        readmitters,
        vec![(ADMISSION_MINTER.to_string(), 1)],
        "`ModelRegistry::readmit_on_journaled_admission` is called in production code outside the kernel's \
         log-backed readmission"
    );
}

/// The vacuity guards: a proof nothing demands, or one a caller can obtain
/// without the constructor, protects nothing. The behavioural half is in
/// `qip-ai/tests/model_infrastructure.rs` and
/// `qip-kernel/tests/promotion_tests.rs`; this half holds the shapes those
/// tests cannot see — derives, field visibility, and the deep brain's order.
///
/// Mutations: derive `Clone` on either proof — the derive assertion fires;
/// make a field `pub` — the field assertion fires; promote in the deep brain
/// before testing — the order assertion fires; restore through
/// `self.registry.register(` — the restore assertion fires.
#[test]
fn the_proofs_cannot_be_copied_or_decoded_and_the_deep_brain_obtains_them() {
    let source = qip_acceptance::read(REGISTRY);
    let production = qip_acceptance::production_text(&source);
    for declaration in [
        "pub struct PromotionClearance {",
        "pub struct JournaledAdmission {",
    ] {
        let at = production
            .find(declaration)
            .unwrap_or_else(|| panic!("premise: the registry declares `{declaration}`"));
        let derive = production[..at]
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with("#[derive("))
            .expect("premise: the proof carries a derive");
        assert_eq!(
            derive.trim(),
            "#[derive(Debug, PartialEq, Eq)]",
            "`{declaration}` must not derive Clone, Default, Serialize or Deserialize: each \
             is a way to hold one the constructor did not hand out"
        );
        let body = at + declaration.len();
        let fields =
            &production[body..body + production[body..].find('}').expect("premise: a body")];
        assert!(
            fields.contains("artifact_digest: String"),
            "premise: the field list was read: {fields}"
        );
        assert!(
            token_offsets(fields, "pub").is_empty(),
            "a public field on a proof lets a caller relabel one: {fields}"
        );
    }

    let registry = code(&source);
    let promote = function_body(&registry, "promote_artifact");
    let demanded = token_offsets(promote, "clearances")
        .first()
        .copied()
        .expect("`promote_artifact` no longer reads the clearance it demands");
    let moved = promote
        .find("self.promote(")
        .expect("premise: `promote_artifact` moves the card through `promote`");
    assert!(
        demanded < moved,
        "`promote_artifact` moves the card before it reads the clearance"
    );

    let desk = code(&qip_acceptance::read(DEEP_BRAIN));
    let round = function_body(&desk, "shadow_round");
    let tested = round
        .find("test_model_and_student_for_promotion(")
        .expect("the deep brain no longer runs the promotion tests with the student");
    let promoted = round
        .find("promote_model_from(")
        .expect("the deep brain no longer promotes through the kernel");
    assert!(
        tested < promoted,
        "the deep brain promotes before it runs the promotion tests"
    );
    assert_eq!(
        calls(&desk, "promote_model_from"),
        1,
        "the deep brain promotes at a second site that this order check does not read"
    );
    let restore = function_body(&desk, "restore_registry");
    assert_eq!(
        calls(restore, "readmit_promoted_card"),
        1,
        "the deep brain's restore no longer readmits through the log"
    );
    assert!(
        !restore.contains("self.registry.register("),
        "the deep brain's restore registers a card without the log's admission"
    );
}
