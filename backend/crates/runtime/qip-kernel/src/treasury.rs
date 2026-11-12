//! The treasury, composed (ADR 0388).
//!
//! # What this closes
//!
//! `qip-treasury` held the Treasury Bank's books and the funding planner, and
//! no binary linked it: before ADR 0385 the same code sat inside
//! `qip-capital`, linked and called by nothing. Every DECIDE built a
//! pre-positioning plan saying how much to move and where, and nothing ever
//! said which source each unit came from, so a plan could be reported against
//! capital no source was named for.
//!
//! [`Treasury`] is the caller. The platform holds one, and DECIDE asks it to
//! name the source of every move the plan proposes
//! ([`Treasury::fund_plan`]), from the plan's own budget as the central
//! treasury's own capital.
//!
//! # What an undeclared treasury does
//!
//! Three things are a person's to give and nobody has given them: the reserve
//! floor, a leverage ceiling per strategy and region, and a permission per
//! financing function and counterparty. [`Treasury::undeclared`] holds none.
//! It is the only treasury a composition root builds today, and it is not a
//! treasury with defaults:
//!
//! * **It permits no financing.** Every financing source offered is left out
//!   before anything is summed, and the funding says why
//!   ([`TreasuryFunding::financing_withheld`]). Borrowed money cannot be
//!   judged against a reserve nobody has stated.
//! * **It applies no reserve floor to own capital, and says so.** A floor of
//!   zero would be a figure this module chose. The plan's budget is already
//!   the allocator's drawdown-adjusted headroom, which is the limit that
//!   governs own capital until a floor is declared.
//!
//! # Where every figure comes from
//!
//! Nothing here is a constant of this module. A requirement's amount is the
//! move's `amount_base`; the one source's balance is the plan's `budget`; a
//! floor, a ceiling and a permission come from a [`TreasuryDeclaration`] or
//! are absent.
//!
//! # One source for the fact
//!
//! No funding record is journalled. The funding is a pure function of the
//! journalled plan (`PrePositioningPlanned`) and the declaration the platform
//! was assembled with, so a second record would be a second claim about the
//! same plan. The stage sentence carries it for the operator.

use std::collections::BTreeMap;

use qip_capital_fabric::plan::PrePositioningPlan;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_treasury::funding::{
    FundingPlan, FundingRefusal, FundingRequirement, FundingSource, RequirementKind, SourceBalance,
    plan_funding,
};
use qip_treasury::treasury::{
    CapitalBook, FinancingFunction, FinancingPermissions, LeverageBook, LeverageDecision, Use,
};

/// The name a pre-positioning placement is requested under.
///
/// The requester is the planner, not a trading strategy, so a leverage
/// decision for a financed pre-positioning move is recorded under this name
/// and the move's region.
pub const PRE_POSITIONING_REQUESTER: &str = "pre-positioning";

/// What a person has declared for the treasury. Empty by default.
///
/// Every field is absent until somebody gives it. There is no figure here
/// and no constructor that supplies one.
#[derive(Clone, Debug, Default)]
pub struct TreasuryDeclaration {
    /// The liquidity that must stay uncommitted. `None` is "not declared",
    /// which is not zero.
    pub reserve_floor: Option<Decimal>,
    /// Leverage ceilings, each per strategy and region.
    pub leverage: Vec<LeverageDecision>,
    /// Financing functions permitted, each at one counterparty, for the
    /// treasury's own entity and jurisdiction.
    pub permissions: Vec<(FinancingFunction, String)>,
}

/// The treasury's books as the kernel holds them.
#[derive(Clone, Debug)]
pub struct Treasury {
    entity: String,
    jurisdiction: String,
    reserve_floor: Option<Decimal>,
    leverage: LeverageBook,
    permissions: FinancingPermissions,
}

/// Every requirement funded or refused, and why no financing was counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreasuryFunding {
    /// The source of every unit, or the refusal.
    pub plan: FundingPlan,
    /// Why every financing source was left out. Empty only where a reserve
    /// floor is declared; a single source may still be left out for want of
    /// its own permission, which the planner's refusal counts.
    pub financing_withheld: Vec<String>,
    /// Whether a declared reserve floor was applied to own capital.
    pub reserve_floor_applied: bool,
}

