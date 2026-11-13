//! The journalled statement of the limit in force (ADR 0405 D8).
//!
//! The operator's limit set is a ceiling, and since ADR 0405 the pre-trade
//! check runs against that ceiling narrowed by the desk mandate's risk
//! tolerance. A decision is only reproducible from the event log if the log
//! says which bounds it was checked against, and until this record the log
//! could not: the ceiling comes from a file and the tolerance from the
//! mandate registry, and neither was written down beside the decisions they
//! governed.
//!
//! One record per process start, written at assembly, and only when the
//! tolerance is below one. At one the limit in force is the ceiling itself
//! and there is nothing to state. The desk's mandate cannot be replaced while
//! a process runs — the registry has no such path — so one record covers every
//! decision of that boot.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_risk::limits::LimitSet;
use serde::{Deserialize, Serialize};

/// The producer of [`LimitsNarrowed`]. Its own origin, so a reader of the
/// other `RiskEvaluated` producers never meets this body.
pub const LIMIT_NARROWING_ORIGIN: &str = "kernel/limit-narrowing";

/// The configuration variable a composition root reads the desk mandate's
/// risk tolerance from. Read in `apps/*/src/main.rs` and nowhere else; the
/// kernel never reads the environment.
pub const DESK_RISK_TOLERANCE_VARIABLE: &str = "QIP_DESK_RISK_TOLERANCE";

/// The desk mandate's risk tolerance a composition root was configured with
/// (ADR 0405): the setting's value, or one when it is not set.
///
/// Unset is one, and one is not a figure anybody invented: it is the
/// operator's limit set in force exactly as written, which is what every
/// binary ran before the term could be stated.
///
/// A setting that is present and is not a decimal fraction in `[0, 1]` is an
/// error, never a default and never a clamp. Read as unset, a typo would run
/// a desk that believed it had halved its limits at the full ceiling; clamped,
/// `50` meant as a percentage would do the same with nothing to notice.
/// `Mandate::new` refuses the same range again at assembly — this refusal
/// exists so the message can name the variable the operator has to fix.
pub fn desk_risk_tolerance_from_setting(setting: Option<&str>) -> Result<Decimal> {
    let Some(setting) = setting else {
        return Ok(Decimal::ONE);
    };
    let tolerance = Decimal::parse(setting.trim()).ok_or_else(|| {
        Error::invalid(format!(
            "configuration: {DESK_RISK_TOLERANCE_VARIABLE}={setting:?} is not a decimal \
             fraction; write a fraction in [0, 1] such as 0.5, or unset it to run the \
             operator's limits exactly as written"
        ))
    })?;
    if tolerance.is_negative() || tolerance > Decimal::ONE {
        return Err(Error::invalid(format!(
            "configuration: {DESK_RISK_TOLERANCE_VARIABLE}={tolerance} is outside [0, 1]; \
             write a fraction such as 0.5 (if this was a percentage, divide by 100), or unset \
             it to run the operator's limits exactly as written — the value is not clamped"
        )));
    }
    Ok(tolerance)
}

/// The start-up line for the tolerance a root resolved, so the process says
/// which limits it will check orders against before it serves anything.
pub fn describe_desk_risk_tolerance(tolerance: Decimal, stated: bool) -> String {
    let source = if stated {
        format!("{DESK_RISK_TOLERANCE_VARIABLE} set")
    } else {
        format!("{DESK_RISK_TOLERANCE_VARIABLE} unset")
    };
    if tolerance == Decimal::ONE {
        format!(
            "desk risk tolerance 1 ({source}): the operator's limit set is in force exactly \
             as written"
        )
    } else {
        format!(
            "desk risk tolerance {tolerance} ({source}): every maximum in the operator's \
             limit set is in force at {tolerance} of its ceiling, every minimum where the \
             operator put it (ADR 0405)"
        )
    }
}

/// One limit, as the operator wrote it and as it is in force.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NarrowedLimit {
    pub name: String,
    /// [`qip_risk::limits::LimitKind::label`].
    pub kind: String,
    /// A minimum is not narrowed, and says so here rather than leaving a
    /// reader to infer it from two equal numbers.
    pub is_minimum: bool,
    /// The operator's bound.
    pub ceiling: f64,
    /// The bound an order is checked against.
    pub in_force: f64,
}

