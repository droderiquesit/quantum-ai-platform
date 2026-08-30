//! robots.txt precedence, which is the part that is easy to get subtly wrong.
//!
//! Each test uses several overlapping rules rather than one, because a parser
//! that applies rules in file order passes every single-rule test and blocks
//! the entire site the first time a publisher writes `Disallow: /` above an
//! `Allow:` for the one path they meant to open.

#![allow(clippy::panic_in_result_fn)]

use qip_core::Duration;
use qip_core::error::Result;
use qip_data_finder::robots::{PathVerdict, RobotsPolicy};

const AGENT: &str = "qip-data-finder/1.0";

/// A publisher who blocks the site, opens one directory, and closes one file
/// inside it — three overlapping rules on every path under `/data`.
fn overlapping() -> RobotsPolicy {
    RobotsPolicy::parse(
        "User-agent: *\n\
         Disallow: /\n\
         Allow: /data/\n\
         Disallow: /data/private/\n\
         Crawl-delay: 5\n",
    )
}

#[test]
fn the_longest_matching_rule_decides_rather_than_the_first_one_in_the_file() -> Result<()> {
    let policy = overlapping();

    // `/` and `/data/` both match; the longer one is the Allow.
    assert_eq!(
        policy.verdict(AGENT, "/data/prices.json"),
        PathVerdict::AllowedByRule {
            pattern: "/data/".to_string()
        }
    );
    // All three match; the longest is the inner Disallow.
    assert_eq!(
        policy.verdict(AGENT, "/data/private/keys.json"),
        PathVerdict::DisallowedByRule {
            pattern: "/data/private/".to_string()
        }
    );
    // Only `Disallow: /` matches.
    assert_eq!(
        policy.verdict(AGENT, "/admin"),
        PathVerdict::DisallowedByRule {
            pattern: "/".to_string()
        }
    );
    Ok(())
}

#[test]
fn allow_beats_disallow_when_the_two_patterns_are_the_same_length() -> Result<()> {
    // Same length, opposite senses, and the Disallow written first. A parser
    // that keeps the first match, or the last, gets one of these wrong.
    let disallow_first = RobotsPolicy::parse("User-agent: *\nDisallow: /feed/x\nAllow: /feed/x\n");
    let allow_first = RobotsPolicy::parse("User-agent: *\nAllow: /feed/x\nDisallow: /feed/x\n");

    for policy in [&disallow_first, &allow_first] {
        assert_eq!(
            policy.verdict(AGENT, "/feed/x/1"),
            PathVerdict::AllowedByRule {
                pattern: "/feed/x".to_string()
            },
            "an equal-length Allow must win regardless of file order"
        );
    }
    Ok(())
}

#[test]
fn a_group_naming_this_crawler_replaces_the_wildcard_group_entirely() -> Result<()> {
    // The wildcard group is permissive and the specific one is not. A parser
    // that merges them would let the wildcard's Allow override.
    let policy = RobotsPolicy::parse(
        "User-agent: *\n\
         Allow: /\n\
         \n\
         User-agent: qip-data-finder/1.0\n\
         Disallow: /data/\n",
    );

    assert_eq!(
        policy.verdict(AGENT, "/data/prices.json"),
        PathVerdict::DisallowedByRule {
            pattern: "/data/".to_string()
        }
    );
    // A different crawler still gets the wildcard group.
    assert_eq!(
        policy.verdict("some-other-bot", "/data/prices.json"),
        PathVerdict::AllowedByRule {
            pattern: "/".to_string()
        }
    );
    Ok(())
}

#[test]
fn consecutive_user_agent_lines_share_one_group_and_a_rule_starts_the_next() -> Result<()> {
    let policy = RobotsPolicy::parse(
        "User-agent: alpha\n\
         User-agent: beta\n\
         Disallow: /shared/\n\
         User-agent: gamma\n\
         Allow: /shared/\n",
    );

    assert_eq!(policy.groups().len(), 2);
    assert!(
        policy.verdict("alpha", "/shared/x").is_forbidden_for_test(),
        "the first group binds both agents named above its rules"
    );
    assert!(policy.verdict("beta", "/shared/x").is_forbidden_for_test());
    assert!(!policy.verdict("gamma", "/shared/x").is_forbidden_for_test());
    Ok(())
}

