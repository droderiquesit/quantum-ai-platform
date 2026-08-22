//! Name comparison.
//!
//! Company names are compared after stripping legal forms and punctuation,
//! because "Northwind Semiconductor Corporation", "Northwind Semiconductor
//! Corp." and "NORTHWIND SEMICONDUCTOR CORP" are the same company written
//! three ways. What is deliberately *not* stripped is anything that
//! distinguishes entities: "Series A" and "Series B", "Holdings" and
//! "Operating", roman numerals in fund names.

use serde::{Deserialize, Serialize};

/// Legal forms removed before comparison.
const LEGAL_FORMS: [&str; 34] = [
    "inc",
    "incorporated",
    "corp",
    "corporation",
    "co",
    "company",
    "ltd",
    "limited",
    "llc",
    "llp",
    "lp",
    "plc",
    "sa",
    "sas",
    "ag",
    "gmbh",
    "nv",
    "bv",
    "ab",
    "as",
    "oyj",
    "spa",
    "srl",
    "kk",
    "pte",
    "pty",
    "sarl",
    "kgaa",
    "se",
    "aktiengesellschaft",
    "holdings",
    "holding",
    "group",
    "the",
];

/// Normalise a name for comparison: lowercase, punctuation removed, legal forms
/// stripped, whitespace collapsed.
pub fn normalise_name(name: &str) -> String {
    let lowered: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let tokens: Vec<&str> = lowered
        .split_whitespace()
        .filter(|t| !LEGAL_FORMS.contains(t))
        .collect();
    if tokens.is_empty() {
        // A name consisting only of legal forms keeps them; stripping to
        // nothing would make every such name identical.
        return lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    tokens.join(" ")
}

/// Tokens of a normalised name.
fn tokens(name: &str) -> Vec<String> {
    normalise_name(name)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Jaro similarity in `[0, 1]`.
pub fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut a_matched = vec![false; a.len()];
    let mut b_matched = vec![false; b.len()];
    let mut matches = 0usize;

    for (i, ch) in a.iter().enumerate() {
        let start = i.saturating_sub(window);
        let end = (i + window + 1).min(b.len());
        for j in start..end {
            if !b_matched[j] && b[j] == *ch {
                a_matched[i] = true;
                b_matched[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }

    // Half the number of matched characters that are out of order.
    let mut transpositions = 0usize;
    let mut k = 0usize;
    for i in 0..a.len() {
        if !a_matched[i] {
            continue;
        }
        while !b_matched[k] {
            k += 1;
        }
        if a[i] != b[k] {
            transpositions += 1;
        }
        k += 1;
    }

    let m = matches as f64;
    (m / a.len() as f64 + m / b.len() as f64 + (m - transpositions as f64 / 2.0) / m) / 3.0
}

/// Jaro-Winkler: Jaro, boosted for a shared prefix.
///
/// The prefix boost matters for company names, where the distinguishing word
/// almost always comes first.
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let base = jaro(a, b);
    if base < 0.7 {
        return base;
    }
    let prefix = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count() as f64;
    base + 0.1 * prefix * (1.0 - base)
}

/// Fraction of the smaller token set contained in the larger.
///
/// Catches the abbreviation case: "Northwind Semi" against "Northwind
/// Semiconductor Corporation" shares every token it has.
pub fn token_containment(a: &str, b: &str) -> f64 {
    let a_tokens = tokens(a);
    let b_tokens = tokens(b);
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let (small, large) = if a_tokens.len() <= b_tokens.len() {
        (&a_tokens, &b_tokens)
    } else {
        (&b_tokens, &a_tokens)
    };
    let contained = small
        .iter()
        .filter(|token| {
            large
                .iter()
                // A token counts as present if it matches exactly or is a
                // prefix of a longer word: "semi" of "semiconductor".
                .any(|other| other == *token || (token.len() >= 4 && other.starts_with(*token)))
        })
        .count();
    contained as f64 / small.len() as f64
}

/// Below this, a character-level similarity between two words is noise.
///
/// Any two English words share letters: "materials" and "mining" score around
/// 0.52 on Jaro-Winkler, and "aurora" and "materials" around 0.61. Carrying
/// those through as partial evidence puts unrelated company names above a
/// resolver's lower threshold, so they are treated as no evidence at all.
const TOKEN_NOISE_FLOOR: f64 = 0.75;

/// Similarity between two individual tokens.
fn token_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    // An abbreviation is a prefix: "semi" of "semiconductor". Treated as a near
    // match, since that is exactly how company names get shortened in practice.
    if (a.len() >= 4 && b.starts_with(a)) || (b.len() >= 4 && a.starts_with(b)) {
        return 0.95;
    }
    let similarity = jaro_winkler(a, b);
    if similarity < TOKEN_NOISE_FLOOR {
        0.0
    } else {
        similarity
    }
}

/// Mean best-match similarity, taken over the larger token set.
///
/// Scoring over the larger set is what makes an extra distinguishing word
/// count: "Kestrel Mining" against "Kestrel Materials" must not score as a
/// match merely because both begin with "Kestrel".
fn token_alignment(a: &str, b: &str) -> f64 {
    let a_tokens = tokens(a);
    let b_tokens = tokens(b);
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let (small, large) = if a_tokens.len() <= b_tokens.len() {
        (&a_tokens, &b_tokens)
    } else {
        (&b_tokens, &a_tokens)
    };
    let total: f64 = large
        .iter()
        .map(|token| {
            small
                .iter()
                .map(|other| token_similarity(token, other))
                .fold(0.0f64, f64::max)
        })
        .sum();
    total / large.len() as f64
}

/// Combined name similarity in `[0, 1]`.
///
/// Token-wise rather than over the whole string. Jaro-Winkler applied to a full
/// company name rewards a shared prefix so heavily that two different companies
/// sharing a first word score as a match, which is the single most damaging
/// error a resolver can make.
pub fn name_similarity(a: &str, b: &str) -> f64 {
    let normalised_a = normalise_name(a);
    let normalised_b = normalise_name(b);
    if normalised_a == normalised_b {
        return 1.0;
    }
    if normalised_a.is_empty() || normalised_b.is_empty() {
        return 0.0;
    }
    token_alignment(&normalised_a, &normalised_b).clamp(0.0, 1.0)
}

/// Why the resolver linked or declined to link two things.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchEvidence {
    /// What produced the signal, e.g. `isin`, `name`, `country`.
    pub signal: String,
    /// Strength in `[0, 1]`; negative values are disconfirming.
    pub strength: f64,
    pub detail: String,
}

impl MatchEvidence {
    pub fn new(signal: impl Into<String>, strength: f64, detail: impl Into<String>) -> Self {
        Self {
            signal: signal.into(),
            strength,
            detail: detail.into(),
        }
    }

    pub fn is_disconfirming(&self) -> bool {
        self.strength < 0.0
    }
}
