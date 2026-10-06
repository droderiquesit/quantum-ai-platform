//! The nine things the Asset Brain must hold for every position, in one record
//! that refuses to exist without all of them (blueprint ASSET-004, ASSET-012).
//!
//! Before this module the nine facts lived in nine types in nine crates and
//! none of them refused on behalf of the other eight, so a position with a
//! confident mark and no exit plan was perfectly constructible. A default is
//! no answer here: "no hedge" and "no commitment" are real states, but they
//! must be *stated* ([`Declared::NoneApplicable`] with a reason), never
//! inferred from an empty collection nobody filled in.
//!
//! The mandate, hedge and corporate-action facts live in crates this library
//! may not depend on (`qip-capital`, `qip-risk`, `qip-market`), so they are
//! carried here by the identifier of the record that holds them.

use crate::asset_class::AssetClass;
use crate::cashflow::Commitment;
use crate::valuation::AssetValuation;
use qip_core::{Decimal, Duration, Error, Result, Timestamp};
use serde::{Deserialize, Serialize};

/// A fact that is either present or explicitly declared not to apply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Declared<T> {
    /// The fact does not apply, and this is why.
    NoneApplicable {
        reason: String,
    },
    Present(T),
}

impl<T> Declared<T> {
    fn check(&self, field: &str) -> Result<()> {
        if let Self::NoneApplicable { reason } = self
            && reason.trim().is_empty()
        {
            return Err(Error::invalid(format!(
                "the position record's {field} is declared not applicable with no reason; \
                 state why, because an unexplained absence is indistinguishable from an \
                 omission"
            )));
        }
        Ok(())
    }
}

/// How the position is to leave the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitRoute {
    /// Sold on its venue.
    Sale,
    /// Redeemed with the issuer or fund administrator.
    Redemption,
    /// Held to maturity.
    Maturity,
    /// Sold on the secondary market at a discount to mark (ASSET-013).
    SecondarySale,
}

/// When and how a position leaves, checked against its lockup at the one place
/// a plan can be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitPlan {
    exit_at: Timestamp,
    route: ExitRoute,
}

impl ExitPlan {
    /// Refuses a plan dated before `lockup_expires`. The boundary is
    /// inclusive: a plan dated exactly at expiry is admitted, because the
    /// holding is free at that instant, and a plan one instant earlier is a
    /// plan to do something the fund's terms forbid.
    pub fn new(
        exit_at: Timestamp,
        route: ExitRoute,
        lockup_expires: Option<Timestamp>,
    ) -> Result<Self> {
        if let Some(expiry) = lockup_expires
            && exit_at < expiry
        {
            return Err(Error::invalid(format!(
                "an exit planned for {} falls inside the lockup, which expires at {}; date \
                 the exit at or after expiry",
                exit_at.to_rfc3339(),
                expiry.to_rfc3339()
            )));
        }
        Ok(Self { exit_at, route })
    }

    pub fn exit_at(&self) -> Timestamp {
        self.exit_at
    }

    pub fn route(&self) -> ExitRoute {
        self.route
    }
}

/// Every field optional on the way in; [`Self::build`] is the only way out.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PositionRecordBuilder {
    asset: Option<String>,
    class: Option<AssetClass>,
    mandate: Option<String>,
    target_exposure: Option<Decimal>,
    valuation: Option<AssetValuation>,
    horizon: Option<Duration>,
    expected_exit: Option<Duration>,
    hedges: Option<Declared<Vec<String>>>,
    corporate_actions: Option<Declared<Vec<String>>>,
    commitments: Option<Declared<Commitment>>,
    exit_plan: Option<ExitPlan>,
}

macro_rules! setter {
    ($name:ident, $ty:ty) => {
        pub fn $name(mut self, v: $ty) -> Self {
            self.$name = Some(v);
            self
        }
    };
}

