//! robots.txt, parsed and applied.
//!
//! The precedence rules are the whole point of this module, and getting them
//! subtly wrong is worse than not having it: a crawler that believes it is
//! obeying a publisher while ignoring the rule that mattered is a crawler
//! whose operator will say, truthfully and uselessly, that it checked.
//!
//! Two rules from RFC 9309 are implemented exactly:
//!
//! * **Longest match wins.** `Disallow: /` and `Allow: /public/` both match
//!   `/public/prices`, and the longer pattern decides. Applying the rules in
//!   file order instead would block the whole site.
//! * **`Allow` beats `Disallow` at equal length.** Where two rules of the same
//!   length both match, the permissive one wins, because that is what a
//!   publisher writing both means.
//!
//! What this module deliberately does *not* do is treat a missing robots.txt
//! as permission. That judgement lives in [`crate::legal`], because it is a
//! judgement about our own conduct rather than about the file.

use qip_core::Duration;
use serde::{Deserialize, Serialize};

/// One `Allow` or `Disallow` line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsRule {
    /// Whether the rule permits.
    pub allow: bool,
    /// The path pattern, which may contain `*` and end in `$`.
    pub pattern: String,
}

impl RobotsRule {
    /// Specificity, as RFC 9309 defines it: the octet length of the pattern.
    ///
    /// Length of the *pattern*, not of the matched span. `/a*` and `/ab` both
    /// match `/abc`; the standard makes them equally specific, and measuring
    /// the match instead would silently prefer wildcards.
    pub fn specificity(&self) -> usize {
        self.pattern.len()
    }

    /// Whether this rule's pattern matches `path`.
    pub fn matches(&self, path: &str) -> bool {
        pattern_matches(&self.pattern, path)
    }
}

/// One `User-agent` group and the rules under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotsGroup {
    agents: Vec<String>,
    rules: Vec<RobotsRule>,
    crawl_delay: Option<Duration>,
}

impl RobotsGroup {
    pub fn agents(&self) -> &[String] {
        &self.agents
    }

    pub fn rules(&self) -> &[RobotsRule] {
        &self.rules
    }

    pub fn crawl_delay(&self) -> Option<Duration> {
        self.crawl_delay
    }

    /// Whether this group is addressed to `agent`, which is already lowercase.
    fn addresses(&self, agent: &str) -> bool {
        self.agents.iter().any(|declared| declared == agent)
    }

    fn is_wildcard(&self) -> bool {
        self.agents.iter().any(|declared| declared == "*")
    }
}

/// Which rule decided, and how.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PathVerdict {
    /// A rule matched and permits.
    AllowedByRule { pattern: String },
    /// A rule matched and forbids.
    DisallowedByRule { pattern: String },
    /// A group addressed us and none of its rules matched this path.
    AllowedByAbsenceOfRule,
    /// The file was served but no group addresses this agent, not even `*`.
    NoApplicableGroup,
}

impl PathVerdict {
    pub fn permits(&self) -> bool {
        !matches!(self, Self::DisallowedByRule { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::AllowedByRule { pattern } => format!("allowed by `Allow: {pattern}`"),
            Self::DisallowedByRule { pattern } => format!("forbidden by `Disallow: {pattern}`"),
            Self::AllowedByAbsenceOfRule => {
                "the group addressing this crawler states no rule for this path".to_string()
            }
            Self::NoApplicableGroup => {
                "the served robots.txt addresses no group covering this crawler".to_string()
            }
        }
    }
}

/// A parsed robots.txt.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RobotsPolicy {
    groups: Vec<RobotsGroup>,
    sitemaps: Vec<String>,
}

impl RobotsPolicy {
    /// Parse a robots.txt body.
    ///
    /// Never fails. A robots.txt is written by hand on the far side of a
    /// network and is routinely malformed; refusing to parse one would turn
    /// every typo on a publisher's server into a source we cannot assess,
    /// which is the opposite of cautious. Unparseable lines are skipped and
    /// the rules that did parse still bind.
    pub fn parse(body: &str) -> Self {
        let mut groups: Vec<RobotsGroup> = Vec::new();
        let mut sitemaps = Vec::new();
        // A `User-agent` line after a rule starts a new group; one after
        // another `User-agent` extends the current group. Losing that
        // distinction merges every group in the file into one.
        let mut open_for_agents = false;

        for raw in body.lines() {
            let line = match raw.split_once('#') {
                Some((before, _)) => before.trim(),
                None => raw.trim(),
            };
            if line.is_empty() {
                continue;
            }
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let field = field.trim().to_ascii_lowercase();
            let value = value.trim();

            match field.as_str() {
                "user-agent" => {
                    let agent = value.to_ascii_lowercase();
                    if agent.is_empty() {
                        continue;
                    }
                    if open_for_agents {
                        if let Some(group) = groups.last_mut() {
                            group.agents.push(agent);
                            continue;
                        }
                    }
                    groups.push(RobotsGroup {
                        agents: vec![agent],
                        rules: Vec::new(),
                        crawl_delay: None,
                    });
                    open_for_agents = true;
                }
                "allow" | "disallow" => {
                    let Some(group) = groups.last_mut() else {
                        continue;
                    };
                    open_for_agents = false;
                    // An empty `Disallow` is the documented way to say "no
                    // restriction"; recording it as a zero-length pattern
                    // would make it match everything.
                    if value.is_empty() {
                        continue;
                    }
                    group.rules.push(RobotsRule {
                        allow: field == "allow",
                        pattern: value.to_string(),
                    });
                }
                "crawl-delay" => {
                    let Some(group) = groups.last_mut() else {
                        continue;
                    };
                    open_for_agents = false;
                    if let Ok(seconds) = value.parse::<f64>()
                        && seconds.is_finite()
                        && seconds >= 0.0
                    {
                        let nanos = (seconds * 1_000_000_000.0).round();
                        if nanos <= i64::MAX as f64 {
                            group.crawl_delay = Some(Duration::from_nanos(nanos as i64));
                        }
                    }
                }
                "sitemap" => sitemaps.push(value.to_string()),
                _ => {}
            }
        }

        Self { groups, sitemaps }
    }

