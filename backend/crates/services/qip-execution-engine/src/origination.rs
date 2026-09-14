//! Blueprint §29.3: the gate every originated market must clear first.
//!
//! Creating a market means being the counterparty when nobody else will be.
//! The blueprint's own warning is that doing it without understanding *why*
//! nobody else will is how a platform becomes exit liquidity rather than the
//! house, and its "gate before any market creation" table names five checks.
//! All five are here, and an [`OriginationMandate`] is the only thing that
//! carries the fact that they passed.
//!
//! # Why a type and not a function
//!
//! [`OriginationMandate`] has private fields, one constructor
//! ([`OriginationMandate::admit`]), and **no `Deserialize`**. A mandate
//! therefore cannot be decoded into existence out of a config file, a policy
//! frame or any other payload; the five refusals are the only door. That is
//! the same discipline the paper-trading boundary is held by — a guarantee
//! the type system holds beats one a runtime check holds — and it matters
//! more here than in most places, because the mandate is what lets
//! [`crate::quoting`] quote an instrument that has no observable market at
//! all.
//!
//! # What this deliberately cannot do
//!
//! Nothing in this module constructs an [`crate::order::Order`], names a
//! venue, or reaches a [`crate::broker::Broker`]. An origination mandate
//! raises no ceiling anywhere: it bounds one, and the bound it carries is
//! refused above [`ORIGINATION_MAX_EXPOSURE`], which is a constant in this
//! file and not a configurable. The blueprint's fourth gate is the words
//! "bounded maximum exposure, **hard-coded**", and a number a deployment can
//! raise is not that.
//!
//! # Money and statistics
//!
//! A valuation and an exposure ceiling are money and are [`Decimal`].
//! Confidence and a price-impact estimate are statistics and are `f64`. The
//! two never mix in this file: no `f64` is ever converted into a currency
//! amount here.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::Serialize;

/// The most exposure any originated position may be granted, ever.
///
/// Two hundred and fifty thousand, hard-coded, because the blueprint's fourth
/// gate is the word "hard-coded" and a ceiling a deployment can edit is a
/// ceiling an incident can edit. [`OriginationMandate::admit`] **refuses** a
/// request above it rather than lowering the request to it: a desk that asked
/// for ten million and silently received this would believe something false
/// about its own book, which is the failure
/// `.claude/rules/01-security-and-safety.md` names for the autonomy ceiling
/// and the same failure here.
pub const ORIGINATION_MAX_EXPOSURE: Decimal = Decimal::from_raw(250_000_000_000_000);

/// The confidence a valuation must carry before the platform may quote on it.
///
/// The blueprint's first gate is "a defensible valuation with method and
/// confidence", and its reason is that "quoting without a price is gambling
/// with extra steps". A valuation the platform is three-quarters sure of is
/// the floor; below it the honest action is to decline to make the market.
pub const ORIGINATION_MIN_VALUATION_CONFIDENCE: f64 = 0.75;

/// Observations an adverse-selection model must be fitted on before it counts
/// as one.
///
/// Thirty, the same order as every other evidence bar in this workspace, and
/// the reason the blueprint gives is specific: "origination concentrates
/// adverse selection more than any other activity". A model fitted on four
/// trades is a number, not a model.
pub const ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE: usize = 30;

/// A price with a method and a confidence attached.
///
/// The method is carried as text because the platform values illiquid and
/// originated exposure by several routes and a report has to say which one
/// was used; it is refused when empty, because "valued somehow" is what the
/// gate exists to catch.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Valuation {
    pub method: String,
    /// Money. The price the platform would defend.
    pub value: Decimal,
    /// A statistic, in `(0, 1]`.
    pub confidence: f64,
}

/// Why nobody else is quoting this.
///
/// The blueprint's second gate: "if the reason is information you lack, you
/// are the counterparty they are avoiding". That sentence is the whole reason
/// this is an enum and not a string — a free-text explanation can say
/// anything, and a gate that reads free text is a gate that approves anything
/// phrased confidently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsenceCause {
    /// The instrument is new or obscure and nobody has looked at it yet.
    NotYetCovered,
    /// The economics are real but too small for the incumbents' cost base.
    BelowIncumbentCostBase,
    /// A regulatory, custody or operational barrier kept others out, and this
    /// platform has cleared it. The barrier is named so a reader can check.
    BarrierCleared { barrier: String },
    /// Others will not price it because they know something this platform
    /// does not. **Refused**: this is the arm the second gate exists for.
    InformationWeLack,
    /// Nobody established why. **Refused**: an unexplained absence is the
    /// `InformationWeLack` case that has not been recognised yet, and
    /// admitting it would make the gate a formality.
    Unexplained,
}