/// The limit set in force for this process, and what it was derived from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitsNarrowed {
    /// The name of the operator's limit set.
    pub ceiling_set: String,
    /// The desk mandate's risk tolerance, read from the mandate registry.
    pub tolerance: Decimal,
    /// Every limit of the set, in the set's own order.
    pub limits: Vec<NarrowedLimit>,
    pub at: Timestamp,
}

impl LimitsNarrowed {
    /// The record for a ceiling and the set in force derived from it.
    ///
    /// Refuses two sets that do not line up rule for rule. The record's whole
    /// use is that a reader can put each bound in force beside the ceiling it
    /// came from; a record that paired one rule's ceiling with another's
    /// bound would be worse than none.
    pub fn between(
        ceiling: &LimitSet,
        in_force: &LimitSet,
        tolerance: Decimal,
        at: Timestamp,
    ) -> Result<Self> {
        if ceiling.len() != in_force.len() {
            return Err(Error::invalid(format!(
                "the limit set in force has {} limits and its ceiling {} has {}; the set in \
                 force must be the ceiling narrowed, rule for rule",
                in_force.len(),
                ceiling.name,
                ceiling.len()
            )));
        }
        let mut limits = Vec::with_capacity(ceiling.len());
        for (above, below) in ceiling.limits.iter().zip(&in_force.limits) {
            if above.name != below.name || above.kind.label() != below.kind.label() {
                return Err(Error::invalid(format!(
                    "the limit in force {} ({}) does not line up with the ceiling's {} ({}); \
                     the set in force must be the ceiling narrowed, rule for rule",
                    below.name,
                    below.kind.label(),
                    above.name,
                    above.kind.label()
                )));
            }
            limits.push(NarrowedLimit {
                name: above.name.clone(),
                kind: above.kind.label().to_string(),
                is_minimum: above.kind.is_minimum(),
                ceiling: above.kind.bound(),
                in_force: below.kind.bound(),
            });
        }
        Ok(Self {
            ceiling_set: ceiling.name.clone(),
            tolerance,
            limits,
            at,
        })
    }
}

impl EventBody for LimitsNarrowed {
    const TOPIC: Topic = Topic::RiskEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    // The three readings a root can meet. The dangerous one is the third: a
    // setting nobody can read taken as unset runs the full ceiling under a
    // desk that believes it narrowed.
    #[test]
    fn an_unset_tolerance_is_one_a_stated_fraction_is_itself_and_anything_else_is_refused() {
        assert_eq!(
            desk_risk_tolerance_from_setting(None).ok(),
            Some(Decimal::ONE)
        );
        assert_eq!(
            desk_risk_tolerance_from_setting(Some("0.5")).ok(),
            Some(dec!("0.5"))
        );
        assert_eq!(
            desk_risk_tolerance_from_setting(Some(" 0 ")).ok(),
            Some(Decimal::ZERO)
        );
        assert_eq!(
            desk_risk_tolerance_from_setting(Some("1")).ok(),
            Some(Decimal::ONE)
        );
        for (setting, needle) in [
            ("half", "is not a decimal fraction"),
            ("", "is not a decimal fraction"),
            ("NaN", "is not a decimal fraction"),
            ("50", "is outside [0, 1]"),
            ("1.000000001", "is outside [0, 1]"),
            ("-0.1", "is outside [0, 1]"),
        ] {
            let error = desk_risk_tolerance_from_setting(Some(setting))
                .expect_err("an unreadable tolerance is not a tolerance");
            assert!(
                error.message().contains(needle)
                    && error.message().contains(DESK_RISK_TOLERANCE_VARIABLE)
                    && error.message().contains("write a fraction"),
                "{setting:?}: {}",
                error.message()
            );
        }
    }

    // The start-up line is what an operator reads to learn which limits are
    // in force; at one it must not suggest anything was narrowed.
    #[test]
    fn the_start_up_line_says_whether_the_operators_limits_are_in_force_as_written() {
        let unset = describe_desk_risk_tolerance(Decimal::ONE, false);
        assert!(
            unset.contains("unset") && unset.contains("exactly as written"),
            "{unset}"
        );
        let narrowed = describe_desk_risk_tolerance(dec!("0.5"), true);
        assert!(
            narrowed.contains("QIP_DESK_RISK_TOLERANCE set")
                && narrowed.contains("at 0.5 of its ceiling")
                && !narrowed.contains("exactly as written"),
            "{narrowed}"
        );
    }
}