impl Treasury {
    /// The treasury nobody has declared anything for.
    ///
    /// `entity` is who would be financing and `jurisdiction` where: the other
    /// two parts of the permission a financing source needs.
    pub fn undeclared(entity: &str, jurisdiction: &str) -> Self {
        Self {
            entity: entity.to_owned(),
            jurisdiction: jurisdiction.to_owned(),
            reserve_floor: None,
            leverage: LeverageBook::new(),
            permissions: FinancingPermissions::new(),
        }
    }

    /// A treasury holding what a person declared.
    ///
    /// A negative floor is refused: it would permit committing liquidity the
    /// platform does not have, which is the fact the floor exists to prevent.
    /// A ceiling of zero or less is refused by [`LeverageBook::decide`].
    pub fn declared(
        entity: &str,
        jurisdiction: &str,
        declaration: TreasuryDeclaration,
    ) -> Result<Self> {
        if let Some(floor) = declaration.reserve_floor
            && floor.is_negative()
        {
            return Err(Error::invalid(format!(
                "a reserve floor of {floor} is negative; declare the liquidity that must stay \
                 uncommitted as zero or more"
            )));
        }
        let mut treasury = Self::undeclared(entity, jurisdiction);
        treasury.reserve_floor = declaration.reserve_floor;
        for decision in &declaration.leverage {
            treasury.leverage.decide(
                &decision.strategy,
                &decision.region,
                decision.max_leverage,
                decision.decided_at,
            )?;
        }
        for (function, counterparty) in &declaration.permissions {
            treasury
                .permissions
                .permit(*function, entity, counterparty, jurisdiction);
        }
        Ok(treasury)
    }

    /// Why this treasury counts no financing source at all, if it does not.
    pub fn financing_withheld(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.reserve_floor.is_none() {
            reasons.push(
                "no reserve floor is declared, so borrowed money cannot be judged against what \
                 must stay unspent; the owner declares the floor"
                    .to_owned(),
            );
            if self.leverage.journal().is_empty() {
                reasons.push(
                    "no leverage ceiling is decided for any strategy and region; the owner \
                     decides one before capital is financed"
                        .to_owned(),
                );
            }
        }
        reasons
    }

    /// Refuse an order in a repo with `counterparty` unless this treasury's
    /// one permission registry holds the repo permission for it, under the
    /// treasury's own entity and jurisdiction (ADR 0398).
    ///
    /// The registry is the one [`Self::fund`] hands the planner, so a repo
    /// order and a financed placement can never read two different answers
    /// to who the platform may finance with. Undeclared, it refuses every
    /// counterparty.
    pub fn require_repo(&self, counterparty: &str) -> Result<()> {
        self.permissions
            .require_repo(&self.entity, counterparty, &self.jurisdiction)
    }

    /// Name the source of every requirement, or refuse it whole.
    ///
    /// Three refusals beyond the planner's own, each applied to a whole
    /// requirement and each returning that requirement's draws to the ones
    /// after it:
    ///
    /// * with no reserve floor declared, every financing source is left out
    ///   before anything is summed;
    /// * a requirement that draws on financing must pass the leverage
    ///   decision for its strategy and region, with the requirement as the
    ///   notional and its own-capital draws as the capital;
    /// * with a floor declared, a requirement whose own-capital draws would
    ///   take the uncommitted reserve below it is refused, not trimmed.
    pub fn fund(
        &self,
        requirements: &[FundingRequirement],
        balances: &[SourceBalance],
    ) -> Result<TreasuryFunding> {
        let financing_withheld = self.financing_withheld();
        let offered: Vec<SourceBalance> = balances
            .iter()
            .filter(|balance| financing_withheld.is_empty() || is_own(&balance.source))
            .cloned()
            .collect();
        let own_total = offered
            .iter()
            .filter(|balance| is_own(&balance.source))
            .fold(Decimal::ZERO, |sum, balance| sum + balance.available);

        // A requirement the treasury refuses is taken out and the rest are
        // planned again, so its draws go back to the sources. Each pass
        // excludes one requirement, so the loop ends within their count.
        let mut excluded: BTreeMap<String, String> = BTreeMap::new();
        loop {
            let live: Vec<FundingRequirement> = requirements
                .iter()
                .filter(|requirement| !excluded.contains_key(&requirement.id))
                .cloned()
                .collect();
            let mut plan = plan_funding(
                &live,
                &offered,
                &self.permissions,
                &self.entity,
                &self.jurisdiction,
            )?;
            if let Some((id, reason)) = self.first_breach(&plan, &live, own_total)? {
                excluded.insert(id, reason);
                continue;
            }
            for requirement in requirements {
                if let Some(reason) = excluded.get(&requirement.id) {
                    plan.refused.push(FundingRefusal {
                        requirement: requirement.id.clone(),
                        reason: reason.clone(),
                    });
                }
            }
            return Ok(TreasuryFunding {
                plan,
                financing_withheld,
                reserve_floor_applied: self.reserve_floor.is_some(),
            });
        }
    }

