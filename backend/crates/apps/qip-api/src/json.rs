//! JSON encoding helpers.
//!
//! `serde_json` is available and is used wherever a type derives
//! `Serialize`. These helpers exist for the response bodies that are
//! assembled from several sources at once, where building an intermediate
//! struct would add a type per endpoint for no gain.
//!
//! [`string`] is the one that matters: it escapes control characters and
//! quotes, so a value that came from outside — a rationale, an error message,
//! an agent id — cannot break out of the string it is being written into.

/// Encode a string as a JSON string literal, quotes included.
pub fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // Every other control character, escaped numerically. Passing one
            // through unescaped produces JSON a strict parser rejects.
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Encode a float, using `null` for the values JSON cannot represent.
///
/// `NaN` and the infinities are not JSON numbers. Writing them anyway produces
/// a document that some parsers accept and others reject, which is worse than
/// either.
pub fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".to_string()
    }
}