impl AbsenceCause {
    /// A bounded token for a report, so a reader matching on a cause matches a
    /// delimited word rather than a substring of a sentence.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotYetCovered => "not_yet_covered",
            Self::BelowIncumbentCostBase => "below_incumbent_cost_base",
            Self::BarrierCleared { .. } => "barrier_cleared",
            Self::InformationWeLack => "information_we_lack",
            Self::Unexplained => "unexplained",
        }
    }

    /// Whether this cause is one the platform may originate against.
    ///
    /// Matched exhaustively and deliberately: a sixth cause added to the enum
    /// becomes a compile error here rather than falling through a wildcard
    /// into the admitting side, which is the direction a mistake must never
    /// fall in a gate.
    pub const fn is_admissible(&self) -> bool {
        match self {
            Self::NotYetCovered | Self::BelowIncumbentCostBase | Self::BarrierCleared { .. } => {
                true
            }
            Self::InformationWeLack | Self::Unexplained => false,
        }
    }
}

/// A causal account of the absence, with whatever established it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AbsenceExplanation {
    pub cause: AbsenceCause,
    /// What established the cause. Refused when empty: a cause asserted with
    /// nothing behind it is the `Unexplained` arm wearing another name.
    pub evidence: String,
}

/// What the platform has measured about being picked off in this instrument
/// class.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AdverseSelectionModel {
    /// The class the model was fitted for. Refused when it does not match the
    /// request's class: a model for listed equity says nothing about a
    /// bespoke structured payoff, and borrowing one across classes is exactly
    /// how the concentration the blueprint warns about goes unmeasured.
    pub instrument_class: String,
    /// A statistic: the permanent move against the maker after a fill, in
    /// basis points.
    pub price_impact_bps: f64,
    /// Observations behind the estimate.
    pub sample: usize,
}

/// A person's approval, for one instrument class.
///
/// The blueprint's fifth gate, and its reason is that origination is "a
/// business decision, not a strategy promotion". Nothing in this workspace
/// constructs one of these from a model output, a config value or an agent
/// finding — an approval has to be handed in from outside, and until a desk
/// hands one in every origination request is refused. That is the fail-closed
/// direction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClassApproval {
    pub instrument_class: String,
    /// The authenticated operator identity that signed. Refused when empty.
    pub operator: String,
    pub approved_at: Timestamp,
    /// The digest of what was signed, so the approval is bound to a document
    /// rather than floating free. Refused when empty.
    pub digest: String,
}

/// Everything a market-creation request has to present.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OriginationRequest {
    pub object_id: String,
    pub instrument_class: String,
    pub valuation: Valuation,
    pub absence: AbsenceExplanation,
    pub adverse_selection: AdverseSelectionModel,
    /// Money. The most this originated position may ever reach.
    pub exposure_ceiling: Decimal,
    pub approval: ClassApproval,
}

/// The fact that all five gates passed, and the only thing that carries it.
///
/// Private fields, one constructor, no `Deserialize`. See this module's
/// header for why each of those three is load-bearing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OriginationMandate {
    object_id: String,
    instrument_class: String,
    valuation: Valuation,
    cause: AbsenceCause,
    adverse_selection_bps: f64,
    exposure_ceiling: Decimal,
    approved_by: String,
    approved_at: Timestamp,
}