    /// The first funded requirement the leverage book or the reserve floor
    /// refuses, in the order the plan funded them.
    fn first_breach(
        &self,
        plan: &FundingPlan,
        live: &[FundingRequirement],
        own_total: Decimal,
    ) -> Result<Option<(String, String)>> {
        let mut book = match self.reserve_floor {
            Some(floor) => Some(CapitalBook::new(own_total, floor)?),
            None => None,
        };
        for funded in &plan.funded {
            let Some(requirement) = live.iter().find(|r| r.id == funded.requirement) else {
                return Err(Error::invalid(format!(
                    "the funding plan names requirement {} and none was asked for; the plan \
                     cannot be attributed",
                    funded.requirement
                )));
            };
            let own = funded
                .draws
                .iter()
                .filter(|draw| is_own(&draw.source))
                .fold(Decimal::ZERO, |sum, draw| sum + draw.amount);
            if own < requirement.amount
                && let Err(error) = self.leverage.check(
                    &requirement.strategy,
                    &requirement.region,
                    requirement.amount,
                    own,
                )
            {
                return Ok(Some((requirement.id.clone(), error.message().to_owned())));
            }
            if let Some(book) = book.as_mut()
                && own.is_positive()
            {
                let kind = match requirement.kind {
                    RequirementKind::Grant => Use::Grant,
                    RequirementKind::Placement => Use::Placement,
                };
                if let Err(error) = book.commit(&requirement.id, kind, own) {
                    return Ok(Some((requirement.id.clone(), error.message().to_owned())));
                }
            }
        }
        Ok(None)
    }

    /// Name the source of every move a pre-positioning plan proposes.
    ///
    /// One requirement per move, in the plan's order, for the move's
    /// `amount_base`: the figure that faced the allocator's limits. One
    /// source, `book`, holding the plan's `budget`: the headroom the plan was
    /// allowed to spend, taken from the plan so the funding and the plan
    /// cannot disagree about it. No financing source is offered, because
    /// nothing in the tree reports a balance at a financing counterparty.
    pub fn fund_plan(&self, plan: &PrePositioningPlan, book: &str) -> Result<TreasuryFunding> {
        let requirements: Vec<FundingRequirement> = plan
            .moves
            .iter()
            .enumerate()
            .map(|(index, planned)| FundingRequirement {
                id: format!("pre-position:{}:{index}", plan.at.as_nanos()),
                kind: RequirementKind::Placement,
                strategy: PRE_POSITIONING_REQUESTER.to_owned(),
                region: planned.to.region.to_string(),
                amount: planned.amount_base,
            })
            .collect();
        let balances = [SourceBalance {
            source: FundingSource::OwnCapital {
                book: book.to_owned(),
            },
            available: plan.budget,
        }];
        self.fund(&requirements, &balances)
    }
}

impl TreasuryFunding {
    /// One line for the stage sentence: what was sourced, what was refused,
    /// and the standing on financing and the reserve floor.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "treasury: {} placement(s) sourced, {} refused",
            self.plan.funded.len(),
            self.plan.refused.len()
        );
        if self.financing_withheld.is_empty() {
            line.push_str("; financing counted where permitted");
        } else {
            line.push_str(&format!(
                "; no financing counted ({})",
                self.financing_withheld.join("; ")
            ));
        }
        if !self.reserve_floor_applied {
            line.push_str("; no reserve floor applied to own capital, none being declared");
        }
        line
    }
}

fn is_own(source: &FundingSource) -> bool {
    matches!(source, FundingSource::OwnCapital { .. })
}
