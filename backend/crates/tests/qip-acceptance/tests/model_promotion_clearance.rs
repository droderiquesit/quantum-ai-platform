//! R12-MODEL-4021 (blueprint §10.1): a model reaches production only on a
//! promotion clearance its adversarial perturbation suite was measured for.
//!
//! `qip_ai::registry::PromotionClearance` has private fields and no
//! `Deserialize`, `Default` or `Clone`, and `ModelRegistry::promote_artifact`
//! — so `Platform::promote_model` and `promote_model_from`, the doors the
//! deep brain's promote stage uses — refuses a promotion without one minted
//! for the same reference and bytes. Rust cannot restrict a public
//! constructor to one other crate, so this scan is what makes a clearance
//! mean "the kernel ran every promotion test, the attack included, and every
//! one passed" rather than "somebody called the constructor". It follows the
//! pattern ADR 0430 set for `qip_risk::gate::DeterministicGate`.
//!
//! Tests (`tests/` directories and `#[cfg(test)]` items) may mint one: a unit
//! test of the registry is not a path a model reaches production by.

/// The one production source that may mint a clearance.
const MINTER: &str = "backend/crates/runtime/qip-kernel/src/central/models.rs";
/// Where the type, its constructor and the demand live.
const REGISTRY: &str = "backend/crates/libs/qip-ai/src/registry.rs";

fn relative(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    display
        .rsplit_once("backend/crates/")
        .map_or(display.clone(), |(_, tail)| {
            format!("backend/crates/{tail}")
        })
}

/// The body of the first `fn name(` in `source`, through its balanced braces.
fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}(");
    let start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("no `{signature}` in the registry"));
    let open = start
        + source[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{signature}` has no body"));
    let mut depth = 0usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` never closes")
}

/// Mutation: call the constructor in `qip-kernel/src/model_serving.rs` — the
/// scan names that file. Remove the call from `run_promotion_tests` — the
/// minter count assertion fires.
#[test]
fn only_the_kernels_promotion_tests_mint_a_promotion_clearance() {
    let call = ["minted_by_measured_promotion_tests", "("].concat();
    let definition = ["fn ", &call].concat();

    let mut offenders = Vec::new();
    let mut minter_calls = 0;
    let mut scanned = 0;
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let relative = relative(&path);
        // Production source only: a crate's `src/`, never its `tests/`.
        if !relative.contains("/src/") {
            continue;
        }
        scanned += 1;
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
        let production = qip_acceptance::production_text(&source);
        let calls = production.matches(&call).count() - production.matches(&definition).count();
        if calls == 0 {
            continue;
        }
        if relative == MINTER {
            minter_calls += calls;
        } else {
            offenders.push(relative);
        }
    }
    assert!(
        scanned > 100,
        "premise: the scan read the workspace's production sources, and it read {scanned}"
    );
    assert!(
        offenders.is_empty(),
        "these mint a promotion clearance in production code without being the kernel's \
         `run_promotion_tests`: {offenders:?}. A clearance is the measurement itself; obtain \
         one by running the promotion tests (R12-MODEL-4021)"
    );
    assert_eq!(
        minter_calls, 1,
        "`run_promotion_tests` mints the clearance at {minter_calls} sites, not one; with \
         none, nothing could ever be promoted and every caller holds what nobody produces"
    );
}

/// The vacuity guards: a scan protecting a type nothing demands, or a type a
/// caller can obtain without the constructor, protects nothing.
///
/// Mutation: drop the clearance match from `promote_artifact` — the first
/// assertion fires. Derive `Clone` or `Deserialize` on the clearance — the
/// derive assertion fires.
#[test]
fn promotion_demands_the_clearance_and_the_clearance_cannot_be_obtained_another_way() {
    let source = qip_acceptance::read(REGISTRY);
    let production = qip_acceptance::production_text(&source);

    let promote = function_body(&production, "promote_artifact");
    assert!(
        promote.contains("self.clearances.get(&artifact.reference)"),
        "`ModelRegistry::promote_artifact` no longer demands a clearance: a passing record \
         nobody measured would promote again"
    );
    assert!(
        promote.contains("clearance.artifact_digest != artifact.digest"),
        "`promote_artifact` no longer binds the clearance to the bytes being promoted"
    );

    let declaration = "pub struct PromotionClearance {";
    let at = production
        .find(declaration)
        .expect("premise: the registry declares the clearance");
    let derive = production[..at]
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("#[derive("))
        .expect("premise: the clearance carries a derive");
    assert_eq!(
        derive.trim(),
        "#[derive(Debug, PartialEq, Eq)]",
        "the clearance must not derive Clone, Default, Serialize or Deserialize: each is a \
         way to hold one the constructor did not hand out"
    );
    let body = at + declaration.len();
    let fields = &production[body..body + production[body..].find('}').expect("premise: a body")];
    assert!(
        fields.contains("artifact_digest: String"),
        "premise: the field list was read: {fields}"
    );
    assert!(
        !fields.contains("pub "),
        "a public field on the clearance lets a caller relabel one: {fields}"
    );
    assert!(
        production.contains("pub fn lodge_clearance(&mut self, clearance: PromotionClearance)"),
        "the registry must take a clearance by value, so only what was minted can be lodged"
    );
    // The deep brain's promote stage measures through the kernel before it
    // promotes, so the clearance it is refused without is one it obtains.
    let desk = qip_acceptance::production_text(&qip_acceptance::read(
        "backend/crates/apps/qip-deepbrain/src/learning.rs",
    ));
    let tested = desk
        .find("test_model_for_promotion(")
        .expect("the deep brain no longer runs the promotion tests");
    let promoted = desk
        .find("promote_model_from(")
        .expect("the deep brain no longer promotes through the kernel");
    assert!(
        tested < promoted,
        "the deep brain promotes before it runs the promotion tests"
    );
}