#[test]
fn wildcards_and_the_end_anchor_match_the_way_the_standard_says() -> Result<()> {
    let policy = RobotsPolicy::parse(
        "User-agent: *\n\
         Allow: /\n\
         Disallow: /*.csv$\n\
         Disallow: /tmp/*/scratch\n",
    );

    assert!(
        policy
            .verdict(AGENT, "/exports/daily.csv")
            .is_forbidden_for_test(),
        "a star must span path separators"
    );
    assert!(
        policy
            .verdict(AGENT, "/a.csv/b.csv")
            .is_forbidden_for_test(),
        "an anchored suffix must be matched from the right, not at its first occurrence"
    );
    assert!(
        !policy
            .verdict(AGENT, "/exports/daily.csv.gz")
            .is_forbidden_for_test(),
        "the end anchor must stop the rule matching a longer path"
    );
    assert!(
        policy
            .verdict(AGENT, "/tmp/17/scratch")
            .is_forbidden_for_test(),
        "a star in the middle must match one segment"
    );
    Ok(())
}

#[test]
fn an_empty_disallow_value_restricts_nothing() -> Result<()> {
    // The documented way to say "everything is permitted". Recorded as a
    // zero-length pattern it would match every path instead.
    let policy = RobotsPolicy::parse("User-agent: *\nDisallow:\n");
    assert_eq!(
        policy.verdict(AGENT, "/anything"),
        PathVerdict::AllowedByAbsenceOfRule
    );
    Ok(())
}

#[test]
fn the_crawl_delay_binding_this_crawler_is_the_slowest_one_stated() -> Result<()> {
    assert_eq!(
        overlapping().crawl_delay_for(AGENT),
        Some(Duration::from_secs(5))
    );

    // Two groups both name us and disagree. Obeying the faster one would
    // breach the slower one.
    let contradictory = RobotsPolicy::parse(
        "User-agent: qip-data-finder/1.0\n\
         Crawl-delay: 2\n\
         Disallow: /a\n\
         User-agent: qip-data-finder/1.0\n\
         Crawl-delay: 9\n\
         Disallow: /b\n",
    );
    assert_eq!(
        contradictory.crawl_delay_for(AGENT),
        Some(Duration::from_secs(9))
    );

    // A fractional delay is legal and common.
    let fractional = RobotsPolicy::parse("User-agent: *\nCrawl-delay: 0.5\n");
    assert_eq!(
        fractional.crawl_delay_for(AGENT),
        Some(Duration::from_millis(500))
    );
    Ok(())
}

#[test]
fn comments_and_malformed_lines_do_not_discard_the_rules_that_parsed() -> Result<()> {
    // Publishers hand-write these files. A parser that gives up on the first
    // bad line turns a typo on their server into a source we cannot assess.
    let policy = RobotsPolicy::parse(
        "# a comment\n\
         User-agent: *   # trailing comment\n\
         this line has no colon\n\
         Disallow: /private/\n\
         Sitemap: https://example.com/sitemap.xml\n",
    );

    assert!(policy.verdict(AGENT, "/private/x").is_forbidden_for_test());
    assert_eq!(policy.sitemaps(), ["https://example.com/sitemap.xml"]);
    Ok(())
}

#[test]
fn a_served_file_that_addresses_nobody_is_reported_as_having_no_group() -> Result<()> {
    let policy = RobotsPolicy::parse("User-agent: googlebot\nDisallow: /\n");
    assert_eq!(
        policy.verdict(AGENT, "/data"),
        PathVerdict::NoApplicableGroup
    );
    Ok(())
}

/// Local helper so the assertions above read as questions about permission.
trait VerdictExt {
    fn is_forbidden_for_test(&self) -> bool;
}

impl VerdictExt for PathVerdict {
    fn is_forbidden_for_test(&self) -> bool {
        !self.permits()
    }
}