impl OriginationMandate {
    /// Admit a request, or refuse it naming what to present instead.
    ///
    /// The five checks are in the blueprint's own order, and each refusal
    /// names the thing to do next rather than only the thing that was wrong,
    /// because a refusal a desk cannot act on is a refusal they will route
    /// around.
    pub fn admit(request: OriginationRequest) -> Result<Self> {
        let OriginationRequest {
            object_id,
            instrument_class,
            valuation,
            absence,
            adverse_selection,
            exposure_ceiling,
            approval,
        } = request;

        if object_id.trim().is_empty() {
            return Err(Error::invalid(
                "an origination request must name the object it creates a market in",
            ));
        }
        if instrument_class.trim().is_empty() {
            return Err(Error::invalid(
                "an origination request must name an instrument class; the approval and the \
                 adverse-selection model are both per class and neither can be checked without it",
            ));
        }

        // Gate one: a defensible valuation with method and confidence.
        if valuation.method.trim().is_empty() {
            return Err(Error::invalid(format!(
                "{object_id} has no valuation method; name the route the price was reached by, \
                 because a report has to say which one was used"
            )));
        }
        if !valuation.value.is_positive() {
            return Err(Error::invalid(format!(
                "{object_id} was valued at {}; present a positive price, because quoting around a \
                 non-positive fair value is not a market",
                valuation.value
            )));
        }
        if !valuation.confidence.is_finite()
            || valuation.confidence <= 0.0
            || valuation.confidence > 1.0
        {
            return Err(Error::invalid(format!(
                "{object_id}'s valuation confidence is {}; present a number in (0, 1]",
                valuation.confidence
            )));
        }
        if valuation.confidence < ORIGINATION_MIN_VALUATION_CONFIDENCE {
            return Err(Error::denied(format!(
                "{object_id}'s valuation confidence is {} and origination needs at least \
                 {ORIGINATION_MIN_VALUATION_CONFIDENCE}; improve the valuation or decline to make \
                 the market",
                valuation.confidence
            )));
        }

        // Gate two: a causal explanation for the absence of other participants.
        if absence.evidence.trim().is_empty() {
            return Err(Error::invalid(format!(
                "{object_id}'s absence explanation cites nothing; present what established the \
                 cause '{}'",
                absence.cause.as_str()
            )));
        }
        if !absence.cause.is_admissible() {
            return Err(Error::denied(format!(
                "{object_id}'s absence of other participants is '{}'; establish a cause that is \
                 not information this platform lacks, or decline to make the market — if the \
                 reason is information you lack, you are the counterparty they are avoiding",
                absence.cause.as_str()
            )));
        }

        // Gate three: an adverse-selection model for this instrument class.
        if adverse_selection.instrument_class != instrument_class {
            return Err(Error::invalid(format!(
                "{object_id} is class '{instrument_class}' and its adverse-selection model was \
                 fitted for '{}'; fit one for this class, because origination concentrates \
                 adverse selection more than any other activity",
                adverse_selection.instrument_class
            )));
        }
        if !adverse_selection.price_impact_bps.is_finite()
            || adverse_selection.price_impact_bps < 0.0
        {
            return Err(Error::invalid(format!(
                "{object_id}'s adverse-selection model reports {} basis points of impact; present \
                 a finite, non-negative estimate",
                adverse_selection.price_impact_bps
            )));
        }
        if adverse_selection.sample < ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE {
            return Err(Error::denied(format!(
                "{object_id}'s adverse-selection model was fitted on {} observation(s) and \
                 origination needs at least {ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE}; gather \
                 more before creating the market",
                adverse_selection.sample
            )));
        }

        // Gate four: bounded maximum exposure, hard-coded.
        if !exposure_ceiling.is_positive() {
            return Err(Error::invalid(format!(
                "{object_id} was granted an exposure ceiling of {exposure_ceiling}; present a \
                 positive bound, because an originated position may have no exit"
            )));
        }
        if exposure_ceiling > ORIGINATION_MAX_EXPOSURE {
            return Err(Error::denied(format!(
                "{object_id} asked for an exposure ceiling of {exposure_ceiling} and the \
                 hard-coded maximum is {ORIGINATION_MAX_EXPOSURE}; ask for no more than the \
                 maximum — it is not lowered for you, because a desk that asked for more and \
                 received less silently would believe something false about its own book"
            )));
        }

        // Gate five: human approval per instrument class.
        if approval.instrument_class != instrument_class {
            return Err(Error::denied(format!(
                "{object_id} is class '{instrument_class}' and the approval presented covers \
                 '{}'; present an approval for this class — origination is a business decision \
                 per class, not a strategy promotion",
                approval.instrument_class
            )));
        }
        if approval.operator.trim().is_empty() {
            return Err(Error::invalid(
                "an origination approval must name the operator who signed it; an approval nobody \
                 is named on is one nobody can be asked about",
            ));
        }
        if approval.digest.trim().is_empty() {
            return Err(Error::invalid(
                "an origination approval must carry the digest of what was signed; an approval \
                 bound to no document approves whatever is presented next",
            ));
        }

        Ok(Self {
            object_id,
            instrument_class,
            valuation,
            cause: absence.cause,
            adverse_selection_bps: adverse_selection.price_impact_bps,
            exposure_ceiling,
            approved_by: approval.operator,
            approved_at: approval.approved_at,
        })
    }

    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    pub fn instrument_class(&self) -> &str {
        &self.instrument_class
    }