impl PositionRecordBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn asset(mut self, v: impl Into<String>) -> Self {
        self.asset = Some(v.into());
        self
    }
    pub fn mandate(mut self, v: impl Into<String>) -> Self {
        self.mandate = Some(v.into());
        self
    }
    setter!(class, AssetClass);
    setter!(target_exposure, Decimal);
    setter!(valuation, AssetValuation);
    setter!(horizon, Duration);
    setter!(expected_exit, Duration);
    setter!(hedges, Declared<Vec<String>>);
    setter!(corporate_actions, Declared<Vec<String>>);
    setter!(commitments, Declared<Commitment>);
    setter!(exit_plan, ExitPlan);

    /// Refuses, naming the field, the first of the nine that is absent.
    pub fn build(self) -> Result<PositionRecord> {
        fn need<T>(v: Option<T>, field: &str) -> Result<T> {
            v.ok_or_else(|| {
                Error::invalid(format!(
                    "the position record is missing its {field}; supply it, because a record \
                     with a defaulted field reads as complete and is not"
                ))
            })
        }
        let asset = need(self.asset, "asset")?;
        let class = need(self.class, "asset class")?;
        let mandate = need(self.mandate, "mandate")?;
        let target_exposure = need(self.target_exposure, "target exposure")?;
        let valuation = need(self.valuation, "valuation")?;
        let horizon = need(self.horizon, "investment horizon")?;
        let expected_exit = need(self.expected_exit, "expected liquidity")?;
        let hedges = need(self.hedges, "hedges")?;
        let corporate_actions = need(self.corporate_actions, "corporate actions")?;
        let commitments = need(self.commitments, "commitments")?;
        let exit_plan = need(self.exit_plan, "exit plan")?;

        if asset.trim().is_empty() || mandate.trim().is_empty() {
            return Err(Error::invalid(
                "the position record's asset and mandate must name something; supply an \
                 identifier rather than an empty string",
            ));
        }
        if valuation.asset() != asset {
            return Err(Error::invalid(format!(
                "the valuation is of {} but the record is for {asset}; attach the mark struck on \
                 this position",
                valuation.asset()
            )));
        }
        if target_exposure < Decimal::ZERO {
            return Err(Error::invalid(
                "the position record's target exposure is negative; state a share of the book \
                 of zero or more and carry direction elsewhere",
            ));
        }
        if horizon.as_nanos() <= 0 || expected_exit.as_nanos() < 0 {
            return Err(Error::invalid(
                "the position record's horizon must be positive and its expected exit time \
                 non-negative",
            ));
        }
        hedges.check("hedges")?;
        corporate_actions.check("corporate actions")?;
        commitments.check("commitments")?;
        Ok(PositionRecord {
            asset,
            class,
            mandate,
            target_exposure,
            valuation,
            horizon,
            expected_exit,
            hedges,
            corporate_actions,
            commitments,
            exit_plan,
        })
    }
}

/// A position with all nine economic facts. Deserialisation routes through
/// the builder, so a document cannot smuggle in what `build` would refuse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PositionRecordBuilder")]
pub struct PositionRecord {
    asset: String,
    class: AssetClass,
    mandate: String,
    target_exposure: Decimal,
    valuation: AssetValuation,
    horizon: Duration,
    expected_exit: Duration,
    hedges: Declared<Vec<String>>,
    corporate_actions: Declared<Vec<String>>,
    commitments: Declared<Commitment>,
    exit_plan: ExitPlan,
}

impl TryFrom<PositionRecordBuilder> for PositionRecord {
    type Error = Error;
    fn try_from(b: PositionRecordBuilder) -> Result<Self> {
        b.build()
    }
}

