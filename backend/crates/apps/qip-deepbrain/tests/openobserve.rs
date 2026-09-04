//! The copied OpenObserve drain, and the only thing about it that is this
//! crate's own: that it is still the same code.

use std::path::Path;

/// Every line of a drain module from its first `use` — the module doc above
/// it is the one part that is deliberately per-crate, because what each root
/// can reach differs and saying so is the point of having a doc.
fn below_the_header(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    let start = text
        .lines()
        .position(|line| line.starts_with("use "))
        .unwrap_or_else(|| panic!("{} has no `use` line to split on", path.display()));
    text.lines().skip(start).map(str::to_string).collect()
}

/// Three composition roots carry the same drain because it has no shared home
/// it is allowed to live in, and a copy that drifts is the failure to fear
/// rather than a tidiness complaint. The credential refusals in
/// `OpenObserveConfig::parse` — an empty value, an embedded line break — were
/// added to the original after it was first written; a copy taken before that
/// would today accept a credential this one refuses, and nothing anywhere
/// would say so.
#[test]
fn the_copied_drain_is_still_line_for_line_the_qip_api_original_it_was_taken_from() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let origin = below_the_header(&here.join("../qip-api/src/openobserve.rs"));
    let copy = below_the_header(&here.join("src/openobserve.rs"));

    // The premise. A split that matched nothing would compare two empty
    // vectors and agree with every divergence there is.
    assert!(
        origin.len() > 300,
        "only {} line(s) were read out of the original; the header split has \
         stopped matching and this test compares almost nothing",
        origin.len()
    );
    assert_eq!(
        origin.len(),
        copy.len(),
        "the copy and the original are different lengths, so one has been \
         changed without the other"
    );

    let mut exempted = 0usize;
    for (index, (left, right)) in origin.iter().zip(copy.iter()).enumerate() {
        if left == right {
            continue;
        }
        // The only permitted difference, and it is not one in behaviour: the
        // original's doc links name `crate::mesh`, a module that exists in
        // qip-api and in no other root. The copy names it in prose, and may
        // not invent a local path that happens to resolve here.
        assert!(
            left.contains("crate::mesh") && !right.contains("crate::"),
            "line {} differs from the original for a reason other than the \
             mesh doc link:\n  original: {left}\n  copy:     {right}",
            index + 1
        );
        exempted += 1;
    }
    assert_eq!(
        exempted, 2,
        "the exemption covers exactly the two doc links naming crate::mesh; a \
         third means the rule has widened far enough to cover a real \
         divergence"
    );
}