    pub fn valuation(&self) -> &Valuation {
        &self.valuation
    }

    pub fn cause(&self) -> &AbsenceCause {
        &self.cause
    }

    /// The measured permanent move against the maker after a fill, in basis
    /// points. [`crate::quoting`] charges this into the half spread, so the
    /// model the gate demanded is a model the quotes are actually priced off
    /// rather than a document filed once.
    pub fn adverse_selection_bps(&self) -> f64 {
        self.adverse_selection_bps
    }

    /// Money. The bound the quote loop sizes against.
    pub fn exposure_ceiling(&self) -> Decimal {
        self.exposure_ceiling
    }

    pub fn approved_by(&self) -> &str {
        &self.approved_by
    }

    pub fn approved_at(&self) -> Timestamp {
        self.approved_at
    }

    /// One line for a cycle report: what was approved, by whom, and under what
    /// bound.
    pub fn describe(&self) -> String {
        format!(
            "{} originated as class {} at {} (method {}, confidence {:.2}, absence {}), ceiling \
             {}, approved by {}",
            self.object_id,
            self.instrument_class,
            self.valuation.value,
            self.valuation.method,
            self.valuation.confidence,
            self.cause.as_str(),
            self.exposure_ceiling,
            self.approved_by,
        )
    }
}

#[cfg(test)]
mod tests {
    // The one `f64` compared exactly below is the fixture's own
    // `price_impact_bps` carried through the mandate unchanged. A tolerance
    // would admit a mandate that altered the model's estimate on the way in,
    // which is the thing worth asserting.
    #![allow(clippy::float_cmp)]

    use super::*;
    use qip_core::dec;

    const CLASS: &str = "structured-payoff";

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// A request that clears every gate. Each test spoils exactly one field,
    /// so a refusal is attributable to the gate the test names and nothing
    /// else.
    fn clean() -> OriginationRequest {
        OriginationRequest {
            object_id: "obj-ORIG".to_string(),
            instrument_class: CLASS.to_string(),
            valuation: Valuation {
                method: "component-replication".to_string(),
                value: dec!("100"),
                confidence: 0.90,
            },
            absence: AbsenceExplanation {
                cause: AbsenceCause::BelowIncumbentCostBase,
                evidence: "three dealers quoted a minimum ticket above the whole issue".to_string(),
            },
            adverse_selection: AdverseSelectionModel {
                instrument_class: CLASS.to_string(),
                price_impact_bps: 12.0,
                sample: 40,
            },
            exposure_ceiling: dec!("50000"),
            approval: ClassApproval {
                instrument_class: CLASS.to_string(),
                operator: "risk-desk-operator".to_string(),
                approved_at: at(),
                digest: "sha256:0123456789abcdef".to_string(),
            },
        }
    }

    #[test]
    fn a_request_that_clears_all_five_gates_is_admitted() {
        // The admitting half, and it is the half that makes the other tests
        // mean anything: a gate that refused everything would pass every
        // refusal test in this file and be useless. ADR 0052 records this
        // exact correction being made to a different gate in this workspace.
        let mandate = OriginationMandate::admit(clean()).expect("a complete request is admitted");
        assert_eq!(mandate.object_id(), "obj-ORIG");
        assert_eq!(mandate.exposure_ceiling(), dec!("50000"));
        assert_eq!(mandate.adverse_selection_bps(), 12.0);
        assert_eq!(mandate.cause().as_str(), "below_incumbent_cost_base");
        assert!(mandate.describe().contains("risk-desk-operator"));
    }