impl PositionRecord {
    pub fn asset(&self) -> &str {
        &self.asset
    }
    pub fn class(&self) -> AssetClass {
        self.class
    }
    pub fn mandate(&self) -> &str {
        &self.mandate
    }
    pub fn valuation(&self) -> &AssetValuation {
        &self.valuation
    }
    pub fn exit_plan(&self) -> &ExitPlan {
        &self.exit_plan
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
mod tests {
    use super::*;

    type Strip = fn(PositionRecordBuilder) -> PositionRecordBuilder;

    fn t(day: u32) -> Timestamp {
        Timestamp::from_civil(2026, 1, day)
    }

    fn complete(class: AssetClass) -> Result<PositionRecordBuilder> {
        Ok(PositionRecordBuilder::new()
            .asset("fund-a")
            .class(class)
            .mandate("mandate-1")
            .target_exposure(Decimal::from_int(1))
            .valuation(crate::valuation::IlliquidValuator::at_cost(
                "fund-a",
                Decimal::from_int(100),
                t(1),
                t(2),
            )?)
            .horizon(Duration::from_days(365))
            .expected_exit(Duration::from_days(30))
            .hedges(Declared::NoneApplicable {
                reason: "unhedgeable private holding".into(),
            })
            .corporate_actions(Declared::Present(vec!["ca-1".into()]))
            .commitments(Declared::NoneApplicable {
                reason: "fully funded".into(),
            })
            .exit_plan(ExitPlan::new(t(20), ExitRoute::Redemption, Some(t(10)))?))
    }

    #[test]
    fn a_record_missing_any_one_of_the_nine_fields_is_refused_naming_that_field() -> Result<()> {
        // Premise: the complete record builds, so every refusal below is the
        // omission and nothing else.
        complete(AssetClass::PrivateMarket)?.build()?;
        let cases: [(&str, Strip); 9] = [
            ("mandate", |mut b| {
                b.mandate = None;
                b
            }),
            ("target exposure", |mut b| {
                b.target_exposure = None;
                b
            }),
            ("valuation", |mut b| {
                b.valuation = None;
                b
            }),
            ("investment horizon", |mut b| {
                b.horizon = None;
                b
            }),
            ("expected liquidity", |mut b| {
                b.expected_exit = None;
                b
            }),
            ("hedges", |mut b| {
                b.hedges = None;
                b
            }),
            ("corporate actions", |mut b| {
                b.corporate_actions = None;
                b
            }),
            ("commitments", |mut b| {
                b.commitments = None;
                b
            }),
            ("exit plan", |mut b| {
                b.exit_plan = None;
                b
            }),
        ];
        for (field, strip) in cases {
            let err = strip(complete(AssetClass::PrivateMarket)?)
                .build()
                .err()
                .map(|e| format!("{e:?}"))
                .unwrap_or_default();
            assert!(
                err.contains(&format!("missing its {field};")),
                "omitting {field} gave {err:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_not_applicable_declaration_without_a_reason_is_refused() -> Result<()> {
        let blank = complete(AssetClass::Equity)?.hedges(Declared::NoneApplicable {
            reason: "  ".into(),
        });
        assert!(blank.build().is_err());
        Ok(())
    }

    #[test]
    fn a_complete_record_for_every_class_survives_a_serialisation_round_trip() -> Result<()> {
        assert!(AssetClass::ALL.len() > 1);
        for class in AssetClass::ALL {
            let record = complete(class)?.build()?;
            let json = serde_json::to_string(&record)
                .map_err(|e| Error::invalid(format!("serialise: {e}")))?;
            let back: PositionRecord = serde_json::from_str(&json)
                .map_err(|e| Error::invalid(format!("deserialise: {e}")))?;
            assert_eq!(back, record, "{class:?} lost a field in the round trip");
        }
        Ok(())
    }

    #[test]
    fn a_document_missing_a_field_cannot_deserialise_into_a_record() -> Result<()> {
        let record = complete(AssetClass::Equity)?.build()?;
        let mut v: serde_json::Value =
            serde_json::to_value(&record).map_err(|e| Error::invalid(format!("serialise: {e}")))?;
        let obj = v
            .as_object_mut()
            .ok_or_else(|| Error::invalid("not an object"))?;
        assert!(obj.remove("exit_plan").is_some());
        assert!(serde_json::from_value::<PositionRecord>(v).is_err());
        Ok(())
    }

    #[test]
    fn a_valuation_of_another_asset_is_refused() -> Result<()> {
        let other = crate::valuation::IlliquidValuator::at_cost(
            "fund-b",
            Decimal::from_int(1),
            t(1),
            t(2),
        )?;
        assert!(
            complete(AssetClass::Fund)?
                .valuation(other)
                .build()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn an_exit_dated_one_instant_before_lockup_expiry_is_refused_and_one_at_expiry_admitted()
    -> Result<()> {
        let expiry = t(10);
        let before = Timestamp::from_nanos(expiry.as_nanos() - 1);
        assert!(ExitPlan::new(before, ExitRoute::Sale, Some(expiry)).is_err());
        assert!(ExitPlan::new(expiry, ExitRoute::Sale, Some(expiry)).is_ok());
        // No lockup, no constraint: the refusal is the lockup's, not a blanket.
        assert!(ExitPlan::new(Timestamp::EPOCH, ExitRoute::Sale, None).is_ok());
        Ok(())
    }
}
