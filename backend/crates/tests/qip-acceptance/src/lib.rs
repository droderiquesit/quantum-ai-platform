//! `qip-acceptance` — workspace-level tests.
//!
//! This crate has no library code. It exists so that tests spanning several
//! crates, and tests checking things outside the Rust code entirely, have
//! somewhere to live that participates in `cargo test --workspace`.
//!
//! The tests are in `tests/`:
//!
//! * `infrastructure.rs` checks the Terraform and Kubernetes configuration
//!   structurally. `terraform validate` catches a malformed configuration;
//!   these catch a well-formed one that would deploy something unsafe.
//! * `documentation.rs` checks that what the documentation claims matches what
//!   the code does. Documentation that has drifted is worse than none, because
//!   someone will believe it.
//! * `architecture.rs` reads the dependency graph and asserts the edges that
//!   are *absent*. A present edge is visible in the code that uses it; an
//!   absent one is invisible until someone adds it.
//! * `acceptance.rs` is the end-to-end scenario.
//! * `resilience.rs` is the same platform under load, a degraded feed and an
//!   operator pulling things out from under it.

/// The repository root, found by walking up from this crate.
///
/// Tests read files from the repository, and `CARGO_MANIFEST_DIR` is the only
/// path that is correct whether `cargo test` was run from the root or from the
/// crate.
// Test-support crate: every caller is a test, where a panic is the failure report.
#[allow(clippy::expect_used)]
pub fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("the crate is four levels below the repository root: backend/crates/tests/qip-acceptance")
        .to_path_buf()
}

/// Read a file relative to the repository root, failing loudly if it is
/// missing.
///
/// A test that silently skips when its input is absent is a test that passes
/// after someone deletes the thing it was checking.
pub fn read(relative: &str) -> String {
    let path = repository_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Every file under a directory, recursively, with the given extension.
pub fn files_with_extension(relative: &str, extension: &str) -> Vec<std::path::PathBuf> {
    let root = repository_root().join(relative);
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The production text of a Rust source file: comment lines dropped and every
/// `#[cfg(test)]` item skipped by brace depth, wherever in the file it sits.
///
/// For the scans that assert a name is *absent* from production code. Cutting
/// at the first `#[cfg(test)]` instead would stop reading at a test-only
/// helper near the top of a file and call everything below it unscanned
/// production clean; keeping the tests would report a fixture as a violation.
/// A caller asserts the result is non-empty for a file it expects to hold
/// production code, because a helper that returned nothing would make every
/// absence trivially true.
///
/// Braces inside string and character literals do not count. Until
/// 2026-10-31 they did, and a test fixture reading `"{ this is not a limits
/// file"` in three composition roots left the test module's depth at one
/// through the end of the file, so anything written below that module would
/// have been stripped as test code and never scanned. Nothing was below it
/// then; the point is that nothing could have been found there.
pub fn production_text(source: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.trim() != "#[cfg(test)]" {
            kept.push(line);
            continue;
        }
        // Skip the attributed item. One with no body ends at its semicolon;
        // one with a body ends where its braces balance.
        let mut depth = 0usize;
        let mut opened = false;
        let mut in_string = Text::Code;
        for body in lines.by_ref() {
            // A comment line inside the item has no code braces, and a
            // quotation mark in one is prose, not an open string.
            if in_string == Text::Code && body.trim_start().starts_with("//") {
                continue;
            }
            let (opens, closes) = code_braces(body, &mut in_string);
            depth += opens;
            opened |= depth > 0;
            depth = depth.saturating_sub(closes);
            if (opened && depth == 0) || (!opened && body.trim_end().ends_with(';')) {
                break;
            }
        }
    }
    kept.join("\n")
}

/// Where a scan of one line stands between lines: in code, inside an
/// ordinary `"…"` string, or inside a raw string `r"…"` / `r#"…"#` whose
/// closing quote must be followed by that many `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Text {
    Code,
    Quoted,
    Raw(usize),
}