    pub fn groups(&self) -> &[RobotsGroup] {
        &self.groups
    }

    pub fn sitemaps(&self) -> &[String] {
        &self.sitemaps
    }

    /// The rules that bind `agent`, merged.
    ///
    /// Groups naming the agent exactly win over `*` entirely: a publisher who
    /// wrote a group for us meant it to replace the general one, not to add
    /// to it. Where several groups name the same agent — legal, and common in
    /// generated files — their rules merge.
    fn applicable_groups(&self, agent: &str) -> Vec<&RobotsGroup> {
        let agent = agent.to_ascii_lowercase();
        let named: Vec<&RobotsGroup> = self
            .groups
            .iter()
            .filter(|group| group.addresses(&agent))
            .collect();
        if !named.is_empty() {
            return named;
        }
        self.groups
            .iter()
            .filter(|group| group.is_wildcard())
            .collect()
    }

    /// The crawl delay that binds `agent`, if the file states one.
    ///
    /// The longest delay across applicable groups, not the first: two groups
    /// disagreeing is a publisher contradiction, and the slower reading is
    /// the one that cannot breach the faster one.
    pub fn crawl_delay_for(&self, agent: &str) -> Option<Duration> {
        self.applicable_groups(agent)
            .into_iter()
            .filter_map(RobotsGroup::crawl_delay)
            .max()
    }

    /// Every `Disallow` pattern binding `agent`, for the emitted policy.
    pub fn disallowed_paths_for(&self, agent: &str) -> Vec<String> {
        let mut paths: Vec<String> = self
            .applicable_groups(agent)
            .into_iter()
            .flat_map(|group| group.rules.iter())
            .filter(|rule| !rule.allow)
            .map(|rule| rule.pattern.clone())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// Whether `agent` may fetch `path`, and which rule decided.
    pub fn verdict(&self, agent: &str, path: &str) -> PathVerdict {
        let groups = self.applicable_groups(agent);
        if groups.is_empty() {
            return PathVerdict::NoApplicableGroup;
        }

        let mut best: Option<&RobotsRule> = None;
        for rule in groups.iter().flat_map(|group| group.rules.iter()) {
            if !rule.matches(path) {
                continue;
            }
            best = Some(match best {
                None => rule,
                Some(current) => {
                    if rule.specificity() > current.specificity() {
                        rule
                    } else if rule.specificity() == current.specificity() && rule.allow {
                        // Equal length: the permissive rule wins, whichever
                        // order the file listed them in.
                        rule
                    } else {
                        current
                    }
                }
            });
        }

        match best {
            Some(rule) if rule.allow => PathVerdict::AllowedByRule {
                pattern: rule.pattern.clone(),
            },
            Some(rule) => PathVerdict::DisallowedByRule {
                pattern: rule.pattern.clone(),
            },
            None => PathVerdict::AllowedByAbsenceOfRule,
        }
    }
}

/// Glob match over a robots path pattern.
///
/// `*` matches any run of characters, a trailing `$` anchors the end, and
/// everything else is literal. Matching is a prefix match otherwise: `/a`
/// matches `/abc`, which is what makes `Disallow: /` block a site.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(stripped) => (stripped, true),
        None => (pattern, false),
    };

    let segments: Vec<&str> = pattern.split('*').collect();
    let last = segments.len().saturating_sub(1);
    let mut cursor = 0usize;
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        let Some(remaining) = path.get(cursor..) else {
            return false;
        };
        if index == 0 {
            if !remaining.starts_with(segment) {
                return false;
            }
            cursor += segment.len();
            continue;
        }
        // The final literal of an anchored pattern has to land on the end of
        // the path, so it is matched from the right. Matching it from the
        // left would make `/*.json$` miss `/a.json/b.json`, where the star is
        // free to swallow the first match.
        if anchored && index == last {
            if !remaining.ends_with(segment) {
                return false;
            }
            cursor = path.len();
            continue;
        }
        match remaining.find(segment) {
            Some(offset) => cursor += offset + segment.len(),
            None => return false,
        }
    }

    if !anchored {
        return true;
    }
    // An anchored pattern ending in `*` may consume any tail; otherwise the
    // final literal must land exactly on the end of the path.
    if pattern.ends_with('*') {
        return true;
    }
    cursor == path.len()
}