    #[test]
    fn a_market_whose_absence_is_information_the_platform_lacks_is_refused() {
        // The blueprint's own sentence, as a branch: "if the reason is
        // information you lack, you are the counterparty they are avoiding".
        // The premise is asserted first — the same request with an
        // admissible cause is admitted — so this cannot pass by the request
        // being malformed in some other way.
        assert!(OriginationMandate::admit(clean()).is_ok(), "the premise");

        for cause in [AbsenceCause::InformationWeLack, AbsenceCause::Unexplained] {
            let token = cause.as_str().to_string();
            let mut request = clean();
            request.absence.cause = cause;
            let refusal = OriginationMandate::admit(request)
                .expect_err("an inadmissible absence must be refused");
            assert_eq!(refusal.code(), "denied");
            // Delimited: `'information_we_lack'` inside quotes, not a bare
            // substring, because `unexplained` is a substring of nothing here
            // today and would be of `unexplained_basis` tomorrow.
            assert!(
                refusal.message().contains(&format!("'{token}'")),
                "the refusal did not name the cause: {}",
                refusal.message()
            );
        }
    }

    #[test]
    fn an_exposure_ceiling_above_the_hard_coded_maximum_is_refused_and_never_lowered_to_it() {
        // The `MaxExpectedShortfall` shape, avoided: a ceiling that silently
        // became the maximum would read as a granted bound and be a different
        // bound than the desk asked for. Both halves are asserted — one unit
        // above the maximum is refused, the maximum itself is admitted — so a
        // gate that refused every ceiling would fail here.
        let mut request = clean();
        request.exposure_ceiling = ORIGINATION_MAX_EXPOSURE + Decimal::from_raw(1);
        let refusal =
            OriginationMandate::admit(request).expect_err("a ceiling above the maximum is refused");
        assert_eq!(refusal.code(), "denied");

        let mut request = clean();
        request.exposure_ceiling = ORIGINATION_MAX_EXPOSURE;
        let mandate = OriginationMandate::admit(request).expect("the maximum itself is admissible");
        assert_eq!(
            mandate.exposure_ceiling(),
            ORIGINATION_MAX_EXPOSURE,
            "the granted ceiling is not the one that was asked for"
        );
    }

    #[test]
    fn an_approval_for_another_instrument_class_does_not_approve_this_one() {
        // Per class, as the blueprint says, because origination concentrates
        // adverse selection per class. An approval for listed equity carried
        // across to a bespoke payoff is the cheapest way to make gate five a
        // formality.
        assert!(OriginationMandate::admit(clean()).is_ok(), "the premise");
        let mut request = clean();
        request.approval.instrument_class = "common-stock".to_string();
        let refusal = OriginationMandate::admit(request)
            .expect_err("an approval for another class must be refused");
        assert_eq!(refusal.code(), "denied");

        // And the same for the adverse-selection model, which is also per
        // class and refused as invalid rather than denied: a model fitted for
        // the wrong class is a malformed request, not a policy refusal.
        let mut request = clean();
        request.adverse_selection.instrument_class = "common-stock".to_string();
        let refusal = OriginationMandate::admit(request)
            .expect_err("a model fitted for another class must be refused");
        assert_eq!(refusal.code(), "invalid");
    }

    #[test]
    fn a_valuation_below_the_confidence_floor_and_a_model_below_the_sample_bar_are_both_refused() {
        // Gates one and three, each at the boundary and each with its
        // admitting neighbour asserted, so neither reads as a control that
        // refuses everything.
        let mut request = clean();
        request.valuation.confidence = ORIGINATION_MIN_VALUATION_CONFIDENCE - 0.01;
        assert!(
            OriginationMandate::admit(request).is_err(),
            "a valuation one hundredth below the floor was admitted"
        );
        let mut request = clean();
        request.valuation.confidence = ORIGINATION_MIN_VALUATION_CONFIDENCE;
        assert!(
            OriginationMandate::admit(request).is_ok(),
            "the floor itself was refused"
        );

        let mut request = clean();
        request.adverse_selection.sample = ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE - 1;
        assert!(
            OriginationMandate::admit(request).is_err(),
            "a model one observation short of the bar was admitted"
        );
        let mut request = clean();
        request.adverse_selection.sample = ORIGINATION_MIN_ADVERSE_SELECTION_SAMPLE;
        assert!(
            OriginationMandate::admit(request).is_ok(),
            "the bar itself was refused"
        );
    }
}