/// The braces on one line that are code, not text: `{` and `}` outside every
/// string and `'…'` character literal. `state` carries an open string
/// literal across lines and comes back updated. A lifetime (`'a`) is not a
/// character literal: a `'` is one only when the closing `'` follows within
/// one character or one escape. A raw string has no escapes and ends only at
/// `"` followed by its own number of `#`, so a lone `"` or a trailing `\`
/// inside one is text.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn code_braces(line: &str, state: &mut Text) -> (usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    let (mut opens, mut closes) = (0usize, 0usize);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match *state {
            Text::Quoted => match c {
                '\\' => i += 1,
                '"' => *state = Text::Code,
                _ => {}
            },
            Text::Raw(hashes) => {
                if c == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
                    *state = Text::Code;
                    i += hashes;
                }
            }
            Text::Code => match c {
                '"' => *state = Text::Quoted,
                // `r"…"` or `br"…"`, not the `r` inside an identifier.
                'r' if i == 0 || chars[i - 1] == 'b' || !is_ident_char(chars[i - 1]) => {
                    let mut hashes = 0;
                    while chars.get(i + 1 + hashes) == Some(&'#') {
                        hashes += 1;
                    }
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        *state = Text::Raw(hashes);
                        i += 1 + hashes;
                    }
                }
                '\'' => {
                    let close = if chars.get(i + 1) == Some(&'\\') {
                        i + 3
                    } else {
                        i + 2
                    };
                    if chars.get(close) == Some(&'\'') {
                        i = close;
                    }
                }
                '{' => opens += 1,
                '}' => closes += 1,
                _ => {}
            },
        }
        i += 1;
    }
    (opens, closes)
}

#[cfg(test)]
mod tests {
    use super::production_text;

    /// A brace inside a test fixture's string once swallowed everything
    /// below the test module; a production item written after it was
    /// unscanned. Mutation: count braces with `matches('{')` again in
    /// `production_text` — the trailing `fn after_tests` is stripped and
    /// this fails.
    #[test]
    fn a_brace_inside_a_test_strings_literal_does_not_swallow_the_production_code_below_it() {
        let source = "fn before() {}\n#[cfg(test)]\nmod tests {\n    const BAD: &str = \"{ not json\";\n    const C: char = '{';\n    fn t<'a>() {}\n}\nfn after_tests() {}\n";
        let kept = production_text(source);
        assert!(kept.contains("fn before()"));
        assert!(!kept.contains("not json"), "{kept}");
        assert!(kept.contains("fn after_tests()"), "{kept}");
    }

    /// A string literal spanning lines carries its state across them, so a
    /// brace on its second line is still text. Mutation: reset the state to
    /// `Text::Code` at the top of each line in `production_text`'s skip loop
    /// — the `{` on the second line opens a depth nothing closes and
    /// `fn production` is stripped.
    #[test]
    fn a_string_literal_spanning_lines_keeps_its_braces_as_text() {
        let source = "#[cfg(test)]\nmod tests {\n    const S: &str = \"line one\n{ line two\";\n}\nfn production() {}\n";
        let kept = production_text(source);
        assert!(kept.contains("fn production()"), "{kept}");
        assert!(!kept.contains("line two"), "{kept}");
    }

    /// A raw string with an odd number of quotes, or one ending in a
    /// backslash, has no escapes and ends only at its own closing sequence;
    /// read as an ordinary string either leaves the scanner inside a string
    /// it never leaves, and everything below the test module is stripped.
    /// Mutation: in `code_braces`, make `'r'` fall through to the ordinary
    /// `'"'` arm (delete the `'r'` arm) — `fn production` is stripped.
    #[test]
    fn a_raw_string_with_an_odd_quote_or_a_trailing_backslash_does_not_swallow_the_code_below() {
        let odd = "#[cfg(test)]\nmod tests {\n    const S: &str = r#\"name=\"\"#;\n    const T: &str = r\"C:\\dir\\\";\n    const U: &str = br\"{\";\n}\nfn production() {}\n";
        let kept = production_text(odd);
        assert!(kept.contains("fn production()"), "{kept}");
        assert!(!kept.contains("name="), "{kept}");
    }

    /// A comment line inside a test module is prose: an unbalanced `"` in
    /// it opens no string and a `{` in it opens no block. Mutation: feed
    /// comment lines to `code_braces` (drop the `starts_with("//")` skip in
    /// the item loop) — the `"` in the doc line opens a string the `}`
    /// closes nothing in, and `fn production` is stripped.
    #[test]
    fn a_comment_inside_a_test_module_opens_no_string_and_no_block() {
        let source = "#[cfg(test)]\nmod tests {\n    /// matches a lone \" and a stray {\n    fn t() {}\n}\nfn production() {}\n";
        let kept = production_text(source);
        assert!(kept.contains("fn production()"), "{kept}");
        assert!(!kept.contains("fn t()"), "{kept}");
    }
}
