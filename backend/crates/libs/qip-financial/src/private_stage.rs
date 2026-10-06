//! Lifecycle stage of a private-company or fund position (blueprint ASSET-009).
//!
//! The stage is a pure function of the recorded events and changes only when
//! one is applied; nothing sets it directly, so a stage a log cannot explain
//! cannot exist. `replay` rebuilds it from the log alone.

use qip_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateStage {
    /// Commitment signed, nothing drawn.
    Committed,
    /// At least one capital call funded and no distribution yet.
    Funding,
    /// Distributions have begun; later calls (recallable capital) keep it here.
    Distributing,
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateEvent {
    Commitment,
    CapitalCall,
    Distribution,
    Exit,
}

impl PrivateStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::Funding => "funding",
            Self::Distributing => "distributing",
            Self::Exited => "exited",
        }
    }

    /// The stage after `event`, or a refusal naming the legal order. `None`
    /// is the position before its commitment is recorded.
    pub fn apply(current: Option<Self>, event: PrivateEvent) -> Result<Self> {
        use PrivateEvent as E;
        use PrivateStage as S;
        match (current, event) {
            (None, E::Commitment) => Ok(S::Committed),
            (Some(S::Committed | S::Funding), E::CapitalCall) => Ok(S::Funding),
            (Some(S::Distributing), E::CapitalCall) => Ok(S::Distributing),
            (Some(S::Funding | S::Distributing), E::Distribution) => Ok(S::Distributing),
            (Some(S::Funding | S::Distributing), E::Exit) => Ok(S::Exited),
            (cur, ev) => Err(Error::invalid(format!(
                "{ev:?} is not recorded against a position that is {}; a position begins with a \
                 commitment, is called before it distributes, and exits only once funded",
                cur.map_or("not yet committed", Self::as_str)
            ))),
        }
    }

    /// Rebuild the stage from a log, refusing an empty or illegal one.
    pub fn replay(events: &[PrivateEvent]) -> Result<Self> {
        let mut at = None;
        for e in events {
            at = Some(Self::apply(at, *e)?);
        }
        at.ok_or_else(|| {
            Error::invalid("an empty event log names no position; record a commitment")
        })
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
mod tests {
    use super::*;
    use PrivateEvent::*;

    fn walk(events: &[PrivateEvent]) -> Result<Vec<PrivateStage>> {
        let mut at = None;
        let mut out = Vec::new();
        for e in events {
            let s = PrivateStage::apply(at, *e)?;
            out.push(s);
            at = Some(s);
        }
        Ok(out)
    }

    #[test]
    fn a_fund_and_a_company_each_change_stage_on_exactly_their_matching_event() -> Result<()> {
        use PrivateStage::*;
        // Fund: calls, distributions, a late recall, exit.
        let fund = [
            Commitment,
            CapitalCall,
            CapitalCall,
            Distribution,
            CapitalCall,
            Exit,
        ];
        assert_eq!(
            walk(&fund)?,
            [
                Committed,
                Funding,
                Funding,
                Distributing,
                Distributing,
                Exited
            ]
        );
        // Company: one funding round, straight to exit.
        let company = [Commitment, CapitalCall, Exit];
        assert_eq!(walk(&company)?, [Committed, Funding, Exited]);
        // Replaying the log rebuilds the same stage.
        assert_eq!(PrivateStage::replay(&fund)?, Exited);
        assert_eq!(PrivateStage::replay(&fund[..4])?, Distributing);
        Ok(())
    }

    #[test]
    fn an_event_out_of_order_is_refused_and_no_stage_is_invented() {
        // Premise: the legal order is accepted.
        assert!(PrivateStage::replay(&[Commitment, CapitalCall, Distribution]).is_ok());
        assert!(PrivateStage::replay(&[CapitalCall]).is_err());
        assert!(PrivateStage::replay(&[Commitment, Distribution]).is_err());
        assert!(PrivateStage::replay(&[Commitment, Exit]).is_err());
        assert!(PrivateStage::replay(&[Commitment, CapitalCall, Exit, CapitalCall]).is_err());
        assert!(PrivateStage::replay(&[Commitment, Commitment]).is_err());
        assert!(PrivateStage::replay(&[]).is_err());
    }
}
